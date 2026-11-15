//! Listening socket management: bind with port fallback, accept loop, per-connection task
//! spawning, rebind and shutdown.

use crate::conn::{self, ConnCtx};
use crate::io::{ClientIo, ClientTcp};
use crate::shared::{CounterGuard, Shared};
use bytes::BytesMut;
use socket2::{Domain, Protocol, Socket, Type};
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// How many ports after the preferred one are tried when it is busy.
pub(crate) const PORT_FALLBACK_RANGE: u16 = 100;

/// A running accept loop.
pub(crate) struct ListenerHandle {
    pub addr: SocketAddr,
    stop: CancellationToken,
    /// The accept loop hands the listening socket back here when it stops.
    done: Option<oneshot::Receiver<TcpListener>>,
}

impl ListenerHandle {
    /// Stop accepting and take the listening socket back from the accept loop (`None` if it
    /// was already stopped or didn't stop in time). Dropping the result closes the socket.
    pub(crate) async fn stop(&mut self) -> Option<TcpListener> {
        self.stop.cancel();
        let done = self.done.take()?;
        tokio::time::timeout(Duration::from_secs(5), done)
            .await
            .ok()?
            .ok()
    }
}

/// Parse a bind address (`127.0.0.1`, `0.0.0.0`, `::`, `[::1]`, `localhost`, a hostname).
pub(crate) fn parse_bind_ip(s: &str) -> io::Result<IpAddr> {
    let s = s.trim().trim_start_matches('[').trim_end_matches(']');
    if s.is_empty() || s.eq_ignore_ascii_case("localhost") {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    if let Ok(ip) = s.parse() {
        return Ok(ip);
    }
    (s, 0)
        .to_socket_addrs()?
        .next()
        .map(|a| a.ip())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("can't resolve bind address {s:?}"),
            )
        })
}

fn try_bind(ip: IpAddr, port: u16) -> io::Result<TcpListener> {
    let addr = SocketAddr::new(ip, port);
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if ip.is_ipv6() && ip.is_unspecified() {
        // Dual-stack: `::` also accepts IPv4 clients.
        let _ = socket.set_only_v6(false);
    }
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

/// Bind `bind_address:port`, falling back to the next ports (up to +100) when busy.
/// Port 0 lets the OS choose.
pub(crate) fn bind(port: u16, bind_address: &str) -> io::Result<TcpListener> {
    let ip = parse_bind_ip(bind_address)?;
    if port == 0 {
        return try_bind(ip, 0);
    }
    let last = port.saturating_add(PORT_FALLBACK_RANGE);
    let mut last_err = None;
    for p in port..=last {
        match try_bind(ip, p) {
            Ok(l) => return Ok(l),
            Err(e) if port_busy(&e) => last_err = Some(e),
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::new(io::ErrorKind::AddrInUse, "no free port")))
}

/// Errors after which the next port is tried.
pub(crate) fn port_busy(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::AddrInUse | io::ErrorKind::PermissionDenied
    )
}

/// Errors after which `accept` should simply be retried.
pub(crate) fn accept_error_is_transient(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}

/// Start accepting connections on `listener`.
pub(crate) fn spawn_accept_loop(
    shared: Arc<Shared>,
    listener: TcpListener,
) -> io::Result<ListenerHandle> {
    let addr = listener.local_addr()?;
    let stop = shared.shutdown.child_token();
    let (done_tx, done_rx) = oneshot::channel();
    let loop_stop = stop.clone();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                r = listener.accept() => r,
                _ = loop_stop.cancelled() => break,
            };
            match accepted {
                Ok((stream, peer)) => spawn_connection(&shared, stream, peer),
                Err(e) if accept_error_is_transient(&e) => {}
                Err(e) => {
                    // Usually EMFILE/ENFILE: back off instead of spinning.
                    tracing::warn!(error = %e, "accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        // Nobody waiting (the handle was dropped): the socket comes back and is closed here.
        let _ = done_tx.send(listener);
    });
    Ok(ListenerHandle {
        addr,
        stop,
        done: Some(done_rx),
    })
}

fn spawn_connection(shared: &Arc<Shared>, stream: TcpStream, peer: SocketAddr) {
    let local = match stream.local_addr() {
        Ok(a) => a,
        Err(_) => return,
    };
    let _ = stream.set_nodelay(true);
    if shared.upstream.is_own_connection(peer, local) {
        // An upstream proxy / PAC / DNS override points back at Akev itself: refuse instead of
        // recursing until file descriptors run out.
        tracing::warn!(%peer, "proxy loop detected (connection from Akev's own upstream socket); closing");
        return;
    }
    // The client access policy: refused clients are closed without a response.
    if !shared.admit(peer.ip()) {
        return;
    }
    shared
        .stats
        .total_connections
        .fetch_add(1, Ordering::Relaxed);
    let reset = Arc::new(AtomicBool::new(false));
    let io: ClientIo = Box::new(ClientTcp::new(
        stream,
        shared.stats.bytes.clone(),
        reset.clone(),
    ));
    let mut ctx = ConnCtx::new(peer, local, reset);
    let loopback = akev_core::access::is_loopback(peer.ip());
    ctx.remote = !loopback;
    ctx.authenticated = !shared.settings().access.needs_auth(peer.ip());
    if loopback && shared.settings().capture.detect_processes {
        // Resolve the owning process while the client is still sending its first request, so
        // the lookup doesn't add latency to it (the first exchange awaits the same result).
        let warm = ctx.clone();
        let warm_shared = shared.clone();
        tokio::spawn(async move {
            let _ = warm.process(&warm_shared).await;
        });
    }
    let task_shared = shared.clone();
    let ip = peer.ip();
    shared.tracker.spawn(async move {
        let _active = CounterGuard::new(&task_shared.stats.active_connections);
        // Cut when the client gets blocked meanwhile.
        let client = task_shared.clients.register(ip);
        let _ = ctx.conn.client_slot.set(client.id);
        tokio::select! {
            _ = conn::handle(task_shared.clone(), io, BytesMut::new(), ctx) => {}
            _ = client.token.cancelled() => {
                tracing::debug!(client = %ip, "closing a connection of a client that is now refused");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_address_parsing() {
        assert_eq!(
            parse_bind_ip("127.0.0.1").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            parse_bind_ip("localhost").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            parse_bind_ip("[::1]").unwrap(),
            "::1".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            parse_bind_ip("0.0.0.0").unwrap(),
            "0.0.0.0".parse::<IpAddr>().unwrap()
        );
    }

    #[tokio::test]
    async fn falls_back_to_next_free_port() {
        let first = bind(0, "127.0.0.1").unwrap();
        let busy = first.local_addr().unwrap().port();
        if busy > u16::MAX - PORT_FALLBACK_RANGE {
            return;
        }
        let second = bind(busy, "127.0.0.1").unwrap();
        let got = second.local_addr().unwrap().port();
        assert!(
            got > busy && got <= busy + PORT_FALLBACK_RANGE,
            "{busy} → {got}"
        );
    }
}
