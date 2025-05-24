#[cfg(not(feature = "websocket"))]
use std::{convert::Infallible, marker::PhantomData};
use std::{
    io,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite};
#[cfg(feature = "websocket")]
use watermelon_proto::proto::error::FrameDecoderError;
use watermelon_proto::{
    Connect,
    error::ServerError,
    proto::{ClientOp, ServerOp, error::DecoderError},
};

pub use self::streaming::{StreamingConnection, StreamingReadError};
#[cfg(feature = "websocket")]
pub use self::websocket::{WebsocketConnection, WebsocketReadError};

mod streaming;
#[cfg(feature = "websocket")]
mod websocket;

#[derive(Debug)]
#[expect(clippy::large_enum_variant)]
pub enum Connection<S1, S2> {
    Streaming(StreamingConnection<S1>),
    Websocket(WebsocketConnection<S2>),
}

#[derive(Debug)]
#[cfg(not(feature = "websocket"))]
#[doc(hidden)]
pub struct WebsocketConnection<S> {
    _socket: PhantomData<S>,
    _impossible: Infallible,
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectionReadError {
    #[error("streaming connection error")]
    Streaming(#[source] StreamingReadError),
    #[cfg(feature = "websocket")]
    #[error("websocket connection error")]
    Websocket(#[source] WebsocketReadError),
}

impl<S1, S2> Connection<S1, S2>
where
    S1: AsyncRead + AsyncWrite + Unpin,
    S2: AsyncRead + AsyncWrite + Unpin,
{
    pub fn poll_read_next(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<ServerOp, ConnectionReadError>> {
        match self {
            Self::Streaming(streaming) => streaming
                .poll_read_next(cx)
                .map_err(ConnectionReadError::Streaming),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket
                .poll_read_next(cx)
                .map_err(ConnectionReadError::Websocket),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    /// Read the next incoming server operation.
    ///
    /// # Errors
    ///
    /// Returns an error if reading or decoding the message fails.
    pub async fn read_next(&mut self) -> Result<ServerOp, ConnectionReadError> {
        match self {
            Self::Streaming(streaming) => streaming
                .read_next()
                .await
                .map_err(ConnectionReadError::Streaming),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket
                .read_next()
                .await
                .map_err(ConnectionReadError::Websocket),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    pub fn flushes_automatically_when_full(&self) -> bool {
        match self {
            Self::Streaming(_streaming) => true,
            #[cfg(feature = "websocket")]
            Self::Websocket(_websocket) => false,
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    pub fn should_flush(&self) -> bool {
        match self {
            Self::Streaming(streaming) => streaming.may_flush(),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.should_flush(),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    pub fn may_enqueue_more_ops(&mut self) -> bool {
        match self {
            Self::Streaming(streaming) => streaming.may_enqueue_more_ops(),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.may_enqueue_more_ops(),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    pub fn enqueue_write_op(&mut self, item: &ClientOp) {
        match self {
            Self::Streaming(streaming) => streaming.enqueue_write_op(item),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.enqueue_write_op(item),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    /// Convenience function for writing enqueued messages and flushing.
    ///
    /// # Errors
    ///
    /// Returns an error if writing or flushing fails.
    pub async fn write_and_flush(&mut self) -> io::Result<()> {
        if let Self::Streaming(streaming) = self {
            while streaming.may_write() {
                streaming.write_next().await?;
            }
        }

        self.flush().await
    }

    pub fn poll_flush(&mut self, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self {
            Self::Streaming(streaming) => streaming.poll_flush(cx),
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.poll_flush(cx),
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    /// Flush any buffered writes to the connection
    ///
    /// # Errors
    ///
    /// Returns an error if flushing fails
    pub async fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Streaming(streaming) => streaming.flush().await,
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.flush().await,
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }

    /// Shutdown the connection
    ///
    /// # Errors
    ///
    /// Returns an error if shutting down the connection fails.
    /// Implementations usually ignore this error.
    pub async fn shutdown(&mut self) -> io::Result<()> {
        match self {
            Self::Streaming(streaming) => streaming.shutdown().await,
            #[cfg(feature = "websocket")]
            Self::Websocket(websocket) => websocket.shutdown().await,
            #[cfg(not(feature = "websocket"))]
            Self::Websocket(_) => unreachable!(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("proto")]
    Proto(#[source] DecoderError),
    #[error("server")]
    ServerError(#[source] ServerError),
    #[error("io")]
    Io(#[source] io::Error),
    #[error("unexpected ServerOp")]
    UnexpectedOp,
}

/// Send the `CONNECT` command to a pre-establised connection `conn`.
///
/// # Errors
///
/// Returns an error if connecting fails
pub async fn connect<S1, S2, F>(
    conn: &mut Connection<S1, S2>,
    connect: Connect,
    after_connect: F,
) -> Result<(), ConnectError>
where
    S1: AsyncRead + AsyncWrite + Unpin,
    S2: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce(&mut Connection<S1, S2>),
{
    conn.enqueue_write_op(&ClientOp::Connect {
        connect: Box::new(connect),
    });
    conn.write_and_flush().await.map_err(ConnectError::Io)?;

    after_connect(conn);
    conn.enqueue_write_op(&ClientOp::Ping);
    conn.write_and_flush().await.map_err(ConnectError::Io)?;

    loop {
        match conn.read_next().await {
            Ok(ServerOp::Success) => {
                // Success. Repeat to receive the PONG
            }
            Ok(ServerOp::Pong) => {
                // Success. We've received the PONG,
                // possibly after having received OK.
                return Ok(());
            }
            Ok(ServerOp::Ping) => {
                // I guess this could somehow happen. Handle it and repeat
                conn.enqueue_write_op(&ClientOp::Pong);
            }
            Ok(ServerOp::Error { error }) => return Err(ConnectError::ServerError(error)),
            Ok(ServerOp::Info { .. } | ServerOp::Message { .. }) => {
                return Err(ConnectError::UnexpectedOp);
            }
            Err(ConnectionReadError::Streaming(StreamingReadError::Decoder(err))) => {
                return Err(ConnectError::Proto(err));
            }
            Err(ConnectionReadError::Streaming(StreamingReadError::Io(err))) => {
                return Err(ConnectError::Io(err));
            }
            #[cfg(feature = "websocket")]
            Err(ConnectionReadError::Websocket(WebsocketReadError::Decoder(
                FrameDecoderError::Decoder(err),
            ))) => return Err(ConnectError::Proto(err)),
            #[cfg(feature = "websocket")]
            Err(ConnectionReadError::Websocket(WebsocketReadError::Decoder(
                FrameDecoderError::IncompleteFrame,
            ))) => todo!(),
            #[cfg(feature = "websocket")]
            Err(ConnectionReadError::Websocket(WebsocketReadError::Io(err))) => {
                return Err(ConnectError::Io(err));
            }
            #[cfg(feature = "websocket")]
            Err(ConnectionReadError::Websocket(WebsocketReadError::Closed)) => todo!(),
        }
    }
}
