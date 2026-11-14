//! HTTP/3 client (quinn + h3): requests to HTTPS origins over QUIC.
//!
//! * **When**: per [`UpstreamHttpVersion`] on the request and, for `auto`, the
//!   `ProxySettings::http3.upstream` mode — `off`, `alt-svc` (origins that advertised `h3` in an
//!   `Alt-Svc` response header, cached for its `ma`, as browsers do) or `prefer` (every HTTPS
//!   origin). Never through an upstream proxy (QUIC can't be tunnelled through HTTP CONNECT),
//!   never for upgrades (WebSocket) or `http1_only` requests.
//! * **Fallback**: a QUIC connection that can't be established (UDP blocked, no listener,
//!   handshake or certificate failure) or a request the server refused before processing falls
//!   back to TCP (HTTP/2 or HTTP/1.1) with the untouched request body, and the origin is marked
//!   "broken" for a while (5 min, doubling up to 1 h) so later requests go straight to TCP.
//!   Automatic attempts give the handshake at most [`AUTO_HANDSHAKE_TIMEOUT`]; a request that
//!   explicitly asked for HTTP/3 gets the full connect timeout.
//! * **TLS**: the same verification as TCP connections (trusted CA files, ignore-errors host
//!   list / insecure requests, client certificates, precise certificate errors) — the rustls
//!   config of the TCP path with ALPN `h3` and its own session cache.
//! * **Connections**: one multiplexed QUIC connection per (origin, alternative, TLS mode),
//!   shared by concurrent requests (the first request's connect is awaited by the others),
//!   closed by QUIC's idle timeout. One client UDP socket per address family, created lazily.
//! * **Info / timings**: `UpstreamInfo { alpn: "h3", tls_version: "TLSv1.3", certificate, ... }`,
//!   responses with `HttpVersion::H3`. The QUIC handshake (transport + TLS in one round trip)
//!   is reported as `connect_ms`; there is no separate TLS phase.

use super::alt_svc::{H3Knowledge, H3Route};
use super::config::Config;
use super::connect::{ATTEMPT_DELAY, interleave};
use super::error::{error, internal, timeout as timeout_error};
use super::proxy::{Route, host_port};
use super::tls::Alpn;
use super::{
    BodyChunk, BodyStream, ConnectTimings, Plan, RequestBody, UpstreamError, UpstreamResponse,
    cert, info_with_reuse, is_upgrade_request,
};
use akev_core::settings::Http3UpstreamMode;
use akev_core::{ErrorKind, Headers, HttpVersion, ResponseHead, UpstreamInfo, canonical_reason};
use bytes::{Buf, Bytes};
use futures::StreamExt;
use futures::stream::FuturesUnordered;
use h3::error::{Code, ConnectionError as H3ConnError, StreamError};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
use std::time::Duration;
use tokio::time::Instant;

/// Handshake budget for automatic (`alt-svc` / `prefer`) attempts: a network that silently
/// drops UDP must not stall requests for the whole connect timeout before TCP is tried.
pub(crate) const AUTO_HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// Largest response header section accepted.
const MAX_FIELD_SECTION: u64 = 256 * 1024;
/// Most attempts on fresh connections when a connection turns out to be closing.
const MAX_ATTEMPTS: usize = 2;
/// Requests in flight on one pooled connection before a new connection is opened for others.
const MAX_STREAMS_PER_CONNECTION: usize = 64;
/// How long a failed connect is reused for requests that queued behind it.
const CONNECT_ERROR_TTL: Duration = Duration::from_secs(5);

type SendRequest = h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>;
type TxStream = h3::client::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>;
type RxStream = h3::client::RequestStream<h3_quinn::RecvStream, Bytes>;

/// HTTP version for a request Akev sends upstream (Compose / Repeat: `SendOptions`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum UpstreamHttpVersion {
    /// Settings decide: HTTP/2 when the server offers it via ALPN, HTTP/3 per
    /// `ProxySettings::http3.upstream`.
    #[default]
    Auto,
    /// HTTP/1.1 only.
    Http1,
    /// TCP: HTTP/2 when the server offers it (else HTTP/1.1); never HTTP/3.
    Http2,
    /// HTTP/3 first, whatever the settings say; falls back to TCP when QUIC fails (like
    /// `curl --http3`).
    Http3,
}

/// Ports of the client UDP sockets of every live HTTP/3 client in this process (loop
/// detection: Akev's QUIC listener must not serve its own upstream connections).
fn own_ports() -> &'static Mutex<HashMap<u16, u32>> {
    static PORTS: OnceLock<Mutex<HashMap<u16, u32>>> = OnceLock::new();
    PORTS.get_or_init(Default::default)
}

/// Whether `peer` (the source of a QUIC packet received by the inbound listener on
/// `local_ip`) is one of this process's own HTTP/3 client sockets: same machine (loopback, or
/// the packet's own destination address) and a port of a live client socket.
pub(crate) fn is_own_endpoint(peer: SocketAddr, local_ip: Option<IpAddr>) -> bool {
    let canonical = |ip: IpAddr| super::proxy::bypass::canonical_ip(ip);
    let ip = canonical(peer.ip());
    let same_host = ip.is_loopback() || local_ip.map(canonical) == Some(ip);
    same_host && own_ports().lock().contains_key(&peer.port())
}

/// Whether the OS has a route to `addr` (connecting a UDP socket sends nothing).
fn routable(addr: SocketAddr) -> bool {
    let bind: SocketAddr = if addr.is_ipv6() {
        (Ipv6Addr::UNSPECIFIED, 0).into()
    } else {
        (Ipv4Addr::UNSPECIFIED, 0).into()
    };
    std::net::UdpSocket::bind(bind)
        .and_then(|s| s.connect(addr))
        .is_ok()
}

/// A client UDP socket (one per address family).
struct ClientEndpoint {
    endpoint: quinn::Endpoint,
    port: u16,
}

impl ClientEndpoint {
    fn bind(v6: bool) -> Result<Self, String> {
        let addr: SocketAddr = if v6 {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        };
        let endpoint = quinn::Endpoint::client(addr).map_err(|e| e.to_string())?;
        let port = endpoint.local_addr().map(|a| a.port()).unwrap_or(0);
        *own_ports().lock().entry(port).or_insert(0) += 1;
        Ok(ClientEndpoint { endpoint, port })
    }
}

impl Drop for ClientEndpoint {
    fn drop(&mut self) {
        let mut ports = own_ports().lock();
        if let Some(n) = ports.get_mut(&self.port) {
            *n -= 1;
            if *n == 0 {
                ports.remove(&self.port);
            }
        }
        self.endpoint.close(0u32.into(), b"");
    }
}

#[derive(Default)]
struct Endpoints {
    v4: Option<Result<Arc<ClientEndpoint>, String>>,
    v6: Option<Result<Arc<ClientEndpoint>, String>>,
}

/// Pool key: one connection per origin name, alternative and TLS mode.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct H3Key {
    generation: u64,
    /// Origin host: TLS server name and certificate identity.
    origin: Arc<str>,
    /// Where to connect (the origin, or the host of an `Alt-Svc` alternative).
    host: Arc<str>,
    port: u16,
    insecure: bool,
    client_cert: Option<usize>,
}

/// A shared HTTP/3 connection.
#[derive(Clone)]
struct H3Handle {
    send: SendRequest,
    quic: quinn::Connection,
    info: Arc<UpstreamInfo>,
    peer: Arc<str>,
    closed: Arc<AtomicBool>,
    /// Requests in flight on the connection.
    active: Arc<AtomicUsize>,
}

impl H3Handle {
    fn usable(&self) -> bool {
        !self.closed.load(Ordering::Acquire) && self.quic.close_reason().is_none()
    }

    /// Whether another request can start without waiting for a stream (servers commonly
    /// allow 100 concurrent streams; long-lived requests such as SSE must not block others).
    fn has_capacity(&self) -> bool {
        self.active.load(Ordering::Acquire) < MAX_STREAMS_PER_CONNECTION
    }
}

/// Counts a request in flight on a connection while alive.
struct ActiveGuard(Arc<AtomicUsize>);

impl ActiveGuard {
    fn new(handle: &H3Handle) -> Self {
        handle.active.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(handle.active.clone())
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Default)]
struct PoolState {
    conns: HashMap<H3Key, H3Handle>,
    locks: HashMap<H3Key, Arc<tokio::sync::Mutex<()>>>,
    /// Last connect failure per key (waiters that queued behind it reuse it).
    errors: HashMap<H3Key, (Instant, UpstreamError)>,
}

type QuicConfigKey = (u64, bool, Option<usize>);

/// HTTP/3 state of an upstream client.
pub(crate) struct H3Client {
    pub(crate) knowledge: H3Knowledge,
    endpoints: Mutex<Endpoints>,
    pool: Arc<Mutex<PoolState>>,
    configs: Mutex<HashMap<QuicConfigKey, quinn::ClientConfig>>,
    last_mode: Mutex<Http3UpstreamMode>,
}

impl Default for H3Client {
    fn default() -> Self {
        H3Client {
            knowledge: H3Knowledge::default(),
            endpoints: Mutex::default(),
            pool: Arc::default(),
            configs: Mutex::default(),
            last_mode: Mutex::new(Http3UpstreamMode::Off),
        }
    }
}

/// Where and how to try HTTP/3 for one request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub route: H3Route,
    /// The request explicitly asked for HTTP/3.
    pub forced: bool,
}

/// Result of an HTTP/3 attempt.
pub(crate) enum H3Outcome {
    Done(Box<UpstreamResponse>),
    /// Use TCP instead (nothing of the body was consumed).
    Fallback(RequestBody),
    /// Failed after the request reached the server.
    Failed(UpstreamError),
}

impl H3Client {
    /// Whether (and where) this request should try HTTP/3 first.
    pub(crate) fn candidate(
        &self,
        cfg: &Config,
        plan: &Plan<'_>,
        version: UpstreamHttpVersion,
        http1_only: bool,
        routes: &[Route],
    ) -> Option<Candidate> {
        let mode = cfg.settings.http3.upstream;
        {
            let mut last = self.last_mode.lock();
            if *last != mode {
                // A changed mode deserves a fresh try for origins that failed before.
                *last = mode;
                self.knowledge.clear_broken();
            }
        }
        let target = plan.target;
        if target.scheme != "https" || http1_only || is_upgrade_request(plan.head) {
            return None;
        }
        // QUIC can't go through an upstream proxy.
        if !matches!(routes, [Route::Direct]) {
            return None;
        }
        let advertised = || self.knowledge.advertised(&target.host, target.port);
        let origin = || H3Route {
            host: target.host.clone(),
            port: target.port,
        };
        let forced = match version {
            UpstreamHttpVersion::Http1 | UpstreamHttpVersion::Http2 => return None,
            UpstreamHttpVersion::Http3 => true,
            UpstreamHttpVersion::Auto => false,
        };
        if forced {
            return Some(Candidate {
                route: advertised().unwrap_or_else(origin),
                forced: true,
            });
        }
        if self.knowledge.is_broken(&target.host, target.port) {
            return None;
        }
        let route = match mode {
            Http3UpstreamMode::Off => return None,
            Http3UpstreamMode::AltSvc => advertised()?,
            Http3UpstreamMode::Prefer => advertised().unwrap_or_else(origin),
        };
        Some(Candidate {
            route,
            forced: false,
        })
    }

    /// Record the `Alt-Svc` header of a response from an HTTPS origin.
    pub(crate) fn observe(&self, host: &str, port: u16, tls: bool, headers: &Headers) {
        if tls {
            self.knowledge
                .observe(host, port, headers.get_all("alt-svc"));
        }
    }

    /// Send the request over HTTP/3.
    pub(crate) async fn send(
        &self,
        cfg: &Arc<Config>,
        plan: &Plan<'_>,
        candidate: &Candidate,
        body: RequestBody,
    ) -> H3Outcome {
        let target = plan.target;
        let key = H3Key {
            generation: cfg.generation,
            origin: Arc::from(target.host.as_str()),
            host: Arc::from(candidate.route.host.as_str()),
            port: candidate.route.port,
            insecure: plan.insecure,
            client_cert: plan.client_cert,
        };
        let handshake_timeout = if candidate.forced {
            cfg.connect_timeout
        } else {
            Some(
                cfg.connect_timeout
                    .map_or(AUTO_HANDSHAKE_TIMEOUT, |t| t.min(AUTO_HANDSHAKE_TIMEOUT)),
            )
        };
        let mut body = body;
        let mut force_new = false;
        for _ in 0..MAX_ATTEMPTS {
            let (handle, reused, timings) =
                match self.acquire(cfg, &key, handshake_timeout, force_new).await {
                    Ok(acquired) => acquired,
                    Err(e) => {
                        let backoff = self.knowledge.mark_broken(&target.host, target.port);
                        tracing::debug!(
                            origin = %key.origin,
                            alt = %host_port(&key.host, key.port),
                            error = %e,
                            retry_in_s = backoff.as_secs(),
                            "HTTP/3 unavailable; falling back to TCP"
                        );
                        return H3Outcome::Fallback(body);
                    }
                };
            match send_on(&handle, plan, body).await {
                Ok(out) => {
                    self.knowledge.mark_working(&target.host, target.port);
                    self.observe(&target.host, target.port, true, &out.head.headers);
                    return H3Outcome::Done(Box::new(UpstreamResponse {
                        head: out.head,
                        body: out.body,
                        info: info_with_reuse(&handle.info, reused),
                        timings: if reused {
                            ConnectTimings::default()
                        } else {
                            timings
                        },
                        request_sent_ms: out.request_sent_ms,
                    }));
                }
                Err(SendError::Retry { error, body: b }) => {
                    tracing::debug!(peer = %handle.peer, %error, "HTTP/3 request not processed; retrying on a new connection");
                    self.remove(&key, &handle);
                    body = b;
                    force_new = true;
                }
                Err(SendError::Fallback { error, body: b }) => {
                    tracing::debug!(peer = %handle.peer, %error, "HTTP/3 request not sendable; falling back to TCP");
                    return H3Outcome::Fallback(b);
                }
                Err(SendError::Fatal(e)) => return H3Outcome::Failed(e),
            }
        }
        H3Outcome::Fallback(body)
    }

    fn pooled(&self, key: &H3Key) -> Option<H3Handle> {
        let mut pool = self.pool.lock();
        match pool.conns.get(key) {
            Some(h) if h.usable() => h.has_capacity().then(|| h.clone()),
            Some(_) => {
                pool.conns.remove(key);
                None
            }
            None => None,
        }
    }

    fn remove(&self, key: &H3Key, handle: &H3Handle) {
        let mut pool = self.pool.lock();
        if pool
            .conns
            .get(key)
            .is_some_and(|h| h.quic.stable_id() == handle.quic.stable_id())
        {
            pool.conns.remove(key);
        }
    }

    /// A pooled connection, or a new one (concurrent first requests share it).
    async fn acquire(
        &self,
        cfg: &Arc<Config>,
        key: &H3Key,
        handshake_timeout: Option<Duration>,
        force_new: bool,
    ) -> Result<(H3Handle, bool, ConnectTimings), UpstreamError> {
        if !force_new && let Some(h) = self.pooled(key) {
            return Ok((h, true, ConnectTimings::default()));
        }
        let lock = {
            let mut pool = self.pool.lock();
            pool.locks.entry(key.clone()).or_default().clone()
        };
        let waited_from = Instant::now();
        let guard = lock.lock().await;
        let result = self
            .acquire_locked(cfg, key, handshake_timeout, force_new, waited_from)
            .await;
        let mut pool = self.pool.lock();
        drop(guard);
        // Forget the lock once nobody else holds or waits for it.
        if Arc::strong_count(&lock) <= 2 {
            pool.locks.remove(key);
        }
        result
    }

    /// [`acquire`](Self::acquire) once the key's connect lock is held.
    async fn acquire_locked(
        &self,
        cfg: &Arc<Config>,
        key: &H3Key,
        handshake_timeout: Option<Duration>,
        force_new: bool,
        waited_from: Instant,
    ) -> Result<(H3Handle, bool, ConnectTimings), UpstreamError> {
        if !force_new && let Some(h) = self.pooled(key) {
            return Ok((h, true, ConnectTimings::default()));
        }
        if let Some((at, e)) = self.pool.lock().errors.get(key) {
            // The connect this request queued behind just failed: don't retry serially.
            if *at >= waited_from {
                return Err(e.clone());
            }
        }
        let result = self.connect(cfg, key, handshake_timeout).await;
        let mut pool = self.pool.lock();
        match &result {
            Ok((handle, _)) => {
                pool.errors.remove(key);
                pool.conns.insert(key.clone(), handle.clone());
            }
            Err(e) => {
                pool.errors
                    .retain(|_, (at, _)| at.elapsed() < CONNECT_ERROR_TTL);
                pool.errors.insert(key.clone(), (Instant::now(), e.clone()));
            }
        }
        result.map(|(h, t)| (h, false, t))
    }

    fn endpoint(&self, v6: bool) -> Result<Arc<ClientEndpoint>, String> {
        let mut eps = self.endpoints.lock();
        let slot = if v6 { &mut eps.v6 } else { &mut eps.v4 };
        if slot.is_none() {
            *slot = Some(ClientEndpoint::bind(v6).map(Arc::new));
        }
        slot.clone().unwrap_or_else(|| Err("no endpoint".into()))
    }

    /// The quinn client config for a TLS mode (the TCP path's rustls config with ALPN `h3`).
    fn quic_config(
        &self,
        cfg: &Config,
        insecure: bool,
        client_cert: Option<usize>,
    ) -> Result<quinn::ClientConfig, UpstreamError> {
        let key = (cfg.generation, insecure, client_cert);
        if let Some(c) = self.configs.lock().get(&key) {
            return Ok(c.clone());
        }
        let base = cfg.tls.client_config(insecure, Alpn::Http1, client_cert)?;
        let mut tls = (*base).clone();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        // Own session cache: tickets from TCP connections must not be offered over QUIC.
        tls.resumption = rustls::client::Resumption::in_memory_sessions(256);
        tls.enable_early_data = false;
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).map_err(|e| {
            error(
                ErrorKind::Internal,
                format!("QUIC TLS configuration error: {e}"),
                "TLS_CONFIG",
            )
        })?;
        let mut client = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.max_idle_timeout(quinn::IdleTimeout::try_from(Duration::from_secs(30)).ok());
        client.transport_config(Arc::new(transport));
        let mut configs = self.configs.lock();
        configs.retain(|(generation, _, _), _| *generation == cfg.generation);
        configs.insert(key, client.clone());
        Ok(client)
    }

    /// DNS → QUIC handshake (addresses raced, IPv6 first) → HTTP/3 settings exchange.
    async fn connect(
        &self,
        cfg: &Arc<Config>,
        key: &H3Key,
        handshake_timeout: Option<Duration>,
    ) -> Result<(H3Handle, ConnectTimings), UpstreamError> {
        let peer = host_port(&key.host, key.port);
        let mut timings = ConnectTimings::default();
        let started = Instant::now();
        let dns = cfg.dns.resolve(&key.host);
        let addrs = match cfg.connect_timeout {
            Some(t) => tokio::time::timeout(t, dns)
                .await
                .map_err(|_| timeout_error(format_args!("DNS lookup for {}", key.host), t))??,
            None => dns.await?,
        };
        timings.dns_ms = Some(started.elapsed().as_secs_f64() * 1000.0);
        let client_config = self.quic_config(cfg, key.insecure, key.client_cert)?;
        let t = Instant::now();
        let race = self.race(&addrs, key, &peer, client_config);
        let conn = match handshake_timeout {
            Some(limit) => tokio::time::timeout(limit, race)
                .await
                .map_err(|_| timeout_error(format_args!("QUIC handshake with {peer}"), limit))??,
            None => race.await?,
        };
        timings.connect_ms = Some(t.elapsed().as_secs_f64() * 1000.0);

        let mut info = UpstreamInfo {
            remote_addr: Some(conn.remote_address().to_string()),
            tls_version: Some("TLSv1.3".into()),
            alpn: Some("h3".into()),
            ..Default::default()
        };
        if let Some(data) = conn
            .handshake_data()
            .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok())
            && let Some(p) = &data.protocol
        {
            info.alpn = Some(String::from_utf8_lossy(p).into_owned());
        }
        if let Some(certs) = conn.peer_identity().and_then(|id| {
            id.downcast::<Vec<rustls::pki_types::CertificateDer<'static>>>()
                .ok()
        }) {
            info.certificate = certs.first().and_then(|der| cert::summarize(der));
            info.chain = cfg.tls.describe_chain(&certs, key.insecure);
        }

        let mut builder = h3::client::builder();
        builder.max_field_section_size(MAX_FIELD_SECTION);
        let build = builder.build::<_, _, Bytes>(h3_quinn::Connection::new(conn.clone()));
        let (mut driver, send) = tokio::time::timeout(Duration::from_secs(10), build)
            .await
            .map_err(|_| {
                timeout_error(
                    format_args!("HTTP/3 setup with {peer}"),
                    Duration::from_secs(10),
                )
            })?
            .map_err(|e| map_conn_error(&e, &peer))?;
        let closed = Arc::new(AtomicBool::new(false));
        let handle = H3Handle {
            send,
            quic: conn.clone(),
            info: Arc::new(info),
            peer: Arc::from(peer.as_str()),
            closed: closed.clone(),
            active: Arc::default(),
        };
        // Drive the connection (control streams, GOAWAY) until it closes, then forget it.
        let pool: Weak<Mutex<PoolState>> = Arc::downgrade(&self.pool);
        let key = key.clone();
        let id = conn.stable_id();
        tokio::spawn(async move {
            let e = driver.wait_idle().await;
            closed.store(true, Ordering::Release);
            if !e.is_h3_no_error() {
                tracing::debug!(%peer, error = %e, "HTTP/3 connection closed");
            }
            if let Some(pool) = pool.upgrade() {
                let mut pool = pool.lock();
                if pool
                    .conns
                    .get(&key)
                    .is_some_and(|h| h.quic.stable_id() == id)
                {
                    pool.conns.remove(&key);
                }
            }
        });
        Ok((handle, timings))
    }

    /// Start a handshake with each address in turn ([`ATTEMPT_DELAY`] apart, or as soon as one
    /// fails); the first to complete wins.
    async fn race(
        &self,
        addrs: &[IpAddr],
        key: &H3Key,
        peer: &str,
        config: quinn::ClientConfig,
    ) -> Result<quinn::Connection, UpstreamError> {
        let mut pending = interleave(addrs).into_iter();
        let mut attempts = FuturesUnordered::new();
        let mut last: Option<UpstreamError> = None;
        let start = |ip: IpAddr| -> Result<quinn::Connecting, UpstreamError> {
            let addr = SocketAddr::new(ip, key.port);
            if !routable(addr) {
                // E.g. an IPv6 address on a machine without IPv6 connectivity: UDP has no
                // handshake to fail fast like TCP's, so skip it instead of waiting.
                return Err(error(
                    ErrorKind::HostUnreachable,
                    format!("no route to {addr}"),
                    "ENETUNREACH",
                ));
            }
            let ep = self.endpoint(ip.is_ipv6()).map_err(|e| {
                error(
                    ErrorKind::HostUnreachable,
                    format!("cannot open a UDP socket for {peer}: {e}"),
                    "EADDRNOTAVAIL",
                )
            })?;
            ep.endpoint
                .connect_with(config.clone(), addr, &key.origin)
                .map_err(|e| internal(format!("cannot start a QUIC connection to {peer}: {e}")))
        };
        let mut push_next = |attempts: &mut FuturesUnordered<quinn::Connecting>,
                             last: &mut Option<UpstreamError>| {
            for ip in pending.by_ref() {
                match start(ip) {
                    Ok(connecting) => {
                        attempts.push(connecting);
                        return true;
                    }
                    Err(e) => *last = Some(e),
                }
            }
            false
        };
        let mut exhausted = !push_next(&mut attempts, &mut last);
        loop {
            if attempts.is_empty() && exhausted {
                break;
            }
            let stagger = tokio::time::sleep(ATTEMPT_DELAY);
            tokio::select! {
                Some(result) = attempts.next() => match result {
                    Ok(conn) => return Ok(conn),
                    Err(e) => {
                        let mapped = map_quic_error(&e, peer);
                        if mapped.kind == ErrorKind::UpstreamTlsFailed {
                            // The server answered: its other addresses won't do better.
                            return Err(mapped);
                        }
                        // Prefer the more informative error (a TLS failure over a timeout).
                        if last.as_ref().is_none_or(|l| rank(&mapped) >= rank(l)) {
                            last = Some(mapped);
                        }
                        if !exhausted {
                            exhausted = !push_next(&mut attempts, &mut last);
                        }
                    }
                },
                _ = stagger, if !exhausted => {
                    exhausted = !push_next(&mut attempts, &mut last);
                }
                else => break,
            }
        }
        Err(last.unwrap_or_else(|| {
            error(
                ErrorKind::DnsFailed,
                format!("no addresses to connect to for {peer}"),
                "ENOTFOUND",
            )
        }))
    }
}

fn rank(e: &UpstreamError) -> u8 {
    match e.kind {
        ErrorKind::UpstreamTlsFailed => 5,
        ErrorKind::ConnectionRefused | ErrorKind::ConnectionReset => 4,
        ErrorKind::UpstreamProtocolError => 3,
        ErrorKind::Timeout => 2,
        _ => 1,
    }
}

/// Classify a QUIC connection failure.
fn map_quic_error(e: &quinn::ConnectionError, peer: &str) -> UpstreamError {
    use quinn::ConnectionError as E;
    match e {
        E::TimedOut => error(
            ErrorKind::Timeout,
            format!("QUIC connection to {peer} timed out"),
            "ETIMEDOUT",
        ),
        E::TransportError(t) => {
            let code = u64::from(t.code);
            if (0x100..0x200).contains(&code) {
                error(
                    ErrorKind::UpstreamTlsFailed,
                    format!("TLS handshake with {peer} (QUIC) failed: {}", t.reason),
                    "QUIC_TLS_FAILED",
                )
            } else {
                error(
                    ErrorKind::UpstreamProtocolError,
                    format!("QUIC connection to {peer} failed: {t}"),
                    "QUIC_TRANSPORT_ERROR",
                )
            }
        }
        E::ConnectionClosed(c) => {
            let code = u64::from(c.error_code);
            if (0x100..0x200).contains(&code) {
                error(
                    ErrorKind::UpstreamTlsFailed,
                    format!("{peer} aborted the TLS handshake (QUIC): {c}"),
                    "QUIC_TLS_ALERT",
                )
            } else {
                error(
                    ErrorKind::ConnectionReset,
                    format!("{peer} closed the QUIC connection: {c}"),
                    "QUIC_CLOSED",
                )
            }
        }
        E::ApplicationClosed(c) => error(
            ErrorKind::ConnectionReset,
            format!("{peer} closed the HTTP/3 connection: {c}"),
            "H3_CLOSED",
        ),
        E::Reset => error(
            ErrorKind::ConnectionReset,
            format!("{peer} reset the QUIC connection"),
            "ECONNRESET",
        ),
        E::VersionMismatch => error(
            ErrorKind::UpstreamProtocolError,
            format!("{peer} doesn't support QUIC version 1"),
            "QUIC_VERSION_MISMATCH",
        ),
        E::LocallyClosed | E::CidsExhausted => {
            internal(format!("QUIC connection to {peer} closed locally: {e}"))
        }
    }
}

fn map_conn_error(e: &H3ConnError, peer: &str) -> UpstreamError {
    match e {
        H3ConnError::Timeout { .. } => error(
            ErrorKind::Timeout,
            format!("HTTP/3 connection to {peer} timed out"),
            "ETIMEDOUT",
        ),
        other => error(
            ErrorKind::ConnectionReset,
            format!("HTTP/3 connection to {peer} failed: {other}"),
            "H3_CONNECTION_ERROR",
        ),
    }
}

fn map_stream_error(e: &StreamError, peer: &str) -> UpstreamError {
    match e {
        StreamError::RemoteTerminate { code, .. } => error(
            ErrorKind::ConnectionReset,
            format!("{peer} reset the HTTP/3 stream ({code:?})"),
            "H3_STREAM_RESET",
        ),
        StreamError::ConnectionError { 0: c, .. } => map_conn_error(c, peer),
        StreamError::HeaderTooBig {
            actual_size,
            max_size,
            ..
        } => error(
            ErrorKind::UpstreamProtocolError,
            format!(
                "HTTP/3 header section too large for {peer} ({actual_size} bytes, limit {max_size})"
            ),
            "H3_HEADER_TOO_BIG",
        ),
        StreamError::RemoteClosing { .. } => error(
            ErrorKind::ConnectionReset,
            format!("{peer} is closing the HTTP/3 connection (GOAWAY)"),
            "H3_GOAWAY",
        ),
        StreamError::StreamError { code, reason, .. } => error(
            ErrorKind::UpstreamProtocolError,
            format!("HTTP/3 error from {peer}: {reason} ({code:?})"),
            "H3_PROTOCOL_ERROR",
        ),
        other => error(
            ErrorKind::ConnectionReset,
            format!("HTTP/3 stream to {peer} failed: {other}"),
            "H3_STREAM_ERROR",
        ),
    }
}

/// Whether a failure before any response guarantees the server didn't process the request.
fn not_processed(e: &StreamError) -> bool {
    match e {
        StreamError::RemoteTerminate { code, .. } => *code == Code::H3_REQUEST_REJECTED,
        StreamError::RemoteClosing { .. } => true,
        _ => false,
    }
}

enum SendError {
    /// The request wasn't processed: retry on a new connection.
    Retry {
        error: UpstreamError,
        body: RequestBody,
    },
    /// The request can't go over HTTP/3 (invalid for it): use TCP.
    Fallback {
        error: UpstreamError,
        body: RequestBody,
    },
    Fatal(UpstreamError),
}

struct Sent {
    head: ResponseHead,
    body: BodyStream,
    request_sent_ms: Option<f64>,
}

enum BodyError {
    Source(UpstreamError),
    H3(StreamError),
}

async fn send_body(mut tx: TxStream, body: RequestBody) -> Result<Instant, BodyError> {
    match body {
        RequestBody::Empty => {}
        RequestBody::Full(b) => {
            if !b.is_empty() {
                tx.send_data(b).await.map_err(BodyError::H3)?;
            }
        }
        RequestBody::Stream { mut stream, .. } => {
            let mut trailers: Option<Headers> = None;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(BodyChunk::Data(d)) => {
                        if !d.is_empty() {
                            tx.send_data(d).await.map_err(BodyError::H3)?;
                        }
                    }
                    Ok(BodyChunk::Trailers(t)) => match trailers.as_mut() {
                        Some(existing) => existing.0.extend(t.0),
                        None => trailers = Some(t),
                    },
                    Err(e) => {
                        tx.stop_stream(Code::H3_REQUEST_CANCELLED);
                        return Err(BodyError::Source(e));
                    }
                }
            }
            if let Some(t) = trailers.filter(|t| !t.is_empty()) {
                tx.send_trailers(t.to_header_map())
                    .await
                    .map_err(BodyError::H3)?;
            }
        }
    }
    tx.finish().await.map_err(BodyError::H3)?;
    Ok(Instant::now())
}

/// Aborts a still-running request body sender when the response is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<Result<Instant, BodyError>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Send one request on `handle`.
async fn send_on(handle: &H3Handle, plan: &Plan<'_>, body: RequestBody) -> Result<Sent, SendError> {
    let peer = handle.peer.clone();
    let mut request = match super::h2::build_request(plan.head, &plan.target.authority, &body) {
        Ok(r) => r,
        Err(error) => return Err(SendError::Fallback { error, body }),
    };
    *request.version_mut() = http::Version::HTTP_3;
    let active = ActiveGuard::new(handle);
    let mut sender = handle.send.clone();
    let stream = match sender.send_request(request).await {
        Ok(s) => s,
        Err(e @ StreamError::HeaderTooBig { .. }) => {
            return Err(SendError::Fallback {
                error: map_stream_error(&e, &peer),
                body,
            });
        }
        Err(e) => {
            return Err(SendError::Retry {
                error: map_stream_error(&e, &peer),
                body,
            });
        }
    };
    let (tx, mut rx) = stream.split();
    let replay = match &body {
        RequestBody::Empty => Some(RequestBody::Empty),
        RequestBody::Full(b) => Some(RequestBody::Full(b.clone())),
        RequestBody::Stream { .. } => None,
    };
    let mut sender_task = Some(tokio::spawn(send_body(tx, body)));
    let mut sent: Option<Instant> = None;
    let mut deadline: Option<(Instant, Duration)> = None;
    let head = {
        let response = async {
            let mut informational = 0;
            loop {
                let r = rx.recv_response().await?;
                // Interim (1xx) responses are skipped; the final one follows on the stream.
                if r.status().is_informational() && informational < 8 {
                    informational += 1;
                    continue;
                }
                return Ok::<_, StreamError>(r);
            }
        };
        tokio::pin!(response);
        loop {
            let timer = async {
                match deadline {
                    Some((at, _)) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let join = async {
                match sender_task.as_mut() {
                    Some(h) => h.await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                res = &mut response => break res,
                joined = join => {
                    sender_task = None;
                    match joined {
                        Ok(Ok(at)) => {
                            sent = Some(at);
                            deadline = plan.head_timeout.map(|t| (Instant::now() + t, t));
                        }
                        Ok(Err(BodyError::Source(e))) => return Err(SendError::Fatal(e)),
                        // The server may still answer (it may stop reading after responding
                        // early); keep waiting for the response.
                        Ok(Err(BodyError::H3(e))) => {
                            tracing::debug!(%peer, error = %e, "HTTP/3 request body not fully sent");
                        }
                        Err(join) => {
                            return Err(SendError::Fatal(internal(format!(
                                "request body sender failed: {join}"
                            ))));
                        }
                    }
                }
                _ = timer => {
                    if let Some(h) = sender_task.take() {
                        h.abort();
                    }
                    let after = deadline.map(|(_, d)| d).unwrap_or_default();
                    return Err(SendError::Fatal(timeout_error(
                        format_args!("waiting for a response from {peer}"),
                        after,
                    )));
                }
            }
        }
    };
    let response = match head {
        Ok(r) => r,
        Err(e) => {
            if let Some(h) = sender_task.take() {
                h.abort();
            }
            return Err(match (not_processed(&e), replay) {
                (true, Some(body)) => SendError::Retry {
                    error: map_stream_error(&e, &peer),
                    body,
                },
                _ => SendError::Fatal(map_stream_error(&e, &peer)),
            });
        }
    };
    let status = response.status().as_u16();
    let head = ResponseHead {
        status,
        status_message: canonical_reason(status).to_owned(),
        http_version: HttpVersion::H3,
        headers: Headers::from_header_map(response.headers()),
    };
    let request_sent_ms =
        sent.map(|at| at.saturating_duration_since(plan.started).as_secs_f64() * 1000.0);
    let state = BodyState {
        rx,
        trailers: false,
        idle_timeout: plan.idle_timeout,
        peer,
        _sender: sender_task.map(AbortOnDrop),
        _conn: handle.clone(),
        _active: active,
    };
    Ok(Sent {
        head,
        body: Box::pin(futures::stream::unfold(Some(state), next_chunk)),
        request_sent_ms,
    })
}

struct BodyState {
    rx: RxStream,
    trailers: bool,
    idle_timeout: Option<Duration>,
    peer: Arc<str>,
    /// Keeps a still-running request body upload alive while the response streams.
    _sender: Option<AbortOnDrop>,
    /// Keeps the connection open while the response streams.
    _conn: H3Handle,
    /// Counts the request as in flight until the response body is done or dropped.
    _active: ActiveGuard,
}

enum Next {
    Data(Bytes),
    End,
    Trailers(Headers),
    Error(UpstreamError),
}

async fn with_idle<T>(
    idle: Option<Duration>,
    peer: &str,
    fut: impl std::future::Future<Output = T>,
) -> Result<T, UpstreamError> {
    match idle {
        Some(d) => tokio::time::timeout(d, fut).await.map_err(|_| {
            timeout_error(
                format_args!("waiting for response body data from {peer}"),
                d,
            )
        }),
        None => Ok(fut.await),
    }
}

async fn next_chunk(
    state: Option<BodyState>,
) -> Option<(Result<BodyChunk, UpstreamError>, Option<BodyState>)> {
    let mut st = state?;
    loop {
        let peer = st.peer.clone();
        let next = if !st.trailers {
            match with_idle(st.idle_timeout, &peer, st.rx.recv_data()).await {
                Err(e) => Next::Error(e),
                Ok(Ok(Some(mut buf))) => Next::Data(buf.copy_to_bytes(buf.remaining())),
                Ok(Ok(None)) => Next::End,
                Ok(Err(e)) => Next::Error(map_stream_error(&e, &peer)),
            }
        } else {
            match with_idle(st.idle_timeout, &peer, st.rx.recv_trailers()).await {
                Err(e) => Next::Error(e),
                Ok(Ok(Some(map))) => Next::Trailers(Headers::from_header_map(&map)),
                Ok(Ok(None)) => return None,
                Ok(Err(e)) => Next::Error(map_stream_error(&e, &peer)),
            }
        };
        match next {
            Next::Data(d) if d.is_empty() => continue,
            Next::Data(d) => return Some((Ok(BodyChunk::Data(d)), Some(st))),
            Next::End => st.trailers = true,
            Next::Trailers(t) => return Some((Ok(BodyChunk::Trailers(t)), None)),
            Next::Error(e) => return Some((Err(e), None)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_names() {
        for (v, s) in [
            (UpstreamHttpVersion::Auto, "\"auto\""),
            (UpstreamHttpVersion::Http1, "\"http1\""),
            (UpstreamHttpVersion::Http2, "\"http2\""),
            (UpstreamHttpVersion::Http3, "\"http3\""),
        ] {
            assert_eq!(serde_json::to_string(&v).unwrap(), s);
            assert_eq!(serde_json::from_str::<UpstreamHttpVersion>(s).unwrap(), v);
        }
    }

    #[test]
    fn quic_errors_are_classified() {
        let e = map_quic_error(&quinn::ConnectionError::TimedOut, "h:443");
        assert_eq!(e.kind, ErrorKind::Timeout);
        let e = map_quic_error(&quinn::ConnectionError::Reset, "h:443");
        assert_eq!(e.kind, ErrorKind::ConnectionReset);
        let e = map_quic_error(&quinn::ConnectionError::VersionMismatch, "h:443");
        assert_eq!(e.kind, ErrorKind::UpstreamProtocolError);
        assert!(rank(&error(ErrorKind::UpstreamTlsFailed, "x", "X")) > rank(&e));
    }

    #[tokio::test]
    async fn own_endpoints_are_recognised() {
        let ep = ClientEndpoint::bind(false).unwrap();
        let port = ep.port;
        let lan: IpAddr = [192, 168, 7, 9].into();
        assert!(is_own_endpoint(
            SocketAddr::from(([127, 0, 0, 1], port)),
            None
        ));
        assert!(is_own_endpoint(SocketAddr::new(lan, port), Some(lan)));
        assert!(!is_own_endpoint(SocketAddr::new(lan, port), None));
        assert!(!is_own_endpoint(
            SocketAddr::from(([8, 8, 8, 8], port)),
            None
        ));
        drop(ep);
        assert!(!is_own_endpoint(
            SocketAddr::from(([127, 0, 0, 1], port)),
            None
        ));
    }
}
