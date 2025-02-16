use std::{
    future, io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll, Wake, Waker},
};

use bytes::Bytes;
use futures_core::Stream as _;
use futures_sink::Sink;
use http::Uri;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_websockets::{ClientBuilder, Message, WebSocketStream};
use watermelon_proto::proto::{
    decode_frame, error::FrameDecoderError, ClientOp, FramedEncoder, ServerOp,
};

#[derive(Debug)]
pub struct WebsocketConnection<S> {
    socket: WebSocketStream<S>,
    encoder: FramedEncoder,
    residual_frame: Bytes,
    should_flush: bool,
    // FIXME: switch to `std::task::Waker::noop` with MSRV >= 1.85
    noop_waker_polyfill: Waker,
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
            noop_waker_polyfill: Waker::from(Arc::new(NoopWakerPolyfill)),
        })
    }

    pub fn poll_read_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<ServerOp, WebsocketReadError>> {
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
                    return Poll::Ready(Err(WebsocketReadError::Io(websockets_error_to_io(err))))
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
        let mut cx = Context::from_waker(&self.noop_waker_polyfill);
        Pin::new(&mut self.socket).poll_ready(&mut cx).is_ready()
    }

    /// Enqueue `item` to be written.
    #[expect(clippy::missing_panics_doc)]
    pub fn enqueue_write_op(&mut self, item: &ClientOp) {
        let payload = self.encoder.encode(item);
        Pin::new(&mut self.socket)
            .start_send(Message::binary(payload))
            .unwrap();
        self.should_flush = true;
    }

    pub fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
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

#[derive(Debug)]
struct NoopWakerPolyfill;

impl Wake for NoopWakerPolyfill {
    fn wake(self: Arc<Self>) {}

    fn wake_by_ref(self: &Arc<Self>) {}
}
