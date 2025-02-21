use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_rustls::{TlsConnector, client::TlsStream, rustls::pki_types::ServerName};

#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "using TLS is the recommended thing, we do not want to affect it"
)]
pub enum ConnectionSecurity<S> {
    Plain(S),
    Tls(TlsStream<S>),
}

impl<S> ConnectionSecurity<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub(crate) async fn upgrade_tls(
        self,
        connector: &TlsConnector,
        domain: ServerName<'static>,
    ) -> io::Result<Self> {
        let conn = match self {
            Self::Plain(conn) => conn,
            Self::Tls(_) => unreachable!("trying to upgrade to Tls a Tls connection"),
        };

        let conn = connector.connect(domain, conn).await?;
        Ok(Self::Tls(conn))
    }
}

impl<S> AsyncRead for ConnectionSecurity<S>
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
            Self::Tls(conn) => Pin::new(conn).poll_read(cx, buf),
        }
    }
}

impl<S> AsyncWrite for ConnectionSecurity<S>
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
            Self::Tls(conn) => Pin::new(conn).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_flush(cx),
            Self::Tls(conn) => Pin::new(conn).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_shutdown(cx),
            Self::Tls(conn) => Pin::new(conn).poll_shutdown(cx),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(conn) => Pin::new(conn).poll_write_vectored(cx, bufs),
            Self::Tls(conn) => Pin::new(conn).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(conn) => conn.is_write_vectored(),
            Self::Tls(conn) => conn.is_write_vectored(),
        }
    }
}
