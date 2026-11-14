//! Upstream (proxy → server) connectivity: DNS (system / overrides / DoH), TCP connect with
//! timeouts, upstream proxies (HTTP CONNECT, HTTPS, SOCKS5, system, PAC-lite), TLS with
//! configurable verification (extra CAs, ignore-errors host list, client certificates,
//! certificate capture), connection pooling for HTTP/1.1 (keep-alive) and HTTP/2
//! (multiplexed), and request sending with exact header casing/order on HTTP/1.
//!
//! This is the boundary between the inbound side (`server`, `pipeline`) and the network.
//!
//! # Overview
//!
//! * **DNS**: IP literals skip DNS, `settings.dns_overrides` (exact + globs, IPs or aliases),
//!   `localhost`/`*.localhost` resolve locally, a TTL-honouring cache, hickory-resolver with
//!   the system configuration or DNS-over-HTTPS (`settings.doh_url`).
//! * **TCP**: happy eyeballs (IPv6/IPv4 interleaved, 250 ms stagger), `TCP_NODELAY`,
//!   keepalive; `settings.connect_timeout_ms` bounds DNS + connect + proxy handshake + TLS.
//! * **Upstream proxies** per `settings.upstream_proxy`: direct, manual (`http://` →
//!   absolute-form for plain-HTTP targets and `CONNECT` for TLS/WebSocket targets,
//!   `https://` proxies over TLS, `socks4[a]://`, `socks5[h]://` with credentials), system
//!   (env vars, GNOME/KDE, macOS, Windows) and PAC (QuickJS). `no_proxy` supports globs,
//!   CIDRs, IPv6 and `<local>`. Proxy failures are [`ErrorKind::UpstreamProxyFailed`].
//! * **TLS** (rustls): webpki + OS roots + `settings.trusted_ca_paths`, per-host
//!   verification bypass, precise certificate error classification, leaf certificate summary
//!   in [`UpstreamInfo::certificate`], client certificates (PEM / PKCS#12), ALPN
//!   `h2`/`http/1.1`.
//! * **TLS fingerprint** ([`fingerprint`]): per `settings.tls_profile` (+ per-host overrides,
//!   `tls-profile` rules, `mirror-client`), origin connections can present a browser's
//!   ClientHello and HTTP/2 preface (BoringSSL backend, cargo feature `impersonate`); every
//!   TLS connection reports the JA3/JA4 it sent in [`UpstreamInfo::fingerprint`].
//! * **HTTP/1.1**: hand-written client with verbatim headers, streaming bodies both ways,
//!   trailers, `Expect: 100-continue`, keep-alive pool (90 s idle, 8 idle per host). Stale
//!   keep-alive connections are detected before reuse and requests that hit one anyway are
//!   retried transparently on a fresh connection. Connections authenticated with NTLM /
//!   Negotiate are pinned to the client connection they were authenticated for
//!   ([`ConnectionOwner`]).
//! * **HTTP/2**: multiplexed connections per pool key via the `h2` crate; concurrent first
//!   requests wait for the first connection and multiplex instead of each opening one. An
//!   additional connection is opened only when the existing ones are saturated (streams
//!   stalled by slow consumers must never block the other clients' requests).
//! * **Pool keys**: scheme, host, port, upstream proxy (+ username), verification mode,
//!   client certificate and the ALPN offer. Settings changes affecting DNS, TLS or proxies
//!   drop the pools.
//! * **Timings**: DNS / connect / TLS per new connection (`None` when reused, with
//!   [`UpstreamInfo::reused`] set) and `request_sent_ms`.
//! * **Errors**: every failure is an [`UpstreamError`] with a precise [`ErrorKind`] and a
//!   stable code (`ECONNREFUSED`, `ENOTFOUND`, `CERT_HAS_EXPIRED`, `TLS_ALERT_*`, `H2_*`,
//!   `PROXY_*`, `SOCKS_*`...).
//! * **Loop detection**: [`UpstreamClient::is_own_connection`] tells the inbound side when an
//!   accepted connection is one of our own outgoing connections.
//!
//! [`UpstreamInfo::certificate`]: akev_core::UpstreamInfo::certificate
//! [`UpstreamInfo::reused`]: akev_core::UpstreamInfo::reused
//! [`UpstreamInfo::fingerprint`]: akev_core::UpstreamInfo::fingerprint

use akev_core::settings::ProxySettings;
use akev_core::*;
use bytes::Bytes;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};

mod alt_svc;
mod cert;
mod config;
mod connect;
mod dns;
mod error;
pub mod fingerprint;
mod glob;
mod h1;
mod h2;
mod h3;
mod io;
mod pool;
mod proxy;
pub(crate) mod tls;

use arc_swap::ArcSwap;
use bytes::BytesMut;
use config::Config;
use connect::{ConnectRequest, Established, TlsBackend, TlsTarget};
use fingerprint::Selection;
use futures::StreamExt;
use futures::future::BoxFuture;
use pool::{ConnMeta, H1Conn, Pool, PoolKey};
use proxy::{PacFetcher, ProxyTarget, Route, SelectEnv, host_port};
use std::sync::Arc;
use std::time::Duration;
use tls::Alpn;
use tokio::time::Instant;

pub use h3::UpstreamHttpVersion;
pub(crate) use h3::is_own_endpoint as is_own_quic_endpoint;

/// Any bidirectional byte stream (TCP, TLS-over-TCP, tunnelled through an upstream proxy).
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Io for T {}
/// A boxed [`Io`].
pub type BoxIo = Box<dyn Io>;

/// A streaming body: chunks of bytes, ending with optional trailers.
pub type BodyStream =
    Pin<Box<dyn futures::Stream<Item = std::result::Result<BodyChunk, UpstreamError>> + Send>>;

/// One item of a [`BodyStream`].
#[derive(Debug, Clone)]
pub enum BodyChunk {
    /// Body bytes (transfer-encoding removed, content-encoding untouched).
    Data(Bytes),
    /// Trailers (after the last data chunk).
    Trailers(Headers),
}

/// Body for an outgoing request.
pub enum RequestBody {
    /// No body.
    Empty,
    /// A complete body (`Content-Length` is fixed up to match on HTTP/1).
    Full(Bytes),
    /// Streamed (request body still arriving from the client). `length` = known Content-Length.
    Stream {
        /// The body chunks (and optional trailers, sent when the body is chunked / HTTP/2).
        stream: BodyStream,
        /// Known length; `None` → chunked on HTTP/1 (unless the head declares a length).
        length: Option<u64>,
    },
}

impl std::fmt::Debug for RequestBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestBody::Empty => f.write_str("Empty"),
            RequestBody::Full(b) => f.debug_tuple("Full").field(&b.len()).finish(),
            RequestBody::Stream { length, .. } => f
                .debug_struct("Stream")
                .field("length", length)
                .finish_non_exhaustive(),
        }
    }
}

/// A classified upstream failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{message}")]
pub struct UpstreamError {
    /// Classification (drives the UI's icon and help text).
    pub kind: ErrorKind,
    /// Human readable message.
    pub message: String,
    /// Stable code (`ECONNREFUSED`, `CERT_HAS_EXPIRED`, `TLS_ALERT_HANDSHAKE_FAILURE`, ...).
    pub code: Option<String>,
}

/// A failed [`UpstreamClient::send_returning_body`].
#[derive(Debug)]
pub struct SendFailure {
    /// Why the request failed.
    pub error: UpstreamError,
    /// The request body, handed back when the request failed before a streamed body was
    /// polled at all (DNS, connect, TLS or upstream proxy failure, a stale connection detected
    /// before the body was sent...), so the caller can still read it — e.g. to record what the
    /// client sent. `None` once the body was (even partly) consumed.
    pub unsent_body: Option<RequestBody>,
}

impl UpstreamError {
    /// Convert into the error recorded on the exchange.
    pub fn to_exchange_error(&self) -> ExchangeError {
        ExchangeError {
            kind: self.kind,
            message: self.message.clone(),
            code: self.code.clone(),
        }
    }
}

/// Per-request overrides.
#[derive(Clone, Debug, Default)]
pub struct RequestOptions {
    /// Ignore certificate errors regardless of settings.
    pub insecure: bool,
    /// Response head timeout (ms). 0 = settings default.
    pub timeout_ms: u64,
    /// The client connection the request arrived on. Upstream HTTP/1.1 connections that carry
    /// connection-oriented authentication (NTLM / Negotiate) are pinned to it, see
    /// [`ConnectionOwner`]. Without an owner, such connections are never reused.
    pub owner: Option<ConnectionOwner>,
    /// HTTP version to use (`Http1`: don't offer h2 via ALPN; `Http3`: try HTTP/3 first whatever
    /// the settings say).
    pub http_version: UpstreamHttpVersion,
    /// Upstream TLS fingerprint inputs: a `tls-profile` rule and the client's browser family.
    pub fingerprint: fingerprint::FingerprintOptions,
    /// The request comes from another machine: refuse targets on this machine's loopback or
    /// unspecified addresses (`127.0.0.0/8`, `::1`, `0.0.0.0`, `localhost`), also when a name
    /// resolves to one, so the proxy can't be used to reach local-only services.
    pub restrict_loopback: bool,
}

/// Fuzzing (`akev_proxy::fuzzing`): parse an upstream response head and apply the framing
/// rules to it.
#[cfg(feature = "fuzzing")]
pub(crate) fn fuzz_response_head(data: &[u8]) {
    if let Ok(Some((mut head, _))) = h1::parse_head(data) {
        let _ = h1::normalize_framing_headers(&mut head);
        for method in ["GET", "HEAD"] {
            let _ = h1::response_framing(method, head.status, &head.headers);
        }
        let _ = h1::response_keep_alive(head.version, &head.headers);
    }
}

/// The refusal of a loopback target for a client on another machine.
fn loopback_refused(host: &str) -> UpstreamError {
    error::error(
        ErrorKind::ConnectionRefused,
        format!(
            "Akev refused to connect a device on the network to {host}, which is this computer's own address. To allow it, turn on \"Let devices reach this computer's localhost\" in Settings > Proxy."
        ),
        "LOOPBACK_TARGET_REFUSED",
    )
}

/// Refuse when `addrs` (what `host` resolved to) include a loopback or unspecified address.
pub(crate) fn check_loopback_addrs(
    host: &str,
    addrs: &[std::net::IpAddr],
) -> Result<(), UpstreamError> {
    if addrs
        .iter()
        .any(|ip| akev_core::access::is_loopback_target(*ip))
    {
        return Err(loopback_refused(host));
    }
    Ok(())
}

/// Identifies a client (downstream) connection, for connection pinning.
///
/// Some servers authenticate the TCP connection rather than each request (NTLM and
/// Negotiate on IIS, some SSO gateways): once a request on a connection carried such
/// credentials, later requests on it are served as that user even without credentials. An
/// upstream HTTP/1.1 connection that carried `Authorization: NTLM|Negotiate` (or got such a
/// `WWW-Authenticate` / `Proxy-Authenticate` challenge) is therefore pinned to the owner of
/// that request: it is only reused for requests with the same owner — which also keeps a
/// multi-leg handshake on one connection — and closed once every clone of the owner is
/// dropped. Create one per client connection and pass a clone with each of its requests.
#[derive(Clone)]
pub struct ConnectionOwner(Arc<u64>);

impl ConnectionOwner {
    /// A new, unique owner.
    pub fn new() -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        ConnectionOwner(Arc::new(
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        ))
    }

    pub(crate) fn downgrade(&self) -> WeakOwner {
        WeakOwner(Arc::downgrade(&self.0))
    }
}

impl Default for ConnectionOwner {
    fn default() -> Self {
        Self::new()
    }
}

impl PartialEq for ConnectionOwner {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for ConnectionOwner {}

impl std::fmt::Debug for ConnectionOwner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConnectionOwner({})", self.0)
    }
}

/// A [`ConnectionOwner`] reference that doesn't keep the owner alive (held by pinned
/// connections, so they can be closed once their client connection is gone).
#[derive(Clone, Debug)]
pub(crate) struct WeakOwner(std::sync::Weak<u64>);

impl WeakOwner {
    /// Whether this refers to `owner`. (The weak reference keeps the allocation alive, so its
    /// address can't be reused by a new owner.)
    pub(crate) fn is(&self, owner: &ConnectionOwner) -> bool {
        std::ptr::eq(self.0.as_ptr(), Arc::as_ptr(&owner.0))
    }

    /// Whether both refer to the same owner.
    pub(crate) fn same(&self, other: &WeakOwner) -> bool {
        self.0.ptr_eq(&other.0)
    }

    /// Whether the owner still exists.
    pub(crate) fn is_alive(&self) -> bool {
        self.0.strong_count() > 0
    }
}

/// Connection-level timings for a request (None when a pooled connection was reused).
#[derive(Clone, Debug, Default)]
pub struct ConnectTimings {
    /// DNS resolution (of the target, or of the upstream proxy's host when proxied).
    pub dns_ms: Option<f64>,
    /// TCP connect (plus the upstream proxy handshake, when proxied).
    pub connect_ms: Option<f64>,
    /// TLS handshake with the origin.
    pub tls_ms: Option<f64>,
}

/// A response from the upstream server.
pub struct UpstreamResponse {
    /// Response head (HTTP/1: exact header casing/order and reason phrase).
    pub head: ResponseHead,
    /// Streaming body (backpressured: data is read from the network as it is polled).
    pub body: BodyStream,
    /// Connection details.
    pub info: UpstreamInfo,
    /// Connection timings (all `None` when a pooled connection was reused).
    pub timings: ConnectTimings,
    /// ms (monotonic, relative to the `send` call) when the request was fully written.
    pub request_sent_ms: Option<f64>,
}

impl std::fmt::Debug for UpstreamResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamResponse")
            .field("head", &self.head)
            .field("info", &self.info)
            .field("timings", &self.timings)
            .field("request_sent_ms", &self.request_sent_ms)
            .finish_non_exhaustive()
    }
}

/// An established upgraded connection (WebSocket after `101`), with any bytes the server sent
/// after the response head already buffered in `leftover`.
pub struct Upgraded {
    /// The raw upgraded stream.
    pub io: BoxIo,
    /// Bytes received after the `101` head (to be processed before reading `io`).
    pub leftover: Bytes,
}

impl std::fmt::Debug for Upgraded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Upgraded")
            .field("leftover", &self.leftover.len())
            .finish_non_exhaustive()
    }
}

/// The upstream client. Cheap to clone (all clones share pools and settings).
#[derive(Clone)]
pub struct UpstreamClient {
    inner: Arc<ClientInner>,
}

impl std::fmt::Debug for UpstreamClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpstreamClient")
            .field("generation", &self.inner.config.load().generation)
            .finish()
    }
}

struct ClientInner {
    config: ArcSwap<Config>,
    pool: Arc<Pool>,
    pac: proxy::pac::PacEngine,
    update_lock: parking_lot::Mutex<()>,
    /// HTTP/3: Alt-Svc knowledge, QUIC connections.
    h3: h3::H3Client,
}

/// Timeout for PAC script downloads.
const PAC_FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Largest PAC script accepted.
const PAC_MAX_SIZE: usize = 4 * 1024 * 1024;

impl UpstreamClient {
    /// The compiled upstream TLS settings (roots, certificate-error exceptions, client
    /// certificates) of the current settings generation.
    pub(crate) fn tls_context(&self) -> Arc<tls::TlsContext> {
        self.inner.config.load().tls.clone()
    }

    /// Create a client. Invalid user settings (bad proxy URL, unreadable certificate files...)
    /// don't fail construction — they surface as descriptive errors on the affected requests,
    /// so a typo in `settings.json` can never prevent the proxy from starting.
    pub fn new(settings: &ProxySettings) -> std::result::Result<Self, UpstreamError> {
        tls::warm_up();
        let config = Config::build(settings, None);
        let pool = Pool::new(config.generation);
        Ok(UpstreamClient {
            inner: Arc::new(ClientInner {
                config: ArcSwap::from_pointee(config),
                pool,
                pac: proxy::pac::PacEngine::new(),
                update_lock: parking_lot::Mutex::new(()),
                h3: h3::H3Client::default(),
            }),
        })
    }

    /// Apply new settings (drops pools whose TLS/proxy config changed).
    pub fn update_settings(&self, settings: &ProxySettings) {
        let _guard = self.inner.update_lock.lock();
        let old = self.inner.config.load_full();
        let new = Config::build(settings, Some(&old));
        let changed = new.generation != old.generation;
        let generation = new.generation;
        self.inner.config.store(Arc::new(new));
        if changed {
            self.inner.pool.reset(generation);
        }
    }

    /// Send a request to `head.scheme://head.host:head.port`. HTTP/1 requests are written with
    /// `head.headers` verbatim (casing/order); for HTTP/2 upstreams headers are lowercased and
    /// connection-specific headers dropped. Uses pooled connections when possible.
    pub async fn send(
        &self,
        head: &RequestHead,
        body: RequestBody,
        options: &RequestOptions,
    ) -> std::result::Result<UpstreamResponse, UpstreamError> {
        self.inner
            .send(head, body, options)
            .await
            .map_err(|f| f.error)
    }

    /// Like [`send`](Self::send), but a failure hands the request body back when none of it
    /// was consumed (see [`SendFailure::unsent_body`]).
    pub async fn send_returning_body(
        &self,
        head: &RequestHead,
        body: RequestBody,
        options: &RequestOptions,
    ) -> std::result::Result<UpstreamResponse, SendFailure> {
        self.inner.send(head, body, options).await
    }

    /// Send an upgrade request (WebSocket) over a fresh HTTP/1.1 connection. On `101` returns the
    /// response head and the raw upgraded stream; otherwise the response is returned normally.
    pub async fn send_upgrade(
        &self,
        head: &RequestHead,
        options: &RequestOptions,
    ) -> std::result::Result<
        (
            ResponseHead,
            Option<Upgraded>,
            Option<UpstreamResponse>,
            UpstreamInfo,
        ),
        UpstreamError,
    > {
        self.inner.send_upgrade(head, options).await
    }

    /// Open a raw TCP stream to host:port (honouring upstream proxy + DNS overrides). Used for
    /// passthrough tunnels.
    pub async fn connect_raw(
        &self,
        host: &str,
        port: u16,
    ) -> std::result::Result<(BoxIo, UpstreamInfo, ConnectTimings), UpstreamError> {
        self.inner.connect_raw(host, port, false).await
    }

    /// [`UpstreamClient::connect_raw`] for a client that may not reach loopback addresses
    /// (`restrict_loopback`, see [`RequestOptions::restrict_loopback`]).
    pub async fn connect_raw_restricted(
        &self,
        host: &str,
        port: u16,
        restrict_loopback: bool,
    ) -> std::result::Result<(BoxIo, UpstreamInfo, ConnectTimings), UpstreamError> {
        self.inner.connect_raw(host, port, restrict_loopback).await
    }

    /// Proxy-loop detection: whether `peer` — the remote address of a connection accepted by
    /// the inbound side — is the local end of a live outgoing connection opened by an upstream
    /// client in this process. That happens when a request is forwarded back to the proxy
    /// itself (an upstream proxy, system proxy, PAC result or DNS override pointing at Akev),
    /// which would otherwise recurse until resources run out. The pipeline should refuse such
    /// requests (HTTP Toolkit answers them with "Passthrough loop detected").
    ///
    /// IPv4-mapped IPv6 peers (dual-stack listeners) are matched against their IPv4 form. The
    /// check is a lock-free hash lookup, cheap enough to run for every accepted connection.
    pub fn is_own_connection(&self, peer: std::net::SocketAddr) -> bool {
        connect::is_local_endpoint(peer)
    }
}

/// The resolved destination of a request.
struct Target {
    tls: bool,
    /// `http`/`https`/`ws`/`wss`.
    scheme: &'static str,
    /// Normalised host.
    host: String,
    port: u16,
    /// `host[:port]` (IPv6 in brackets, default port omitted): the `Host` written when the
    /// request has none, the absolute-form authority and the HTTP/2 `:authority` fallback.
    authority: String,
}

impl Target {
    fn of(head: &RequestHead) -> Result<Self, UpstreamError> {
        let scheme = match head.scheme.to_ascii_lowercase().as_str() {
            "http" => "http",
            "https" => "https",
            "ws" => "ws",
            "wss" => "wss",
            other => return Err(error::internal(format!("unsupported URL scheme {other:?}"))),
        };
        let tls = matches!(scheme, "https" | "wss");
        let host = glob::normalize_host(&head.host).into_owned();
        if host.is_empty() {
            return Err(error::internal("request has no target host"));
        }
        let default_port = if tls { 443 } else { 80 };
        let port = match head.port {
            0 => default_port,
            p => p,
        };
        let authority = if port == default_port {
            if host.contains(':') {
                format!("[{host}]")
            } else {
                host.clone()
            }
        } else {
            host_port(&host, port)
        };
        Ok(Target {
            tls,
            scheme,
            host,
            port,
            authority,
        })
    }

    fn peer(&self) -> String {
        host_port(&self.host, self.port)
    }
}

enum Acquired {
    H1 {
        conn: H1Conn,
        reused: bool,
        timings: ConnectTimings,
    },
    H2 {
        handle: h2::H2Handle,
        reused: bool,
        timings: ConnectTimings,
    },
}

/// Per-request parameters shared by the attempts.
struct Plan<'a> {
    head: &'a RequestHead,
    target: &'a Target,
    insecure: bool,
    client_cert: Option<usize>,
    allow_h2: bool,
    head_timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    started: Instant,
    /// The client connection the request came from (connection pinning).
    owner: Option<&'a ConnectionOwner>,
    /// TLS fingerprint to present (native for plain-HTTP targets).
    selection: Selection,
    /// See [`RequestOptions::restrict_loopback`].
    restrict_loopback: bool,
}

/// Failure of a single-route attempt.
struct RouteFailure {
    error: UpstreamError,
    /// The request body when it wasn't consumed (see [`SendFailure::unsent_body`]).
    unsent_body: Option<RequestBody>,
    /// Failed while connecting: nothing reached the server, so the next fail-over route may
    /// be tried with `unsent_body`.
    connect_failed: bool,
}

impl RouteFailure {
    fn connecting(error: UpstreamError, body: RequestBody) -> Self {
        RouteFailure {
            error,
            unsent_body: Some(body),
            connect_failed: true,
        }
    }

    fn sending(error: UpstreamError, unsent_body: Option<RequestBody>) -> Self {
        RouteFailure {
            error,
            unsent_body,
            connect_failed: false,
        }
    }
}

fn to_response_head(head: h1::ParsedHead) -> ResponseHead {
    ResponseHead {
        status: head.status,
        status_message: head.reason,
        http_version: head.version,
        headers: head.headers,
    }
}

/// Whether the request asks for a protocol upgrade (`Upgrade` header present).
fn is_upgrade_request(head: &RequestHead) -> bool {
    head.headers
        .get_all("upgrade")
        .any(|v| !v.trim().is_empty())
}

fn info_with_reuse(base: &UpstreamInfo, reused: bool) -> UpstreamInfo {
    let mut info = base.clone();
    info.reused = reused;
    info
}

impl ClientInner {
    fn timeouts(cfg: &Config, options: &RequestOptions) -> (Option<Duration>, Option<Duration>) {
        let head = if options.timeout_ms > 0 {
            Some(Duration::from_millis(options.timeout_ms))
        } else {
            cfg.response_timeout
        };
        (head, cfg.body_idle_timeout)
    }

    /// For [`RequestOptions::restrict_loopback`]: refuse `localhost` names and loopback /
    /// unspecified IP literals on any route, and names resolving to them on direct routes
    /// (pooled connections included: [`connect::establish`] checks every new connection).
    async fn check_target(
        &self,
        cfg: &Config,
        host: &str,
        routes: &[Route],
    ) -> Result<(), UpstreamError> {
        if dns::is_localhost_name(host)
            || dns::parse_ip(host).is_some_and(akev_core::access::is_loopback_target)
        {
            return Err(loopback_refused(host));
        }
        if routes.iter().any(|r| matches!(r, Route::Direct))
            && let Ok(addrs) = cfg.dns.resolve(host).await
        {
            check_loopback_addrs(host, &addrs)?;
        }
        Ok(())
    }

    async fn select_routes(
        self: &Arc<Self>,
        cfg: &Config,
        scheme: &str,
        host: &str,
        port: u16,
        url: &str,
    ) -> Result<Vec<Route>, UpstreamError> {
        if cfg.proxy.is_direct() {
            return Ok(vec![Route::Direct]);
        }
        let target = ProxyTarget {
            scheme,
            host,
            port,
            url,
        };
        let env = SelectEnv {
            pac: &self.pac,
            fetcher: self,
            dns: &cfg.dns,
        };
        cfg.proxy.select(&target, &env).await
    }

    fn tls_target(
        cfg: &Config,
        target: &Target,
        insecure: bool,
        alpn: Alpn,
        client_cert: Option<usize>,
        selection: Selection,
    ) -> Result<Option<TlsTarget>, UpstreamError> {
        if !target.tls {
            return Ok(None);
        }
        #[cfg(feature = "impersonate")]
        let backend = match selection.browser {
            Some(browser) => TlsBackend::Boring(fingerprint::boring::BoringTarget {
                browser,
                alpn,
                tls: cfg.tls.clone(),
                insecure,
                client_cert,
            }),
            None => TlsBackend::Rustls(cfg.tls.client_config(insecure, alpn, client_cert)?),
        };
        #[cfg(not(feature = "impersonate"))]
        let backend = TlsBackend::Rustls(cfg.tls.client_config(insecure, alpn, client_cert)?);
        Ok(Some(TlsTarget {
            backend,
            server_name: tls::server_name(&target.host)?,
            insecure,
            selection,
        }))
    }

    /// The fingerprint for a request to `target`.
    fn selection(cfg: &Config, target: &Target, options: &RequestOptions) -> Selection {
        if target.tls {
            cfg.fingerprint
                .select(&target.host, target.port, &options.fingerprint)
        } else {
            Selection::NATIVE
        }
    }

    async fn send(
        self: &Arc<Self>,
        head: &RequestHead,
        body: RequestBody,
        options: &RequestOptions,
    ) -> Result<UpstreamResponse, SendFailure> {
        let started = Instant::now();
        let cfg = self.config.load_full();
        let target = match Target::of(head) {
            Ok(t) => t,
            Err(error) => {
                return Err(SendFailure {
                    error,
                    unsent_body: Some(body),
                });
            }
        };
        let routes = match self
            .select_routes(&cfg, target.scheme, &target.host, target.port, &head.url)
            .await
        {
            Ok(r) => r,
            Err(error) => {
                return Err(SendFailure {
                    error,
                    unsent_body: Some(body),
                });
            }
        };
        if options.restrict_loopback
            && let Err(error) = self.check_target(&cfg, &target.host, &routes).await
        {
            return Err(SendFailure {
                error,
                unsent_body: Some(body),
            });
        }
        let (head_timeout, idle_timeout) = Self::timeouts(&cfg, options);
        let plan = Plan {
            head,
            target: &target,
            insecure: target.tls
                && (options.insecure || cfg.tls.ignores_errors_for(&target.host, target.port)),
            client_cert: if target.tls {
                cfg.tls.client_cert_for(&target.host, target.port)
            } else {
                None
            },
            // Upgrade requests keep their HTTP/1.1 semantics (HTTP/2 has no Upgrade).
            allow_h2: target.tls
                && options.http_version != UpstreamHttpVersion::Http1
                && !is_upgrade_request(head),
            head_timeout,
            idle_timeout,
            started,
            owner: options.owner.as_ref(),
            selection: Self::selection(&cfg, &target, options),
            restrict_loopback: options.restrict_loopback,
        };
        let mut body = body;
        // HTTP/3 first when the settings / the request ask for it (TCP on failure).
        let http1_only =
            options.http_version == UpstreamHttpVersion::Http1 || !plan.allow_h2 && target.tls;
        if let Some(c) = self
            .h3
            .candidate(&cfg, &plan, options.http_version, http1_only, &routes)
        {
            match self.h3.send(&cfg, &plan, &c, body).await {
                h3::H3Outcome::Done(resp) => return Ok(*resp),
                h3::H3Outcome::Fallback(returned) => body = returned,
                h3::H3Outcome::Failed(error) => {
                    return Err(SendFailure {
                        error,
                        unsent_body: None,
                    });
                }
            }
        }
        let mut routes = routes.iter().peekable();
        while let Some(route) = routes.next() {
            match self.send_via(&cfg, &plan, route, body).await {
                Ok(resp) => {
                    let headers = &resp.head.headers;
                    self.h3
                        .observe(&target.host, target.port, target.tls, headers);
                    return Ok(resp);
                }
                Err(RouteFailure {
                    error,
                    unsent_body: Some(returned),
                    connect_failed: true,
                }) if routes.peek().is_some() => {
                    tracing::debug!(?route, %error, "upstream route failed; trying the next one");
                    body = returned;
                }
                Err(f) => {
                    return Err(SendFailure {
                        error: f.error,
                        unsent_body: f.unsent_body,
                    });
                }
            }
        }
        Err(SendFailure {
            error: error::internal("no upstream route available"),
            unsent_body: Some(body),
        })
    }

    fn pool_key(plan: &Plan<'_>, route: &Route) -> PoolKey {
        // Plain-HTTP requests through an HTTP proxy share connections to the proxy.
        let absolute = matches!(route, Route::Proxy(p) if p.kind.is_http()) && !plan.target.tls;
        PoolKey {
            tls: plan.target.tls,
            host: if absolute {
                Arc::from("")
            } else {
                Arc::from(plan.target.host.as_str())
            },
            port: if absolute { 0 } else { plan.target.port },
            proxy: route.proxy_id(),
            insecure: plan.insecure,
            client_cert: plan.client_cert,
            h1_alpn: plan.target.tls && !plan.allow_h2,
            browser: plan.selection.browser,
            restricted: plan.restrict_loopback,
        }
    }

    /// Send on one route, retrying on a fresh connection when a reused connection turned out
    /// to be stale (or an HTTP/2 stream was refused).
    async fn send_via(
        self: &Arc<Self>,
        cfg: &Arc<Config>,
        plan: &Plan<'_>,
        route: &Route,
        body: RequestBody,
    ) -> Result<UpstreamResponse, RouteFailure> {
        let key = Self::pool_key(plan, route);
        let mut body = body;
        let mut force_new = false;
        let mut attempt = 0;
        loop {
            attempt += 1;
            let acquired = match self.acquire(cfg, plan, route, &key, force_new).await {
                Ok(a) => a,
                Err(e) => return Err(RouteFailure::connecting(e, body)),
            };
            match acquired {
                Acquired::H2 {
                    handle,
                    reused,
                    timings,
                } => {
                    let req = h2::H2Request {
                        head: plan.head,
                        authority: &plan.target.authority,
                        body,
                        head_timeout: plan.head_timeout,
                        idle_timeout: plan.idle_timeout,
                        started: plan.started,
                    };
                    match h2::send(&handle, req).await {
                        Ok(out) => {
                            return Ok(UpstreamResponse {
                                head: out.head,
                                body: out.body,
                                info: info_with_reuse(&handle.meta().info, reused),
                                timings: if reused {
                                    ConnectTimings::default()
                                } else {
                                    timings
                                },
                                request_sent_ms: out.request_sent_ms,
                            });
                        }
                        Err(h2::H2Error::Retryable {
                            error,
                            body: returned,
                        }) if attempt < 3 => {
                            tracing::debug!(error = %error, "retrying request on a new HTTP/2 connection");
                            self.pool.remove_h2(&handle);
                            body = returned;
                            force_new = true;
                        }
                        Err(h2::H2Error::Retryable {
                            error,
                            body: returned,
                        }) => {
                            return Err(RouteFailure::sending(error, Some(returned)));
                        }
                        Err(h2::H2Error::Fatal(error)) => {
                            return Err(RouteFailure::sending(error, None));
                        }
                    }
                }
                Acquired::H1 {
                    conn,
                    reused,
                    timings,
                } => {
                    let meta = conn.meta.clone();
                    let req = h1::H1Request {
                        head: plan.head,
                        authority: &plan.target.authority,
                        body,
                        title_case: cfg.title_case,
                        head_timeout: plan.head_timeout,
                        idle_timeout: plan.idle_timeout,
                        started: plan.started,
                        upgrade: false,
                        owner: plan.owner,
                    };
                    match h1::send(conn, req, Some(Arc::downgrade(&self.pool))).await {
                        Ok(out) => {
                            let body = out
                                .body
                                .unwrap_or_else(|| Box::pin(futures::stream::empty()));
                            return Ok(UpstreamResponse {
                                head: to_response_head(out.head),
                                body,
                                info: info_with_reuse(&meta.info, reused),
                                timings: if reused {
                                    ConnectTimings::default()
                                } else {
                                    timings
                                },
                                request_sent_ms: out.request_sent_ms,
                            });
                        }
                        Err(h1::H1Error::Retryable {
                            error,
                            body: returned,
                        }) if reused && attempt < 3 => {
                            tracing::debug!(error = %error, "stale keep-alive connection; retrying on a new connection");
                            body = returned;
                            force_new = true;
                        }
                        Err(h1::H1Error::Retryable {
                            error,
                            body: returned,
                        }) => {
                            return Err(RouteFailure::sending(error, Some(returned)));
                        }
                        Err(h1::H1Error::Fatal(error)) => {
                            return Err(RouteFailure::sending(error, None));
                        }
                    }
                }
            }
        }
    }

    /// Get a connection for `key`: shared HTTP/2, pooled HTTP/1.1, or a new one.
    async fn acquire(
        &self,
        cfg: &Arc<Config>,
        plan: &Plan<'_>,
        route: &Route,
        key: &PoolKey,
        force_new: bool,
    ) -> Result<Acquired, UpstreamError> {
        if plan.allow_h2
            && let Some(handle) = self.pool.get_h2(key)
        {
            return Ok(Acquired::H2 {
                handle,
                reused: true,
                timings: ConnectTimings::default(),
            });
        }
        if !force_new && let Some(conn) = self.pool.take_h1(key, plan.owner) {
            return Ok(Acquired::H1 {
                conn,
                reused: true,
                timings: ConnectTimings::default(),
            });
        }
        // First connection to a host that may speak h2: concurrent requests wait for it and
        // multiplex instead of each opening a connection.
        let mut guard = None;
        if plan.allow_h2 && !self.pool.is_h1_only(key) {
            let lock = self.pool.connect_lock(key);
            let waited_from = std::time::Instant::now();
            let g = lock.lock_owned().await;
            if let Some(handle) = self.pool.get_h2(key) {
                return Ok(Acquired::H2 {
                    handle,
                    reused: true,
                    timings: ConnectTimings::default(),
                });
            }
            if let Some(e) = self.pool.connect_error_since(key, waited_from) {
                // An additional connection couldn't be opened: wait for a stream on a busy
                // one rather than fail.
                if let Some(handle) = self.pool.any_h2(key) {
                    return Ok(Acquired::H2 {
                        handle,
                        reused: true,
                        timings: ConnectTimings::default(),
                    });
                }
                return Err(e);
            }
            if self.pool.is_h1_only(key) {
                drop(g);
                if !force_new && let Some(conn) = self.pool.take_h1(key, plan.owner) {
                    return Ok(Acquired::H1 {
                        conn,
                        reused: true,
                        timings: ConnectTimings::default(),
                    });
                }
            } else {
                guard = Some(g);
            }
        }
        let alpn = if plan.allow_h2 {
            Alpn::H2AndHttp1
        } else {
            Alpn::Http1
        };
        let result = async {
            let tls = Self::tls_target(
                cfg,
                plan.target,
                plan.insecure,
                alpn,
                plan.client_cert,
                plan.selection,
            )?;
            // TLS targets need a byte tunnel through HTTP proxies (CONNECT); plain-HTTP
            // targets use absolute-form on the proxy connection itself.
            let request = ConnectRequest {
                host: &plan.target.host,
                port: plan.target.port,
                route,
                tunnel: plan.target.tls,
                tls,
                restrict_loopback: plan.restrict_loopback,
            };
            let est = connect::establish(cfg, request).await?;
            self.finish_connect(cfg, plan, key, est).await
        }
        .await;
        if let Err(e) = &result {
            if guard.is_some() {
                self.pool.record_connect_error(key, e);
            }
            // Every HTTP/2 connection was busy and another one couldn't be opened: wait for a
            // stream on a busy one rather than fail.
            if plan.allow_h2
                && let Some(handle) = self.pool.any_h2(key)
            {
                return Ok(Acquired::H2 {
                    handle,
                    reused: true,
                    timings: ConnectTimings::default(),
                });
            }
        }
        drop(guard);
        result
    }

    async fn finish_connect(
        &self,
        cfg: &Config,
        plan: &Plan<'_>,
        key: &PoolKey,
        mut est: Established,
    ) -> Result<Acquired, UpstreamError> {
        let h2_spec = plan
            .selection
            .browser
            .map(|b| fingerprint::profiles::profile(b).h2);
        if est.h2
            && let Some(fp) = est.info.fingerprint.as_mut()
        {
            Arc::make_mut(fp).http2 = Some(h2::preface_fingerprint(h2_spec));
        }
        let meta = Arc::new(ConnMeta {
            key: key.clone(),
            generation: cfg.generation,
            info: est.info,
            peer: plan.target.peer().into(),
            absolute_form: est.absolute_form,
            proxy_auth: est.proxy_auth,
        });
        if est.h2 {
            let handle = h2::handshake(est.io, meta, h2_spec).await?;
            self.pool.put_h2(handle.clone());
            return Ok(Acquired::H2 {
                handle,
                reused: false,
                timings: est.timings,
            });
        }
        if plan.allow_h2 {
            self.pool.set_h1_only(key, cfg.generation);
        }
        Ok(Acquired::H1 {
            conn: H1Conn {
                io: est.io,
                buf: BytesMut::new(),
                meta,
                pinned: None,
            },
            reused: false,
            timings: est.timings,
        })
    }

    async fn send_upgrade(
        self: &Arc<Self>,
        head: &RequestHead,
        options: &RequestOptions,
    ) -> Result<
        (
            ResponseHead,
            Option<Upgraded>,
            Option<UpstreamResponse>,
            UpstreamInfo,
        ),
        UpstreamError,
    > {
        let started = Instant::now();
        let cfg = self.config.load_full();
        let target = Target::of(head)?;
        let routes = self
            .select_routes(&cfg, target.scheme, &target.host, target.port, &head.url)
            .await?;
        if options.restrict_loopback {
            self.check_target(&cfg, &target.host, &routes).await?;
        }
        let (head_timeout, idle_timeout) = Self::timeouts(&cfg, options);
        let insecure = target.tls
            && (options.insecure || cfg.tls.ignores_errors_for(&target.host, target.port));
        let client_cert = if target.tls {
            cfg.tls.client_cert_for(&target.host, target.port)
        } else {
            None
        };
        let selection = Self::selection(&cfg, &target, options);
        let mut last_error = None;
        for route in &routes {
            let tls =
                Self::tls_target(&cfg, &target, insecure, Alpn::Http1, client_cert, selection)?;
            // Upgrades always need a byte stream to the origin: CONNECT through HTTP proxies.
            let request = ConnectRequest {
                host: &target.host,
                port: target.port,
                route,
                tunnel: true,
                tls,
                restrict_loopback: options.restrict_loopback,
            };
            let est = match connect::establish(&cfg, request).await {
                Ok(est) => est,
                Err(e) => {
                    last_error = Some(e);
                    continue;
                }
            };
            let info = est.info.clone();
            let timings = est.timings.clone();
            let meta = Arc::new(ConnMeta {
                key: PoolKey {
                    tls: target.tls,
                    host: Arc::from(target.host.as_str()),
                    port: target.port,
                    proxy: route.proxy_id(),
                    insecure,
                    client_cert,
                    h1_alpn: target.tls,
                    browser: selection.browser,
                    restricted: options.restrict_loopback,
                },
                generation: cfg.generation,
                info: est.info,
                peer: target.peer().into(),
                absolute_form: false,
                proxy_auth: None,
            });
            let conn = H1Conn {
                io: est.io,
                buf: BytesMut::new(),
                meta,
                pinned: None,
            };
            let req = h1::H1Request {
                head,
                authority: &target.authority,
                body: RequestBody::Empty,
                title_case: cfg.title_case,
                head_timeout,
                idle_timeout,
                started,
                upgrade: true,
                owner: None,
            };
            // Upgrade connections are never pooled.
            let out = match h1::send(conn, req, None).await {
                Ok(out) => out,
                Err(h1::H1Error::Retryable { error, .. } | h1::H1Error::Fatal(error)) => {
                    return Err(error);
                }
            };
            let response_head = to_response_head(out.head);
            if let Some((io, leftover)) = out.upgraded {
                return Ok((response_head, Some(Upgraded { io, leftover }), None, info));
            }
            let response = UpstreamResponse {
                head: response_head.clone(),
                body: out
                    .body
                    .unwrap_or_else(|| Box::pin(futures::stream::empty())),
                info: info.clone(),
                timings,
                request_sent_ms: out.request_sent_ms,
            };
            return Ok((response_head, None, Some(response), info));
        }
        Err(last_error.unwrap_or_else(|| error::internal("no upstream route available")))
    }

    async fn connect_raw(
        self: &Arc<Self>,
        host: &str,
        port: u16,
        restrict_loopback: bool,
    ) -> Result<(BoxIo, UpstreamInfo, ConnectTimings), UpstreamError> {
        let cfg = self.config.load_full();
        let host = glob::normalize_host(host).into_owned();
        if host.is_empty() {
            return Err(error::internal("connect_raw: empty host"));
        }
        // Raw tunnels are overwhelmingly TLS; select proxies as for https.
        let url = format!("https://{}/", host_port(&host, port));
        let routes = self.select_routes(&cfg, "https", &host, port, &url).await?;
        if restrict_loopback {
            self.check_target(&cfg, &host, &routes).await?;
        }
        let mut last_error = None;
        for route in &routes {
            let request = ConnectRequest {
                host: &host,
                port,
                route,
                tunnel: true,
                tls: None,
                restrict_loopback,
            };
            match connect::establish(&cfg, request).await {
                Ok(est) => return Ok((est.io, est.info, est.timings)),
                Err(e) => last_error = Some(e),
            }
        }
        Err(last_error.unwrap_or_else(|| error::internal("no upstream route available")))
    }

    /// Fetch a PAC script (directly, never through a proxy), following redirects.
    async fn fetch_text(self: &Arc<Self>, url: &str) -> Result<String, UpstreamError> {
        let pac_error =
            |msg: String, code: &str| error::error(ErrorKind::UpstreamProxyFailed, msg, code);
        let mut current = url.trim().to_owned();
        for _ in 0..5 {
            let parsed = url::Url::parse(&current)
                .map_err(|e| pac_error(format!("invalid PAC URL {current:?}: {e}"), "PAC_URL"))?;
            match parsed.scheme() {
                "file" => {
                    let path = parsed.to_file_path().map_err(|_| {
                        pac_error(format!("invalid PAC file URL {current:?}"), "PAC_URL")
                    })?;
                    return tokio::fs::read_to_string(&path).await.map_err(|e| {
                        pac_error(
                            format!("cannot read PAC file {}: {e}", path.display()),
                            "PAC_FETCH",
                        )
                    });
                }
                "http" | "https" => {}
                other => {
                    return Err(pac_error(
                        format!("unsupported PAC URL scheme {other:?}"),
                        "PAC_URL",
                    ));
                }
            }
            let host = parsed.host_str().unwrap_or_default().to_owned();
            let mut path = parsed.path().to_owned();
            if let Some(q) = parsed.query() {
                path.push('?');
                path.push_str(q);
            }
            let authority = match parsed.port() {
                Some(p) => format!("{host}:{p}"),
                None => host.clone(),
            };
            let head = RequestHead {
                method: "GET".into(),
                url: current.clone(),
                scheme: parsed.scheme().into(),
                port: parsed.port_or_known_default().unwrap_or(80),
                host,
                path,
                http_version: HttpVersion::Http11,
                headers: Headers(vec![
                    ("Host".into(), authority),
                    ("User-Agent".into(), format!("{PRODUCT_NAME}/{VERSION}")),
                    (
                        "Accept".into(),
                        "application/x-ns-proxy-autoconfig, */*".into(),
                    ),
                ]),
            };
            let cfg = self.config.load_full();
            let target = Target::of(&head)?;
            let plan = Plan {
                head: &head,
                target: &target,
                insecure: target.tls && cfg.tls.ignores_errors_for(&target.host, target.port),
                client_cert: None,
                allow_h2: false,
                head_timeout: Some(PAC_FETCH_TIMEOUT),
                idle_timeout: Some(PAC_FETCH_TIMEOUT),
                started: Instant::now(),
                owner: None,
                selection: Selection::NATIVE,
                restrict_loopback: false,
            };
            let resp = tokio::time::timeout(
                PAC_FETCH_TIMEOUT,
                self.send_via(&cfg, &plan, &Route::Direct, RequestBody::Empty),
            )
            .await
            .map_err(|_| {
                error::timeout(
                    format_args!("fetching PAC script {current}"),
                    PAC_FETCH_TIMEOUT,
                )
            })?
            .map_err(|f| f.error)?;
            let status = resp.head.status;
            if matches!(status, 301 | 302 | 303 | 307 | 308)
                && let Some(next) = resp
                    .head
                    .headers
                    .get("location")
                    .and_then(|l| parsed.join(l).ok())
            {
                current = next.to_string();
                continue;
            }
            if status != 200 {
                return Err(pac_error(
                    format!("fetching PAC script {current} failed: HTTP {status}"),
                    "PAC_FETCH",
                ));
            }
            let mut body = resp.body;
            let mut out = Vec::new();
            let collect = async {
                while let Some(chunk) = body.next().await {
                    if let BodyChunk::Data(d) = chunk? {
                        if out.len() + d.len() > PAC_MAX_SIZE {
                            return Err(pac_error("PAC script too large".into(), "PAC_FETCH"));
                        }
                        out.extend_from_slice(&d);
                    }
                }
                Ok(())
            };
            tokio::time::timeout(PAC_FETCH_TIMEOUT, collect)
                .await
                .map_err(|_| {
                    error::timeout(
                        format_args!("downloading PAC script {current}"),
                        PAC_FETCH_TIMEOUT,
                    )
                })??;
            return Ok(String::from_utf8_lossy(&out).into_owned());
        }
        Err(pac_error(
            format!("too many redirects fetching PAC script {url}"),
            "PAC_FETCH",
        ))
    }
}

impl PacFetcher for Arc<ClientInner> {
    fn fetch_pac<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, UpstreamError>> {
        Box::pin(self.fetch_text(url))
    }
}
