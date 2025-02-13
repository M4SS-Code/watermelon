use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

#[cfg(feature = "non-standard-zstd")]
use crate::non_standard_zstd::ZstdStream;

#[derive(Debug)]
pub enum ConnectionCompression<S> {
    Plain(S),
    #[cfg(feature = "non-standard-zstd")]
    Zstd(ZstdStream<S>),
}

impl<S> ConnectionCompression<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[cfg(feature = "non-standard-zstd")]
    pub(crate) fn upgrade_zstd(self, compression_level: u8) -> Self {
        let Self::Plain(socket) = self else {
            unreachable!()
        };

        Self::Zstd(ZstdStream::new(socket, compression_level))
    }

    #[cfg(feature = "non-standard-zstd")]
    pub fn is_zstd_compressed(&self) -> bool {
        matches!(self, Self::Zstd(_))
    }
}

impl<S> AsyncRead for ConnectionCompression<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_read(cx, buf),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => Pin::new(conn).poll_read(cx, buf),
        }
    }
}

impl<S> AsyncWrite for ConnectionCompression<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_write(cx, buf),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => Pin::new(conn).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_flush(cx),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => Pin::new(conn).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_shutdown(cx),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => Pin::new(conn).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_write_vectored(cx, bufs),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => Pin::new(conn).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(conn) => conn.is_write_vectored(),
            #[cfg(feature = "non-standard-zstd")]
            Self::Zstd(conn) => conn.is_write_vectored(),
        }
    }
}
