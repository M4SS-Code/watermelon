use std::{
    future::{self, Future},
    io,
    pin::{Pin, pin},
    task::{Context, Poll},
};

use bytes::Buf;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite};
use watermelon_proto::proto::{
    ClientOp, ServerOp, StreamDecoder, StreamEncoder, error::DecoderError,
};

#[derive(Debug)]
pub struct StreamingConnection<S> {
    socket: S,
    encoder: StreamEncoder,
    decoder: StreamDecoder,
    may_flush: bool,
}

impl<S> StreamingConnection<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(socket: S) -> Self {
        Self {
            socket,
            encoder: StreamEncoder::new(),
            decoder: StreamDecoder::new(),
            may_flush: false,
        }
    }

    pub fn poll_read_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<ServerOp, StreamingReadError>> {
        loop {
            match self.decoder.decode() {
                Ok(Some(server_op)) => return Poll::Ready(Ok(server_op)),
                Ok(None) => {}
                Err(err) => return Poll::Ready(Err(StreamingReadError::Decoder(err))),
            }

            let read_buf_fut = pin!(self.socket.read_buf(self.decoder.read_buf()));
            match read_buf_fut.poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Ok(1..)) => {}
                Poll::Ready(Ok(0)) => {
                    return Poll::Ready(Err(StreamingReadError::Io(
                        io::ErrorKind::UnexpectedEof.into(),
                    )));
                }
                Poll::Ready(Err(err)) => return Poll::Ready(Err(StreamingReadError::Io(err))),
            }
        }
    }

    /// Reads the next [`ServerOp`].
    ///
    /// # Errors
    ///
    /// It returns an error if the content cannot be decoded or if an I/O error occurs.
    pub async fn read_next(&mut self) -> Result<ServerOp, StreamingReadError> {
        future::poll_fn(|cx| self.poll_read_next(cx)).await
    }

    pub fn may_write(&self) -> bool {
        self.encoder.has_remaining()
    }

    pub fn may_flush(&self) -> bool {
        self.may_flush
    }

    pub fn may_enqueue_more_ops(&self) -> bool {
        self.encoder.remaining() < 8_290_304
    }

    pub fn enqueue_write_op(&mut self, item: &ClientOp) {
        self.encoder.enqueue_write_op(item);
    }

    pub fn poll_write_next(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<usize>> {
        let remaining = self.encoder.remaining();
        if remaining == 0 {
            return Poll::Ready(Ok(0));
        }

        let chunk = self.encoder.chunk();
        let write_outcome = if chunk.len() < remaining && self.socket.is_write_vectored() {
            let mut bufs = [io::IoSlice::new(&[]); 64];
            let n = self.encoder.chunks_vectored(&mut bufs);
            debug_assert!(
                n >= 2,
                "perf: chunks_vectored yielded less than 2 chunks despite the apparently fragmented internal encoder representation"
            );

            Pin::new(&mut self.socket).poll_write_vectored(cx, &bufs[..n])
        } else {
            debug_assert!(
                !chunk.is_empty(),
                "perf: chunk shouldn't be empty given that `remaining > 0`"
            );
            Pin::new(&mut self.socket).poll_write(cx, chunk)
        };

        match write_outcome {
            Poll::Pending => {
                self.may_flush = false;
                Poll::Pending
            }
            Poll::Ready(Ok(n)) => {
                self.encoder.advance(n);
                self.may_flush = true;
                Poll::Ready(Ok(n))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
        }
    }

    /// Writes the next chunk of data to the socket.
    ///
    /// It returns the number of bytes that have been written.
    ///
    /// # Errors
    ///
    /// An I/O error is returned if it is not possible to write to the socket.
    pub async fn write_next(&mut self) -> io::Result<usize> {
        future::poll_fn(|cx| self.poll_write_next(cx)).await
    }

    pub fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match Pin::new(&mut self.socket).poll_flush(cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => {
                self.may_flush = false;
                Poll::Ready(Ok(()))
            }
            Poll::Ready(Err(err)) => Poll::Ready(Err(err)),
        }
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
        future::poll_fn(|cx| Pin::new(&mut self.socket).poll_shutdown(cx)).await
    }

    pub fn socket(&self) -> &S {
        &self.socket
    }

    pub fn socket_mut(&mut self) -> &mut S {
        &mut self.socket
    }

    pub fn replace_socket<F, S2>(self, replacer: F) -> StreamingConnection<S2>
    where
        F: FnOnce(S) -> S2,
    {
        StreamingConnection {
            socket: replacer(self.socket),
            encoder: self.encoder,
            decoder: self.decoder,
            may_flush: self.may_flush,
        }
    }

    pub fn into_inner(self) -> S {
        self.socket
    }
}

#[derive(Debug, thiserror::Error)]
pub enum StreamingReadError {
    #[error("decoder")]
    Decoder(#[source] DecoderError),
    #[error("io")]
    Io(#[source] io::Error),
}

#[cfg(test)]
mod tests {
    use std::{
        pin::Pin,
        task::{Context, Poll},
    };

    use claims::assert_matches;
    use futures_util::task;
    use tokio::io::{self, AsyncRead, AsyncWrite, ReadBuf};
    use watermelon_proto::proto::{ClientOp, ServerOp};

    use super::StreamingConnection;

    #[test]
    fn ping_pong() {
        let waker = task::noop_waker();
        let mut cx = Context::from_waker(&waker);

        let (socket, mut conn) = io::duplex(1024);

        let mut client = StreamingConnection::new(socket);

        // Initial state is ok
        assert!(client.poll_read_next(&mut cx).is_pending());
        assert_matches!(client.poll_write_next(&mut cx), Poll::Ready(Ok(0)));

        let mut buf = [0; 1024];
        let mut read_buf = ReadBuf::new(&mut buf);
        assert!(
            Pin::new(&mut conn)
                .poll_read(&mut cx, &mut read_buf)
                .is_pending()
        );

        // Write PING and verify it was received
        client.enqueue_write_op(&ClientOp::Ping);
        assert_matches!(client.poll_write_next(&mut cx), Poll::Ready(Ok(6)));
        assert_matches!(
            Pin::new(&mut conn).poll_read(&mut cx, &mut read_buf),
            Poll::Ready(Ok(()))
        );
        assert_eq!(read_buf.filled(), b"PING\r\n");

        // Receive PONG
        assert_matches!(
            Pin::new(&mut conn).poll_write(&mut cx, b"PONG\r\n"),
            Poll::Ready(Ok(6))
        );
        assert_matches!(
            client.poll_read_next(&mut cx),
            Poll::Ready(Ok(ServerOp::Pong))
        );
        assert!(client.poll_read_next(&mut cx).is_pending());
    }
}
