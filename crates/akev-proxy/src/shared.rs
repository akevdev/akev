//! State shared by every connection of a running proxy: dependencies (CA, rules, sink,
//! breakpoints, upstream client), live settings (swapped atomically), counters and shutdown
//! signals.

use crate::io::ByteCounters;
use crate::process::SystemProcessResolver;
use crate::tls::TlsConfigs;
use crate::upstream::UpstreamClient;
use akev_ca::CertificateAuthority;
use akev_core::access::ClientAccess;
use akev_core::settings::{CaptureSettings, ProxySettings};
use akev_core::{BreakpointHandler, EventSink};
use akev_rules::RulesEngine;
use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio_util::sync::CancellationToken;
use tokio_util::task::TaskTracker;

/// Hostname pattern lists (`*.example.com`, `api.*`, `host:8443`): one syntax for every
/// settings host list, see [`akev_core::hostpat`].
pub(crate) use akev_core::hostpat::HostMatcher;

/// Settings snapshot with pre-compiled matchers. Swapped atomically on update.
pub(crate) struct RuntimeSettings {
    pub proxy: ProxySettings,
    pub capture: CaptureSettings,
    pub passthrough: HostMatcher,
    pub ignore: HostMatcher,
    /// Who may use the proxy (`proxy.access`, compiled).
    pub access: ClientAccess,
}

impl RuntimeSettings {
    pub(crate) fn new(proxy: ProxySettings, capture: CaptureSettings) -> Self {
        let passthrough = HostMatcher::new(&proxy.tls_passthrough);
        let ignore = HostMatcher::new(&capture.ignore_hosts);
        let access = ClientAccess::new(&proxy.access);
        RuntimeSettings {
            proxy,
            capture,
            passthrough,
            ignore,
            access,
        }
    }

    /// Capture limit per body, as usize.
    pub(crate) fn capture_limit(&self) -> usize {
        usize::try_from(self.capture.max_body_bytes).unwrap_or(usize::MAX)
    }
}

/// Live counters (see [`crate::ProxyStats`]).
#[derive(Default)]
pub(crate) struct Stats {
    pub active_connections: AtomicU64,
    pub total_connections: AtomicU64,
    pub active_exchanges: AtomicU64,
    pub bytes: Arc<ByteCounters>,
    /// Bytes held across exchanges for rules, scripts and breakpoints (the
    /// `proxy.ruleBodyBudgetBytes` budget in use).
    pub rule_body_bytes: AtomicU64,
    /// Exchanges whose body was too large for that budget and streamed through untouched.
    pub passed_through: AtomicU64,
    /// Connections closed because the access policy refused the client.
    pub refused_connections: AtomicU64,
    /// Requests / handshakes refused for missing or wrong proxy credentials.
    pub auth_failures: AtomicU64,
}

/// Open client connections by address, so blocking a client cuts its connections too.
#[derive(Default)]
pub(crate) struct Clients {
    next: AtomicU64,
    live: parking_lot::Mutex<HashMap<u64, (IpAddr, CancellationToken)>>,
}

impl Clients {
    /// Track a connection from `ip` until the guard is dropped; its token is cancelled when
    /// the client is refused later ([`Clients::cut`]).
    pub(crate) fn register(&self, ip: IpAddr) -> ClientGuard<'_> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let token = CancellationToken::new();
        self.live.lock().insert(id, (ip, token.clone()));
        ClientGuard {
            clients: self,
            id,
            token,
        }
    }

    /// The connection `id` turned out to be from `ip` (a trusted PROXY protocol header).
    pub(crate) fn update(&self, id: u64, ip: IpAddr) {
        if let Some(entry) = self.live.lock().get_mut(&id) {
            entry.0 = ip;
        }
    }

    /// Cancel the connections of every client `admitted` refuses.
    pub(crate) fn cut(&self, admitted: impl Fn(IpAddr) -> bool) {
        for (ip, token) in self.live.lock().values() {
            if !admitted(*ip) {
                token.cancel();
            }
        }
    }
}

/// A tracked connection (see [`Clients::register`]).
pub(crate) struct ClientGuard<'a> {
    clients: &'a Clients,
    pub id: u64,
    pub token: CancellationToken,
}

impl Drop for ClientGuard<'_> {
    fn drop(&mut self) {
        self.clients.live.lock().remove(&self.id);
    }
}

/// Decrements a counter when dropped.
pub(crate) struct CounterGuard<'a>(&'a AtomicU64);

impl<'a> CounterGuard<'a> {
    pub(crate) fn new(counter: &'a AtomicU64) -> Self {
        counter.fetch_add(1, Ordering::Relaxed);
        CounterGuard(counter)
    }
}

impl Drop for CounterGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Everything a connection task needs.
pub(crate) struct Shared {
    pub ca: Arc<CertificateAuthority>,
    pub rules: Arc<RulesEngine>,
    pub sink: Arc<dyn EventSink>,
    pub breakpoints: Arc<dyn BreakpointHandler>,
    pub process_resolver: Arc<SystemProcessResolver>,
    pub upstream: UpstreamClient,
    pub settings: ArcSwap<RuntimeSettings>,
    pub stats: Stats,
    pub tls: TlsConfigs,
    /// Graceful shutdown requested: stop accepting, finish in-flight work, close idle connections.
    pub shutdown: CancellationToken,
    /// Hard stop: abandon everything still running.
    pub hard_stop: CancellationToken,
    pub tracker: TaskTracker,
    /// Open WebSockets (message injection).
    pub ws: Arc<crate::ws_control::WsRegistry>,
    /// Open client connections by address (cut when a client gets blocked).
    pub clients: Clients,
    /// Told about every admitted client on another machine (`ProxyHandle::set_client_observer`).
    pub client_observer: parking_lot::RwLock<Option<crate::ClientObserver>>,
}

impl Shared {
    pub(crate) fn settings(&self) -> Arc<RuntimeSettings> {
        self.settings.load_full()
    }

    /// Admission of a client at `ip` on a proxy listener: refused clients are counted (the
    /// caller closes the connection without a response); admitted clients on other machines
    /// are reported to the observer.
    pub(crate) fn admit(&self, ip: IpAddr) -> bool {
        if !self.settings().access.admits(ip) {
            self.stats
                .refused_connections
                .fetch_add(1, Ordering::Relaxed);
            tracing::debug!(client = %ip, "client refused by the proxy access policy; closing");
            return false;
        }
        if !akev_core::access::is_loopback(ip)
            && let Some(observer) = self.client_observer.read().clone()
        {
            observer(akev_core::access::canonical_ip(ip));
        }
        true
    }

    /// Count a refused authentication.
    pub(crate) fn auth_failed(&self, client: std::net::SocketAddr, how: &str) {
        self.stats.auth_failures.fetch_add(1, Ordering::Relaxed);
        tracing::debug!(%client, "proxy authentication failed ({how})");
    }
}
