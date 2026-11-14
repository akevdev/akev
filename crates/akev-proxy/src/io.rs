//! Low-level stream plumbing shared by every protocol layer: a replaying ("rewind") wrapper for
//! bytes consumed while sniffing, the metered client TCP stream (byte counters + reset-on-drop)
//! and small buffered-read helpers.

use crate::upstream::BoxIo;
use bytes::{Buf, Bytes, BytesMut};
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// Client-side stream at any layer (TCP, TLS, tunnelled HTTP/2 stream, ...).
pub(crate) type ClientIo = BoxIo;

/// Minimum free space kept in read buffers before each read.
const READ_CHUNK: usize = 32 * 1024;

/// A stream that first replays `prefix` (bytes already consumed while sniffing / parsing) and
/// then continues with the inner stream. Writes go straight through.
pub(crate) struct Rewind<T> {
    prefix: Bytes,
    inner: T,
}

impl<T> Rewind<T> {
    pub(crate) fn new(prefix: Bytes, inner: T) -> Self {
        Rewind { prefix, inner }
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Rewind<T> {
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

impl<T: AsyncWrite + Unpin> AsyncWrite for Rewind<T> {
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

/// Wrap `io` so `prefix` is read again first (no-op when the prefix is empty).
pub(crate) fn rewind(io: ClientIo, prefix: BytesMut) -> ClientIo {
    if prefix.is_empty() {
        io
    } else {
        Box::new(Rewind::new(prefix.freeze(), io))
    }
}

/// Global byte counters updated by every accepted client socket.
#[derive(Default)]
pub(crate) struct ByteCounters {
    pub bytes_in: AtomicU64,
    pub bytes_out: AtomicU64,
}

/// The accepted client TCP stream: counts bytes for [`crate::ProxyStats`] and, when its shared
/// reset flag is set, closes with `SO_LINGER=0` so the peer sees a TCP RST instead of a FIN
/// (used by `reset` rules / breakpoint resets).
pub(crate) struct ClientTcp {
    stream: TcpStream,
    counters: Arc<ByteCounters>,
    reset: Arc<AtomicBool>,
}

impl ClientTcp {
    pub(crate) fn new(
        stream: TcpStream,
        counters: Arc<ByteCounters>,
        reset: Arc<AtomicBool>,
    ) -> Self {
        ClientTcp {
            stream,
            counters,
            reset,
        }
    }
}

impl Drop for ClientTcp {
    fn drop(&mut self) {
        if self.reset.load(Ordering::Relaxed) {
            // Best effort: the RST is sent when the socket is closed right after this.
            let _ = self.stream.set_zero_linger();
        }
    }
}

impl AsyncRead for ClientTcp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.stream).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = res {
            let n = (buf.filled().len() - before) as u64;
            if n > 0 {
                self.counters.bytes_in.fetch_add(n, Ordering::Relaxed);
            }
        }
        res
    }
}

impl AsyncWrite for ClientTcp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.stream).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res {
            self.counters
                .bytes_out
                .fetch_add(n as u64, Ordering::Relaxed);
        }
        res
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.stream).poll_write_vectored(cx, bufs);
        if let Poll::Ready(Ok(n)) = res {
            self.counters
                .bytes_out
                .fetch_add(n as u64, Ordering::Relaxed);
        }
        res
    }

    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Counts bytes read from (client → upstream) and written to (upstream → client) a stream.
pub(crate) struct Counted<T> {
    inner: T,
    read: Arc<AtomicU64>,
    written: Arc<AtomicU64>,
}

impl<T> Counted<T> {
    pub(crate) fn new(inner: T) -> Self {
        Counted {
            inner,
            read: Arc::default(),
            written: Arc::default(),
        }
    }

    /// (bytes read, bytes written) counters.
    pub(crate) fn counters(&self) -> (Arc<AtomicU64>, Arc<AtomicU64>) {
        (self.read.clone(), self.written.clone())
    }
}

impl<T: AsyncRead + Unpin> AsyncRead for Counted<T> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = res {
            self.read
                .fetch_add((buf.filled().len() - before) as u64, Ordering::Relaxed);
        }
        res
    }
}

impl<T: AsyncWrite + Unpin> AsyncWrite for Counted<T> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res {
            self.written.fetch_add(n as u64, Ordering::Relaxed);
        }
        res
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Read more bytes into `buf` (growing it as needed). Returns the number of bytes read
/// (0 = EOF).
pub(crate) async fn read_more<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
) -> io::Result<usize> {
    if buf.capacity() - buf.len() < READ_CHUNK / 4 {
        buf.reserve(READ_CHUNK);
    }
    io.read_buf(buf).await
}

/// [`read_more`] with a timeout; a timeout is reported as `io::ErrorKind::TimedOut`.
pub(crate) async fn read_more_timeout<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
    timeout: Duration,
) -> io::Result<usize> {
    match tokio::time::timeout(timeout, read_more(io, buf)).await {
        Ok(res) => res,
        Err(_) => Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "timed out waiting for data",
        )),
    }
}

/// Read until `buf` holds at least `n` bytes. Fails with `UnexpectedEof` if the stream ends first.
pub(crate) async fn fill_to<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
    n: usize,
    timeout: Duration,
) -> io::Result<()> {
    while buf.len() < n {
        if read_more_timeout(io, buf, timeout).await? == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("connection closed before {n} bytes were received"),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn rewind_replays_prefix_then_inner() {
        let (mut a, b) = tokio::io::duplex(64);
        a.write_all(b" world").await.unwrap();
        drop(a);
        let mut r = Rewind::new(Bytes::from_static(b"hello"), b);
        let mut out = String::new();
        r.read_to_string(&mut out).await.unwrap();
        assert_eq!(out, "hello world");
    }

    #[tokio::test]
    async fn fill_to_reports_eof() {
        let (mut a, mut b) = tokio::io::duplex(64);
        a.write_all(b"ab").await.unwrap();
        drop(a);
        let mut buf = BytesMut::new();
        let err = fill_to(&mut b, &mut buf, 4, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::UnexpectedEof);
        assert_eq!(&buf[..], b"ab");
    }
}
