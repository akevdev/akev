//! WireGuard mode in the application: runs the embedded WireGuard server
//! (`akev-wireguard`) according to `settings.proxy.wireguard`, hands its connections to the
//! current proxy, serves `/api/wireguard*` and lists the `wireguard` interceptor on the
//! Connect page.

use crate::app::App;
use crate::error::{ApiError, ApiResult, JsonBody};
use crate::routes::common::{Params, attachment};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use akev_core::settings::WireguardSettings;
use akev_intercept::{InterceptContext, Interceptor};
use akev_wireguard::config::{CONFIG_FILE_NAME, client_config, endpoint};
use akev_wireguard::dns::SystemDns;
use akev_wireguard::{
    CLIENT_ADDRESS, DNS_ADDRESS, INTERCEPTOR_ID, PeerStatus, ProxyBridge, ServerOptions,
    ServerStatus, WireguardKeys, WireguardServer,
};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, Weak};

/// Owns the WireGuard server and its keys.
pub(crate) struct WireguardManager {
    data_dir: PathBuf,
    app: OnceLock<Weak<App>>,
    keys: Mutex<Option<WireguardKeys>>,
    server: Mutex<Option<WireguardServer>>,
    /// Why the server isn't running although it's enabled.
    error: Mutex<Option<String>>,
    /// Serializes start / stop / regenerate.
    lifecycle: tokio::sync::Mutex<()>,
}

/// `GET /api/wireguard` (see the HTTP API reference, akev.dev/docs/api).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WireguardInfo {
    /// `settings.proxy.wireguard.enabled`.
    pub enabled: bool,
    /// The server is listening.
    pub running: bool,
    /// UDP port in the client configuration (the bound port while running).
    pub port: u16,
    /// `settings.proxy.wireguard.bindAddress` (absent = all interfaces).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,
    /// The bound socket address while running.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_addr: Option<String>,
    /// Why the server isn't running although it's enabled (e.g. port in use).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Hosts devices can use to reach this machine (LAN addresses); the first is the default.
    pub endpoints: Vec<String>,
    /// `host:port` written into `config`.
    pub endpoint: String,
    /// The client configuration (`.conf` file content).
    pub config: String,
    /// What to encode in the QR code (the configuration, as the WireGuard apps expect).
    pub qr_payload: String,
    /// Suggested file name for the configuration download.
    pub config_file_name: &'static str,
    /// Akev's public key (the client's `[Peer] PublicKey`).
    pub server_public_key: String,
    /// The device's tunnel address.
    pub client_address: String,
    /// The DNS relay address.
    pub dns_address: String,
    /// When the keys were generated (unix ms).
    pub keys_created_at: u64,
    /// The client's handshake state (defaults while stopped).
    pub peer: PeerStatus,
    /// Server counters (absent while stopped).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<ServerStatus>,
}

/// `PUT /api/wireguard` body: fields to change in `settings.proxy.wireguard`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WireguardPatch {
    pub enabled: Option<bool>,
    pub port: Option<u16>,
    /// `null` or `""` = all interfaces.
    #[serde(default, deserialize_with = "double_option")]
    pub bind_address: Option<Option<String>>,
}

fn double_option<'de, D>(d: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::deserialize(d)?))
}

/// The UDP address to bind for these settings.
pub(crate) fn bind_addr(s: &WireguardSettings) -> Result<SocketAddr, String> {
    let ip = match s.bind_address.as_deref().map(str::trim) {
        None | Some("") => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        Some(b) => b
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<IpAddr>()
            .map_err(|_| format!("proxy.wireguard.bindAddress {b:?} is not an IP address"))?,
    };
    Ok(SocketAddr::new(ip, s.port))
}

/// Validation for `settings.proxy.wireguard`.
pub(crate) fn validate(s: &WireguardSettings) -> Result<(), String> {
    bind_addr(s).map(|_| ())
}

fn valid_endpoint_host(host: &str) -> bool {
    let h = host.trim_start_matches('[').trim_end_matches(']');
    !h.is_empty()
        && h.len() <= 253
        && (h.parse::<IpAddr>().is_ok()
            || h.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'))
}

impl WireguardManager {
    pub(crate) fn new(data_dir: PathBuf) -> Arc<Self> {
        Arc::new(WireguardManager {
            data_dir,
            app: OnceLock::new(),
            keys: Mutex::new(None),
            server: Mutex::new(None),
            error: Mutex::new(None),
            lifecycle: tokio::sync::Mutex::new(()),
        })
    }

    /// Connect to the application (for the proxy handle and settings updates).
    pub(crate) fn attach(&self, app: &Arc<App>) {
        let _ = self.app.set(Arc::downgrade(app));
    }

    fn app(&self) -> Option<Arc<App>> {
        self.app.get().and_then(Weak::upgrade)
    }

    /// The keys (loaded, or generated and saved, on first use).
    fn keys(&self) -> ApiResult<WireguardKeys> {
        let mut keys = self.keys.lock();
        if let Some(k) = keys.as_ref() {
            return Ok(k.clone());
        }
        let k = WireguardKeys::load_or_generate(&self.data_dir)
            .map_err(|e| ApiError::internal(format!("WireGuard keys: {e}")))?;
        *keys = Some(k.clone());
        Ok(k)
    }

    fn status(&self) -> Option<ServerStatus> {
        self.server.lock().as_ref().map(WireguardServer::status)
    }

    /// Start, stop or restart the server to match `settings`. When starting fails, the
    /// previous server (if any) is restored and the error returned.
    pub(crate) async fn apply(&self, settings: &WireguardSettings) -> Result<(), String> {
        let _guard = self.lifecycle.lock().await;
        self.apply_locked(settings, false).await
    }

    async fn apply_locked(&self, settings: &WireguardSettings, force: bool) -> Result<(), String> {
        if !settings.enabled {
            self.stop_server().await;
            *self.error.lock() = None;
            return Ok(());
        }
        let bind = bind_addr(settings)?;
        let current = self.server.lock().as_ref().map(|s| s.local_addr());
        if !force
            && let Some(addr) = current
            && addr.ip() == bind.ip()
            && (bind.port() == 0 || addr.port() == bind.port())
        {
            return Ok(());
        }
        let keys = self.keys().map_err(|e| e.message.clone())?;
        let previous = current;
        self.stop_server().await;
        match self.start_server(bind, keys.clone()).await {
            Ok(server) => {
                *self.server.lock() = Some(server);
                *self.error.lock() = None;
                Ok(())
            }
            Err(e) => {
                let message = format!("could not start the WireGuard server on UDP {bind}: {e}");
                tracing::warn!("{message}");
                if let Some(prev) = previous
                    && let Ok(server) = self.start_server(prev, keys).await
                {
                    *self.server.lock() = Some(server);
                    return Err(message);
                }
                *self.error.lock() = Some(message.clone());
                Err(message)
            }
        }
    }

    async fn start_server(
        &self,
        bind: SocketAddr,
        keys: WireguardKeys,
    ) -> std::io::Result<WireguardServer> {
        let app = self.app.get().cloned().unwrap_or_default();
        let bridge = ProxyBridge::new(move || app.upgrade().map(|a| a.proxy()));
        WireguardServer::start(
            ServerOptions::new(bind, keys),
            Arc::new(bridge),
            SystemDns::new(),
        )
        .await
    }

    async fn stop_server(&self) {
        let server = self.server.lock().take();
        if let Some(server) = server {
            server.stop().await;
        }
    }

    /// Generate new keys (invalidating every configuration handed out) and restart a running
    /// server with them.
    pub(crate) async fn regenerate(&self, settings: &WireguardSettings) -> ApiResult<()> {
        let _guard = self.lifecycle.lock().await;
        let dir = self.data_dir.clone();
        let keys = tokio::task::spawn_blocking(move || WireguardKeys::regenerate(&dir))
            .await
            .map_err(|e| ApiError::internal(format!("key generation task failed: {e}")))?
            .map_err(|e| ApiError::internal(format!("WireGuard keys: {e}")))?;
        *self.keys.lock() = Some(keys);
        if settings.enabled {
            self.apply_locked(settings, true)
                .await
                .map_err(ApiError::conflict)?;
        }
        Ok(())
    }

    /// Stop the server (application shutdown); the setting is left as it is.
    pub(crate) async fn shutdown(&self) {
        let _guard = self.lifecycle.lock().await;
        self.stop_server().await;
    }

    /// Everything the UI needs. `host` picks the endpoint written into the configuration.
    pub(crate) fn info(
        &self,
        settings: &WireguardSettings,
        host: Option<&str>,
    ) -> ApiResult<WireguardInfo> {
        let keys = self.keys()?;
        let stats = self.status();
        let port = stats
            .as_ref()
            .and_then(|s| s.listen_addr.parse::<SocketAddr>().ok())
            .map(|a| a.port())
            .unwrap_or(settings.port);
        let endpoints = endpoint_hosts(settings);
        let host = match host.map(str::trim).filter(|h| !h.is_empty()) {
            Some(h) if valid_endpoint_host(h) => h.to_string(),
            Some(h) => {
                return Err(ApiError::bad_request(format!(
                    "{h:?} is not a valid host name or IP address"
                )));
            }
            None => endpoints
                .first()
                .cloned()
                .unwrap_or_else(|| Ipv4Addr::LOCALHOST.to_string()),
        };
        let config = client_config(&keys, &host, port);
        Ok(WireguardInfo {
            enabled: settings.enabled,
            running: stats.is_some(),
            port,
            bind_address: settings.bind_address.clone(),
            listen_addr: stats.as_ref().map(|s| s.listen_addr.clone()),
            error: if settings.enabled {
                self.error.lock().clone()
            } else {
                None
            },
            endpoints,
            endpoint: endpoint(&host, port),
            qr_payload: config.clone(),
            config,
            config_file_name: CONFIG_FILE_NAME,
            server_public_key: keys.server.public_key().to_base64(),
            client_address: format!("{CLIENT_ADDRESS}/32"),
            dns_address: DNS_ADDRESS.to_string(),
            keys_created_at: keys.created_at,
            peer: stats
                .as_ref()
                .map(|s| s.peer.clone())
                .unwrap_or_else(|| PeerStatus {
                    public_key: keys.client.public_key().to_base64(),
                    ..PeerStatus::default()
                }),
            stats,
        })
    }
}

/// Addresses of this machine a device can reach the WireGuard port on.
fn endpoint_hosts(settings: &WireguardSettings) -> Vec<String> {
    if let Ok(bind) = bind_addr(settings)
        && !bind.ip().is_unspecified()
    {
        return vec![bind.ip().to_string()];
    }
    crate::net::lan_ipv4_addresses()
        .into_iter()
        .map(|ip| ip.to_string())
        .collect()
}

// ---------- routes ----------

/// `GET /api/wireguard[?host=]`
pub(crate) async fn get_info(
    State(app): State<Arc<App>>,
    params: Params,
) -> ApiResult<Json<WireguardInfo>> {
    let settings = app.settings().proxy.wireguard;
    app.wireguard
        .info(&settings, params.non_empty("host"))
        .map(Json)
}

/// `PUT /api/wireguard` — change `settings.proxy.wireguard` (persisted, applied live).
pub(crate) async fn put(
    State(app): State<Arc<App>>,
    JsonBody(patch): JsonBody<WireguardPatch>,
) -> ApiResult<Json<WireguardInfo>> {
    let mut settings = app.settings();
    let wg = &mut settings.proxy.wireguard;
    if let Some(enabled) = patch.enabled {
        wg.enabled = enabled;
    }
    if let Some(port) = patch.port {
        if port == 0 {
            return Err(ApiError::bad_request("port must be between 1 and 65535"));
        }
        wg.port = port;
    }
    if let Some(bind) = patch.bind_address {
        wg.bind_address = bind.map(|b| b.trim().to_string()).filter(|b| !b.is_empty());
    }
    // `ui: null` keeps the stored UI preferences.
    settings.ui = serde_json::Value::Null;
    let unchanged = settings.proxy.wireguard == app.settings().proxy.wireguard;
    let saved = app.update_settings(settings).await?;
    let effective = app.effective_proxy(&saved.proxy).wireguard;
    if unchanged && effective.enabled && app.wireguard.status().is_none() {
        // Nothing changed, so nothing was applied: retry a server that failed to start.
        let _ = app.wireguard.apply(&effective).await;
    }
    app.wireguard.info(&saved.proxy.wireguard, None).map(Json)
}

/// `POST /api/wireguard/regenerate`
pub(crate) async fn regenerate(State(app): State<Arc<App>>) -> ApiResult<Json<WireguardInfo>> {
    let settings = app.settings().proxy.wireguard;
    app.wireguard.regenerate(&settings).await?;
    app.notice(
        "warning",
        "New WireGuard keys were generated. Devices using the old configuration must import the new one.",
    );
    refresh_interceptors_later(&app);
    app.wireguard.info(&settings, None).map(Json)
}

/// `GET /api/wireguard/config[?host=]` — the client configuration as a `.conf` download.
pub(crate) async fn config_file(
    State(app): State<Arc<App>>,
    params: Params,
) -> ApiResult<Response> {
    let settings = app.settings().proxy.wireguard;
    let info = app.wireguard.info(&settings, params.non_empty("host"))?;
    let mut resp = attachment(info.config, "text/plain; charset=utf-8", CONFIG_FILE_NAME);
    resp.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    Ok(resp)
}

fn refresh_interceptors_later(app: &Arc<App>) {
    let app = app.clone();
    tokio::spawn(async move {
        app.refresh_interceptors(true).await;
    });
}

// ---------- interceptor ----------

/// The built-in interceptors plus `wireguard` (listed right after `ios`).
pub(crate) fn interceptor_registry(
    manager: &Arc<WireguardManager>,
) -> akev_intercept::InterceptorRegistry {
    let mut list = akev_intercept::builtin_interceptors(akev_intercept::Sys::system());
    let at = list
        .iter()
        .position(|i| i.id() == "ios")
        .map_or(list.len(), |i| i + 1);
    list.insert(at, Arc::new(WireguardInterceptor(manager.clone())));
    akev_intercept::InterceptorRegistry::with(list)
}

/// The `wireguard` entry of the interceptor catalogue: "activating" it turns WireGuard mode
/// on (persisted in the settings); the UI shows the QR code panel.
pub(crate) struct WireguardInterceptor(pub Arc<WireguardManager>);

impl WireguardInterceptor {
    async fn set_enabled(&self, enabled: bool) -> akev_intercept::Result<Option<String>> {
        let app = self.0.app().ok_or_else(|| {
            akev_intercept::InterceptError::Unavailable("Akev is shutting down".into())
        })?;
        let mut settings = app.settings();
        if settings.proxy.wireguard.enabled != enabled {
            settings.proxy.wireguard.enabled = enabled;
            settings.ui = serde_json::Value::Null;
            app.update_settings(settings)
                .await
                .map_err(|e| akev_intercept::InterceptError::Failed(e.message.clone()))?;
        } else if enabled && self.0.status().is_none() {
            // Enabled but not running (it failed to start): try again.
            let _ = self
                .0
                .apply(&app.effective_proxy(&settings.proxy).wireguard)
                .await;
        }
        Ok(self.0.status().map(|s| s.listen_addr))
    }
}

#[async_trait::async_trait]
impl Interceptor for WireguardInterceptor {
    fn id(&self) -> &str {
        INTERCEPTOR_ID
    }

    async fn info(&self, _ctx: &InterceptContext) -> InterceptorInfo {
        let status = self.0.status();
        let error = self.0.error.lock().clone();
        InterceptorInfo {
            id: INTERCEPTOR_ID.into(),
            name: "WireGuard (phones & any device)".into(),
            description: "Scan a QR code with the WireGuard app: all of the device's traffic is intercepted, no proxy settings needed. iOS, Android, macOS, Windows and Linux.".into(),
            category: InterceptorCategory::Mobile,
            tags: [
                "wireguard", "vpn", "ios", "iphone", "ipad", "android", "phone", "tablet",
                "qr", "tunnel", "mobile",
            ]
            .into_iter()
            .map(String::from)
            .collect(),
            icon: "wireguard".into(),
            supported: true,
            // Enabled but not running (e.g. port in use): the card says why; the panel still
            // opens to change the port or retry.
            available: error.is_none(),
            active: status.is_some(),
            unavailable_reason: error,
            version: None,
            metadata: serde_json::json!({
                "listenAddr": status.as_ref().map(|s| s.listen_addr.clone()),
                "connected": status.as_ref().is_some_and(|s| s.peer.connected),
                "lastHandshakeAt": status.as_ref().and_then(|s| s.peer.last_handshake_at),
            }),
            needs_config: true,
        }
    }

    async fn activate(
        &self,
        _ctx: &InterceptContext,
        _options: serde_json::Value,
    ) -> akev_intercept::Result<ActivationResult> {
        match self.set_enabled(true).await? {
            Some(addr) => Ok(ActivationResult {
                success: true,
                message: Some(format!(
                    "WireGuard server listening on UDP {addr}. Scan the QR code with the WireGuard app."
                )),
                metadata: serde_json::Value::Null,
            }),
            None => Err(akev_intercept::InterceptError::Failed(
                self.0
                    .error
                    .lock()
                    .clone()
                    .unwrap_or_else(|| "the WireGuard server didn't start".into()),
            )),
        }
    }

    async fn deactivate(
        &self,
        _ctx: &InterceptContext,
        _options: serde_json::Value,
    ) -> akev_intercept::Result<()> {
        self.set_enabled(false).await.map(|_| ())
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> akev_intercept::Result<()> {
        // A setting, not a session: the server stops with the application and starts again
        // with it (see `App::shutdown`).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_addresses() {
        let mut s = WireguardSettings::default();
        assert_eq!(bind_addr(&s).unwrap(), "0.0.0.0:51820".parse().unwrap());
        s.bind_address = Some("[::]".into());
        s.port = 4000;
        assert_eq!(bind_addr(&s).unwrap(), "[::]:4000".parse().unwrap());
        s.bind_address = Some(" ".into());
        assert_eq!(bind_addr(&s).unwrap(), "0.0.0.0:4000".parse().unwrap());
        s.bind_address = Some("example.com".into());
        assert!(validate(&s).unwrap_err().contains("not an IP address"));
    }

    #[test]
    fn endpoint_hosts_follow_a_specific_bind_address() {
        let s = WireguardSettings {
            bind_address: Some("192.168.7.9".into()),
            ..Default::default()
        };
        assert_eq!(endpoint_hosts(&s), vec!["192.168.7.9".to_string()]);
    }

    #[test]
    fn endpoint_host_validation() {
        assert!(valid_endpoint_host("192.168.1.2"));
        assert!(valid_endpoint_host("[fd00::1]"));
        assert!(valid_endpoint_host("my-pc.lan"));
        assert!(!valid_endpoint_host("evil\nhost"));
        assert!(!valid_endpoint_host("a b"));
        assert!(!valid_endpoint_host(""));
    }

    #[test]
    fn patch_parsing() {
        let p: WireguardPatch = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(p.enabled, Some(true));
        assert!(p.bind_address.is_none());
        let p: WireguardPatch = serde_json::from_str(r#"{"bindAddress":null}"#).unwrap();
        assert_eq!(p.bind_address, Some(None));
        assert!(serde_json::from_str::<WireguardPatch>(r#"{"nope":1}"#).is_err());
    }
}
