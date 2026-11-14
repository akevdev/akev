//! Connections handed to the proxy by another component that already knows where they were
//! going — the WireGuard server's userspace TCP stack. They are served exactly like the
//! inside of a CONNECT tunnel to the original destination: protocol sniffing, TLS MITM with
//! the client's SNI (or passthrough), HTTP/1 + HTTP/2 parsing, rules, recording, and raw
//! tunnelling for anything else.

use crate::conn::{self, ConnCtx};
use crate::io::ByteCounters;
use crate::shared::CounterGuard;
use crate::target::HostPort;
use crate::upstream::BoxIo;
use akev_core::ProxyMode;
use bytes::BytesMut;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// A client connection with a known original destination (see
/// [`crate::ProxyHandle::serve_tunnelled`]).
pub struct TunnelledConnection {
    /// The client's byte stream.
    pub stream: BoxIo,
    /// The client's address (recorded as `ClientInfo::remote_addr`).
    pub client: SocketAddr,
    /// Where the client was connecting to (IP and port).
    pub destination: SocketAddr,
    /// How the connection reached Akev (`ClientInfo::via`).
    pub via: ProxyMode,
    /// Interceptor id recorded for its exchanges (`ClientInfo::interceptor`, `source:`).
    pub interceptor: Option<String>,
    /// Set by the proxy when the client connection must be closed with a TCP RST (`reset`
    /// rules, breakpoint resets); the stream's owner applies it when the stream is dropped.
    pub reset: Arc<AtomicBool>,
}

impl crate::ProxyHandle {
    /// Serve a connection whose destination is already known, as if the client had opened a
    /// CONNECT tunnel to `destination`. Returns `false` (dropping the connection) when the
    /// proxy is shutting down.
    pub fn serve_tunnelled(&self, connection: TunnelledConnection) -> bool {
        let shared = self.inner.shared.clone();
        if shared.shutdown.is_cancelled() {
            return false;
        }
        shared
            .stats
            .total_connections
            .fetch_add(1, Ordering::Relaxed);
        let TunnelledConnection {
            stream,
            client,
            destination,
            via,
            interceptor,
            reset,
        } = connection;
        let host = match destination.ip() {
            IpAddr::V6(v6) => v6
                .to_ipv4_mapped()
                .map(|v4| v4.to_string())
                .unwrap_or_else(|| v6.to_string()),
            IpAddr::V4(v4) => v4.to_string(),
        };
        let target = HostPort::new(host, destination.port());
        let ctx = ConnCtx::tunnelled(client, self.local_addr(), reset, target, via, interceptor);
        let io: BoxIo = Box::new(Metered {
            inner: stream,
            counters: shared.stats.bytes.clone(),
        });
        let task_shared = shared.clone();
        shared.tracker.spawn(async move {
            let _active = CounterGuard::new(&task_shared.stats.active_connections);
            conn::handle(task_shared.clone(), io, BytesMut::new(), ctx).await;
        });
        true
    }
}

/// Counts a client stream's bytes in the proxy's global counters (like accepted sockets).
struct Metered {
    inner: BoxIo,
    counters: Arc<ByteCounters>,
}

impl AsyncRead for Metered {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let res = Pin::new(&mut self.inner).poll_read(cx, buf);
        let n = (buf.filled().len() - before) as u64;
        if n > 0 {
            self.counters.bytes_in.fetch_add(n, Ordering::Relaxed);
        }
        res
    }
}

impl AsyncWrite for Metered {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let res = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = res {
            self.counters
                .bytes_out
                .fetch_add(n as u64, Ordering::Relaxed);
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
