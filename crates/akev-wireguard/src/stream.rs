//! [`TcpStream`]: one TCP connection of the tunnelled device, terminated by the userspace
//! stack and exposed as a Tokio `AsyncRead + AsyncWrite` byte stream.
//!
//! The smoltcp socket itself lives on the server task; the stream and the task share a small
//! staging state per connection:
//!
//! * **device → application**: the task moves received bytes from the socket into `rx` while
//!   it holds less than [`STAGING_LIMIT`]. When the application doesn't read, the socket's
//!   receive buffer fills up and the TCP window closes — the device is slowed down, nothing is
//!   dropped or buffered without bound.
//! * **application → device**: writes append to `tx` up to [`STAGING_LIMIT`] and then return
//!   `Pending` until the task has moved bytes into the socket's send buffer, which drains as
//!   the device acknowledges data.
//!
//! Shutting down the write half sends a FIN once everything written was handed to the
//! socket. Dropping the stream closes the connection gracefully, or with a TCP RST when the
//! [reset flag](TcpStream::reset_flag) was set.

use bytes::{Buf, BytesMut};
use parking_lot::Mutex;
use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::sync::Notify;

/// Bytes staged per direction between a socket and its stream.
pub(crate) const STAGING_LIMIT: usize = 64 * 1024;

#[derive(Default)]
pub(crate) struct State {
    /// Device → application bytes not read yet.
    pub rx: BytesMut,
    /// The device closed its sending half (FIN) and all its data is in `rx`.
    pub rx_eof: bool,
    /// The server task has more received data waiting than `rx` could take.
    pub rx_pending: bool,
    /// Application → device bytes not handed to the socket yet.
    pub tx: BytesMut,
    /// The application shut down its writing half.
    pub tx_shutdown: bool,
    /// The application dropped the stream.
    pub dropped: bool,
    /// The application asked for a TCP RST.
    pub abort: bool,
    /// The connection is gone without a graceful close: reset by the device, timed out, or
    /// torn down because the server stopped.
    pub ended: bool,
    read_waker: Option<Waker>,
    write_waker: Option<Waker>,
}

impl State {
    pub(crate) fn wake_reader(&mut self) -> Option<Waker> {
        self.read_waker.take()
    }

    pub(crate) fn wake_writer(&mut self) -> Option<Waker> {
        self.write_waker.take()
    }
}

/// State shared by a [`TcpStream`] and the server task.
pub(crate) struct Shared {
    pub state: Mutex<State>,
    /// Wakes the server task (shared by all connections of a server).
    pub wake: Arc<Notify>,
    /// Set (e.g. by the proxy) to close with a TCP RST instead of a FIN.
    pub reset: Arc<AtomicBool>,
}

impl Shared {
    pub(crate) fn new(wake: Arc<Notify>) -> Arc<Self> {
        Arc::new(Shared {
            state: Mutex::new(State::default()),
            wake,
            reset: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Mark the connection as gone (socket reset or server stopped) and wake both halves.
    pub(crate) fn end(&self) {
        let (r, w) = {
            let mut st = self.state.lock();
            st.ended = true;
            st.tx.clear();
            (st.wake_reader(), st.wake_writer())
        };
        wake(r);
        wake(w);
    }
}

pub(crate) fn wake(w: Option<Waker>) {
    if let Some(w) = w {
        w.wake();
    }
}

/// A TCP connection from the tunnelled device.
///
/// `peer_addr` is the device's address inside the tunnel (`10.0.0.2:port`); `local_addr` is
/// the destination the device connected to (the original destination IP and port).
pub struct TcpStream {
    shared: Arc<Shared>,
    peer: SocketAddr,
    local: SocketAddr,
}

impl TcpStream {
    pub(crate) fn new(shared: Arc<Shared>, peer: SocketAddr, local: SocketAddr) -> Self {
        TcpStream {
            shared,
            peer,
            local,
        }
    }

    /// The device's address inside the tunnel.
    pub fn peer_addr(&self) -> SocketAddr {
        self.peer
    }

    /// The original destination the device connected to.
    pub fn local_addr(&self) -> SocketAddr {
        self.local
    }

    /// When set before the stream is dropped, the connection is closed with a TCP RST.
    pub fn reset_flag(&self) -> Arc<AtomicBool> {
        self.shared.reset.clone()
    }
}

impl std::fmt::Debug for TcpStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TcpStream")
            .field("peer", &self.peer)
            .field("local", &self.local)
            .finish()
    }
}

impl AsyncRead for TcpStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let mut notify = false;
        let res = {
            let mut st = self.shared.state.lock();
            if !st.rx.is_empty() {
                let n = st.rx.len().min(buf.remaining());
                buf.put_slice(&st.rx[..n]);
                st.rx.advance(n);
                if st.rx_pending {
                    st.rx_pending = false;
                    notify = true;
                }
                Poll::Ready(Ok(()))
            } else if st.rx_eof {
                Poll::Ready(Ok(()))
            } else if st.ended {
                Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::ConnectionReset,
                    "the tunnelled connection was reset",
                )))
            } else {
                st.read_waker = Some(cx.waker().clone());
                Poll::Pending
            }
        };
        if notify {
            self.shared.wake.notify_one();
        }
        res
    }
}

impl AsyncWrite for TcpStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = {
            let mut st = self.shared.state.lock();
            if st.ended || st.tx_shutdown {
                return Poll::Ready(Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "the tunnelled connection is closed",
                )));
            }
            if buf.is_empty() {
                return Poll::Ready(Ok(0));
            }
            let room = STAGING_LIMIT.saturating_sub(st.tx.len());
            if room == 0 {
                st.write_waker = Some(cx.waker().clone());
                return Poll::Pending;
            }
            let n = room.min(buf.len());
            st.tx.extend_from_slice(&buf[..n]);
            Poll::Ready(Ok(n))
        };
        self.shared.wake.notify_one();
        res
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        // Like a TCP socket: written bytes are already queued for transmission.
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        {
            let mut st = self.shared.state.lock();
            if st.tx_shutdown || st.ended {
                return Poll::Ready(Ok(()));
            }
            st.tx_shutdown = true;
        }
        self.shared.wake.notify_one();
        Poll::Ready(Ok(()))
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        {
            let mut st = self.shared.state.lock();
            st.dropped = true;
            st.rx.clear();
            if self.shared.reset.load(Ordering::Relaxed) {
                st.abort = true;
            }
        }
        self.shared.wake.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn pair() -> (Arc<Shared>, TcpStream) {
        let shared = Shared::new(Arc::new(Notify::new()));
        let stream = TcpStream::new(
            shared.clone(),
            "10.0.0.2:5000".parse().unwrap(),
            "1.2.3.4:80".parse().unwrap(),
        );
        (shared, stream)
    }

    #[tokio::test]
    async fn reads_staged_bytes_then_eof() {
        let (shared, mut s) = pair();
        {
            let mut st = shared.state.lock();
            st.rx.extend_from_slice(b"hello");
            st.rx_eof = true;
        }
        let mut out = Vec::new();
        s.read_to_end(&mut out).await.unwrap();
        assert_eq!(out, b"hello");
    }

    #[tokio::test]
    async fn read_waits_and_wakes() {
        let (shared, mut s) = pair();
        let reader = tokio::spawn(async move {
            let mut buf = [0u8; 8];
            let n = s.read(&mut buf).await.unwrap();
            buf[..n].to_vec()
        });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        let w = {
            let mut st = shared.state.lock();
            st.rx.extend_from_slice(b"abc");
            st.wake_reader()
        };
        wake(w);
        assert_eq!(reader.await.unwrap(), b"abc");
    }

    #[tokio::test]
    async fn reading_drained_staging_asks_for_more() {
        let (shared, mut s) = pair();
        {
            let mut st = shared.state.lock();
            st.rx.extend_from_slice(b"x");
            st.rx_pending = true;
        }
        let mut b = [0u8; 4];
        assert_eq!(s.read(&mut b).await.unwrap(), 1);
        assert!(!shared.state.lock().rx_pending);
        // The server task was notified (a permit is stored).
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            shared.wake.notified(),
        )
        .await
        .expect("server task woken");
    }

    #[tokio::test]
    async fn writes_apply_backpressure() {
        let (shared, mut s) = pair();
        let big = vec![7u8; STAGING_LIMIT + 10];
        let n = s.write(&big).await.unwrap();
        assert_eq!(n, STAGING_LIMIT);
        let blocked =
            tokio::time::timeout(std::time::Duration::from_millis(50), s.write(b"more")).await;
        assert!(
            blocked.is_err(),
            "writes beyond the staging limit must wait"
        );
        let w = {
            let mut st = shared.state.lock();
            st.tx.advance(1000);
            st.wake_writer()
        };
        wake(w);
        assert_eq!(s.write(b"more").await.unwrap(), 4);
    }

    #[tokio::test]
    async fn reset_and_shutdown() {
        let (shared, mut s) = pair();
        s.shutdown().await.unwrap();
        assert!(shared.state.lock().tx_shutdown);
        assert_eq!(
            s.write(b"x").await.unwrap_err().kind(),
            io::ErrorKind::BrokenPipe
        );
        shared.end();
        let mut b = [0u8; 1];
        assert_eq!(
            s.read(&mut b).await.unwrap_err().kind(),
            io::ErrorKind::ConnectionReset
        );
    }

    #[test]
    fn drop_requests_close_or_abort() {
        let (shared, s) = pair();
        drop(s);
        let st = shared.state.lock();
        assert!(st.dropped && !st.abort);
        drop(st);
        let (shared, s) = pair();
        s.reset_flag().store(true, Ordering::Relaxed);
        drop(s);
        assert!(shared.state.lock().abort);
    }
}
