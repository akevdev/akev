//! The running application: owns the CA, rules engine, store, proxy, interceptors and the
//! state shared by the HTTP API, the live stream and MCP.

use crate::breakpoints::{BreakpointManager, BreakpointNotice};
use crate::error::{ApiError, ApiResult};
use crate::license::LicenseManager;
use crate::persist::{self, DaemonInfo, LoadOutcome, RULES_FILE, SETTINGS_FILE, StateFile};
use crate::sessions::{RestoredSession, Sessions};
use crate::sink::RecordingSink;
use crate::stream::messages::ServerMessage;
use akev_ca::{CaInfo, CertificateAuthority};
use akev_core::EventSink;
use akev_core::rules::RuleSet;
use akev_core::settings::{ProxySettings, Settings, UpstreamProxyMode};
use akev_intercept::{InterceptContext, InterceptorRegistry};
use akev_proxy::{ProxyHandle, ProxyOptions};
use akev_rules::RulesEngine;
use akev_store::Store;
use axum::extract::ws::Utf8Bytes;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, watch};
use tokio::task::JoinHandle;

/// Capacity of the app-level broadcast bus (control messages for stream connections).
const BUS_CAPACITY: usize = 1024;

/// `Status` (see the HTTP API reference, akev.dev/docs/api).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// Akev version.
    pub version: &'static str,
    /// Where the proxy listens.
    pub proxy: ProxyStatus,
    /// Base URL of this API (for clients on this machine).
    pub api_url: String,
    /// `https://` URL other devices use when the API is reachable from the network.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network_url: Option<String>,
    /// Data directory in use.
    pub data_dir: String,
    /// The CA certificate clients must trust.
    pub ca: CaInfo,
    /// When the instance started (unix ms).
    pub started_at: f64,
    /// Live counters.
    pub stats: StatusStats,
    /// `linux`, `macos` or `windows`.
    pub platform: &'static str,
    /// Running inside the desktop shell (`AkevOptions::desktop`).
    pub desktop: bool,
    /// The autosaved session restored at startup (once loaded).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_session: Option<RestoredSession>,
    /// Plan and licence state (`GET /api/license`).
    pub license: akev_core::license::LicenseStatus,
}

/// Proxy part of [`Status`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProxyStatus {
    /// Actual listening port.
    pub port: u16,
    /// The bind address in effect (settings, or `--proxy-bind` for this run).
    pub bind_address: String,
    /// `host:port` addresses clients can use, incl. LAN IPs when bound to all interfaces.
    pub addresses: Vec<String>,
    /// The HTTP/3 (QUIC) listener (`settings.proxy.http3.listen`).
    pub http3: akev_proxy::Http3ListenerStatus,
}

/// Live counters (also streamed every 2 s as `stats`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusStats {
    /// Exchanges currently stored.
    pub exchanges: u64,
    /// Body bytes held in memory.
    pub memory_body_bytes: u64,
    /// Body bytes spilled to disk.
    pub spilled_body_bytes: u64,
    /// Open client connections.
    pub active_connections: u64,
    /// Client connections accepted since start.
    pub total_connections: u64,
    /// Exchanges evicted by `capture.maxExchanges` since start.
    pub evicted: u64,
    /// Bodies (and message payloads) not recorded because capture was overloaded.
    pub dropped_bodies: u64,
    /// Spilled bodies deleted to stay within `capture.spillMaxBytes`.
    pub disk_dropped_bodies: u64,
    /// Exchanges whose bodies were too large for `proxy.ruleBodyBudgetBytes` and streamed
    /// through untouched.
    pub passed_through: u64,
    /// Breakpoint pauses released automatically or skipped because of
    /// `breakpoints.maxPaused`.
    pub auto_released: u64,
    /// Bytes the spill files occupy on disk.
    pub spill_disk_bytes: u64,
    /// Connections refused by the client access policy (`proxy.access`).
    pub refused_connections: u64,
    /// Proxy requests refused for missing or wrong proxy credentials.
    pub auth_failures: u64,
}

/// Everything resolved during startup, handed to [`App::new`].
pub(crate) struct AppParts {
    /// Running inside the desktop shell (`AkevOptions::desktop`).
    pub desktop: bool,
    pub data_dir: PathBuf,
    pub temp_dir: Option<tempfile::TempDir>,
    pub persist: bool,
    pub token: String,
    pub api_addr: SocketAddr,
    pub api_url: String,
    /// `https://` URL for other machines (network-exposed APIs only).
    pub network_url: Option<String>,
    /// The API's TLS certificate (network-exposed APIs only).
    pub api_tls: Option<Arc<crate::tls::ApiTls>>,
    pub settings: Settings,
    pub ca: Arc<CertificateAuthority>,
    pub ca_path: PathBuf,
    pub rules: Arc<RulesEngine>,
    pub store: Arc<Store>,
    pub proxy: ProxyHandle,
    pub recording: Arc<RecordingSink>,
    pub breakpoints: Arc<BreakpointManager>,
    pub bus: broadcast::Sender<Utf8Bytes>,
    pub interceptors: InterceptorRegistry,
    /// The proxy port that was asked for (settings or `--proxy-port`), 0 = any.
    pub preferred_proxy_port: u16,
    /// The proxy bind address in effect (settings or `--proxy-bind`), normalized.
    pub proxy_bind: String,
    /// MCP may activate interceptors that launch arbitrary programs.
    pub mcp_allow_launch: bool,
    /// MCP tool calls may reveal secrets (`reveal: true`).
    pub mcp_reveal_secrets: bool,
    /// Folders MCP may read and write files in.
    pub mcp_files: crate::mcp::files::McpFiles,
    /// UI-only session secret for network-exposed APIs (see `auth::Security::with_ui_session`).
    pub ui_session: Option<String>,
    /// Protobuf schema registry (`/api/protos`).
    pub protos: Arc<crate::protos::ProtoRegistry>,
    /// Saved sessions & autosave.
    pub sessions: Arc<Sessions>,
    /// WireGuard mode (`settings.proxy.wireguard`).
    pub wireguard: Arc<crate::wireguard::WireguardManager>,
    /// Akev Pro licence and trial.
    pub license: Arc<LicenseManager>,
}

pub(crate) struct App {
    pub data_dir: PathBuf,
    temp_dir: Mutex<Option<tempfile::TempDir>>,
    pub token: String,
    pub api_addr: SocketAddr,
    pub api_url: String,
    /// `https://` URL other machines use (network-exposed APIs only).
    pub network_url: Option<String>,
    /// The API's TLS certificate, re-issued when the CA is regenerated.
    pub api_tls: Option<Arc<crate::tls::ApiTls>>,
    pub started_at: f64,
    pub desktop: bool,
    settings: RwLock<Settings>,
    /// `settings.json` / `rules.json` writers (None for ephemeral sessions).
    settings_file: Option<Arc<StateFile>>,
    rules_file: Option<Arc<StateFile>>,
    /// Serializes rule set replacements with their persistence (saves stay in order).
    rules_update: Mutex<()>,
    /// Serializes settings updates / proxy restarts (they await the proxy).
    pub reconfigure: tokio::sync::Mutex<()>,
    ca: RwLock<Arc<CertificateAuthority>>,
    pub ca_path: PathBuf,
    pub rules: Arc<RulesEngine>,
    pub store: Arc<Store>,
    proxy: RwLock<ProxyHandle>,
    pub recording: Arc<RecordingSink>,
    pub breakpoints: Arc<BreakpointManager>,
    pub interceptors: InterceptorRegistry,
    /// The CA in the operating system's certificate stores (`/api/ca/trust`; REST only).
    pub ca_trust: akev_intercept::CaTrust,
    /// New-version checks (`/api/update`).
    pub updates: crate::updates::UpdateChecker,
    /// The proxy port asked for (settings, `--proxy-port`, or the last port set through the
    /// settings); differs from the actual one when it was busy.
    preferred_proxy_port: std::sync::atomic::AtomicU16,
    /// The proxy bind address in effect. Starts as the settings' address or the
    /// `--proxy-bind` override (which is never persisted) and follows explicit changes of
    /// `proxy.bindAddress` through the settings.
    proxy_bind: RwLock<String>,
    /// MCP may activate interceptors that launch arbitrary programs (`electron`).
    pub mcp_allow_launch: bool,
    /// MCP tool calls may pass `reveal: true` (`--mcp-reveal-secrets`).
    pub mcp_reveal_secrets: bool,
    /// Folders MCP may read and write files in (`<data dir>/exports`, `--mcp-allow-path`).
    pub mcp_files: crate::mcp::files::McpFiles,
    /// Secret of the UI-only session cookie (network-exposed APIs only).
    pub ui_session: Option<String>,
    intercept_ctx: RwLock<Arc<InterceptContext>>,
    pub bus: broadcast::Sender<Utf8Bytes>,
    /// Last broadcast `interceptors` message (re-sent on stream resync).
    pub last_interceptors: Mutex<Option<Utf8Bytes>>,
    shutdown_requested: watch::Sender<bool>,
    shutdown_started: AtomicBool,
    shutdown_done: watch::Sender<bool>,
    pub tasks: Mutex<Vec<JoinHandle<()>>>,
    daemon_info: Mutex<Option<DaemonInfo>>,
    suggest_cache: Mutex<Option<(Instant, Arc<akev_filter::SuggestContext>)>>,
    /// Bounds concurrent body views (decode + format + structured decode): each can hold
    /// several copies of a large body, so parallel views must not multiply peak memory.
    body_views: Arc<tokio::sync::Semaphore>,
    /// Protobuf schemas used to decode protobuf / gRPC bodies (`/api/protos`).
    pub protos: Arc<crate::protos::ProtoRegistry>,
    /// Saved sessions & autosave.
    pub sessions: Arc<Sessions>,
    /// WireGuard mode: the embedded WireGuard server and its keys.
    pub wireguard: Arc<crate::wireguard::WireguardManager>,
    /// Akev Pro licence and trial (see `license.rs`).
    pub license: Arc<LicenseManager>,
    /// Addresses of devices on other machines that connected to the proxy (see
    /// [`client_observer`]).
    pub seen_clients: Arc<Mutex<std::collections::HashSet<IpAddr>>>,
}

/// How many body views may be built at once (see [`App::body_view_permit`]).
pub(crate) const BODY_VIEW_PERMITS: usize = 3;

impl App {
    pub(crate) fn new(parts: AppParts) -> Arc<App> {
        let ca_info = parts.ca.info();
        let mut ctx = build_intercept_ctx(
            &parts.ca,
            &ca_info,
            &parts.ca_path,
            parts.proxy.local_addr(),
            &parts.data_dir,
            &parts.api_url,
            &parts.token,
        );
        ctx.transparent = Some(transparent_probe(&parts.proxy));
        let (shutdown_requested, _) = watch::channel(false);
        let (shutdown_done, _) = watch::channel(false);
        let settings_file = parts
            .persist
            .then(|| StateFile::new(&parts.data_dir, SETTINGS_FILE));
        let rules_file = parts
            .persist
            .then(|| StateFile::new(&parts.data_dir, RULES_FILE));
        Arc::new(App {
            data_dir: parts.data_dir,
            temp_dir: Mutex::new(parts.temp_dir),
            settings_file,
            rules_file,
            rules_update: Mutex::new(()),
            token: parts.token,
            api_addr: parts.api_addr,
            api_url: parts.api_url,
            network_url: parts.network_url,
            api_tls: parts.api_tls,
            started_at: now_ms(),
            desktop: parts.desktop,
            settings: RwLock::new(parts.settings),
            reconfigure: tokio::sync::Mutex::new(()),
            ca: RwLock::new(parts.ca),
            ca_path: parts.ca_path,
            rules: parts.rules,
            store: parts.store,
            proxy: RwLock::new(parts.proxy),
            recording: parts.recording,
            breakpoints: parts.breakpoints,
            interceptors: parts.interceptors,
            ca_trust: akev_intercept::CaTrust::system(),
            // AKEV_UPDATE_MANIFEST_URL points forks and tests at their own manifest.
            updates: crate::updates::UpdateChecker::new(
                std::env::var("AKEV_UPDATE_MANIFEST_URL")
                    .unwrap_or_else(|_| crate::updates::MANIFEST_URL.to_string()),
            ),
            preferred_proxy_port: std::sync::atomic::AtomicU16::new(parts.preferred_proxy_port),
            proxy_bind: RwLock::new(parts.proxy_bind),
            mcp_allow_launch: parts.mcp_allow_launch,
            mcp_reveal_secrets: parts.mcp_reveal_secrets,
            mcp_files: parts.mcp_files,
            ui_session: parts.ui_session,
            intercept_ctx: RwLock::new(Arc::new(ctx)),
            bus: parts.bus,
            last_interceptors: Mutex::new(None),
            shutdown_requested,
            shutdown_started: AtomicBool::new(false),
            shutdown_done,
            tasks: Mutex::new(Vec::new()),
            daemon_info: Mutex::new(None),
            suggest_cache: Mutex::new(None),
            body_views: Arc::new(tokio::sync::Semaphore::new(BODY_VIEW_PERMITS)),
            protos: parts.protos,
            sessions: parts.sessions,
            wireguard: parts.wireguard,
            license: parts.license,
            seen_clients: Arc::default(),
        })
    }

    /// Report new devices connecting to the proxy to the UI (`client-connected`).
    pub fn watch_clients(&self) {
        self.proxy()
            .set_client_observer(client_observer(self.bus.clone(), self.seen_clients.clone()));
    }

    // ----- accessors -----

    pub fn proxy(&self) -> ProxyHandle {
        self.proxy.read().clone()
    }

    pub fn ca(&self) -> Arc<CertificateAuthority> {
        self.ca.read().clone()
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().clone()
    }

    /// Context for interceptors (LAN addresses refreshed when the network changed, proxy
    /// credentials from the current settings).
    pub fn intercept_ctx(&self) -> Arc<InterceptContext> {
        let ctx = self.intercept_ctx.read().clone();
        let lan: Vec<String> = crate::net::lan_ipv4_addresses()
            .into_iter()
            .map(|ip| ip.to_string())
            .collect();
        let credentials = self
            .settings()
            .proxy
            .access
            .credentials()
            .map(|(u, p)| (u.to_string(), p.to_string()));
        if lan == ctx.lan_addresses && credentials == ctx.proxy_credentials {
            return ctx;
        }
        let mut updated = (*ctx).clone();
        updated.lan_addresses = lan;
        updated.proxy_credentials = credentials;
        let updated = Arc::new(updated);
        *self.intercept_ctx.write() = updated.clone();
        updated
    }

    /// A warning when the proxy couldn't get the port it was asked for.
    pub fn port_fallback_notice(&self) -> Option<String> {
        let preferred = self.preferred_proxy_port.load(Ordering::Acquire);
        let actual = self.proxy().local_addr().port();
        (preferred != 0 && preferred != actual).then(|| {
            format!(
                "Port {preferred} was busy, so the proxy listens on port {actual}. Clients configured for port {preferred} must use {actual} (or free the port and choose “Use port {preferred}” in Settings → Proxy)."
            )
        })
    }

    pub fn ui_url(&self) -> String {
        crate::launcher::ui_url_for(&self.api_url, &self.token)
    }

    /// The proxy bind address in effect (may differ from the stored settings for this run).
    pub fn proxy_bind(&self) -> String {
        self.proxy_bind.read().clone()
    }

    /// Distinct values for filter autocompletion (cached briefly: computing them walks the store).
    pub fn suggest_context(&self) -> Arc<akev_filter::SuggestContext> {
        let mut cache = self.suggest_cache.lock();
        if let Some((at, ctx)) = cache.as_ref()
            && at.elapsed() < Duration::from_secs(2)
        {
            return ctx.clone();
        }
        let ctx = Arc::new(self.store.suggest_context());
        *cache = Some((Instant::now(), ctx.clone()));
        ctx
    }

    // ----- status -----

    pub fn stats(&self) -> StatusStats {
        let store = self.store.stats();
        let proxy = self.proxy().stats();
        StatusStats {
            exchanges: store.exchanges,
            memory_body_bytes: store.memory_body_bytes,
            spilled_body_bytes: store.spilled_body_bytes,
            active_connections: proxy.active_connections,
            total_connections: proxy.total_connections,
            evicted: store.evicted,
            dropped_bodies: store.dropped_bodies + store.dropped_messages,
            disk_dropped_bodies: store.disk_dropped_bodies,
            passed_through: proxy.passed_through,
            auto_released: self.breakpoints.limited_count(),
            spill_disk_bytes: store.spill_disk_bytes,
            refused_connections: proxy.refused_connections,
            auth_failures: proxy.auth_failures,
        }
    }

    pub fn proxy_status(&self) -> ProxyStatus {
        let addr = self.proxy().local_addr();
        let bind_address = self.proxy_bind();
        ProxyStatus {
            port: addr.port(),
            bind_address,
            addresses: proxy_addresses(addr),
            http3: self.proxy().http3_status(),
        }
    }

    pub fn status(&self) -> Status {
        Status {
            version: akev_core::VERSION,
            proxy: self.proxy_status(),
            api_url: self.api_url.clone(),
            network_url: self.network_url.clone(),
            data_dir: self.data_dir.display().to_string(),
            ca: self.ca().info(),
            started_at: self.started_at,
            stats: self.stats(),
            platform: platform(),
            desktop: self.desktop,
            restored_session: self.sessions.restored(),
            license: self.license.status(),
        }
    }

    // ----- broadcasting -----

    /// Broadcast a server message to all stream connections.
    pub fn broadcast(&self, message: &ServerMessage<'_>) -> Option<Utf8Bytes> {
        let text = message.to_text()?;
        if self.bus.receiver_count() > 0 {
            let _ = self.bus.send(text.clone());
        }
        Some(text)
    }

    pub fn notice(&self, level: &str, message: &str) {
        self.broadcast(&ServerMessage::Notice { level, message });
    }

    pub fn has_stream_clients(&self) -> bool {
        self.bus.receiver_count() > 0
    }

    // ----- rules -----

    /// Apply, persist (in the background) and broadcast a new rule set. Returns compilation
    /// errors and duplicate-id problems.
    ///
    /// Compiling runs the top-level code of every script rule (bounded by a watchdog, but it
    /// can take seconds), so async callers use [`App::set_rules_blocking`].
    pub fn set_rules(&self, rules: RuleSet) -> Vec<(String, String)> {
        self.replace_rules(|_| rules)
    }

    /// Replace the rule set with `edit(current)` under the update lock, so concurrent updates
    /// (e.g. two `akev_add_mock` calls) don't lose each other's changes. Compiles; see
    /// [`App::set_rules`].
    pub fn update_rules(&self, edit: impl FnOnce(&mut RuleSet)) -> Vec<(String, String)> {
        self.replace_rules(|engine| {
            let mut rules = engine.rules();
            edit(&mut rules);
            rules
        })
    }

    fn replace_rules(&self, next: impl FnOnce(&RulesEngine) -> RuleSet) -> Vec<(String, String)> {
        let (rules, mut errors) = {
            let _guard = self.rules_update.lock();
            let rules = next(&self.rules);
            self.rules.set_rules(rules.clone());
            self.store.set_rule_titles(rule_titles(&rules));
            if let Some(file) = &self.rules_file {
                file.save(&rules);
            }
            let errors = self.rules.errors();
            (rules, errors)
        };
        errors.extend(duplicate_rule_ids(&rules));
        self.broadcast(&ServerMessage::Rules { rules: &rules });
        errors
    }

    /// [`App::set_rules`] on a blocking thread, off the async workers.
    pub async fn set_rules_blocking(
        self: &Arc<Self>,
        rules: RuleSet,
    ) -> ApiResult<Vec<(String, String)>> {
        let app = Arc::clone(self);
        crate::routes::common::blocking(move || app.set_rules(rules)).await
    }

    /// [`App::update_rules`] on a blocking thread, off the async workers.
    pub async fn update_rules_blocking(
        self: &Arc<Self>,
        edit: impl FnOnce(&mut RuleSet) + Send + 'static,
    ) -> ApiResult<Vec<(String, String)>> {
        let app = Arc::clone(self);
        crate::routes::common::blocking(move || app.update_rules(edit)).await
    }

    // ----- licence -----

    /// `proxy` with the Pro features this instance may not use switched off (what the proxy
    /// and the WireGuard server run with; the stored settings keep them).
    pub fn effective_proxy(&self, proxy: &ProxySettings) -> ProxySettings {
        crate::license::effective_proxy(proxy, &self.license.status())
    }

    /// Bring everything in line with the current licence state after it changed (key added
    /// or removed, trial started or over): rules, proxy settings, the WireGuard server and
    /// schemas, then tell the clients.
    pub async fn apply_license(self: &Arc<Self>) {
        let status = self.license.status();
        {
            let rules = self.rules.clone();
            let locked = self.license.locked_actions();
            // Recompiling runs script rules' top-level code: off the async workers.
            let _ = crate::routes::common::blocking(move || rules.set_locked_actions(locked)).await;
        }
        self.protos
            .set_enabled(status.has(akev_core::license::Feature::ProtoSchemas));
        {
            let _guard = self.reconfigure.lock().await;
            let settings = self.settings();
            let proxy = self.proxy();
            let mut live = self.effective_proxy(&settings.proxy);
            live.port = proxy.local_addr().port();
            live.bind_address = self.proxy_bind();
            let wireguard = live.wireguard.clone();
            proxy.update_settings(live, settings.capture.clone());
            proxy.sync_transparent().await;
            if let Err(e) = self.wireguard.apply(&wireguard).await {
                self.notice("error", &e);
            }
        }
        self.broadcast(&ServerMessage::License { license: &status });
        self.schedule_trial_end();
        let app = Arc::clone(self);
        tokio::spawn(async move {
            app.refresh_interceptors(true).await;
        });
    }

    /// While the trial runs, apply the licence again when it ends.
    pub fn schedule_trial_end(self: &Arc<Self>) {
        let Some(end) = self.license.trial_running_until() else {
            return;
        };
        let wait = Duration::from_millis((end - now_ms()).max(0.0) as u64 + 1_000);
        let app = Arc::downgrade(self);
        let task = tokio::spawn(async move {
            tokio::time::sleep(wait).await;
            if let Some(app) = app.upgrade()
                && !app.shutdown_started()
            {
                tracing::info!("the Akev Pro trial ended");
                app.apply_license().await;
            }
        });
        self.tasks.lock().push(task);
    }

    // ----- body views -----

    /// Wait for one of the [`BODY_VIEW_PERMITS`] body-view slots. Hold the permit while the
    /// view is built (and until its result is serialized, where practical).
    pub async fn body_view_permit(&self) -> ApiResult<tokio::sync::OwnedSemaphorePermit> {
        Arc::clone(&self.body_views)
            .acquire_owned()
            .await
            .map_err(|_| ApiError::internal("body view limiter closed"))
    }

    // ----- settings -----

    /// Persist the settings snapshot. Call while holding the settings lock so saves are
    /// requested in the same order as the changes.
    fn save_settings(&self, settings: &Settings) {
        if let Some(file) = &self.settings_file {
            file.save(settings);
        }
    }

    /// Apply new settings live: rebinds the proxy when port/bind address change, updates proxy
    /// and store settings, persists and broadcasts.
    ///
    /// Only the listener fields the client actually changed (compared with the stored
    /// settings) move the proxy; the other one keeps its live value. So changing just the bind
    /// address keeps the port the proxy listens on now (a `--proxy-port` override or a
    /// fallback port) and changing just the port keeps a `--proxy-bind` override. CLI
    /// overrides are never persisted.
    pub async fn update_settings(&self, mut new: Settings) -> ApiResult<Settings> {
        new.proxy.bind_address = normalize_bind(&new.proxy.bind_address);
        validate_settings(&new).map_err(ApiError::bad_request)?;
        let _guard = self.reconfigure.lock().await;
        let old = self.settings();
        // Turning a Pro feature on needs Pro; settings that are already on stay as they are
        // (they just don't take effect without it, see `effective_proxy`).
        if let Some(feature) = crate::license::features_turned_on(&old, &new)
            .into_iter()
            .find(|f| !self.license.has(*f))
        {
            return Err(crate::license::pro_required(feature));
        }
        let proxy = self.proxy();
        let current = proxy.local_addr();
        let port_changed = old.proxy.port != new.proxy.port;
        let bind_changed = old.proxy.bind_address != new.proxy.bind_address;
        let target_port = if port_changed {
            new.proxy.port
        } else {
            current.port()
        };
        let target_bind = if bind_changed {
            new.proxy.bind_address.clone()
        } else {
            self.proxy_bind()
        };
        let already_there = target_port != 0
            && target_port == current.port()
            && target_bind.trim().parse::<IpAddr>().ok() == Some(current.ip());
        if (port_changed || bind_changed) && !already_there {
            let addr = proxy.rebind(target_port, &target_bind).await.map_err(|e| {
                ApiError::conflict(format!(
                    "could not move the proxy to {target_bind}:{target_port}: {e}"
                ))
            })?;
            tracing::info!("proxy moved to {addr}");
            self.on_proxy_address_changed(addr);
        }
        if port_changed {
            self.preferred_proxy_port
                .store(new.proxy.port, Ordering::Release);
        }
        if bind_changed {
            *self.proxy_bind.write() = target_bind;
        }
        // The proxy's runtime copy describes the live listener (as after `rebind`), with the
        // Pro features this instance may use.
        let mut live = self.effective_proxy(&new.proxy);
        live.port = proxy.local_addr().port();
        live.bind_address = self.proxy_bind();
        proxy.update_settings(live, new.capture.clone());
        proxy.sync_transparent().await;
        // Lowering the limits evicts, spills or deletes spill files: off the async workers.
        let store = self.store.clone();
        let capture = new.capture.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || store.update_settings(capture)).await {
            tracing::error!(error = %e, "applying capture settings failed");
        }
        self.breakpoints.set_limits(&new.breakpoints);
        {
            let mut current = self.settings.write();
            // Keep UI preferences patched concurrently via `PATCH /api/settings/ui` when the
            // client didn't send any (`ui: null`).
            if new.ui.is_null() {
                new.ui = current.ui.clone();
            }
            *current = new.clone();
            self.save_settings(&current);
        }
        if old.sessions != new.sessions {
            self.sessions.settings_changed();
        }
        // The settings are the desired state: a WireGuard server that can't start (port in
        // use) reports why in `GET /api/wireguard` and a notice.
        let wireguard = self.effective_proxy(&new.proxy).wireguard;
        if self.effective_proxy(&old.proxy).wireguard != wireguard
            && let Err(e) = self.wireguard.apply(&wireguard).await
        {
            self.notice("error", &e);
        }
        self.broadcast(&ServerMessage::Settings { settings: &new });
        Ok(new)
    }

    /// Replace `settings.ui` (opaque UI preferences).
    pub fn set_ui_settings(&self, ui: serde_json::Value) {
        let snapshot = {
            let mut s = self.settings.write();
            s.ui = ui;
            self.save_settings(&s);
            s.clone()
        };
        self.broadcast(&ServerMessage::Settings {
            settings: &snapshot,
        });
    }

    /// Move the proxy to `port` on `bind` (default: the bind address in effect) — always,
    /// unlike [`App::update_settings`], which only moves listener fields that changed. This is
    /// how the proxy gets to its stored port after a busy-port fallback or a `--proxy-port`
    /// override. The stored settings are not changed; the address becomes the preferred one.
    pub async fn rebind_proxy(&self, port: u16, bind: Option<&str>) -> ApiResult<ProxyStatus> {
        if port == 0 {
            return Err(ApiError::bad_request("port must be between 1 and 65535"));
        }
        let bind = normalize_bind(bind.unwrap_or(&self.proxy_bind()));
        let Ok(target) = bind.parse::<IpAddr>() else {
            return Err(ApiError::bad_request(format!(
                "bindAddress {bind:?} is not an IP address"
            )));
        };
        let _guard = self.reconfigure.lock().await;
        let proxy = self.proxy();
        let before = proxy.local_addr();
        let busy = |e: &dyn std::fmt::Display| {
            ApiError::conflict(format!("could not move the proxy to {bind}:{port}: {e}"))
        };
        if before.port() != port {
            // The proxy's own bind falls back to the next free port; this request is for
            // exactly `port`, so check it's free first (and undo a fallback after a race).
            // (Same port, other address: the proxy frees its own socket first.)
            std::net::TcpListener::bind((target, port)).map_err(|e| busy(&e))?;
        }
        let addr = proxy.rebind(port, &bind).await.map_err(|e| busy(&e))?;
        if addr.port() != port {
            if let Err(e) = proxy.rebind(before.port(), &before.ip().to_string()).await {
                tracing::warn!("could not move the proxy back to {before}: {e}");
            }
            let now = proxy.local_addr();
            if now != before {
                self.on_proxy_address_changed(now);
            }
            return Err(busy(&"the port is in use"));
        }
        self.preferred_proxy_port.store(port, Ordering::Release);
        *self.proxy_bind.write() = bind;
        if addr != before {
            tracing::info!("proxy moved to {addr}");
            self.on_proxy_address_changed(addr);
        }
        Ok(self.proxy_status())
    }

    fn on_proxy_address_changed(&self, addr: SocketAddr) {
        {
            let mut ctx = self.intercept_ctx.write();
            let mut updated = (**ctx).clone();
            updated.proxy_port = addr.port();
            updated.proxy_host = proxy_host_for(addr);
            *ctx = Arc::new(updated);
        }
        let info = {
            let mut guard = self.daemon_info.lock();
            if let Some(info) = guard.as_mut() {
                info.proxy_port = addr.port();
            }
            guard.clone()
        };
        if let Some(info) = info
            && let Err(e) = info.write(&self.data_dir)
        {
            tracing::warn!("failed to update daemon.json: {e}");
        }
    }

    // ----- recording -----

    pub fn set_recording(&self, on: bool) {
        if self.recording.set_recording(on) {
            self.broadcast(&ServerMessage::Capture { recording: on });
        }
    }

    // ----- CA -----

    /// Generate a fresh CA (persisted to the data dir, the previous files kept as `*.bak`) and
    /// restart the proxy with it. Only ever on the user's request: devices trust the old one.
    pub async fn regenerate_ca(&self) -> ApiResult<CaInfo> {
        let _guard = self.reconfigure.lock().await;
        let dir = self.data_dir.clone();
        let ca = tokio::task::spawn_blocking(move || CertificateAuthority::regenerate(&dir))
            .await
            .map_err(|e| ApiError::internal(format!("CA generation task failed: {e}")))?
            .map_err(|e| ApiError::internal(format!("failed to generate a new CA: {e}")))?;
        let ca = Arc::new(ca);
        let old_proxy = self.proxy();
        let addr = old_proxy.local_addr();
        // The new proxy takes over the old one's listening socket, so it keeps the port and
        // bind address (including `--proxy-port` / `--proxy-bind` overrides): clients keep
        // working, a session kept local with `--proxy-bind 127.0.0.1` isn't reopened to the
        // network, and no other program can take the port while the proxy restarts.
        let listener = old_proxy.shutdown_keep_listener().await;
        let settings = self.settings();
        let mut proxy_settings = settings.proxy.clone();
        proxy_settings.port = addr.port();
        proxy_settings.bind_address = self.proxy_bind();
        let new_proxy = match self
            .start_proxy(proxy_settings.clone(), &settings, ca.clone(), listener)
            .await
        {
            Ok(p) => p,
            Err(first) => {
                let preferred = self.preferred_proxy_port.load(Ordering::Acquire);
                tracing::warn!(
                    "restarting proxy on port {} failed ({first}); trying port {preferred}",
                    addr.port()
                );
                let mut fallback = proxy_settings.clone();
                fallback.port = preferred;
                self.start_proxy(fallback, &settings, ca.clone(), None)
                    .await
                    .map_err(|e| {
                        ApiError::internal(format!(
                            "failed to restart the proxy with the new CA: {e}"
                        ))
                    })?
            }
        };
        let new_addr = new_proxy.local_addr();
        new_proxy.set_client_observer(client_observer(self.bus.clone(), self.seen_clients.clone()));
        *self.proxy.write() = new_proxy;
        *self.ca.write() = ca.clone();
        let info = ca.info();
        {
            let current = self.intercept_ctx();
            let ctx = build_intercept_ctx(
                &ca,
                &info,
                &self.ca_path,
                new_addr,
                &self.data_dir,
                &self.api_url,
                &self.token,
            );
            let mut ctx = ctx;
            ctx.lan_addresses = current.lan_addresses.clone();
            ctx.transparent = Some(transparent_probe(&self.proxy()));
            *self.intercept_ctx.write() = Arc::new(ctx);
        }
        if new_addr != addr {
            self.on_proxy_address_changed(new_addr);
        }
        ensure_ca_file(&self.ca_path, &ca.chain_pem());
        if let Some(api_tls) = &self.api_tls
            && let Err(e) = api_tls.reissue(&ca)
        {
            tracing::warn!("could not re-issue the API's TLS certificate from the new CA: {e}");
        }
        self.notice(
            "warning",
            "A new Akev CA certificate was generated. Clients that trusted the old certificate must trust the new one.",
        );
        Ok(info)
    }

    async fn start_proxy(
        &self,
        proxy: akev_core::settings::ProxySettings,
        settings: &Settings,
        ca: Arc<CertificateAuthority>,
        listener: Option<akev_proxy::Listener>,
    ) -> akev_proxy::Result<ProxyHandle> {
        let options = ProxyOptions {
            settings: self.effective_proxy(&proxy),
            capture: settings.capture.clone(),
            ca,
            rules: self.rules.clone(),
            sink: self.recording.clone() as Arc<dyn EventSink>,
            breakpoints: self.breakpoints.clone(),
        };
        match listener {
            Some(l) => akev_proxy::start_on(options, l).await,
            None => akev_proxy::start(options).await,
        }
    }

    // ----- interceptors -----

    /// List interceptors and broadcast the list if activation state changed.
    pub async fn refresh_interceptors(
        &self,
        force: bool,
    ) -> Vec<akev_core::interceptor::InterceptorInfo> {
        let ctx = self.intercept_ctx();
        let list = self.interceptors.list(&ctx).await;
        let msg = ServerMessage::Interceptors {
            interceptors: &list,
        };
        if let Some(text) = msg.to_text() {
            let changed = {
                let mut last = self.last_interceptors.lock();
                let changed = force || last.as_ref().map(|t| t.as_str()) != Some(text.as_str());
                *last = Some(text.clone());
                changed
            };
            if changed && self.has_stream_clients() {
                let _ = self.bus.send(text);
            }
        }
        list
    }

    // ----- lifecycle -----

    /// Write `daemon.json` for CLI / MCP discovery — unless another live Akev process already
    /// advertises itself in this data directory (e.g. `akev capture --data-dir` next to a
    /// running `akev start`), whose discovery must not be hijacked.
    pub fn register_daemon(&self) {
        let info = DaemonInfo {
            pid: std::process::id(),
            api_url: self.api_url.clone(),
            token: self.token.clone(),
            proxy_port: self.proxy().local_addr().port(),
            version: akev_core::VERSION.to_string(),
        };
        if let Some(existing) = DaemonInfo::read(&self.data_dir)
            && existing.pid != info.pid
            && existing.api_url != info.api_url
            && persist::process_alive(existing.pid)
            && persist::api_listening(&existing.api_url)
        {
            tracing::info!(
                "another Akev (pid {}) is registered in {}; not taking over daemon.json",
                existing.pid,
                self.data_dir.display()
            );
            return;
        }
        if let Err(e) = info.write(&self.data_dir) {
            tracing::warn!("failed to write daemon.json: {e}");
        }
        *self.daemon_info.lock() = Some(info);
    }

    pub fn shutdown_receiver(&self) -> watch::Receiver<bool> {
        self.shutdown_requested.subscribe()
    }

    /// Ask the embedding application to shut down (`POST /api/shutdown`).
    pub fn request_shutdown(&self) {
        self.shutdown_requested.send_replace(true);
    }

    pub async fn wait_shutdown_requested(&self) {
        let mut rx = self.shutdown_requested.subscribe();
        let _ = rx.wait_for(|v| *v).await;
    }

    /// Whether [`App::shutdown`] has begun.
    pub fn shutdown_started(&self) -> bool {
        self.shutdown_started.load(Ordering::Acquire)
    }

    /// Graceful shutdown (idempotent; concurrent callers wait for completion).
    pub async fn shutdown(&self) {
        if self.shutdown_started.swap(true, Ordering::AcqRel) {
            let mut rx = self.shutdown_done.subscribe();
            let _ = rx.wait_for(|v| *v).await;
            return;
        }
        tracing::info!("shutting down");
        self.shutdown_requested.send_replace(true);
        self.breakpoints.resume_all();
        let ctx = self.intercept_ctx();
        if tokio::time::timeout(Duration::from_secs(10), self.interceptors.shutdown(&ctx))
            .await
            .is_err()
        {
            tracing::warn!("timed out deactivating interceptors");
        }
        if tokio::time::timeout(Duration::from_secs(5), self.wireguard.shutdown())
            .await
            .is_err()
        {
            tracing::warn!("timed out stopping the WireGuard server");
        }
        if tokio::time::timeout(Duration::from_secs(5), self.proxy().shutdown())
            .await
            .is_err()
        {
            tracing::warn!("timed out stopping the proxy");
        }
        self.flush_state();
        self.sessions.autosave_on_shutdown(self).await;
        let tasks: Vec<JoinHandle<()>> = std::mem::take(&mut *self.tasks.lock());
        for mut task in tasks {
            if tokio::time::timeout(Duration::from_secs(3), &mut task)
                .await
                .is_err()
            {
                task.abort();
            }
        }
        if let Some(info) = self.daemon_info.lock().take()
            && let Err(e) = info.remove_if_owned(&self.data_dir)
        {
            tracing::warn!("failed to remove daemon.json: {e}");
        }
        // The browser launcher page holds the token: remove it once it can't be used anymore.
        if let Err(e) = crate::launcher::remove_ui_launcher_if_owned(&self.data_dir, &self.ui_url())
        {
            tracing::warn!("failed to remove the UI launcher page: {e}");
        }
        if let Some(dir) = self.temp_dir.lock().take()
            && let Err(e) = dir.close()
        {
            tracing::debug!("failed to remove temporary data dir: {e}");
        }
        self.shutdown_done.send_replace(true);
        tracing::info!("shutdown complete");
    }

    /// Synchronously persist settings & rules that have pending or failed saves. Rules loaded
    /// from `--rules FILE` are only written to rules.json once changed via the API.
    pub fn flush_state(&self) {
        if let Some(file) = self.settings_file.as_ref().filter(|f| !f.is_clean()) {
            let settings = self.settings();
            if let Err(e) = file.save_blocking(&settings) {
                tracing::warn!("failed to save settings: {e}");
            }
        }
        if let Some(file) = self.rules_file.as_ref().filter(|f| !f.is_clean()) {
            let rules = self.rules.rules();
            if let Err(e) = file.save_blocking(&rules) {
                tracing::warn!("failed to save rules: {e}");
            }
        }
    }
}

/// Rule ids that appear more than once (they'd share match counters).
pub(crate) fn duplicate_rule_ids(rules: &RuleSet) -> Vec<(String, String)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    fn walk<'a>(
        items: &'a [akev_core::rules::RuleItem],
        seen: &mut std::collections::HashSet<&'a str>,
        out: &mut Vec<(String, String)>,
    ) {
        for item in items {
            let id = match item {
                akev_core::rules::RuleItem::Rule(r) => r.id.as_str(),
                akev_core::rules::RuleItem::Group(g) => {
                    walk(&g.items, seen, out);
                    g.id.as_str()
                }
            };
            if !seen.insert(id) {
                out.push((
                    id.to_string(),
                    "duplicate rule id: rule ids must be unique".to_string(),
                ));
            }
        }
    }
    walk(&rules.items, &mut seen, &mut out);
    out
}

/// `localhost` → `127.0.0.1`, `[::1]` → `::1`, trimmed.
pub(crate) fn normalize_bind(bind: &str) -> String {
    let b = bind.trim().trim_start_matches('[').trim_end_matches(']');
    if b.eq_ignore_ascii_case("localhost") || b.is_empty() {
        "127.0.0.1".to_string()
    } else {
        b.to_string()
    }
}

/// Validate user-supplied settings before applying them.
pub(crate) fn validate_settings(s: &Settings) -> Result<(), String> {
    let bind = s.proxy.bind_address.trim();
    if bind
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
        .is_err()
    {
        return Err(format!("proxy.bindAddress {bind:?} is not an IP address"));
    }
    for o in &s.proxy.dns_overrides {
        if o.host.trim().is_empty() {
            return Err("proxy.dnsOverrides: host must not be empty".into());
        }
        if o.address.trim().parse::<IpAddr>().is_err() {
            return Err(format!(
                "proxy.dnsOverrides: {:?} is not an IP address",
                o.address
            ));
        }
    }
    if let Some(doh) = &s.proxy.doh_url {
        match url::Url::parse(doh) {
            Ok(u) if u.scheme() == "https" => {}
            _ => return Err(format!("proxy.dohUrl {doh:?} must be an https:// URL")),
        }
    }
    if matches!(
        s.proxy.upstream_proxy.mode,
        UpstreamProxyMode::Manual | UpstreamProxyMode::Pac
    ) {
        let url = s.proxy.upstream_proxy.url.trim();
        let parsed =
            url::Url::parse(url).map_err(|e| format!("proxy.upstreamProxy.url {url:?}: {e}"))?;
        let ok = match s.proxy.upstream_proxy.mode {
            UpstreamProxyMode::Manual => {
                matches!(
                    parsed.scheme(),
                    "http" | "https" | "socks4" | "socks4a" | "socks5" | "socks5h"
                )
            }
            _ => matches!(parsed.scheme(), "http" | "https" | "file"),
        };
        if !ok || (parsed.scheme() != "file" && parsed.host_str().is_none()) {
            return Err(format!(
                "proxy.upstreamProxy.url {url:?} is not a supported proxy URL"
            ));
        }
    }
    if s.proxy
        .tls_profile
        .overrides
        .iter()
        .any(|o| o.host.trim().is_empty())
    {
        return Err("proxy.tlsProfile.overrides: host must not be empty".into());
    }
    if s.capture.memory_budget_bytes > 0 && s.capture.memory_budget_bytes < 1024 * 1024 {
        return Err("capture.memoryBudgetBytes must be at least 1 MiB".into());
    }
    crate::routes::transparent::validate(s)?;
    if !(1..=86_400).contains(&s.sessions.autosave_interval_secs) {
        return Err("sessions.autosaveIntervalSecs must be between 1 and 86400".into());
    }
    crate::wireguard::validate(&s.proxy.wireguard)?;
    s.proxy.access.validate()?;
    Ok(())
}

/// Reachable `host:port` strings for a proxy listening on `addr`.
pub(crate) fn proxy_addresses(addr: SocketAddr) -> Vec<String> {
    let port = addr.port();
    if addr.ip().is_unspecified() {
        let mut out = vec![format!("127.0.0.1:{port}")];
        out.extend(
            crate::net::lan_ipv4_addresses()
                .into_iter()
                .map(|ip| format!("{ip}:{port}")),
        );
        out
    } else {
        vec![addr.to_string()]
    }
}

/// Host clients on this machine should use for a proxy bound to `addr`.
pub(crate) fn proxy_host_for(addr: SocketAddr) -> String {
    match addr.ip() {
        ip if ip.is_unspecified() => "127.0.0.1".to_string(),
        ip => ip.to_string(),
    }
}

pub(crate) fn build_intercept_ctx(
    ca: &CertificateAuthority,
    info: &CaInfo,
    ca_path: &Path,
    proxy_addr: SocketAddr,
    data_dir: &Path,
    api_url: &str,
    token: &str,
) -> InterceptContext {
    InterceptContext {
        proxy_host: proxy_host_for(proxy_addr),
        proxy_port: proxy_addr.port(),
        lan_addresses: crate::net::lan_ipv4_addresses()
            .into_iter()
            .map(|ip| ip.to_string())
            .collect(),
        ca_pem: ca.cert_pem().to_string(),
        ca_der: ca.cert_der().to_vec(),
        ca_path: ca_path.to_path_buf(),
        ca_spki_fingerprint: info.spki_fingerprint.clone(),
        ca_subject_hash_old: info.subject_hash_old.clone(),
        data_dir: data_dir.to_path_buf(),
        api_url: api_url.to_string(),
        api_token: token.to_string(),
        transparent: None,
        proxy_credentials: None,
    }
}

/// Interceptors read the transparent listener's live status through the running proxy.
fn transparent_probe(proxy: &ProxyHandle) -> akev_core::transparent::TransparentProbeHandle {
    akev_core::transparent::TransparentProbeHandle::new(proxy.clone())
}

/// Make sure `ca.pem` on disk matches the in-memory CA (interceptors point clients at it).
pub(crate) fn ensure_ca_file(path: &Path, pem: &str) {
    let current = std::fs::read_to_string(path).ok();
    if current.as_deref() != Some(pem)
        && let Err(e) = persist::write_atomic(path, pem.as_bytes(), 0o644)
    {
        tracing::warn!("failed to write {}: {e}", path.display());
    }
}

pub(crate) fn platform() -> &'static str {
    if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(windows) {
        "windows"
    } else {
        "linux"
    }
}

pub(crate) use akev_core::time::now_ms;

/// Load settings honouring the ephemeral flag.
pub(crate) fn load_settings(data_dir: &Path, persist: bool) -> Settings {
    if !persist {
        return Settings::default();
    }
    let (settings, outcome) =
        persist::load_json_or_default::<Settings>(&data_dir.join(SETTINGS_FILE));
    log_outcome("settings", &outcome);
    settings
}

/// Load the rule set: explicit rules file (Akev or HTTP Toolkit format) > persisted rules.
pub(crate) fn load_rules(
    data_dir: &Path,
    persist: bool,
    rules_file: Option<&Path>,
) -> anyhow::Result<RuleSet> {
    if let Some(path) = rules_file {
        let bytes = std::fs::read(path)
            .map_err(|e| anyhow::anyhow!("failed to read rules file {}: {e}", path.display()))?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|e| anyhow::anyhow!("rules file {} is not valid JSON: {e}", path.display()))?;
        return parse_rule_import(&value)
            .map_err(|e| anyhow::anyhow!("rules file {}: {e}", path.display()));
    }
    if !persist {
        return Ok(RuleSet::default());
    }
    let (rules, outcome) = persist::load_json_or_default::<RuleSet>(&data_dir.join(RULES_FILE));
    log_outcome("rules", &outcome);
    Ok(rules)
}

fn log_outcome(what: &str, outcome: &LoadOutcome) {
    match outcome {
        LoadOutcome::Loaded | LoadOutcome::Missing => {}
        LoadOutcome::Recovered(backup) => {
            tracing::warn!(
                "{what} file was corrupt; backed up to {} and reset to defaults",
                backup.display()
            )
        }
        LoadOutcome::Unreadable(e) => {
            tracing::warn!("could not read {what} file ({e}); using defaults")
        }
    }
}

/// Accept an Akev `RuleSet` or an HTTP Toolkit rules export.
///
/// The format is detected first (an HTTP Toolkit export with no rules would otherwise also
/// parse as an empty Akev set); if the preferred parse fails the other format is tried, and
/// the error explains both failures.
pub(crate) fn parse_rule_import(value: &serde_json::Value) -> Result<RuleSet, String> {
    let akev = || serde_json::from_value::<RuleSet>(value.clone()).map_err(|e| e.to_string());
    let htk = || akev_rules::htk_import::convert(value).map_err(|e| e.to_string());
    let both_failed = |akev_err: String, htk_err: String| {
        format!("not an Akev rule set ({akev_err}) nor an HTTP Toolkit rules export ({htk_err})")
    };
    if !value.is_object() && !value.is_array() {
        return Err("expected a JSON object (rule set) or an HTTP Toolkit rules export".into());
    }
    if akev_rules::htk_import::looks_like_htk(value) {
        htk().or_else(|htk_err| akev().map_err(|akev_err| both_failed(akev_err, htk_err)))
    } else {
        akev().or_else(|akev_err| htk().map_err(|htk_err| both_failed(akev_err, htk_err)))
    }
}

/// Fan breakpoint notices out to stream clients.
pub(crate) fn breakpoint_notifier(
    bus: broadcast::Sender<Utf8Bytes>,
) -> crate::breakpoints::Notifier {
    Arc::new(move |notice: BreakpointNotice| {
        if bus.receiver_count() == 0 {
            return;
        }
        let text = match &notice {
            BreakpointNotice::Paused(exchange) => ServerMessage::Paused { exchange }.to_text(),
            BreakpointNotice::Resumed(id) => ServerMessage::Resumed { id: *id }.to_text(),
            BreakpointNotice::MessagePaused(message) => {
                ServerMessage::MessagePaused { message }.to_text()
            }
            BreakpointNotice::MessageResumed { id, pause_id } => ServerMessage::MessageResumed {
                id: *id,
                pause_id: *pause_id,
            }
            .to_text(),
            BreakpointNotice::Limited { message, .. } => ServerMessage::Notice {
                level: "warning",
                message,
            }
            .to_text(),
        };
        if let Some(text) = text {
            let _ = bus.send(text);
        }
    })
}

/// Most device addresses remembered for `client-connected` (beyond, new ones are still
/// reported but no longer remembered).
const MAX_SEEN_CLIENTS: usize = 4096;

/// Broadcast `client-connected` the first time a device on another machine connects this
/// run (the proxy calls this on every admitted connection, so it must stay cheap).
pub(crate) fn client_observer(
    bus: broadcast::Sender<Utf8Bytes>,
    seen: Arc<Mutex<std::collections::HashSet<IpAddr>>>,
) -> akev_proxy::ClientObserver {
    Arc::new(move |ip: IpAddr| {
        {
            let mut seen = seen.lock();
            if seen.contains(&ip) {
                return;
            }
            if seen.len() < MAX_SEEN_CLIENTS {
                seen.insert(ip);
            }
        }
        tracing::info!(client = %ip, "new device connected to the proxy");
        if bus.receiver_count() > 0 {
            let address = ip.to_string();
            if let Some(text) = (ServerMessage::ClientConnected { address: &address }).to_text() {
                let _ = bus.send(text);
            }
        }
    })
}

pub(crate) fn new_bus() -> broadcast::Sender<Utf8Bytes> {
    broadcast::channel(BUS_CAPACITY).0
}

/// Rule id → display title (explicit title, else the generated summary) for `rule:` filters.
pub(crate) fn rule_titles(rules: &RuleSet) -> std::collections::HashMap<String, String> {
    rules
        .walk()
        .into_iter()
        .map(|(rule, _)| {
            let title = rule
                .title
                .clone()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or_else(|| akev_rules::describe::summarize(rule));
            (rule.id.clone(), title)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::rules::{Action, Rule, RuleGroup, RuleItem};
    use akev_core::settings::DnsOverride;

    fn rule(id: &str) -> RuleItem {
        RuleItem::Rule(Rule {
            id: id.into(),
            title: None,
            enabled: true,
            matchers: vec![],
            action: Action::Close,
            max_matches: None,
            note: None,
        })
    }

    #[test]
    fn duplicate_ids_are_reported() {
        let rs = RuleSet {
            version: 1,
            items: vec![
                rule("a"),
                RuleItem::Group(RuleGroup {
                    id: "g".into(),
                    title: "G".into(),
                    enabled: true,
                    collapsed: false,
                    items: vec![rule("b"), rule("a")],
                }),
                rule("b"),
            ],
        };
        let dups = duplicate_rule_ids(&rs);
        assert_eq!(
            dups.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
            vec!["a", "b"]
        );
        assert!(duplicate_rule_ids(&RuleSet::default()).is_empty());
    }

    #[test]
    fn settings_validation() {
        let mut s = Settings::default();
        assert!(validate_settings(&s).is_ok());
        s.proxy.bind_address = "0.0.0.0".into();
        assert!(validate_settings(&s).is_ok());
        s.proxy.bind_address = "::".into();
        assert!(validate_settings(&s).is_ok());
        s.proxy.bind_address = "my-host".into();
        assert!(validate_settings(&s).is_err());
        s.proxy.bind_address = "127.0.0.1".into();
        s.proxy.dns_overrides.push(DnsOverride {
            host: "a.test".into(),
            address: "nope".into(),
        });
        assert!(validate_settings(&s).is_err());
        s.proxy.dns_overrides[0].address = "10.0.0.1".into();
        assert!(validate_settings(&s).is_ok());
        s.proxy.doh_url = Some("http://dns.test/q".into());
        assert!(validate_settings(&s).is_err());
        s.proxy.doh_url = Some("https://dns.test/dns-query".into());
        assert!(validate_settings(&s).is_ok());
        s.proxy.upstream_proxy.mode = UpstreamProxyMode::Manual;
        s.proxy.upstream_proxy.url = "ftp://x".into();
        assert!(validate_settings(&s).is_err());
        s.proxy.upstream_proxy.url = "socks5://user:pw@127.0.0.1:1080".into();
        assert!(validate_settings(&s).is_ok());
        s.capture.memory_budget_bytes = 10;
        assert!(validate_settings(&s).is_err());
    }

    #[test]
    fn proxy_address_helpers() {
        let loopback: SocketAddr = "127.0.0.1:8000".parse().unwrap();
        assert_eq!(proxy_addresses(loopback), vec!["127.0.0.1:8000"]);
        assert_eq!(proxy_host_for(loopback), "127.0.0.1");
        let any: SocketAddr = "0.0.0.0:8001".parse().unwrap();
        let addrs = proxy_addresses(any);
        assert_eq!(addrs[0], "127.0.0.1:8001");
        assert!(addrs.iter().all(|a| a.ends_with(":8001")));
        assert_eq!(proxy_host_for(any), "127.0.0.1");
        let lan: SocketAddr = "192.168.1.4:8000".parse().unwrap();
        assert_eq!(proxy_host_for(lan), "192.168.1.4");
    }

    #[test]
    fn rules_load_from_persisted_and_ephemeral() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            load_rules(dir.path(), false, None).unwrap(),
            RuleSet::default()
        );
        let rs = RuleSet {
            version: 1,
            items: vec![rule("x")],
        };
        persist::save_state(dir.path(), RULES_FILE, &rs).unwrap();
        assert_eq!(load_rules(dir.path(), true, None).unwrap(), rs);
        // ephemeral ignores persisted rules
        assert_eq!(
            load_rules(dir.path(), false, None).unwrap(),
            RuleSet::default()
        );
        // explicit file wins
        let file = dir.path().join("custom.json");
        let other = RuleSet {
            version: 1,
            items: vec![rule("y")],
        };
        std::fs::write(&file, serde_json::to_vec(&other).unwrap()).unwrap();
        assert_eq!(load_rules(dir.path(), true, Some(&file)).unwrap(), other);
        assert!(load_rules(dir.path(), true, Some(&dir.path().join("missing.json"))).is_err());
        std::fs::write(&file, b"not json").unwrap();
        assert!(load_rules(dir.path(), true, Some(&file)).is_err());
    }

    #[test]
    fn rule_import_detects_formats() {
        // Akev rule set.
        let akev = serde_json::to_value(RuleSet {
            version: 1,
            items: vec![rule("a")],
        })
        .unwrap();
        assert_eq!(parse_rule_import(&akev).unwrap().items.len(), 1);
        // HTTP Toolkit export (root group with a mock rule).
        let htk = serde_json::json!({
            "id": "root", "title": "HTTP Toolkit Rules", "isRoot": true,
            "items": [{
                "id": "mock-login", "type": "http", "activated": true, "title": "Mock login",
                "matchers": [{"method": 1, "uiType": "POST", "type": "method"}, {"type": "simple-path", "path": "/login"}],
                "steps": [{"type": "simple", "status": 200, "data": "{\"ok\":true}", "headers": {"content-type": "application/json"}}],
                "completionChecker": {"type": "always"}
            }]
        });
        let converted = parse_rule_import(&htk).unwrap();
        assert_eq!(converted.walk().len(), 1);
        // An empty HTTP Toolkit export is still recognised (not mistaken for an Akev set).
        let empty_htk = serde_json::json!({"id": "root", "isRoot": true, "items": []});
        assert!(parse_rule_import(&empty_htk).unwrap().items.is_empty());
        // Garbage.
        assert!(parse_rule_import(&serde_json::json!("rules")).is_err());
        assert!(parse_rule_import(&serde_json::json!(42)).is_err());
        // Unknown items never fail the whole import (they come back disabled with a note).
        let odd = parse_rule_import(&serde_json::json!({"items": [{"type": "nope"}]}));
        if let Ok(rules) = odd {
            assert!(rules.walk().iter().all(|(r, _)| !r.enabled));
        }
    }

    #[test]
    fn settings_load_ephemeral_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = Settings::default();
        s.proxy.port = 9999;
        persist::save_state(dir.path(), SETTINGS_FILE, &s).unwrap();
        assert_eq!(load_settings(dir.path(), true).proxy.port, 9999);
        assert_eq!(load_settings(dir.path(), false), Settings::default());
    }

    #[test]
    fn ca_file_is_written_when_different() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ca.pem");
        ensure_ca_file(&path, "PEM-1");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "PEM-1");
        ensure_ca_file(&path, "PEM-2");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "PEM-2");
    }

    #[test]
    fn new_devices_are_announced_once() {
        let bus = new_bus();
        let mut rx = bus.subscribe();
        let observer = client_observer(bus, Arc::default());
        let lan: IpAddr = "192.168.1.23".parse().unwrap();
        observer(lan);
        observer(lan);
        observer("10.0.0.2".parse().unwrap());
        let first: serde_json::Value =
            serde_json::from_str(rx.try_recv().unwrap().as_str()).unwrap();
        assert_eq!(
            first,
            serde_json::json!({"type": "client-connected", "address": "192.168.1.23"})
        );
        let second: serde_json::Value =
            serde_json::from_str(rx.try_recv().unwrap().as_str()).unwrap();
        assert_eq!(second["address"], "10.0.0.2");
        assert!(rx.try_recv().is_err(), "each device once");
    }

    #[test]
    fn access_settings_are_validated() {
        let mut s = Settings::default();
        s.proxy.access.block = vec!["192.168.1.0/24".into()];
        assert!(validate_settings(&s).is_ok());
        s.proxy.access.allow = vec!["phone".into()];
        let err = validate_settings(&s).unwrap_err();
        assert!(err.contains("proxy.access.allow"), "{err}");
    }
}
