use std::{
    fmt::{self, Debug, Formatter},
    io,
    pin::Pin,
    task::{Context, Poll},
};

use async_compression::{
    Level,
    tokio::{bufread::ZstdDecoder, write::ZstdEncoder},
};
use tokio::io::{AsyncRead, AsyncWrite, BufReader, ReadBuf};

use crate::util::MaybeConnection;

pub struct ZstdStream<S> {
    decoder: ZstdDecoder<BufReader<MaybeConnection<S>>>,
    encoder: ZstdEncoder<MaybeConnection<S>>,
}

impl<S> ZstdStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    #[must_use]
    pub fn new(stream: S, compression_level: u8) -> Self {
        Self {
            decoder: ZstdDecoder::new(BufReader::new(MaybeConnection(Some(stream)))),
            encoder: ZstdEncoder::with_quality(
                MaybeConnection(None),
                Level::Precise(compression_level.into()),
            ),
        }
    }
}

impl<S> AsyncRead for ZstdStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if let Some(stream) = self.encoder.get_mut().0.take() {
            self.decoder.get_mut().get_mut().0 = Some(stream);
        }

        Pin::new(&mut self.decoder).poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for ZstdStream<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if let Some(stream) = self.decoder.get_mut().get_mut().0.take() {
            self.encoder.get_mut().0 = Some(stream);
        }

        Pin::new(&mut self.encoder).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        if let Some(stream) = self.decoder.get_mut().get_mut().0.take() {
            self.encoder.get_mut().0 = Some(stream);
        }

        Pin::new(&mut self.encoder).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        if let Some(stream) = &self.encoder.get_ref().0 {
            stream.is_write_vectored()
        } else if let Some(stream) = &self.decoder.get_ref().get_ref().0 {
            stream.is_write_vectored()
        } else {
            unreachable!()
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(stream) = self.decoder.get_mut().get_mut().0.take() {
            self.encoder.get_mut().0 = Some(stream);
        }

        Pin::new(&mut self.encoder).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if let Some(stream) = self.decoder.get_mut().get_mut().0.take() {
            self.encoder.get_mut().0 = Some(stream);
        }

        Pin::new(&mut self.encoder).poll_shutdown(cx)
    }
}

impl<S> Debug for ZstdStream<S> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        f.debug_struct("ZstdStream").finish_non_exhaustive()
    }
}
