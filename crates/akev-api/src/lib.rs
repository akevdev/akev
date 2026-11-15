//! # akev-api
//!
//! The Akev application core. [`start`] wires everything together:
//! CA (`akev-ca`) → rules engine (`akev-rules`) → store (`akev-store`) → proxy
//! (`akev-proxy`) → interceptors (`akev-intercept`), then serves the HTTP API, the live
//! WebSocket stream, the MCP endpoint and the embedded web UI on a localhost port.
//! Both the CLI daemon (`akev`) and the desktop app (Tauri) call [`start`].
//!
//! The full HTTP contract is specified in the HTTP API reference, akev.dev/docs/api.
//!
//! Persistence (in `data_dir`): `ca.pem`/`ca.key`, `settings.json`, `rules.json`,
//! `daemon.json` (discovery file for the CLI / MCP bridge, removed on shutdown),
//! `open-ui.html` (private browser launcher page, see [`write_ui_launcher`]; removed on
//! shutdown), `spill/` (body spill files), `protos/` (imported protobuf schemas and their
//! URL mappings), `sessions/` (saved sessions and the autosave, see `sessions.rs`),
//! `license.json` (Akev Pro licence key and trial start, see `license.rs`), `akev.lock` +
//! `instance.json` (one instance per data directory, see `datadir_lock.rs`), `logs/` and
//! `crashes/` (written by the binaries, see [`observability`]).

mod app;
mod auth;
mod body;
mod breakpoints;
mod datadir_lock;
mod diagnostics;
mod error;
mod launcher;
mod license;
mod llm;
pub mod mcp;
mod net;
pub mod observability;
mod persist;
mod protos;
mod routes;
mod sessions;
mod sink;
mod stream;
mod tls;
mod ui;
pub mod updates;
mod wireguard;
mod ws_breakpoints;

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use akev_ca::CertificateAuthority;
use akev_core::EventSink;
use akev_proxy::ProxyOptions;
use akev_rules::RulesEngine;
use akev_store::Store;
use anyhow::Context;
use axum::serve::ListenerExt;

use crate::app::{App, AppParts};
use crate::breakpoints::BreakpointManager;
use crate::sink::RecordingSink;

pub use crate::app::{ProxyStatus, Status, StatusStats};
pub use crate::datadir_lock::{InstanceInfo, LockError};
pub use crate::diagnostics::Diagnostics;
pub use crate::error::ApiError;
pub use crate::launcher::{
    UI_LAUNCHER_FILE, ui_launcher_path, ui_launcher_url, ui_url_for, write_ui_launcher,
};
pub use crate::license::{LicenseManager, builtin_public_keys, default_trial_file};
pub use crate::persist::{DaemonInfo, default_data_dir};

/// Minimum length of a user-chosen API token when the API is reachable from the network
/// (`api_bind` is not a loopback address). Generated tokens are 64 hex characters.
pub const MIN_NETWORK_TOKEN_LEN: usize = 24;

/// Options for [`start`].
#[derive(Clone, Debug)]
pub struct AkevOptions {
    /// Data directory (default: platform data dir / `akev`).
    pub data_dir: Option<PathBuf>,
    /// Override the proxy port from settings for this run (never persisted; kept when only
    /// the bind address is changed through the settings, and across CA regeneration).
    pub proxy_port: Option<u16>,
    /// Override the proxy bind address from settings for this run (e.g. `0.0.0.0` in
    /// containers; never persisted, kept when only the port changes and across CA
    /// regeneration).
    pub proxy_bind: Option<String>,
    /// API/UI port (0 = random free port; default 45456, falling back to random if busy).
    pub api_port: u16,
    /// API bind address (default 127.0.0.1). Binding elsewhere requires the token for all
    /// access, throttles peers that keep failing authentication, requires a custom `token`
    /// of at least [`MIN_NETWORK_TOKEN_LEN`] characters, and serves other machines over
    /// HTTPS only (certificate from Akev's CA; see [`AkevHandle::network_url`]).
    pub api_bind: String,
    /// API token. `None` = random per run (256 bits).
    pub token: Option<String>,
    /// Don't load/save settings & rules (ephemeral session, e.g. tests and CI capture).
    pub ephemeral: bool,
    /// Initial rules file to load (overrides persisted rules for this run).
    pub rules_file: Option<PathBuf>,
    /// Allowed extra Origins for the API (CORS), e.g. a dev UI server `http://localhost:5173`.
    pub allowed_origins: Vec<String>,
    /// Let MCP clients (AI agents) activate interceptors that launch an arbitrary local
    /// executable (`electron`). Off by default: MCP feeds captured — possibly attacker
    /// controlled — traffic to the agent, so a prompt injection could otherwise start
    /// programs. The REST API and the UI are not affected.
    pub mcp_allow_launch: bool,
    /// Akev Pro licence key for this run (`--license` / `AKEV_LICENSE`); takes precedence
    /// over the stored one and is never written to disk.
    pub license: Option<String>,
    /// Ed25519 public keys (base64url) trusted for licence keys. Defaults to the keys built
    /// into this binary.
    pub license_public_keys: Vec<String>,
    /// Per-user file recording the Pro trial, shared by every data directory on this computer
    /// (the CLI and desktop app pass [`default_trial_file`]). `None` keeps the trial in the
    /// data directory's `license.json` (embedders, tests).
    pub trial_file: Option<PathBuf>,
    /// Let MCP tool calls pass `reveal: true` to see secrets in captured traffic
    /// (credential headers, cookies, secret query parameters, tokens in bodies). Off by
    /// default: MCP output is redacted, so a prompt injection can't get an agent to read
    /// them out. Exports from the REST API and the UI are not affected.
    pub mcp_reveal_secrets: bool,
    /// Folders MCP clients may make Akev read and write files in (`--mcp-allow-path`), in
    /// addition to `<data dir>/exports`: HAR exports (`akev_export_har`) and the files that
    /// rules created or edited through MCP read (`file` bodies, `map-local`). Other paths are
    /// refused, so a prompt injection can't read `~/.ssh` through a mock rule. Rules made in
    /// the UI or through the REST API are not restricted.
    pub mcp_allow_paths: Vec<PathBuf>,
    /// Running inside the desktop shell: reported as `Status.desktop`, and named in the
    /// data-directory lock.
    pub desktop: bool,
}

impl Default for AkevOptions {
    fn default() -> Self {
        AkevOptions {
            data_dir: None,
            proxy_port: None,
            proxy_bind: None,
            api_port: 45456,
            api_bind: "127.0.0.1".into(),
            token: None,
            ephemeral: false,
            rules_file: None,
            allowed_origins: Vec::new(),
            mcp_allow_launch: false,
            license: None,
            license_public_keys: license::builtin_public_keys(),
            trial_file: None,
            mcp_reveal_secrets: false,
            mcp_allow_paths: Vec::new(),
            desktop: false,
        }
    }
}

/// A running Akev instance.
pub struct AkevHandle {
    app: Arc<App>,
    /// The data directory lock (none for ephemeral sessions), released after shutdown.
    lock: parking_lot::Mutex<Option<datadir_lock::DataDirLock>>,
}

impl AkevHandle {
    /// e.g. `http://127.0.0.1:45456`: the URL for clients on this machine (plain HTTP keeps
    /// working for them even when the API is reachable from the network).
    pub fn api_url(&self) -> String {
        self.app.api_url.clone()
    }

    /// For an API reachable from the network: the `https://` URL other devices use (its
    /// certificate is issued by Akev's CA), e.g. `https://192.168.1.23:45456`.
    pub fn network_url(&self) -> Option<String> {
        self.app.network_url.clone()
    }

    /// The address the API server listens on.
    pub fn api_addr(&self) -> SocketAddr {
        self.app.api_addr
    }

    /// The proxy's current listening address (changes when settings move the proxy).
    pub fn proxy_addr(&self) -> SocketAddr {
        self.app.proxy().local_addr()
    }

    /// The API token.
    pub fn token(&self) -> &str {
        &self.app.token
    }

    /// URL that opens the UI already authenticated (`{api_url}/?token=...`).
    ///
    /// Don't pass it on a command line (e.g. to a browser launcher): other local users can
    /// read process arguments. Use [`AkevHandle::write_ui_launcher`] instead.
    pub fn ui_url(&self) -> String {
        self.app.ui_url()
    }

    /// Write the private launcher page that redirects to [`AkevHandle::ui_url`] into the data
    /// directory and return its path; open it with [`ui_launcher_url`]. Removed on shutdown.
    pub fn write_ui_launcher(&self) -> std::io::Result<PathBuf> {
        write_ui_launcher(&self.app.data_dir, &self.app.ui_url())
    }

    /// The data directory in use (a temporary directory for ephemeral sessions without an
    /// explicit `data_dir`).
    pub fn data_dir(&self) -> &std::path::Path {
        &self.app.data_dir
    }

    /// Resolves when shutdown was requested via the API (`POST /api/shutdown`) or [`AkevHandle::shutdown`].
    pub async fn wait(&self) {
        self.app.wait_shutdown_requested().await;
    }

    /// Graceful shutdown: deactivate interceptors, stop proxy, persist state, then release
    /// the data directory.
    pub async fn shutdown(&self) {
        self.app.shutdown().await;
        drop(self.lock.lock().take());
    }

    // ----- additions beyond the original frozen surface -----

    /// Path of the CA certificate (PEM) clients should trust.
    /// Environment variables that point command-line tools at this proxy and make them trust
    /// its CA (the terminal interceptor's set; see `akev_intercept::proxy_env_vars`).
    pub fn proxy_env_vars(&self) -> Vec<(String, String)> {
        akev_intercept::proxy_env_vars(&self.app.intercept_ctx())
    }

    pub fn ca_path(&self) -> &Path {
        &self.app.ca_path
    }

    /// The traffic store (for embedders such as `akev capture`).
    pub fn store(&self) -> &Arc<Store> {
        &self.app.store
    }

    /// The rules engine.
    pub fn rules(&self) -> &Arc<RulesEngine> {
        &self.app.rules
    }

    /// The running proxy (replaced when the CA is regenerated).
    pub fn proxy(&self) -> akev_proxy::ProxyHandle {
        self.app.proxy()
    }

    /// Current status snapshot (same as `GET /api/status`).
    pub fn status(&self) -> Status {
        self.app.status()
    }

    /// Current settings (same as `GET /api/settings`), including the UI's synced preferences
    /// (`ui`) that an embedder may honour, e.g. the desktop shell's title bar choice.
    pub fn settings(&self) -> akev_core::settings::Settings {
        self.app.settings()
    }

    /// Turn recording of traffic on/off (same as `PUT /api/capture`).
    pub fn set_recording(&self, recording: bool) {
        self.app.set_recording(recording);
    }

    /// Whether traffic is being recorded.
    pub fn is_recording(&self) -> bool {
        self.app.recording.is_recording()
    }

    /// Import a capture file, chosen by its extension: `.har`, `.akev` (session), `.chlsj`
    /// (Charles), `.saz` (Fiddler), anything else as a mitmproxy flow file. Blocking: call it
    /// off the async workers. Returns the new exchange ids.
    pub fn import_file(&self, path: &Path) -> anyhow::Result<Vec<akev_core::ExchangeId>> {
        let data =
            std::fs::read(path).with_context(|| format!("couldn't read {}", path.display()))?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let store = &self.app.store;
        let ids = match ext.as_str() {
            "har" => store.import_har(&data),
            "akev" => store.load_session(&mut std::io::Cursor::new(data)),
            "chlsj" => store.import_charles(&data),
            "saz" => store.import_fiddler(&data),
            _ => store.import_mitmproxy(&data),
        };
        ids.with_context(|| format!("couldn't import {}", path.display()))
    }

    /// Handle one MCP JSON-RPC payload (a message or a batch) in-process, exactly like
    /// `POST /mcp` (used by the `akev mcp` stdio bridge when it runs an embedded instance).
    /// Returns `None` when there is nothing to answer (notifications).
    pub async fn mcp(&self, payload: &[u8]) -> Option<serde_json::Value> {
        let exec = mcp::tools::AppTools::new(self.app.clone());
        mcp::handle_payload(&exec, payload).await
    }
}

impl Drop for AkevHandle {
    fn drop(&mut self) {
        // The embedder never called `shutdown`: stop the API server right away and, when a
        // runtime is still around, run the graceful shutdown in the background so launched
        // browsers are closed and the system proxy is restored.
        self.app.request_shutdown();
        if !self.app.shutdown_started()
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let app = self.app.clone();
            // The data directory stays locked until the last state is saved.
            let lock = self.lock.lock().take();
            runtime.spawn(async move {
                app.shutdown().await;
                drop(lock);
            });
        }
    }
}

/// Start Akev: load state, start the proxy and serve the API/UI.
pub async fn start(options: AkevOptions) -> anyhow::Result<AkevHandle> {
    let api_bind = match options.api_bind.trim() {
        "" => "127.0.0.1".to_string(),
        b => b.to_string(),
    };
    let network_api = !auth::bind_is_loopback(&api_bind);
    let token = match options.token.clone() {
        Some(t) => {
            let t = validate_token(&t)?;
            if network_api {
                validate_network_token(&t, &api_bind)?;
            }
            t
        }
        None => generate_token(),
    };
    // A network-exposed API also protects the static UI; browsers get a separate,
    // UI-only session cookie for it (never the API token itself).
    let ui_session = network_api.then(generate_token);

    let persist = !options.ephemeral;
    let (data_dir, temp_dir) = match (&options.data_dir, options.ephemeral) {
        (Some(dir), _) => (dir.clone(), None),
        (None, true) => {
            let tmp = tempfile::Builder::new()
                .prefix("akev-")
                .tempdir()
                .context("failed to create a temporary data directory")?;
            (tmp.path().to_path_buf(), Some(tmp))
        }
        (None, false) => (
            default_data_dir().context(
                "could not determine the platform data directory; pass a data directory explicitly",
            )?,
            None,
        ),
    };
    persist::ensure_dir(&data_dir)
        .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
    // One instance per data directory: the desktop app and the daemon would clobber each
    // other's settings, rules and autosave.
    let lock = if persist {
        Some(datadir_lock::DataDirLock::acquire(
            &data_dir,
            options.desktop,
        )?)
    } else {
        None
    };
    observability::register_secret(&token);

    let settings = app::load_settings(&data_dir, persist);
    let rules = app::load_rules(&data_dir, persist, options.rules_file.as_deref())?;
    let license = Arc::new(license::LicenseManager::load(
        &data_dir,
        persist,
        options.license.clone(),
        &options.license_public_keys,
        options.trial_file.clone(),
    ));
    let license_status = license.status();

    let ca_dir = data_dir.clone();
    let ca = tokio::task::spawn_blocking(move || CertificateAuthority::load_or_generate(&ca_dir))
        .await
        .context("CA task failed")?
        .context("failed to load or generate the CA certificate")?;
    let ca = Arc::new(ca);
    let ca_path = data_dir.join("ca.pem");
    // The chain too: `ca.pem` is also the CA's own storage, and a bring-your-own intermediate
    // CA must keep its chain.
    app::ensure_ca_file(&ca_path, &ca.chain_pem());

    let listener = bind_api(&api_bind, options.api_port).await?;
    let api_addr = listener
        .local_addr()
        .context("failed to read the API address")?;
    let api_url = api_url_for(api_addr);
    if let Some(lock) = &lock {
        lock.set_api_url(&api_url);
    }
    // Other machines reach a network-exposed API over HTTPS only.
    let lan = net::lan_ipv4_addresses();
    let api_tls = if network_api {
        let names = tls::certificate_names(api_addr.ip(), &lan);
        Some(tls::ApiTls::new(&ca, names).context("failed to issue the API's TLS certificate")?)
    } else {
        None
    };
    let network_url = network_api.then(|| network_url_for(api_addr, &lan));

    // Compiling runs script rules' top-level code: keep it off the async workers. Rules using
    // Pro actions without Pro are kept but not compiled.
    let locked_actions = license.locked_actions();
    let rules_engine = Arc::new(
        tokio::task::spawn_blocking(move || {
            RulesEngine::with_locked_actions(rules, locked_actions)
        })
        .await
        .context("rules task failed")?,
    );
    let spill_dir = data_dir.join("spill");
    if let Err(e) = persist::ensure_dir(&spill_dir) {
        tracing::warn!(
            "could not create spill directory {}: {e}",
            spill_dir.display()
        );
    }
    let store = Store::new(settings.capture.clone(), Some(&spill_dir));
    store.set_rule_titles(app::rule_titles(&rules_engine.rules()));
    let bus = app::new_bus();
    let request_bodies = {
        let store = Arc::downgrade(&store);
        Arc::new(move |id| {
            store
                .upgrade()
                .and_then(|s| s.body(id, akev_core::Side::Request))
        }) as breakpoints::RequestBodySource
    };
    let store_sink = store.sink();
    let exchange_notices = {
        let sink = store_sink.clone();
        Arc::new(move |id, message: &str| {
            sink.emit(akev_core::ProxyEvent::Notice {
                id,
                message: message.to_string(),
                dropped: None,
            })
        }) as breakpoints::ExchangeNotices
    };
    let breakpoints = Arc::new(
        BreakpointManager::new(app::breakpoint_notifier(bus.clone()))
            .with_request_bodies(request_bodies)
            .with_exchange_notices(exchange_notices),
    );
    breakpoints.set_limits(&settings.breakpoints);
    let recording = Arc::new(RecordingSink::new(store_sink));
    // Imported protobuf schemas are compiled at startup: off the async workers.
    let protos_dir = data_dir.join("protos");
    let protos = Arc::new(
        tokio::task::spawn_blocking(move || {
            if persist {
                protos::ProtoRegistry::load(protos_dir)
            } else {
                protos::ProtoRegistry::in_memory()
            }
        })
        .await
        .context("schema registry task failed")?,
    );
    protos.set_enabled(license_status.has(akev_core::license::Feature::ProtoSchemas));

    // Saved sessions. A restore of the autosave reserves its ids before the proxy starts (so
    // restored traffic sorts first) and loads in the background once the app runs.
    let sessions = Arc::new(sessions::Sessions::new(sessions::library_dir(&data_dir)));
    let pending_restore = {
        let (sessions, wanted) = (sessions.clone(), settings.sessions.clone());
        tokio::task::spawn_blocking(move || sessions.prepare_restore(&wanted))
            .await
            .context("session restore task failed")?
    };

    // `--proxy-port` / `--proxy-bind` apply to this run only: they're not written to the
    // settings, but the app keeps them as the listener in effect (`App::proxy_bind`, the
    // preferred port) until the user changes that field through the settings.
    let mut proxy_settings = license::effective_proxy(&settings.proxy, &license_status);
    if let Some(port) = options.proxy_port {
        proxy_settings.port = port;
    }
    if let Some(bind) = options.proxy_bind.as_ref().filter(|b| !b.trim().is_empty()) {
        proxy_settings.bind_address = app::normalize_bind(bind);
    }
    let preferred_proxy_port = proxy_settings.port;
    let proxy_bind = proxy_settings.bind_address.clone();
    let proxy = akev_proxy::start(ProxyOptions {
        settings: proxy_settings,
        capture: settings.capture.clone(),
        ca: ca.clone(),
        rules: rules_engine.clone(),
        sink: recording.clone() as Arc<dyn EventSink>,
        breakpoints: breakpoints.clone(),
    })
    .await
    .context("failed to start the proxy")?;

    let wireguard = wireguard::WireguardManager::new(data_dir.clone());
    let mcp_files = mcp::files::McpFiles::new(&data_dir, &options.mcp_allow_paths);
    let app = App::new(AppParts {
        desktop: options.desktop,
        data_dir,
        temp_dir,
        persist,
        token: token.clone(),
        api_addr,
        api_url,
        network_url,
        api_tls: api_tls.clone(),
        settings,
        ca,
        ca_path,
        rules: rules_engine,
        store,
        proxy,
        recording,
        breakpoints,
        bus,
        interceptors: wireguard::interceptor_registry(&wireguard),
        preferred_proxy_port,
        proxy_bind,
        mcp_allow_launch: options.mcp_allow_launch,
        mcp_reveal_secrets: options.mcp_reveal_secrets,
        mcp_files,
        ui_session: ui_session.clone(),
        protos,
        sessions,
        wireguard,
        license,
    });
    app.wireguard.attach(&app);
    app.watch_clients();
    // Failures (e.g. port in use) are logged and shown in `GET /api/wireguard`.
    let _ = app
        .wireguard
        .apply(&app.effective_proxy(&app.settings().proxy).wireguard)
        .await;
    app.schedule_trial_end();

    let security = Arc::new(
        auth::Security::new(token, &api_bind, api_addr.port(), &options.allowed_origins)
            .with_ui_session(ui_session)
            .with_tls(api_tls.is_some()),
    );
    let router = routes::router(app.clone(), security);
    let mut shutdown_rx = app.shutdown_receiver();
    let shutdown = async move {
        let _ = shutdown_rx.wait_for(|requested| *requested).await;
    };
    // The peer address feeds the failed-authentication throttle (`auth::guard`).
    let service = router.into_make_service_with_connect_info::<SocketAddr>();
    let server = match api_tls {
        Some(api_tls) => {
            let listener = tls::listen(listener, api_tls)
                .context("failed to start the API's TLS listener")?
                .tap_io(|_| {});
            tokio::spawn(async move {
                let serve = axum::serve(listener, service).with_graceful_shutdown(shutdown);
                if let Err(e) = serve.await {
                    tracing::error!("API server error: {e}");
                }
            })
        }
        None => {
            let listener = listener.tap_io(|tcp| {
                let _ = tcp.set_nodelay(true);
            });
            tokio::spawn(async move {
                let serve = axum::serve(listener, service).with_graceful_shutdown(shutdown);
                if let Err(e) = serve.await {
                    tracing::error!("API server error: {e}");
                }
            })
        }
    };
    app.tasks.lock().push(server);
    stream::spawn_background_tasks(&app);
    sessions::spawn(&app, pending_restore);
    app.register_daemon();
    tracing::info!(
        "Akev {} — proxy on {}, API on {}",
        akev_core::VERSION,
        app.proxy().local_addr(),
        app.api_url
    );
    Ok(AkevHandle {
        app,
        lock: parking_lot::Mutex::new(lock),
    })
}

/// Bind the API listener, falling back to a random port when the preferred one is busy.
async fn bind_api(bind: &str, port: u16) -> anyhow::Result<tokio::net::TcpListener> {
    let ip = parse_bind(bind)?;
    match tokio::net::TcpListener::bind((ip, port)).await {
        Ok(l) => Ok(l),
        Err(e) if port != 0 && e.kind() == std::io::ErrorKind::AddrInUse => {
            tracing::warn!("API port {port} is busy; using a random port");
            tokio::net::TcpListener::bind((ip, 0))
                .await
                .with_context(|| format!("failed to bind the API on {ip}"))
        }
        Err(e) => Err(e).with_context(|| format!("failed to bind the API on {ip}:{port}")),
    }
}

fn parse_bind(bind: &str) -> anyhow::Result<IpAddr> {
    let b = bind.trim().trim_start_matches('[').trim_end_matches(']');
    if b.is_empty() || b == "localhost" {
        return Ok(IpAddr::V4(Ipv4Addr::LOCALHOST));
    }
    b.parse::<IpAddr>()
        .with_context(|| format!("invalid API bind address {bind:?}"))
}

/// The URL local clients use to reach an API listening on `addr`.
fn api_url_for(addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => format!("[{}]", Ipv6Addr::LOCALHOST),
        IpAddr::V6(ip) => format!("[{ip}]"),
        IpAddr::V4(ip) => ip.to_string(),
    };
    format!("http://{host}:{}", addr.port())
}

/// The `https://` URL other machines use for an API listening on `addr`: the bind address,
/// or the first LAN address when bound to all interfaces.
fn network_url_for(addr: SocketAddr, lan: &[Ipv4Addr]) -> String {
    let host = match addr.ip() {
        ip if ip.is_unspecified() => lan
            .first()
            .map(|v4| v4.to_string())
            .unwrap_or_else(|| "localhost".into()),
        IpAddr::V6(ip) => format!("[{ip}]"),
        IpAddr::V4(ip) => ip.to_string(),
    };
    format!("https://{host}:{}", addr.port())
}

/// 32 random bytes, hex encoded.
fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    rand::fill(&mut bytes[..]);
    hex::encode(bytes)
}

/// User-provided tokens must be usable in headers, query strings and cookies.
fn validate_token(token: &str) -> anyhow::Result<String> {
    let t = token.trim();
    let ok = !t.is_empty()
        && t.len() <= 512
        && t.bytes()
            .all(|b| b.is_ascii_graphic() && !matches!(b, b';' | b',' | b'"' | b'\\'));
    if !ok {
        anyhow::bail!(
            "invalid API token: use 1-512 printable ASCII characters without spaces, quotes, `;`, `,` or `\\`"
        );
    }
    Ok(t.to_string())
}

/// A network-exposed API (`api_bind` not loopback) can be brute-forced from the network, so a
/// user-chosen token must be long and not trivially repetitive.
fn validate_network_token(token: &str, bind: &str) -> anyhow::Result<()> {
    let distinct = token
        .bytes()
        .collect::<std::collections::HashSet<u8>>()
        .len();
    if token.len() < MIN_NETWORK_TOKEN_LEN || distinct < 8 {
        anyhow::bail!(
            "the API is bound to {bind}, which is reachable from the network, so a custom API token must be at least {MIN_NETWORK_TOKEN_LEN} characters long and not repetitive (it grants full control, including launching programs). Omit --token / AKEV_TOKEN to use a random 256-bit token."
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
        assert_eq!(validate_token(" secret-123 ").unwrap(), "secret-123");
        assert!(validate_token("").is_err());
        assert!(validate_token("has space").is_err());
        assert!(validate_token("semi;colon").is_err());
        assert!(validate_token(&"x".repeat(600)).is_err());
    }

    #[test]
    fn network_tokens_must_be_strong() {
        assert!(validate_network_token("dev", "0.0.0.0").is_err());
        assert!(validate_network_token(&"a".repeat(40), "0.0.0.0").is_err());
        assert!(validate_network_token("abababababababababababab", "0.0.0.0").is_err());
        assert!(validate_network_token("0123456789abcdef01234567", "0.0.0.0").is_ok());
        assert!(validate_network_token(&generate_token(), "::").is_ok());
        let err = validate_network_token("t", "192.168.1.2")
            .unwrap_err()
            .to_string();
        assert!(err.contains("192.168.1.2") && err.contains("Omit --token"));
    }

    #[tokio::test]
    async fn network_api_refuses_weak_tokens_but_loopback_allows_them() {
        let weak = |bind: &str| AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            api_bind: bind.into(),
            token: Some("dev".into()),
            ..AkevOptions::default()
        };
        let err = match start(weak("0.0.0.0")).await {
            Ok(h) => {
                h.shutdown().await;
                panic!("a 3-character token must not protect a network-exposed API");
            }
            Err(e) => e.to_string(),
        };
        assert!(err.contains("at least 24 characters"), "{err}");
        let handle = start(weak("127.0.0.1")).await.unwrap();
        assert_eq!(handle.token(), "dev");
        handle.shutdown().await;
    }

    #[test]
    fn network_urls() {
        let lan = [Ipv4Addr::new(192, 168, 1, 23)];
        assert_eq!(
            network_url_for("0.0.0.0:45456".parse().unwrap(), &lan),
            "https://192.168.1.23:45456"
        );
        assert_eq!(
            network_url_for("10.0.0.5:1".parse().unwrap(), &lan),
            "https://10.0.0.5:1"
        );
        assert_eq!(
            network_url_for("[fd00::5]:2".parse().unwrap(), &[]),
            "https://[fd00::5]:2"
        );
    }

    #[test]
    fn api_urls() {
        assert_eq!(
            api_url_for("127.0.0.1:45456".parse().unwrap()),
            "http://127.0.0.1:45456"
        );
        assert_eq!(
            api_url_for("0.0.0.0:1".parse().unwrap()),
            "http://127.0.0.1:1"
        );
        assert_eq!(api_url_for("[::]:2".parse().unwrap()), "http://[::1]:2");
        assert_eq!(api_url_for("[::1]:3".parse().unwrap()), "http://[::1]:3");
        assert_eq!(
            api_url_for("192.168.1.2:4".parse().unwrap()),
            "http://192.168.1.2:4"
        );
    }

    #[test]
    fn bind_parsing() {
        assert_eq!(
            parse_bind("localhost").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(parse_bind("").unwrap(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(
            parse_bind("[::1]").unwrap(),
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
        assert!(parse_bind("example.com").is_err());
    }

    #[tokio::test]
    async fn api_port_falls_back_when_busy() {
        let first = bind_api("127.0.0.1", 0).await.unwrap();
        let port = first.local_addr().unwrap().port();
        let second = bind_api("127.0.0.1", port).await.unwrap();
        assert_ne!(second.local_addr().unwrap().port(), port);
    }

    #[tokio::test]
    async fn body_views_run_at_most_three_at_once() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let handle = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
        .unwrap();
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let tasks: Vec<_> = (0..10)
            .map(|_| {
                let (app, running, peak) = (handle.app.clone(), running.clone(), peak.clone());
                tokio::spawn(async move {
                    routes::common::body_view_blocking(&app, move || {
                        peak.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                        std::thread::sleep(std::time::Duration::from_millis(30));
                        running.fetch_sub(1, SeqCst);
                    })
                    .await
                    .unwrap();
                })
            })
            .collect();
        for task in tasks {
            task.await.unwrap();
        }
        let peak = peak.load(SeqCst);
        assert!(peak <= app::BODY_VIEW_PERMITS, "{peak} views at once");
        assert!(peak > 1, "views still run in parallel");
        handle.shutdown().await;
    }

    #[tokio::test]
    async fn network_api_answers_over_tls_verified_by_the_ca() {
        let dir = tempfile::tempdir().unwrap();
        let handle = start(AkevOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            api_bind: "0.0.0.0".into(),
            ..AkevOptions::default()
        })
        .await
        .unwrap();
        let port = handle.api_addr().port();
        let network = handle.network_url().expect("network URL");
        assert!(network.starts_with("https://"), "{network}");
        assert_eq!(
            handle.status().network_url.as_deref(),
            Some(network.as_str())
        );

        let ca = reqwest::Certificate::from_pem(handle.app.ca().cert_pem().as_bytes()).unwrap();
        let trusting = |ca: Option<reqwest::Certificate>| {
            let mut b = reqwest::Client::builder()
                .no_proxy()
                .tls_built_in_root_certs(false);
            if let Some(ca) = ca {
                b = b.add_root_certificate(ca);
            }
            b.build().unwrap()
        };
        let client = trusting(Some(ca.clone()));
        for host in ["127.0.0.1", "localhost"] {
            let resp = client
                .get(format!("https://{host}:{port}/api/status"))
                .bearer_auth(handle.token())
                .send()
                .await
                .unwrap_or_else(|e| panic!("https://{host}: {e}"));
            assert_eq!(resp.status(), 200, "{host}");
            let status: serde_json::Value = resp.json().await.unwrap();
            assert_eq!(status["networkUrl"], network.as_str());
            // Token, Host and Origin checks still apply over TLS.
            let resp = client
                .get(format!("https://{host}:{port}/api/status"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 401, "{host}: the token is still required");
            let resp = client
                .get(format!("https://{host}:{port}/api/status"))
                .bearer_auth(handle.token())
                .header("Origin", "https://evil.example")
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 403, "{host}: foreign origins are refused");
            let resp = client
                .get(format!("https://{host}:{port}/api/status"))
                .bearer_auth(handle.token())
                .header("Origin", format!("https://{host}:{port}"))
                .send()
                .await
                .unwrap();
            assert_eq!(resp.status(), 200, "{host}: the UI's own https origin");
        }
        // The certificate is Akev's: without the CA it isn't trusted.
        let untrusting = trusting(None);
        assert!(
            untrusting
                .get(format!("https://127.0.0.1:{port}/api/status"))
                .send()
                .await
                .is_err()
        );
        // This machine keeps plain HTTP (CLI, MCP bridge, terminal one-liners).
        let plain = reqwest::Client::builder().no_proxy().build().unwrap();
        let resp = plain
            .get(format!("{}/api/status", handle.api_url()))
            .bearer_auth(handle.token())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert!(handle.api_url().starts_with("http://"));

        // A regenerated CA re-issues the API certificate.
        handle.app.regenerate_ca().await.unwrap();
        assert!(
            // A new client: the old one may reuse a connection from before.
            trusting(Some(ca))
                .get(format!("https://127.0.0.1:{port}/api/status"))
                .bearer_auth(handle.token())
                .send()
                .await
                .is_err(),
            "the old CA no longer verifies it"
        );
        let new_ca = reqwest::Certificate::from_pem(handle.app.ca().cert_pem().as_bytes()).unwrap();
        let resp = trusting(Some(new_ca))
            .get(format!("https://127.0.0.1:{port}/api/status"))
            .bearer_auth(handle.token())
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        handle.shutdown().await;

        // Loopback binds stay plain HTTP only.
        let local = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
        .unwrap();
        assert_eq!(local.network_url(), None);
        local.shutdown().await;
    }

    #[tokio::test]
    async fn ca_regeneration_keeps_bak_backups() {
        let dir = tempfile::tempdir().unwrap();
        let handle = start(AkevOptions {
            data_dir: Some(dir.path().to_path_buf()),
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
        .unwrap();
        let before = handle.status().ca;
        assert_eq!(before.expiry, akev_ca::CaExpiry::Valid);
        assert!(before.days_remaining > 3000, "new CAs last 10 years");
        let old_pem = std::fs::read_to_string(dir.path().join("ca.pem")).unwrap();
        let after = handle.app.regenerate_ca().await.unwrap();
        assert_ne!(after.sha256_fingerprint, before.sha256_fingerprint);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("ca.pem.bak")).unwrap(),
            old_pem
        );
        assert!(dir.path().join("ca.key.bak").exists());
        assert!(!dir.path().join("ca.pem.old").exists());
        assert!(!dir.path().join("ca.key.old").exists());
        assert_eq!(
            handle.status().ca.sha256_fingerprint,
            after.sha256_fingerprint
        );
        handle.shutdown().await;
    }

    #[test]
    fn default_options() {
        let o = AkevOptions::default();
        assert_eq!(o.api_port, 45456);
        assert_eq!(o.api_bind, "127.0.0.1");
        assert!(!o.ephemeral);
    }
}
