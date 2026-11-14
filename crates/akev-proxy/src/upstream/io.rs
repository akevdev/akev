//! IO adapters.

use bytes::{Buf, Bytes};
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A stream that yields `prefix` before reading from `inner` (bytes that were read past a
/// protocol handshake, e.g. after an HTTP `CONNECT` response head).
pub(crate) struct PrefixedIo<T> {
    prefix: Bytes,
    inner: T,
}

impl<T> PrefixedIo<T> {
    pub(crate) fn new(prefix: Bytes, inner: T) -> Self {
        PrefixedIo { prefix, inner }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for PrefixedIo<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = self.prefix.len().min(buf.remaining());
            buf.put_slice(&self.prefix[..n]);
            self.prefix.advance(n);
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for PrefixedIo<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.inner.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn prefix_is_read_first() {
        let (a, mut b) = tokio::io::duplex(64);
        let mut io = PrefixedIo::new(Bytes::from_static(b"hello "), a);
        b.write_all(b"world").await.unwrap();
        drop(b);
        let mut out = String::new();
        io.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }
}
