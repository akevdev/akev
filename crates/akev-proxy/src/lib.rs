//! # akev-proxy
//!
//! The intercepting proxy engine. One listening port accepts, with protocol sniffing:
//! * HTTP/1.x proxy requests in absolute form (`GET http://host/path`),
//! * `CONNECT host:port` tunnels (then sniffs the tunnelled bytes again: TLS → MITM unless the
//!   host matches `tls_passthrough` or the ClientHello isn't interceptable; plain HTTP → parsed;
//!   anything else → raw tunnel),
//! * SOCKS4/4a/5 (no-auth and user/pass where the username can tag the client),
//! * TLS directly on the port (transparent mode, SNI decides the upstream),
//! * HTTP/2 prior knowledge (h2c preface) and HTTP/2 over TLS via ALPN,
//! * direct requests (origin-form) — treated as transparent requests to the `Host` header, or
//!   served locally when addressed to the proxy itself / the magic host `akev.cert`
//!   (certificate download & device setup page).
//!
//! HTTP/1 is implemented with a custom codec (httparse) so header casing, order and
//! duplicates are preserved exactly in both directions; HTTP/2 uses the `h2` crate.
//!
//! Every exchange runs through the pipeline (`pipeline` module): capture → rules
//! (`akev-rules`) → optional request breakpoint → delay/throttle → upstream (or
//! mock/map-local/close...) → optional response breakpoint → response transforms → client,
//! emitting [`akev_core::ProxyEvent`]s along the way.

#![warn(missing_docs)]

use akev_ca::CertificateAuthority;
use akev_core::settings::{CaptureSettings, ProxySettings};
use akev_core::*;
use akev_rules::RulesEngine;
use bytes::Bytes;
use std::net::SocketAddr;
use std::sync::Arc;

pub mod client_hello;
/// Entry points for the fuzz targets in `fuzz/` (not a stable API).
#[cfg(feature = "fuzzing")]
#[doc(hidden)]
pub mod fuzzing;
#[allow(missing_docs)]
pub mod upstream;

mod body;
mod conn;
mod errors;
mod events;
mod h2;
mod http1;
mod http3;
mod io;
mod local;
mod pipeline;
mod proxy_protocol;
mod responder;
mod send;
mod server;
mod shared;
mod sniff;
mod socks;
mod target;
mod throttle;
mod tls;
pub mod transparent;
mod tunnel;
mod tunnelled;
mod websocket;
mod ws_control;
mod ws_pump;

pub use tunnelled::TunnelledConnection;
pub use ws_control::WsInjectError;
// HTTP/3: listener status, upstream version choice, and the QUIC crate the public API uses.
pub use http3::Http3ListenerStatus;
pub use quinn;
pub use upstream::UpstreamHttpVersion;

use shared::{RuntimeSettings, Shared};
use std::sync::atomic::Ordering;

/// Errors returned by [`start`] and [`ProxyHandle::rebind`].
#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    /// No port in the fallback range (or the requested address) could be bound.
    #[error("failed to bind proxy port: {0}")]
    Bind(std::io::Error),
    /// Another I/O failure.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// Setup failure (TLS configuration, upstream client...).
    #[error("{0}")]
    Other(String),
}

/// Result type of this crate.
pub type Result<T> = std::result::Result<T, ProxyError>;

/// Called with the address of every client on another machine the access policy admits
/// (`ProxyHandle::set_client_observer`). Called on the accept path: must not block.
pub type ClientObserver = Arc<dyn Fn(std::net::IpAddr) + Send + Sync>;

/// Everything needed to start the proxy.
pub struct ProxyOptions {
    /// Listener, TLS, upstream and protocol settings.
    pub settings: ProxySettings,
    /// What to record (body capture limit, ignored hosts, tunnels, TLS failures...).
    pub capture: CaptureSettings,
    /// Certificate authority minting the leaf certificates for intercepted hosts.
    pub ca: Arc<CertificateAuthority>,
    /// Rules applied to every proxied exchange (and to Compose requests that opt in).
    pub rules: Arc<RulesEngine>,
    /// Receives every [`ProxyEvent`]; called on the hot path, must not block.
    pub sink: Arc<dyn EventSink>,
    /// Asked to resolve exchanges paused by `breakpoint` rules.
    pub breakpoints: Arc<dyn BreakpointHandler>,
}

/// Options for requests issued by Akev itself (Compose / Repeat).
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendOptions {
    /// Run the request through the user's rules (mocks, transforms...).
    pub apply_rules: bool,
    /// Follow redirects (each hop recorded as its own exchange).
    pub follow_redirects: bool,
    /// Response head timeout (ms); 0 = the proxy settings' default.
    pub timeout_ms: u64,
    /// Ignore upstream certificate errors for this request.
    pub insecure: bool,
    /// HTTP version: `auto` (settings decide), `http1`, `http2` (TCP, never HTTP/3) or `http3`
    /// (HTTP/3 first, falling back to TCP when QUIC fails).
    pub http_version: upstream::UpstreamHttpVersion,
}

impl Default for SendOptions {
    fn default() -> Self {
        SendOptions {
            apply_rules: false,
            follow_redirects: false,
            timeout_ms: 60_000,
            insecure: false,
            http_version: upstream::UpstreamHttpVersion::Auto,
        }
    }
}

/// Outcome of [`ProxyHandle::send`]. `body` holds the response body exactly as received (still
/// content-encoded); the API layer decodes it.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    /// The recorded exchange (also visible in the traffic list with `via: composer`).
    pub id: ExchangeId,
    /// The response head (from upstream or a rule); `None` when the exchange failed before
    /// one was received.
    pub response: Option<ResponseHead>,
    /// The response body as received (content-encoding intact).
    #[serde(skip)]
    pub body: Bytes,
    /// Why the exchange failed, if it did.
    pub error: Option<ExchangeError>,
    /// Timings of the (last) exchange.
    pub timing: Timing,
}

/// Live proxy counters.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStats {
    /// Client TCP connections currently open.
    pub active_connections: u64,
    /// Client TCP connections accepted since start.
    pub total_connections: u64,
    /// Exchanges currently in flight.
    pub active_exchanges: u64,
    /// Bytes received from clients (all layers, as seen on the client sockets).
    pub bytes_in: u64,
    /// Bytes sent to clients.
    pub bytes_out: u64,
    /// Bytes currently held for rules, scripts and breakpoints (across exchanges).
    pub rule_body_bytes: u64,
    /// Exchanges whose body was too large for `proxy.ruleBodyBudgetBytes` (or the
    /// per-exchange cap) and streamed through untouched.
    pub passed_through: u64,
    /// Connections refused by the client access policy (`proxy.access`).
    pub refused_connections: u64,
    /// Requests and handshakes refused for missing or wrong proxy credentials.
    pub auth_failures: u64,
}

/// A running proxy. Cloneable handle.
#[derive(Clone)]
pub struct ProxyHandle {
    inner: Arc<Inner>,
}

struct Inner {
    shared: Arc<Shared>,
    addr: parking_lot::RwLock<SocketAddr>,
    listener: tokio::sync::Mutex<server::ListenerHandle>,
    /// The transparent capture listener (`settings.transparent`).
    transparent: transparent::Manager,
    /// HTTP/3 (QUIC) listener.
    quic: http3::QuicListener,
}

/// How long in-flight exchanges get to finish on shutdown before connections are dropped.
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);

/// The listening socket of a stopped proxy, kept open for its successor (see
/// [`ProxyHandle::shutdown_keep_listener`] and [`start_on`]).
#[derive(Debug)]
pub struct Listener(tokio::net::TcpListener);

impl Listener {
    /// The address the socket is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.0.local_addr()
    }
}

/// Start the proxy. Tries `settings.port`, then the following ports up to +100 if busy
/// (port 0 = OS-assigned).
pub async fn start(options: ProxyOptions) -> Result<ProxyHandle> {
    start_with(options, None).await
}

/// Start the proxy on the listening socket of one that was shut down with
/// [`ProxyHandle::shutdown_keep_listener`], ignoring `settings.port` and `bind_address`: the
/// address stays the same, and nothing else can take the port in between.
pub async fn start_on(options: ProxyOptions, listener: Listener) -> Result<ProxyHandle> {
    start_with(options, Some(listener.0)).await
}

async fn start_with(
    options: ProxyOptions,
    listener: Option<tokio::net::TcpListener>,
) -> Result<ProxyHandle> {
    let ProxyOptions {
        settings,
        capture,
        ca,
        rules,
        sink,
        breakpoints,
    } = options;
    let upstream = upstream::UpstreamClient::new(&settings)
        .map_err(|e| ProxyError::Other(format!("upstream client setup failed: {e}")))?;
    let tls = tls::TlsConfigs::new(ca.clone())
        .map_err(|e| ProxyError::Other(format!("TLS setup failed: {e}")))?;
    let process_resolver = Arc::new(process::SystemProcessResolver::new());
    let listener = match listener {
        Some(l) => l,
        None => server::bind(settings.port, &settings.bind_address).map_err(ProxyError::Bind)?,
    };
    let quic = http3::QuicListener::new(ca.clone());
    let shutdown = tokio_util::sync::CancellationToken::new();
    let shared = Arc::new(Shared {
        ca,
        rules,
        sink,
        breakpoints,
        process_resolver,
        upstream,
        settings: arc_swap::ArcSwap::from_pointee(RuntimeSettings::new(settings, capture)),
        stats: shared::Stats::default(),
        tls,
        hard_stop: tokio_util::sync::CancellationToken::new(),
        shutdown,
        tracker: tokio_util::task::TaskTracker::new(),
        ws: Arc::default(),
        clients: shared::Clients::default(),
        client_observer: parking_lot::RwLock::new(None),
    });
    let handle = server::spawn_accept_loop(shared.clone(), listener)?;
    tracing::info!(addr = %handle.addr, "proxy listening");
    let transparent = transparent::Manager::new();
    transparent.reconcile(&shared).await;
    quic.reconcile(&shared, &shared.settings().proxy, handle.addr.port());
    Ok(ProxyHandle {
        inner: Arc::new(Inner {
            shared,
            addr: parking_lot::RwLock::new(handle.addr),
            listener: tokio::sync::Mutex::new(handle),
            transparent,
            quic,
        }),
    })
}

impl ProxyHandle {
    /// Address the proxy is listening on.
    pub fn local_addr(&self) -> SocketAddr {
        *self.inner.addr.read()
    }

    /// Port the proxy is listening on.
    pub fn port(&self) -> u16 {
        self.local_addr().port()
    }

    /// Apply new settings live (everything except port/bind address, which need a restart via
    /// [`ProxyHandle::rebind`]).
    pub fn update_settings(&self, settings: ProxySettings, capture: CaptureSettings) {
        let shared = &self.inner.shared;
        shared.upstream.update_settings(&settings);
        (self.inner.quic).reconcile(shared, &settings, self.port());
        let runtime = Arc::new(RuntimeSettings::new(settings, capture));
        shared.settings.store(runtime.clone());
        // A client that is refused now (blocked, or outside a narrower policy) loses its
        // open connections too.
        shared.clients.cut(|ip| runtime.access.admits(ip));
        // Start / move / stop the transparent listener to match (see `sync_transparent`).
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let this = self.clone();
            rt.spawn(async move { this.sync_transparent().await });
        }
    }

    /// Re-listen on a new port / bind address (existing connections keep running).
    pub async fn rebind(&self, port: u16, bind_address: &str) -> Result<SocketAddr> {
        let shared = &self.inner.shared;
        let mut current = self.inner.listener.lock().await;
        let old_addr = current.addr;
        let new_ip = server::parse_bind_ip(bind_address).map_err(ProxyError::Bind)?;
        if port != 0 && port == old_addr.port() && new_ip == old_addr.ip() {
            return Ok(old_addr);
        }
        let listener = if port != 0 && port == old_addr.port() {
            // Same port on another address: free it first, restore the old socket on failure.
            current.stop().await;
            match server::bind(port, bind_address) {
                Ok(l) => l,
                Err(e) => {
                    if let Ok(l) = server::bind(old_addr.port(), &old_addr.ip().to_string())
                        && let Ok(h) = server::spawn_accept_loop(shared.clone(), l)
                    {
                        *self.inner.addr.write() = h.addr;
                        *current = h;
                    }
                    return Err(ProxyError::Bind(e));
                }
            }
        } else {
            server::bind(port, bind_address).map_err(ProxyError::Bind)?
        };
        let new = server::spawn_accept_loop(shared.clone(), listener)?;
        let addr = new.addr;
        let mut old = std::mem::replace(&mut *current, new);
        old.stop().await;
        *self.inner.addr.write() = addr;
        let settings = shared.settings();
        let mut proxy = settings.proxy.clone();
        proxy.port = addr.port();
        proxy.bind_address = bind_address.to_string();
        shared.settings.store(Arc::new(RuntimeSettings::new(
            proxy,
            settings.capture.clone(),
        )));
        (self.inner.quic).reconcile(shared, &shared.settings().proxy, addr.port());
        tracing::info!(%addr, "proxy re-listening");
        self.inner.transparent.reconcile(shared).await;
        Ok(addr)
    }

    /// Issue a request from Akev itself (Compose / Repeat). The exchange is recorded through
    /// the event sink like any other.
    pub async fn send(&self, request: EditableRequest, options: SendOptions) -> SendResult {
        send::send(&self.inner.shared, request, options).await
    }

    /// Inject a text or binary message into the live intercepted WebSocket `id`: towards the
    /// server (`Direction::Sent`, masked as a client frame) or the client
    /// (`Direction::Received`). It is written as soon as that direction is between messages,
    /// recorded as a `modified` stream message, and its `seq` returned. Fails at once when the
    /// WebSocket isn't open (or that direction finished), for a `ws-echo` connection's server
    /// side, for other message kinds, invalid UTF-8 text, or when too many messages are queued.
    pub async fn inject_ws_message(
        &self,
        id: ExchangeId,
        direction: Direction,
        kind: MessageKind,
        payload: Bytes,
    ) -> std::result::Result<u32, WsInjectError> {
        let written = self.inner.shared.ws.inject(id, direction, kind, payload)?;
        written.await.map_err(|_| WsInjectError::Closed)
    }

    /// Whether `id` is a live intercepted WebSocket (messages can be injected).
    pub fn ws_is_open(&self, id: ExchangeId) -> bool {
        self.inner.shared.ws.is_open(id)
    }

    /// Current counters.
    pub fn stats(&self) -> ProxyStats {
        let s = &self.inner.shared.stats;
        ProxyStats {
            active_connections: s.active_connections.load(Ordering::Relaxed),
            total_connections: s.total_connections.load(Ordering::Relaxed),
            active_exchanges: s.active_exchanges.load(Ordering::Relaxed),
            bytes_in: s.bytes.bytes_in.load(Ordering::Relaxed),
            bytes_out: s.bytes.bytes_out.load(Ordering::Relaxed),
            rule_body_bytes: s.rule_body_bytes.load(Ordering::Relaxed),
            passed_through: s.passed_through.load(Ordering::Relaxed),
            refused_connections: s.refused_connections.load(Ordering::Relaxed),
            auth_failures: s.auth_failures.load(Ordering::Relaxed),
        }
    }

    /// Be told about every client on another machine the access policy admits (each
    /// connection; the observer dedups). Replaces any earlier observer.
    pub fn set_client_observer(&self, observer: ClientObserver) {
        *self.inner.shared.client_observer.write() = Some(observer);
    }

    /// Live status of the transparent capture listener (see [`transparent`]).
    pub fn transparent_status(&self) -> akev_core::transparent::TransparentStatus {
        self.inner.transparent.status()
    }

    /// Apply the current `transparent` settings (and bind address) to the transparent listener
    /// now and return its status. [`ProxyHandle::update_settings`] does this in the background;
    /// await this to see the result.
    pub async fn sync_transparent(&self) -> akev_core::transparent::TransparentStatus {
        self.inner.transparent.reconcile(&self.inner.shared).await;
        self.inner.transparent.status()
    }

    /// Replace how the original destination of connections on the transparent listener is
    /// found (default: [`transparent::KernelOriginalDestination`]). For tests and embedders
    /// with their own redirect mechanism.
    pub fn set_original_destination_lookup(
        &self,
        lookup: Arc<dyn transparent::OriginalDestination>,
    ) {
        self.inner.transparent.set_lookup(lookup);
    }

    /// Stop accepting and close all connections.
    pub async fn shutdown(&self) {
        self.stop(false).await;
    }

    /// Shut down like [`ProxyHandle::shutdown`], but keep the listening socket open and return
    /// it for [`start_on`]. Clients that connect meanwhile wait in its backlog for the new
    /// proxy instead of being refused. `None` if the proxy was already shut down.
    pub async fn shutdown_keep_listener(&self) -> Option<Listener> {
        self.stop(true).await
    }

    async fn stop(&self, keep_listener: bool) -> Option<Listener> {
        let shared = &self.inner.shared;
        shared.shutdown.cancel();
        // Not kept: closed right away, so new clients are refused rather than left waiting.
        let listener = (self.inner.listener.lock().await.stop().await).filter(|_| keep_listener);
        self.inner.transparent.shutdown().await;
        self.inner.quic.stop();
        shared.tracker.close();
        if tokio::time::timeout(SHUTDOWN_GRACE, shared.tracker.wait())
            .await
            .is_err()
        {
            tracing::debug!("shutdown grace period over; dropping remaining connections");
        }
        shared.hard_stop.cancel();
        let _ =
            tokio::time::timeout(std::time::Duration::from_secs(2), shared.tracker.wait()).await;
        listener.map(Listener)
    }
}

impl akev_core::transparent::TransparentProbe for ProxyHandle {
    fn transparent_status(&self) -> akev_core::transparent::TransparentStatus {
        ProxyHandle::transparent_status(self)
    }
}

/// Platform process resolution (Linux: /proc/net/tcp{,6} + /proc/*/fd; macOS: `lsof`/libproc;
/// Windows: GetExtendedTcpTable). Results cached briefly per client port.
pub mod process;
