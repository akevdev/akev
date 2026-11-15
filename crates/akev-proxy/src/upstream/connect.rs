//! Connection establishment: DNS → TCP (happy eyeballs, RFC 8305-lite) → upstream proxy
//! handshake (CONNECT / SOCKS, TLS to HTTPS proxies) → TLS to the origin, each stage bounded
//! by the remaining `connect_timeout` budget and timed separately.

use super::config::Config;
use super::error::{as_proxy_failure, from_io, io_rank, timeout as timeout_error};
use super::fingerprint::{HelloRecorder, Selection};
use super::io::PrefixedIo;
use super::proxy::tunnel::{SocksAddr, http_connect, socks_connect};
use super::proxy::{ProxyKind, Route, host_port};
use super::tls::{self, Alpn};
use super::{BoxIo, ConnectTimings, UpstreamError};
use akev_core::UpstreamInfo;
use dashmap::DashMap;
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use rustls::ClientConfig;
use rustls::pki_types::ServerName;
use std::future::Future;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpSocket, TcpStream};
use tokio::time::Instant;
use tokio_rustls::TlsConnector;

/// Delay before starting the next connection attempt (RFC 8305 "Connection Attempt Delay").
pub(crate) const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// The live outgoing TCP connections opened by this process, as `(local, remote)` pairs.
///
/// The local endpoint alone is not enough: Linux gives one ephemeral port to connections to
/// different destinations, so an unrelated client can connect to the proxy from the very
/// address one of our upstream sockets uses. The count is defensive; a live pair is unique.
fn connections() -> &'static DashMap<(SocketAddr, SocketAddr), u32> {
    static CONNECTIONS: OnceLock<DashMap<(SocketAddr, SocketAddr), u32>> = OnceLock::new();
    CONNECTIONS.get_or_init(DashMap::new)
}

/// IPv4-mapped IPv6 addresses compare as IPv4 (dual-stack listeners report them that way).
fn canonical_endpoint(addr: SocketAddr) -> SocketAddr {
    SocketAddr::new(super::proxy::bypass::canonical_ip(addr.ip()), addr.port())
}

/// Whether a connection accepted from `peer` on `local` is a live outgoing connection made by
/// an upstream client in this process — i.e. the proxy accepted a connection from itself (a
/// proxy loop).
pub(crate) fn is_own_connection(peer: SocketAddr, local: SocketAddr) -> bool {
    connections().contains_key(&(canonical_endpoint(peer), canonical_endpoint(local)))
}

/// An outgoing TCP stream registered for loop detection while it lives.
pub(crate) struct TrackedTcp {
    stream: TcpStream,
    key: Option<(SocketAddr, SocketAddr)>,
}

impl TrackedTcp {
    pub(crate) fn new(stream: TcpStream) -> Self {
        // The kernel's own view of the remote end, which is what the accepting side sees as
        // its local address (also when connecting to an unspecified address means loopback).
        let key = match (stream.local_addr(), stream.peer_addr()) {
            (Ok(local), Ok(remote)) => {
                Some((canonical_endpoint(local), canonical_endpoint(remote)))
            }
            _ => None,
        };
        if let Some(key) = key {
            *connections().entry(key).or_insert(0) += 1;
        }
        TrackedTcp { stream, key }
    }
}

impl Drop for TrackedTcp {
    fn drop(&mut self) {
        if let Some(key) = self.key {
            connections().remove_if_mut(&key, |_, count| {
                *count = count.saturating_sub(1);
                *count == 0
            });
        }
    }
}

impl AsyncRead for TrackedTcp {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for TrackedTcp {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[io::IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
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

/// Order addresses for happy eyeballs: interleave families, IPv6 first (RFC 8305 §4).
pub(crate) fn interleave(addrs: &[IpAddr]) -> Vec<IpAddr> {
    let v6: Vec<IpAddr> = addrs.iter().copied().filter(IpAddr::is_ipv6).collect();
    let v4: Vec<IpAddr> = addrs.iter().copied().filter(IpAddr::is_ipv4).collect();
    let mut out = Vec::with_capacity(addrs.len());
    let (mut a, mut b) = (v6.into_iter(), v4.into_iter());
    loop {
        match (a.next(), b.next()) {
            (None, None) => break,
            (x, y) => out.extend(x.into_iter().chain(y)),
        }
    }
    out
}

fn set_keepalive(stream: &TcpStream) {
    let keepalive = socket2::TcpKeepalive::new().with_time(Duration::from_secs(60));
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "freebsd",
        target_os = "android"
    ))]
    let keepalive = keepalive.with_interval(Duration::from_secs(15));
    if let Err(e) = socket2::SockRef::from(stream).set_tcp_keepalive(&keepalive) {
        tracing::debug!(error = %e, "cannot enable TCP keepalive");
    }
}

async fn connect_addr(addr: SocketAddr) -> (SocketAddr, io::Result<TcpStream>) {
    let result = async {
        let socket = if addr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        socket.set_nodelay(true)?;
        // Firewall mark so transparent-capture redirect rules can skip Akev's own traffic.
        crate::transparent::mark::apply(&socket);
        let stream = socket.connect(addr).await?;
        set_keepalive(&stream);
        Ok(stream)
    }
    .await;
    (addr, result)
}

/// Connect to the first address that answers, starting a new attempt every
/// [`ATTEMPT_DELAY`] (or immediately when one fails). Returns the most informative error if
/// all fail.
pub(crate) async fn happy_eyeballs(
    addrs: &[IpAddr],
    port: u16,
) -> io::Result<(TcpStream, SocketAddr)> {
    let ordered = interleave(addrs);
    let mut pending = ordered.into_iter().map(|ip| SocketAddr::new(ip, port));
    let Some(first) = pending.next() else {
        return Err(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no addresses to connect to",
        ));
    };
    let mut attempts = FuturesUnordered::new();
    attempts.push(connect_addr(first));
    let mut best: Option<io::Error> = None;
    let mut exhausted = false;
    loop {
        let stagger = tokio::time::sleep(ATTEMPT_DELAY);
        tokio::select! {
            Some((addr, result)) = attempts.next() => match result {
                Ok(stream) => return Ok((stream, addr)),
                Err(e) => {
                    tracing::trace!(%addr, error = %e, "connection attempt failed");
                    if best.as_ref().is_none_or(|b| io_rank(&e) >= io_rank(b)) {
                        best = Some(e);
                    }
                    match pending.next() {
                        Some(next) => attempts.push(connect_addr(next)),
                        None => exhausted = true,
                    }
                    if attempts.is_empty() && exhausted {
                        break;
                    }
                }
            },
            _ = stagger, if !exhausted => match pending.next() {
                Some(next) => attempts.push(connect_addr(next)),
                None => exhausted = true,
            },
            else => break,
        }
    }
    Err(best.unwrap_or_else(|| {
        io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "no addresses to connect to",
        )
    }))
}

/// TLS parameters for the origin connection.
pub(crate) struct TlsTarget {
    pub backend: TlsBackend,
    pub server_name: ServerName<'static>,
    /// Certificate verification is skipped (ignored host or insecure request): reported in
    /// [`UpstreamInfo::chain`].
    pub insecure: bool,
    /// The fingerprint chosen (reported in [`UpstreamInfo::fingerprint`]).
    pub selection: Selection,
}

/// Which TLS stack talks to the origin.
pub(crate) enum TlsBackend {
    /// Akev's own (rustls).
    Rustls(Arc<ClientConfig>),
    /// A browser's ClientHello via BoringSSL.
    #[cfg(feature = "impersonate")]
    Boring(super::fingerprint::boring::BoringTarget),
}

/// What to connect to.
pub(crate) struct ConnectRequest<'a> {
    /// Normalised target host.
    pub host: &'a str,
    pub port: u16,
    pub route: &'a Route,
    /// Need a byte tunnel through HTTP proxies (`CONNECT`); otherwise plain-HTTP requests
    /// use absolute-form on the proxy connection itself.
    pub tunnel: bool,
    pub tls: Option<TlsTarget>,
    /// Refuse to connect when the target resolves to a loopback or unspecified address
    /// (requests from other machines, [`super::RequestOptions::restrict_loopback`]).
    pub restrict_loopback: bool,
}

/// An established connection.
pub(crate) struct Established {
    pub io: BoxIo,
    pub info: UpstreamInfo,
    pub timings: ConnectTimings,
    /// ALPN negotiated `h2`.
    pub h2: bool,
    /// Requests must use absolute-form (plain HTTP via an HTTP proxy).
    pub absolute_form: bool,
    /// `Proxy-Authorization` for absolute-form requests.
    pub proxy_auth: Option<String>,
}

fn ms_since(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

/// Run `fut` within the remaining budget.
async fn within<T>(
    deadline: Option<(Instant, Duration)>,
    fut: impl Future<Output = Result<T, UpstreamError>>,
    what: impl FnOnce() -> String,
) -> Result<T, UpstreamError> {
    match deadline {
        Some((at, total)) => match tokio::time::timeout_at(at, fut).await {
            Ok(r) => r,
            Err(_) => Err(timeout_error(what(), total)),
        },
        None => fut.await,
    }
}

/// Establish a connection per `req`.
pub(crate) async fn establish(
    cfg: &Config,
    req: ConnectRequest<'_>,
) -> Result<Established, UpstreamError> {
    let deadline = cfg.connect_timeout.map(|t| (Instant::now() + t, t));
    let peer = host_port(req.host, req.port);
    let mut timings = ConnectTimings::default();
    let mut info = UpstreamInfo::default();
    let mut absolute_form = false;
    let mut proxy_auth = None;

    let mut io: BoxIo = match req.route {
        Route::Direct => {
            let t = Instant::now();
            let addrs = within(deadline, cfg.dns.resolve(req.host), || {
                format!("DNS lookup for {}", req.host)
            })
            .await?;
            timings.dns_ms = Some(ms_since(t));
            if req.restrict_loopback {
                // After DNS too: a name that resolves to this machine is refused.
                super::check_loopback_addrs(req.host, &addrs)?;
            }
            let t = Instant::now();
            let (stream, addr) = within(
                deadline,
                async {
                    happy_eyeballs(&addrs, req.port)
                        .await
                        .map_err(|e| from_io(&e, format_args!("connecting to {peer}")))
                },
                || format!("connecting to {peer}"),
            )
            .await?;
            timings.connect_ms = Some(ms_since(t));
            info.remote_addr = Some(addr.to_string());
            Box::new(TrackedTcp::new(stream))
        }
        Route::Proxy(proxy) => {
            let display = proxy.display.clone();
            let proxy_peer = host_port(&proxy.host, proxy.port);
            info.via_proxy = Some(display.to_string());
            let t = Instant::now();
            let addrs = within(deadline, cfg.dns.resolve(&proxy.host), || {
                format!("DNS lookup for {}", proxy.host)
            })
            .await
            .map_err(|e| as_proxy_failure(e, &display))?;
            let mut dns_ms = ms_since(t);
            let t = Instant::now();
            let (stream, addr) = within(
                deadline,
                async {
                    happy_eyeballs(&addrs, proxy.port)
                        .await
                        .map_err(|e| from_io(&e, format_args!("connecting to {proxy_peer}")))
                },
                || format!("connecting to {proxy_peer}"),
            )
            .await
            .map_err(|e| as_proxy_failure(e, &display))?;
            info.remote_addr = Some(addr.to_string());
            let mut io: BoxIo = Box::new(TrackedTcp::new(stream));
            if proxy.kind == ProxyKind::Https {
                let config = cfg
                    .tls
                    .client_config(
                        cfg.tls.ignores_errors_for(&proxy.host, proxy.port),
                        Alpn::Http1,
                        cfg.tls.client_cert_for(&proxy.host, proxy.port),
                    )
                    .map_err(|e| as_proxy_failure(e, &display))?;
                let name =
                    tls::server_name(&proxy.host).map_err(|e| as_proxy_failure(e, &display))?;
                let stream = within(
                    deadline,
                    async {
                        TlsConnector::from(config)
                            .connect(name, io)
                            .await
                            .map_err(|e| tls::map_handshake_error(&e, &proxy_peer))
                    },
                    || format!("TLS handshake with {proxy_peer}"),
                )
                .await
                .map_err(|e| as_proxy_failure(e, &display))?;
                io = Box::new(stream);
            }
            match proxy.kind {
                ProxyKind::Http | ProxyKind::Https if !req.tunnel => {
                    absolute_form = true;
                    proxy_auth = proxy.basic_auth();
                }
                ProxyKind::Http | ProxyKind::Https => {
                    let leftover = within(
                        deadline,
                        http_connect(&mut io, proxy, req.host, req.port),
                        || format!("opening a tunnel to {peer} through {display}"),
                    )
                    .await
                    .map_err(|e| as_proxy_failure(e, &display))?;
                    if !leftover.is_empty() {
                        io = Box::new(PrefixedIo::new(leftover, io));
                    }
                }
                ProxyKind::Socks4 | ProxyKind::Socks4a | ProxyKind::Socks5 | ProxyKind::Socks5h => {
                    let remote_dns = matches!(proxy.kind, ProxyKind::Socks4a | ProxyKind::Socks5h);
                    let resolved;
                    let addr = match super::dns::parse_ip(req.host) {
                        Some(ip) => SocksAddr::Ip(ip),
                        None if remote_dns => SocksAddr::Domain(req.host),
                        None => {
                            let t = Instant::now();
                            resolved = within(deadline, cfg.dns.resolve(req.host), || {
                                format!("DNS lookup for {}", req.host)
                            })
                            .await?;
                            dns_ms += ms_since(t);
                            let ip = resolved
                                .iter()
                                .find(|ip| ip.is_ipv4())
                                .or(resolved.first())
                                .copied()
                                .ok_or_else(|| super::error::internal("empty DNS answer"))?;
                            SocksAddr::Ip(ip)
                        }
                    };
                    within(
                        deadline,
                        socks_connect(&mut io, proxy, addr, req.port),
                        || format!("SOCKS handshake with {display}"),
                    )
                    .await
                    .map_err(|e| match e.kind {
                        // Target-side failures reported by the proxy keep their own kind.
                        akev_core::ErrorKind::ConnectionRefused
                        | akev_core::ErrorKind::HostUnreachable => e,
                        _ => as_proxy_failure(e, &display),
                    })?;
                }
            }
            timings.dns_ms = Some(dns_ms);
            timings.connect_ms = Some(ms_since(t));
            io
        }
    };

    let mut h2 = false;
    if let Some(target) = req.tls {
        let t = Instant::now();
        // Records the ClientHello actually sent, for the JA3/JA4 in the exchange.
        let recorder = HelloRecorder::new(io);
        let selection = target.selection;
        let insecure = target.insecure;
        match target.backend {
            TlsBackend::Rustls(config) => {
                let stream = within(
                    deadline,
                    async {
                        TlsConnector::from(config)
                            .connect(target.server_name, recorder)
                            .await
                            .map_err(|e| tls::map_handshake_error(&e, &peer))
                    },
                    || format!("TLS handshake with {peer}"),
                )
                .await?;
                let (recorder, conn) = stream.get_ref();
                tls::fill_info(&mut info, conn, &cfg.tls, insecure);
                info.fingerprint = Some(Arc::new(selection.describe(recorder.hello())));
                h2 = conn.alpn_protocol() == Some(b"h2");
                io = Box::new(stream);
            }
            #[cfg(feature = "impersonate")]
            TlsBackend::Boring(boring_target) => {
                use super::fingerprint::boring;
                let stream = within(
                    deadline,
                    boring::connect(boring_target, req.host, target.server_name, recorder, &peer),
                    || format!("TLS handshake with {peer}"),
                )
                .await?;
                boring::fill_info(&mut info, &stream, &cfg.tls, insecure);
                info.fingerprint = Some(Arc::new(selection.describe(stream.get_ref().hello())));
                h2 = boring::negotiated_h2(&stream);
                io = Box::new(stream);
            }
        }
        timings.tls_ms = Some(ms_since(t));
    }
    Ok(Established {
        io,
        info,
        timings,
        h2,
        absolute_form,
        proxy_auth,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_families_v6_first() {
        let addrs: Vec<IpAddr> = ["1.1.1.1", "2.2.2.2", "::1", "3.3.3.3", "::2"]
            .iter()
            .map(|s| s.parse().unwrap())
            .collect();
        let out: Vec<String> = interleave(&addrs).iter().map(|ip| ip.to_string()).collect();
        assert_eq!(out, ["::1", "1.1.1.1", "::2", "2.2.2.2", "3.3.3.3"]);
        assert!(interleave(&[]).is_empty());
    }

    #[tokio::test]
    async fn tracked_streams_register_their_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stream = TcpStream::connect(addr).await.unwrap();
        let (accepted, peer) = listener.accept().await.unwrap();
        assert!(!is_own_connection(peer, addr));
        let tracked = TrackedTcp::new(stream);
        // The accepting side sees our connection as `peer` → `addr`, also when a dual-stack
        // listener reports both as IPv4-mapped addresses.
        assert!(is_own_connection(peer, addr));
        let mapped = |a: SocketAddr| match a.ip() {
            IpAddr::V4(v4) => SocketAddr::new(IpAddr::V6(v4.to_ipv6_mapped()), a.port()),
            IpAddr::V6(_) => a,
        };
        assert!(is_own_connection(mapped(peer), mapped(addr)));
        // The accepted socket itself is not one of ours.
        assert!(!is_own_connection(addr, peer));
        drop(tracked);
        assert!(!is_own_connection(peer, addr));
        drop(accepted);
    }

    /// Linux gives one ephemeral port to connections to different destinations: a client
    /// sharing the local endpoint of one of our upstream sockets is not a loop.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_shared_local_endpoint_is_not_a_loop() {
        use tokio::net::{TcpListener, TcpSocket};
        let ours = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let theirs = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let socket = |local: SocketAddr| {
            let s = TcpSocket::new_v4().unwrap();
            s.set_reuseaddr(true).unwrap();
            s.bind(local).unwrap();
            s
        };
        let first = socket("127.0.0.1:0".parse().unwrap());
        let local = first.local_addr().unwrap();
        let tracked = TrackedTcp::new(first.connect(ours.local_addr().unwrap()).await.unwrap());
        let other = socket(local)
            .connect(theirs.local_addr().unwrap())
            .await
            .unwrap();
        let (_a, ours_peer) = ours.accept().await.unwrap();
        let (_b, theirs_peer) = theirs.accept().await.unwrap();
        assert_eq!(ours_peer, theirs_peer, "both connections come from {local}");
        assert!(is_own_connection(ours_peer, ours.local_addr().unwrap()));
        assert!(!is_own_connection(
            theirs_peer,
            theirs.local_addr().unwrap()
        ));
        drop((tracked, other));
    }

    #[tokio::test]
    async fn falls_back_to_working_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        // ::1 is refused (nothing listens there on this port) or unreachable; 127.0.0.1 works.
        let addrs: Vec<IpAddr> = vec!["::1".parse().unwrap(), "127.0.0.1".parse().unwrap()];
        let (_stream, addr) = happy_eyeballs(&addrs, port).await.unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
    }

    #[tokio::test]
    async fn reports_refused() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let err = happy_eyeballs(&["127.0.0.1".parse().unwrap()], port)
            .await
            .unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn staggers_past_blackholed_address() {
        // 10.255.255.1 is (normally) unroutable/black-holed: the second address must win
        // after the attempt delay instead of waiting for the first attempt's OS timeout.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let addrs: Vec<IpAddr> = vec![
            "10.255.255.1".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
        ];
        let started = std::time::Instant::now();
        let (_s, addr) = tokio::time::timeout(Duration::from_secs(5), happy_eyeballs(&addrs, port))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(addr.ip().to_string(), "127.0.0.1");
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
