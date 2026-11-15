//! Transparent capture: a dedicated listener for connections the kernel redirected to Akev
//! (nftables / iptables `REDIRECT`), so apps that ignore proxy settings can be captured.
//!
//! Each accepted connection's original destination is read from conntrack
//! ([`KernelOriginalDestination`]: `SO_ORIGINAL_DST` / `IP6T_SO_ORIGINAL_DST`, Linux) and the
//! connection is then handled exactly like a `CONNECT` tunnel to it: the first bytes are
//! sniffed, TLS is intercepted with the ClientHello's SNI (or passed through per the
//! `tls_passthrough` patterns / when not interceptable), plain HTTP is parsed, anything else is
//! piped to the original destination untouched. Requests are recorded with
//! `via: transparent`; their URL uses the `Host` header / SNI name with the original port.
//!
//! Connections made to the port directly (not redirected) are handled like direct requests on
//! the main port (`Host` header / SNI decide the destination).
//!
//! Loop protection: the redirect rules must exclude Akev's own upstream connections (by user
//! id, cgroup, or the firewall mark set on them, see [`mark`]); a connection that comes back
//! from one of Akev's own sockets anyway is refused and counted in
//! [`TransparentStatus::loops`].
//!
//! The listener binds the loopback addresses (`127.0.0.1` and `::1`: locally redirected
//! connections arrive there) plus the main proxy's bind address, on
//! `settings.proxy.transparent.port`.

pub(crate) mod mark;
pub mod origdst;

use crate::conn::{self, ConnCtx};
use crate::io::{ClientIo, ClientTcp};
use crate::server::{PORT_FALLBACK_RANGE, accept_error_is_transient, port_busy};
use crate::shared::{CounterGuard, Shared};
use crate::target::HostPort;
use akev_core::ProxyMode;
use akev_core::transparent::{TransparentConnection, TransparentStatus, now_ms};
use bytes::BytesMut;
use parking_lot::{Mutex, RwLock};
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

pub use origdst::parse_sockaddr;

/// How many recent connections the status keeps.
const RECENT: usize = 20;

/// Looks up where a connection accepted on the transparent listener was really going.
///
/// Implemented by [`KernelOriginalDestination`] (the default) and by any
/// `Fn(peer, local) -> io::Result<Option<SocketAddr>>` closure, which tests use to simulate
/// kernel redirects without firewall rules (see `ProxyHandle::set_original_destination_lookup`).
pub trait OriginalDestination: Send + Sync + 'static {
    /// `Ok(Some(addr))` = the destination before the redirect. `Ok(None)` (or the listener's
    /// own address) = the connection wasn't redirected. Errors are logged and treated as
    /// "not redirected".
    fn original_destination(
        &self,
        stream: &TcpStream,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> io::Result<Option<SocketAddr>>;
}

impl<F> OriginalDestination for F
where
    F: Fn(SocketAddr, SocketAddr) -> io::Result<Option<SocketAddr>> + Send + Sync + 'static,
{
    fn original_destination(
        &self,
        _stream: &TcpStream,
        peer: SocketAddr,
        local: SocketAddr,
    ) -> io::Result<Option<SocketAddr>> {
        self(peer, local)
    }
}

/// The kernel's record of the pre-redirect destination (Linux netfilter conntrack). Always
/// "not redirected" on other platforms.
#[derive(Clone, Copy, Debug, Default)]
pub struct KernelOriginalDestination;

impl OriginalDestination for KernelOriginalDestination {
    fn original_destination(
        &self,
        stream: &TcpStream,
        _peer: SocketAddr,
        local: SocketAddr,
    ) -> io::Result<Option<SocketAddr>> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            origdst::original_destination(stream.as_raw_fd(), local)
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = (stream, local);
            Ok(None)
        }
    }
}

/// Whether original destinations can be read on this platform.
pub const SUPPORTED: bool = cfg!(target_os = "linux");

/// What the listener should look like (from the settings).
#[derive(Clone, Debug, PartialEq, Eq)]
struct Want {
    enabled: bool,
    port: u16,
    bind: String,
}

/// Observable listener state.
#[derive(Clone, Debug, Default)]
struct ListenState {
    enabled: bool,
    requested_port: u16,
    port: Option<u16>,
    addresses: Vec<SocketAddr>,
    error: Option<String>,
    mark: u32,
    mark_error: Option<String>,
}

#[derive(Default)]
struct Counters {
    connections: AtomicU64,
    redirected: AtomicU64,
    direct: AtomicU64,
    loops: AtomicU64,
    /// Unix ms of the last redirected connection (0 = none).
    last_redirect_ms: AtomicU64,
}

/// State shared with the accept loops.
struct Core {
    lookup: RwLock<Arc<dyn OriginalDestination>>,
    counters: Counters,
    /// `(sequence, connection)`, newest first.
    recent: Mutex<VecDeque<(u64, TransparentConnection)>>,
    next_seq: AtomicU64,
    state: Mutex<ListenState>,
    /// Last loop warning (rate-limits the log line).
    loop_warned: Mutex<Option<Instant>>,
}

impl Core {
    fn push_recent(&self, entry: TransparentConnection) -> u64 {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let mut recent = self.recent.lock();
        recent.push_front((seq, entry));
        recent.truncate(RECENT);
        seq
    }

    fn set_recent_process(&self, seq: u64, process: String) {
        if let Some((_, entry)) = self.recent.lock().iter_mut().find(|(s, _)| *s == seq) {
            entry.process = Some(process);
        }
    }
}

/// A running accept loop.
struct Loop {
    addr: SocketAddr,
    stop: CancellationToken,
    done: Option<oneshot::Receiver<()>>,
}

impl Loop {
    async fn stop(&mut self) {
        self.stop.cancel();
        if let Some(done) = self.done.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), done).await;
        }
    }
}

struct Active {
    want: Want,
    loops: Vec<Loop>,
}

/// Owns the transparent listener of one proxy: starts, moves and stops it as the settings
/// change, and keeps its counters.
pub(crate) struct Manager {
    core: Arc<Core>,
    active: tokio::sync::Mutex<Option<Active>>,
}

impl Manager {
    pub(crate) fn new() -> Self {
        Manager {
            core: Arc::new(Core {
                lookup: RwLock::new(Arc::new(KernelOriginalDestination)),
                counters: Counters::default(),
                recent: Mutex::new(VecDeque::new()),
                next_seq: AtomicU64::new(0),
                state: Mutex::new(ListenState::default()),
                loop_warned: Mutex::new(None),
            }),
            active: tokio::sync::Mutex::new(None),
        }
    }

    pub(crate) fn set_lookup(&self, lookup: Arc<dyn OriginalDestination>) {
        *self.core.lookup.write() = lookup;
    }

    /// Bring the listener in line with the current settings (`proxy.transparent` and the
    /// bind address): start, move or stop it, and (re)configure the upstream socket mark.
    /// A previous bind failure is retried.
    pub(crate) async fn reconcile(&self, shared: &Arc<Shared>) {
        let mut active = self.active.lock().await;
        let settings = shared.settings();
        let t = &settings.proxy.transparent;
        let mark_error = mark::configure(t.mark);
        {
            let mut st = self.core.state.lock();
            st.enabled = t.enabled;
            st.requested_port = t.port;
            st.mark = t.mark;
            st.mark_error = mark_error;
        }
        let want = Want {
            enabled: t.enabled && !shared.shutdown.is_cancelled(),
            port: t.port,
            bind: settings.proxy.bind_address.clone(),
        };
        if let Some(a) = active.as_ref()
            && a.want == want
            && (!want.enabled || !a.loops.is_empty())
        {
            return;
        }
        if let Some(mut old) = active.take() {
            for l in &mut old.loops {
                l.stop().await;
            }
        }
        let mut loops = Vec::new();
        let mut error = None;
        if want.enabled {
            match bind_all(want.port, &want.bind) {
                Ok(listeners) => {
                    for listener in listeners {
                        match spawn_loop(shared.clone(), self.core.clone(), listener) {
                            Ok(l) => loops.push(l),
                            Err(e) => error = Some(format!("transparent listener failed: {e}")),
                        }
                    }
                }
                Err(e) => {
                    error = Some(format!(
                        "couldn't listen for transparent connections on port {}: {e}",
                        want.port
                    ));
                }
            }
        }
        if let Some(e) = &error {
            tracing::warn!("{e}");
        }
        let addresses: Vec<SocketAddr> = loops.iter().map(|l| l.addr).collect();
        if !addresses.is_empty() {
            tracing::info!(addrs = ?addresses, "transparent capture listening");
        }
        {
            let mut st = self.core.state.lock();
            st.port = addresses.first().map(SocketAddr::port);
            st.addresses = addresses;
            st.error = error;
        }
        *active = Some(Active { want, loops });
    }

    /// Stop listening (proxy shutdown) and stop marking upstream sockets.
    pub(crate) async fn shutdown(&self) {
        let mut active = self.active.lock().await;
        if let Some(mut old) = active.take() {
            for l in &mut old.loops {
                l.stop().await;
            }
        }
        let mut st = self.core.state.lock();
        mark::release(st.mark);
        st.port = None;
        st.addresses.clear();
    }

    pub(crate) fn status(&self) -> TransparentStatus {
        let st = self.core.state.lock().clone();
        let c = &self.core.counters;
        let last = c.last_redirect_ms.load(Ordering::Relaxed);
        TransparentStatus {
            enabled: st.enabled,
            supported: SUPPORTED,
            listening: !st.addresses.is_empty(),
            port: st.port,
            requested_port: st.requested_port,
            addresses: st
                .addresses
                .iter()
                .map(|a| crate::local::display_addr(*a))
                .collect(),
            error: st.error,
            mark: st.mark,
            mark_error: st.mark_error,
            connections: c.connections.load(Ordering::Relaxed),
            redirected: c.redirected.load(Ordering::Relaxed),
            direct: c.direct.load(Ordering::Relaxed),
            loops: c.loops.load(Ordering::Relaxed),
            last_redirect_at: (last > 0).then_some(last as f64),
            recent: self
                .core
                .recent
                .lock()
                .iter()
                .map(|(_, e)| e.clone())
                .collect(),
        }
    }
}

/// Addresses to listen on for a main-proxy bind address: loopback always (locally redirected
/// connections arrive on `127.0.0.1` / `::1`), plus the bind address itself. Returns
/// `(required, optional)`; optional (IPv6) addresses are skipped when unavailable.
fn listen_ips(bind: &str) -> io::Result<(Vec<IpAddr>, Vec<IpAddr>)> {
    let ip = crate::server::parse_bind_ip(bind)?;
    let v4_loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let v6_loopback = IpAddr::V6(Ipv6Addr::LOCALHOST);
    Ok(if ip.is_unspecified() {
        (
            vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)],
            vec![IpAddr::V6(Ipv6Addr::UNSPECIFIED)],
        )
    } else if ip.is_loopback() {
        (vec![v4_loopback], vec![v6_loopback])
    } else {
        (vec![v4_loopback, ip], vec![v6_loopback])
    })
}

fn try_bind(ip: IpAddr, port: u16) -> io::Result<std::net::TcpListener> {
    let addr = SocketAddr::new(ip, port);
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    if ip.is_ipv6() {
        // IPv4 gets its own socket: keep families apart (and `::` from also taking 0.0.0.0).
        socket.set_only_v6(true)?;
    }
    #[cfg(unix)]
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    Ok(socket.into())
}

/// Bind every address on one port. Optional addresses that can't exist here (no IPv6) are
/// skipped; any address whose port is taken fails the whole set.
fn bind_set(
    required: &[IpAddr],
    optional: &[IpAddr],
    port: u16,
) -> io::Result<Vec<std::net::TcpListener>> {
    let mut out = Vec::new();
    let mut port = port;
    for ip in required {
        let l = try_bind(*ip, port)?;
        port = l.local_addr()?.port();
        out.push(l);
    }
    for ip in optional {
        match try_bind(*ip, port) {
            Ok(l) => out.push(l),
            Err(e) if port_busy(&e) => return Err(e),
            Err(e) => tracing::debug!(%ip, error = %e, "skipping transparent listener address"),
        }
    }
    Ok(out)
}

/// Bind the transparent listener sockets, falling back to the next ports when busy.
fn bind_all(port: u16, bind: &str) -> io::Result<Vec<TcpListener>> {
    let (required, optional) = listen_ips(bind)?;
    let std_listeners = if port == 0 {
        // The OS picks the port for the first socket; the others must get the same one.
        let mut last = None;
        let mut found = None;
        for _ in 0..8 {
            match bind_set(&required, &optional, 0) {
                Ok(v) => {
                    found = Some(v);
                    break;
                }
                Err(e) if port_busy(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        found.ok_or_else(|| last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AddrInUse)))?
    } else {
        let mut last = None;
        let mut found = None;
        for p in port..=port.saturating_add(PORT_FALLBACK_RANGE) {
            match bind_set(&required, &optional, p) {
                Ok(v) => {
                    found = Some(v);
                    break;
                }
                Err(e) if port_busy(&e) => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        found.ok_or_else(|| last.unwrap_or_else(|| io::Error::from(io::ErrorKind::AddrInUse)))?
    };
    std_listeners
        .into_iter()
        .map(TcpListener::from_std)
        .collect()
}

fn spawn_loop(shared: Arc<Shared>, core: Arc<Core>, listener: TcpListener) -> io::Result<Loop> {
    let addr = listener.local_addr()?;
    let stop = shared.shutdown.child_token();
    let loop_stop = stop.clone();
    let (done_tx, done_rx) = oneshot::channel();
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                r = listener.accept() => r,
                _ = loop_stop.cancelled() => break,
            };
            match accepted {
                Ok((stream, peer)) => accept(&shared, &core, stream, peer),
                Err(e) if accept_error_is_transient(&e) => {}
                Err(e) => {
                    tracing::warn!(error = %e, "transparent accept failed");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            }
        }
        drop(listener);
        let _ = done_tx.send(());
    });
    Ok(Loop {
        addr,
        stop,
        done: Some(done_rx),
    })
}

fn accept(shared: &Arc<Shared>, core: &Arc<Core>, stream: TcpStream, peer: SocketAddr) {
    let Ok(local) = stream.local_addr() else {
        return;
    };
    let _ = stream.set_nodelay(true);
    core.counters.connections.fetch_add(1, Ordering::Relaxed);
    let task_shared = shared.clone();
    let core = core.clone();
    shared.tracker.spawn(async move {
        let shared = task_shared;
        let _active = CounterGuard::new(&shared.stats.active_connections);
        let original = original_destination(&core, &stream, peer, local);
        // One of Akev's own upstream sockets, as accepted here or as it was sent (the redirect
        // is invisible to the sender, whose remote end is the original destination).
        let own = || {
            shared.upstream.is_own_connection(peer, local)
                || original.is_some_and(|dst| shared.upstream.is_own_connection(peer, dst))
        };
        // One of Akev's own upstream sockets registers itself right after its connect
        // completes, which can be just after we accepted it: check again once it has sent
        // something (or shortly after, for server-speaks-first protocols).
        if !own() {
            let _ = tokio::time::timeout(Duration::from_millis(100), stream.readable()).await;
        }
        if own() {
            refuse_loop(&core, peer);
            return;
        }
        // The client access policy (this machine's own redirected connections always pass).
        if !is_host_address(peer.ip()) && !shared.admit(peer.ip()) {
            return;
        }
        let Some((io, ctx)) = admit(&shared, &core, stream, peer, local, original) else {
            return;
        };
        let client = shared.clients.register(peer.ip());
        let _ = ctx.conn.client_slot.set(client.id);
        tokio::select! {
            _ = conn::handle(shared.clone(), io, BytesMut::new(), ctx) => {}
            _ = client.token.cancelled() => {}
        }
    });
}

fn refuse_loop(core: &Core, peer: SocketAddr) {
    core.counters.loops.fetch_add(1, Ordering::Relaxed);
    let mut warned = core.loop_warned.lock();
    if warned.is_none_or(|at| at.elapsed() > Duration::from_secs(30)) {
        *warned = Some(Instant::now());
        tracing::warn!(
            %peer,
            "transparent capture loop: the redirect rules sent one of Akev's own upstream \
             connections back to it; exclude Akev (meta skuid / meta mark / cgroup). Closing."
        );
    }
}

/// Where a redirected connection was really going; `None` when it wasn't redirected.
fn original_destination(
    core: &Core,
    stream: &TcpStream,
    peer: SocketAddr,
    local: SocketAddr,
) -> Option<SocketAddr> {
    let lookup = core.lookup.read().clone();
    match lookup.original_destination(stream, peer, local) {
        Ok(dst) => origdst::redirected_destination(dst, local),
        Err(e) => {
            tracing::debug!(%peer, error = %e, "original destination lookup failed");
            None
        }
    }
}

/// Record the connection and build its context.
fn admit(
    shared: &Arc<Shared>,
    core: &Arc<Core>,
    stream: TcpStream,
    peer: SocketAddr,
    local: SocketAddr,
    original: Option<SocketAddr>,
) -> Option<(ClientIo, ConnCtx)> {
    shared
        .stats
        .total_connections
        .fetch_add(1, Ordering::Relaxed);
    let at = now_ms();
    match original {
        Some(_) => {
            core.counters.redirected.fetch_add(1, Ordering::Relaxed);
            core.counters
                .last_redirect_ms
                .store(at as u64, Ordering::Relaxed);
        }
        None => {
            core.counters.direct.fetch_add(1, Ordering::Relaxed);
        }
    }
    let seq = core.push_recent(TransparentConnection {
        at,
        client: crate::local::display_addr(peer),
        original_destination: original.map(|d| d.to_string()),
        process: None,
    });
    let reset = Arc::new(AtomicBool::new(false));
    let io: ClientIo = Box::new(ClientTcp::new(
        stream,
        shared.stats.bytes.clone(),
        reset.clone(),
    ));
    let mut ctx = match original {
        Some(dst) => {
            // Process lookup must find the client's socket, whose remote end is the original
            // destination (the redirect is invisible to the client).
            let mut ctx = ConnCtx::new(peer, dst, reset);
            ctx.local = local;
            ctx.enter_tunnel(
                HostPort::new(dst.ip().to_string(), dst.port()),
                ProxyMode::Transparent,
            );
            ctx
        }
        None => {
            // Not redirected: an ordinary proxy client on this port, credentials included.
            let mut ctx = ConnCtx::new(peer, local, reset);
            ctx.authenticated = !shared.settings().access.needs_auth(peer.ip());
            ctx
        }
    };
    // Redirected connections of this machine's own processes are local.
    ctx.remote = !is_host_address(peer.ip());
    if shared.settings().capture.detect_processes && is_host_address(peer.ip()) {
        // Resolve the owning process while the client sends its first bytes (the first
        // exchange awaits the same lookup), and show it in the status.
        let warm = ctx.clone();
        let warm_shared = shared.clone();
        let warm_core = core.clone();
        tokio::spawn(async move {
            if let Some(p) = warm.process(&warm_shared).await {
                warm_core.set_recent_process(seq, p.name);
            }
        });
    }
    Some((io, ctx))
}

/// Whether `ip` is one of this machine's own addresses (loopback, or bindable here). Locally
/// redirected connections keep their original, non-loopback source address. Cached.
pub(crate) fn is_host_address(ip: IpAddr) -> bool {
    let ip = origdst::canonical(SocketAddr::new(ip, 0)).ip();
    if ip.is_loopback() {
        return true;
    }
    if ip.is_unspecified() || ip.is_multicast() {
        return false;
    }
    static CACHE: std::sync::OnceLock<Mutex<HashMap<IpAddr, (Instant, bool)>>> =
        std::sync::OnceLock::new();
    const TTL: Duration = Duration::from_secs(60);
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some((at, local)) = cache.lock().get(&ip)
        && at.elapsed() < TTL
    {
        return *local;
    }
    // Binding succeeds only for addresses assigned to this host.
    let local = std::net::UdpSocket::bind(SocketAddr::new(ip, 0)).is_ok();
    let mut cache = cache.lock();
    if cache.len() > 1024 {
        cache.clear();
    }
    cache.insert(ip, (Instant::now(), local));
    local
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_addresses_always_include_loopback() {
        let lo4 = IpAddr::V4(Ipv4Addr::LOCALHOST);
        let lo6 = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(listen_ips("127.0.0.1").unwrap(), (vec![lo4], vec![lo6]));
        assert_eq!(listen_ips("::1").unwrap(), (vec![lo4], vec![lo6]));
        assert_eq!(
            listen_ips("0.0.0.0").unwrap(),
            (
                vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)],
                vec![IpAddr::V6(Ipv6Addr::UNSPECIFIED)]
            )
        );
        assert_eq!(
            listen_ips("::").unwrap().0,
            vec![IpAddr::V4(Ipv4Addr::UNSPECIFIED)]
        );
        let lan: IpAddr = "192.168.7.9".parse().unwrap();
        assert_eq!(
            listen_ips("192.168.7.9").unwrap(),
            (vec![lo4, lan], vec![lo6])
        );
    }

    #[tokio::test]
    async fn binds_loopback_pair_on_one_port_and_falls_back_when_busy() {
        let listeners = bind_all(0, "127.0.0.1").unwrap();
        let port = listeners[0].local_addr().unwrap().port();
        assert!(
            listeners
                .iter()
                .all(|l| l.local_addr().unwrap().port() == port)
        );
        assert_eq!(listeners[0].local_addr().unwrap().ip(), Ipv4Addr::LOCALHOST);
        if port <= u16::MAX - PORT_FALLBACK_RANGE {
            let next = bind_all(port, "127.0.0.1").unwrap();
            let got = next[0].local_addr().unwrap().port();
            assert!(
                got > port && got <= port + PORT_FALLBACK_RANGE,
                "{port} → {got}"
            );
        }
    }

    #[test]
    fn host_addresses() {
        assert!(is_host_address(IpAddr::V4(Ipv4Addr::LOCALHOST)));
        assert!(is_host_address("::ffff:127.0.0.1".parse().unwrap()));
        assert!(!is_host_address(IpAddr::V4(Ipv4Addr::UNSPECIFIED)));
        // TEST-NET-3 is never assigned to a test machine.
        assert!(!is_host_address("203.0.113.77".parse().unwrap()));
        assert!(!is_host_address("203.0.113.77".parse().unwrap()), "cached");
    }

    #[test]
    fn recent_connections_are_bounded_newest_first() {
        let m = Manager::new();
        let mut last = 0;
        for i in 0..(RECENT as u16 + 5) {
            last = m.core.push_recent(TransparentConnection {
                at: f64::from(i),
                client: format!("127.0.0.1:{i}"),
                original_destination: None,
                process: None,
            });
        }
        m.core.set_recent_process(last, "curl".into());
        m.core.set_recent_process(0, "evicted".into()); // gone: ignored
        let s = m.status();
        assert_eq!(s.recent.len(), RECENT);
        assert_eq!(s.recent[0].client, format!("127.0.0.1:{}", RECENT + 4));
        assert_eq!(s.recent[0].process.as_deref(), Some("curl"));
        assert!(!s.listening && s.port.is_none() && s.last_redirect_at.is_none());
        assert_eq!(s.supported, cfg!(target_os = "linux"));
    }

    #[test]
    fn closures_are_lookups() {
        let lookup: Arc<dyn OriginalDestination> =
            Arc::new(|peer: SocketAddr, _local: SocketAddr| Ok(Some(peer)));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = l.local_addr().unwrap();
            let _c = TcpStream::connect(addr).await.unwrap();
            let (s, peer) = l.accept().await.unwrap();
            assert_eq!(
                lookup.original_destination(&s, peer, addr).unwrap(),
                Some(peer)
            );
            // The kernel lookup on an unredirected socket: not redirected.
            let kernel = KernelOriginalDestination
                .original_destination(&s, peer, addr)
                .unwrap();
            assert_eq!(origdst::redirected_destination(kernel, addr), None);
        });
    }
}
