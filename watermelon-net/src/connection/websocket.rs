use std::{
    future, io,
    pin::Pin,
    task::{Context, Poll, Waker},
};

use bytes::Bytes;
use futures_core::Stream as _;
use futures_sink::Sink;
use http::Uri;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_websockets::{ClientBuilder, Message, WebSocketStream};
use watermelon_proto::proto::{
    ClientOp, FramedEncoder, ServerOp, decode_frame, error::FrameDecoderError,
};

#[derive(Debug)]
pub struct WebsocketConnection<S> {
    socket: WebSocketStream<S>,
    encoder: FramedEncoder,
    residual_frame: Bytes,
    should_flush: bool,
    /// Failure of a `start_send` call made by [`Self::enqueue_write_op`].
    ///
    /// `enqueue_write_op` returns `()`, so the error is stored here and
    /// reported once through the fallible polling methods instead of
    /// panicking.
    send_error: Option<io::Error>,
}

impl<S> WebsocketConnection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Construct a websocket stream to a pre-established connection `socket`.
    ///
    /// # Errors
    ///
    /// Returns an error if the websocket handshake fails.
    pub async fn new(uri: Uri, socket: S) -> io::Result<Self> {
        let (socket, _resp) = ClientBuilder::from_uri(uri)
            .connect_on(socket)
            .await
            .map_err(websockets_error_to_io)?;
        Ok(Self {
            socket,
            encoder: FramedEncoder::new(),
            residual_frame: Bytes::new(),
            should_flush: false,
            send_error: None,
        })
    }

    pub fn poll_read_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<ServerOp, WebsocketReadError>> {
        if let Some(err) = self.send_error.take() {
            return Poll::Ready(Err(WebsocketReadError::Io(err)));
        }

        loop {
            if !self.residual_frame.is_empty() {
                return Poll::Ready(
                    decode_frame(&mut self.residual_frame).map_err(WebsocketReadError::Decoder),
                );
            }

            match Pin::new(&mut self.socket).poll_next(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Some(Ok(message))) if message.is_binary() => {
                    self.residual_frame = message.into_payload().into();
                }
                Poll::Ready(Some(Ok(_message))) => {}
                Poll::Ready(Some(Err(err))) => {
                    return Poll::Ready(Err(WebsocketReadError::Io(websockets_error_to_io(err))));
                }
                Poll::Ready(None) => return Poll::Ready(Err(WebsocketReadError::Closed)),
            }
        }
    }

    /// Reads the next [`ServerOp`].
    ///
    /// # Errors
    ///
    /// It returns an error if the content cannot be decoded or if an I/O error occurs.
    pub async fn read_next(&mut self) -> Result<ServerOp, WebsocketReadError> {
        future::poll_fn(|cx| self.poll_read_next(cx)).await
    }

    pub fn should_flush(&self) -> bool {
        self.should_flush
    }

    pub fn may_enqueue_more_ops(&mut self) -> bool {
        if self.send_error.is_some() {
            return false;
        }

        let mut cx = Context::from_waker(Waker::noop());
        Pin::new(&mut self.socket).poll_ready(&mut cx).is_ready()
    }

    /// Enqueue `item` to be written.
    ///
    /// If the sink fails to accept the frame (e.g. the server initiated the
    /// closing handshake), the error is stored and reported by the next call
    /// to [`Self::poll_read_next`] or [`Self::poll_flush`].
    pub fn enqueue_write_op(&mut self, item: &ClientOp) {
        if self.send_error.is_some() {
            // The connection is broken and about to be reported as such.
            return;
        }

        let payload = self.encoder.encode(item);
        if let Err(err) = Pin::new(&mut self.socket).start_send(Message::binary(payload)) {
            self.send_error = Some(websockets_error_to_io(err));
        }
        self.should_flush = true;
    }

    pub fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(err) = self.send_error.take() {
            return Poll::Ready(Err(err));
        }

        Pin::new(&mut self.socket)
            .poll_flush(cx)
            .map_err(websockets_error_to_io)
    }

    /// Flush any buffered writes to the connection
    ///
    /// # Errors
    ///
    /// Returns an error if flushing fails
    pub async fn flush(&mut self) -> io::Result<()> {
        future::poll_fn(|cx| self.poll_flush(cx)).await
    }

    /// Shutdown the connection
    ///
    /// # Errors
    ///
    /// Returns an error if shutting down the connection fails.
    /// Implementations usually ignore this error.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        future::poll_fn(|cx| Pin::new(&mut self.socket).poll_close(cx))
            .await
            .map_err(websockets_error_to_io)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WebsocketReadError {
    #[error("decoder")]
    Decoder(#[source] FrameDecoderError),
    #[error("io")]
    Io(#[source] io::Error),
    #[error("closed")]
    Closed,
}

fn websockets_error_to_io(err: tokio_websockets::Error) -> io::Error {
    match err {
        tokio_websockets::Error::Io(err) => err,
        err => io::Error::other(err),
    }
}

#[cfg(test)]
mod tests {
    use std::{future, pin::Pin};

    use futures_core::Stream as _;
    use futures_sink::Sink as _;
    use tokio::{io, task::JoinHandle};
    use tokio_websockets::{Message, ServerBuilder};
    use watermelon_proto::proto::ClientOp;

    use super::WebsocketConnection;

    /// Run a websocket handshake on a duplex stream and have the server
    /// immediately initiate the closing handshake.
    async fn closing_server_pair() -> (WebsocketConnection<io::DuplexStream>, JoinHandle<()>) {
        let (client_io, server_io) = io::duplex(4096);
        let server = tokio::spawn(async move {
            let (_request, mut server) = ServerBuilder::new().accept(server_io).await.unwrap();

            let mut server = Pin::new(&mut server);
            server
                .as_mut()
                .start_send(Message::close(None, ""))
                .unwrap();
            let _ = future::poll_fn(|cx| server.as_mut().poll_flush(cx)).await;
            // Drain the client's own close frame
            let _ = future::poll_fn(|cx| server.as_mut().poll_next(cx)).await;
        });
        let client = WebsocketConnection::new("ws://localhost/".parse().unwrap(), client_io)
            .await
            .unwrap();
        (client, server)
    }

    #[tokio::test]
    async fn enqueue_write_op_after_close_does_not_panic() {
        let (mut client, server) = closing_server_pair().await;

        // Observe the server's close frame, transitioning the sink out of
        // the active state. `start_send` calls made from now on fail.
        let _ = client.read_next().await;

        // Used to panic on the failed `start_send`.
        client.enqueue_write_op(&ClientOp::Ping);
        assert!(!client.may_enqueue_more_ops());

        // The failure is reported through the fallible polling methods.
        assert!(client.flush().await.is_err());

        server.await.unwrap();
    }
}
