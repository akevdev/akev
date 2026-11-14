//! Persistent settings (stored as `settings.json` in the Akev data directory).
//! Every field has a default, so a partial file (or `PUT /api/settings` body) loads.

use serde::{Deserialize, Serialize};

pub use crate::access::{ClientPolicy, ProxyAccessSettings};
pub use crate::transparent::TransparentSettings;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub proxy: ProxySettings,
    pub capture: CaptureSettings,
    /// Saved sessions & autosave.
    pub sessions: SessionSettings,
    /// Checking for new versions of Akev.
    pub updates: UpdateSettings,
    /// Limits on exchanges paused at breakpoints.
    pub breakpoints: BreakpointSettings,
    /// Opaque UI preferences (theme, layout, column widths...). Stored server-side so the
    /// desktop app and browser UI share them.
    pub ui: serde_json::Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxySettings {
    /// Preferred proxy port. If busy, the next free port in `port..=port+100` is used.
    pub port: u16,
    /// Bind address. `127.0.0.1` by default; `0.0.0.0` to allow other devices (phones) to connect.
    pub bind_address: String,
    /// Offer HTTP/2 to clients via ALPN.
    pub http2: bool,
    /// Accept SOCKS4/5 connections on the proxy port.
    pub socks: bool,
    /// Hostname globs whose TLS connections are tunnelled untouched (no interception).
    pub tls_passthrough: Vec<String>,
    /// Hostname globs for which upstream certificate errors are ignored (`*` = all).
    pub ignore_upstream_cert_errors: Vec<String>,
    /// Extra CA certificates (PEM file paths) to trust for upstream connections.
    pub trusted_ca_paths: Vec<String>,
    /// Client certificates presented to upstream servers.
    pub client_certificates: Vec<ClientCertificate>,
    /// Upstream TLS fingerprint (browser impersonation).
    pub tls_profile: TlsProfileSettings,
    pub upstream_proxy: UpstreamProxy,
    /// Static DNS overrides (`host` → `ip`), like /etc/hosts.
    pub dns_overrides: Vec<DnsOverride>,
    /// Use DNS-over-HTTPS for upstream resolution (`None` = system resolver).
    pub doh_url: Option<String>,
    /// Upstream connect timeout (ms).
    pub connect_timeout_ms: u64,
    /// How long to wait for the upstream response headers (ms, 0 = no limit). Rules and
    /// Compose requests can set their own.
    pub response_timeout_ms: u64,
    /// How long a response body may go without receiving data before the exchange fails
    /// (ms, 0 = no limit). Long-running streams that keep sending are never cut off.
    pub body_idle_timeout_ms: u64,
    /// Memory shared by all exchanges whose bodies are held in full for rules, scripts or
    /// breakpoints. Once it's in use, further bodies stream through untouched and the
    /// exchange says why its rules didn't apply.
    pub rule_body_budget_bytes: u64,
    /// Keep the original header casing/order when forwarding HTTP/1 requests.
    pub preserve_header_case: bool,
    /// WireGuard mode: an embedded WireGuard server whose clients' traffic is intercepted.
    pub wireguard: WireguardSettings,
    /// Transparent capture listener (connections redirected by the firewall, Linux).
    pub transparent: TransparentSettings,
    /// HTTP/3 (QUIC): upstream use and the inbound QUIC listener.
    pub http3: Http3Settings,
    /// Who may use the proxy: client policy, blocked clients, optional credentials, whether
    /// other machines may reach this one's loopback services, PROXY protocol trust
    /// (see [`ProxyAccessSettings`]).
    pub access: ProxyAccessSettings,
}

/// Default [`ProxySettings::response_timeout_ms`]: two minutes for the response headers.
pub const DEFAULT_RESPONSE_TIMEOUT_MS: u64 = 120_000;
/// Default [`ProxySettings::body_idle_timeout_ms`]: five minutes without body data.
pub const DEFAULT_BODY_IDLE_TIMEOUT_MS: u64 = 300_000;
/// Default [`ProxySettings::rule_body_budget_bytes`]: 1 GiB across all exchanges.
pub const DEFAULT_RULE_BODY_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

impl Default for ProxySettings {
    fn default() -> Self {
        ProxySettings {
            port: 8000,
            bind_address: "127.0.0.1".into(),
            http2: true,
            socks: true,
            tls_passthrough: Vec::new(),
            ignore_upstream_cert_errors: Vec::new(),
            trusted_ca_paths: Vec::new(),
            client_certificates: Vec::new(),
            tls_profile: TlsProfileSettings::default(),
            upstream_proxy: UpstreamProxy::default(),
            dns_overrides: Vec::new(),
            doh_url: None,
            connect_timeout_ms: 15_000,
            response_timeout_ms: DEFAULT_RESPONSE_TIMEOUT_MS,
            body_idle_timeout_ms: DEFAULT_BODY_IDLE_TIMEOUT_MS,
            rule_body_budget_bytes: DEFAULT_RULE_BODY_BUDGET_BYTES,
            preserve_header_case: true,
            wireguard: WireguardSettings::default(),
            transparent: TransparentSettings::default(),
            http3: Http3Settings::default(),
            access: ProxyAccessSettings::default(),
        }
    }
}

/// WireGuard mode (`proxy.wireguard`): devices that import Akev's WireGuard configuration
/// send all their TCP traffic through the proxy.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WireguardSettings {
    /// Run the WireGuard server.
    pub enabled: bool,
    /// UDP port of the WireGuard server.
    pub port: u16,
    /// Address the UDP socket binds to. `None` = all interfaces (`0.0.0.0`), so devices on
    /// the network can reach it (only holders of the client key can use it).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bind_address: Option<String>,
}

impl Default for WireguardSettings {
    fn default() -> Self {
        WireguardSettings {
            enabled: false,
            port: 51820,
            bind_address: None,
        }
    }
}

/// HTTP/3 over QUIC (`ProxySettings::http3`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Http3Settings {
    /// When requests to HTTPS servers go upstream over HTTP/3.
    pub upstream: Http3UpstreamMode,
    /// Accept HTTP/3 from clients on a QUIC (UDP) listener.
    pub listen: bool,
    /// UDP port of the QUIC listener; 0 = the proxy's (TCP) port number.
    pub listen_port: u16,
}

/// When Akev talks HTTP/3 to upstream servers ([`Http3Settings::upstream`]). Whatever the
/// mode, a failed QUIC connection falls back to TCP (HTTP/2 or HTTP/1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum Http3UpstreamMode {
    /// Never (Compose requests may still ask for HTTP/3 explicitly).
    #[default]
    Off,
    /// For origins that advertised HTTP/3 with an `Alt-Svc: h3=...` response header (cached
    /// for its `ma` max-age), like browsers do.
    AltSvc,
    /// Try HTTP/3 first for every HTTPS origin, then remember origins where it failed.
    Prefer,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCertificate {
    /// Hostname glob this certificate applies to (`*` = all).
    pub host: String,
    /// Path to a PKCS#12 (.p12/.pfx) or PEM (cert+key) file.
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub passphrase: Option<String>,
}

/// Which TLS ClientHello (and HTTP/2 connection preface) Akev presents to upstream servers.
///
/// Browser profiles need a build with the `impersonate` cargo feature (BoringSSL); without
/// it they fall back to `native`. Unknown names (e.g. written by a newer Akev) read as
/// `native` rather than failing the whole settings / rules file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TlsProfile {
    /// Akev's own TLS stack (rustls).
    #[default]
    Native,
    /// Desktop Google Chrome.
    Chrome,
    /// Desktop Mozilla Firefox.
    Firefox,
    /// Safari on macOS.
    Safari,
    /// Microsoft Edge (Chromium).
    Edge,
    /// The intercepted client's own browser family (from its JA4 / `User-Agent`), `native`
    /// for clients that aren't a recognised browser.
    MirrorClient,
}

impl TlsProfile {
    /// Every profile, in display order.
    pub const ALL: [TlsProfile; 6] = [
        TlsProfile::Native,
        TlsProfile::Chrome,
        TlsProfile::Firefox,
        TlsProfile::Safari,
        TlsProfile::Edge,
        TlsProfile::MirrorClient,
    ];

    /// The wire name (`native`, `chrome`, ..., `mirror-client`).
    pub fn as_str(self) -> &'static str {
        match self {
            TlsProfile::Native => "native",
            TlsProfile::Chrome => "chrome",
            TlsProfile::Firefox => "firefox",
            TlsProfile::Safari => "safari",
            TlsProfile::Edge => "edge",
            TlsProfile::MirrorClient => "mirror-client",
        }
    }

    /// Parse a wire name (case-insensitive).
    pub fn parse(name: &str) -> Option<TlsProfile> {
        let name = name.trim();
        TlsProfile::ALL
            .into_iter()
            .find(|p| p.as_str().eq_ignore_ascii_case(name))
    }

    /// Whether this is a browser profile (needs the impersonation backend).
    pub fn is_browser(self) -> bool {
        matches!(
            self,
            TlsProfile::Chrome | TlsProfile::Firefox | TlsProfile::Safari | TlsProfile::Edge
        )
    }
}

impl std::fmt::Display for TlsProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for TlsProfile {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for TlsProfile {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(TlsProfile::parse(&name).unwrap_or_default())
    }
}

/// Upstream TLS fingerprint settings (`proxy.tlsProfile`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TlsProfileSettings {
    /// Profile for every upstream TLS connection without an override.
    pub profile: TlsProfile,
    /// Per-host overrides; the first matching entry wins (rules' `tls-profile` action wins
    /// over both).
    pub overrides: Vec<TlsProfileOverride>,
}

/// A per-host [`TlsProfile`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsProfileOverride {
    /// Hostname glob (`*.example.com`, `api.example.com:8443`, `*`).
    pub host: String,
    #[serde(default)]
    pub profile: TlsProfile,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DnsOverride {
    pub host: String,
    pub address: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamProxy {
    pub mode: UpstreamProxyMode,
    /// For `manual`: `http://host:port`, `https://host:port`, `socks5://user:pass@host:port`.
    /// For `pac`: the PAC file URL.
    #[serde(default)]
    pub url: String,
    /// Hostname globs that bypass the upstream proxy.
    #[serde(default)]
    pub no_proxy: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum UpstreamProxyMode {
    /// Connect directly.
    #[default]
    Direct,
    /// Use the OS-configured proxy (env vars / system settings).
    System,
    /// Use `url`.
    Manual,
    /// Evaluate a PAC script at `url`.
    Pac,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CaptureSettings {
    /// Max bytes of each body to keep (bodies are still streamed through in full).
    pub max_body_bytes: u64,
    /// Total memory budget for in-memory bodies; beyond it bodies spill to a temp file.
    pub memory_budget_bytes: u64,
    /// Max exchanges to retain (oldest unpinned are evicted). 0 = unlimited.
    pub max_exchanges: u64,
    /// Disk space for bodies spilled out of memory. Past it the oldest spilled bodies are
    /// deleted; their exchanges stay, marked as having lost the body. 0 = unlimited.
    pub spill_max_bytes: u64,
    /// Captured traffic waiting to be stored (bytes). When storing falls this far behind,
    /// new exchanges wait briefly, then bodies are dropped and counted rather than letting
    /// memory grow.
    pub event_queue_bytes: u64,
    /// Record tunnelled (non-intercepted) connections as rows.
    pub record_tunnels: bool,
    /// Record TLS handshake failures as rows.
    pub record_tls_failures: bool,
    /// Hostname globs whose traffic is proxied but not recorded (noise suppression).
    pub ignore_hosts: Vec<String>,
    /// Detect the local process behind each client connection.
    pub detect_processes: bool,
}

/// Default [`CaptureSettings::max_exchanges`]. At about 4 KB per typical API exchange (1 KB
/// body included, see `docs/PERFORMANCE.md`) a long session levels off around 800 MB.
pub const DEFAULT_MAX_EXCHANGES: u64 = 200_000;
/// Default [`CaptureSettings::spill_max_bytes`]: 4 GiB of spill files.
pub const DEFAULT_SPILL_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
/// Default [`CaptureSettings::event_queue_bytes`]: 64 MiB.
pub const DEFAULT_EVENT_QUEUE_BYTES: u64 = 64 * 1024 * 1024;
/// Smallest effective [`CaptureSettings::event_queue_bytes`] (one large body must fit).
pub const MIN_EVENT_QUEUE_BYTES: u64 = 1024 * 1024;

impl Default for CaptureSettings {
    fn default() -> Self {
        CaptureSettings {
            max_body_bytes: 32 * 1024 * 1024,
            memory_budget_bytes: 512 * 1024 * 1024,
            max_exchanges: DEFAULT_MAX_EXCHANGES,
            spill_max_bytes: DEFAULT_SPILL_MAX_BYTES,
            event_queue_bytes: DEFAULT_EVENT_QUEUE_BYTES,
            record_tunnels: true,
            record_tls_failures: true,
            ignore_hosts: Vec::new(),
            detect_processes: true,
        }
    }
}

/// Checking for new versions: the only request Akev makes on its own (a small JSON file from
/// GitHub Releases, at most once a day).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UpdateSettings {
    /// Look for a new version in the background.
    pub check_automatically: bool,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        UpdateSettings {
            check_automatically: true,
        }
    }
}

/// Autosave of the captured traffic into the session library (`data_dir/sessions/`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionSettings {
    /// Periodically save the captured traffic to the "Autosave" session (when it changed), and
    /// once more on graceful shutdown.
    pub autosave: bool,
    /// Seconds between autosaves (only when the traffic changed since the last one).
    pub autosave_interval_secs: u64,
    /// Load the "Autosave" session back when Akev starts (only while `autosave` is on).
    pub restore_on_startup: bool,
    /// Most exchanges an autosave keeps (the newest; pinned ones always). 0 = unlimited.
    pub autosave_max_exchanges: u64,
    /// Most body bytes an autosave keeps (newest exchanges first; pinned ones always).
    /// 0 = unlimited.
    pub autosave_max_bytes: u64,
}

/// Default [`SessionSettings::autosave_max_exchanges`].
pub const DEFAULT_AUTOSAVE_MAX_EXCHANGES: u64 = 20_000;
/// Default [`SessionSettings::autosave_max_bytes`]: 256 MiB of bodies.
pub const DEFAULT_AUTOSAVE_MAX_BYTES: u64 = 256 * 1024 * 1024;

impl Default for SessionSettings {
    fn default() -> Self {
        SessionSettings {
            autosave: true,
            autosave_interval_secs: 60,
            restore_on_startup: true,
            autosave_max_exchanges: DEFAULT_AUTOSAVE_MAX_EXCHANGES,
            autosave_max_bytes: DEFAULT_AUTOSAVE_MAX_BYTES,
        }
    }
}

/// Exchanges paused at breakpoints (`breakpoints`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BreakpointSettings {
    /// Most exchanges (and WebSocket messages) paused at once. Past it, further matches
    /// continue without pausing and say why. 0 = no limit.
    pub max_paused: u32,
    /// Continue a paused exchange unchanged after this many seconds. 0 = wait for the user.
    pub auto_release_secs: u64,
}

/// Default [`BreakpointSettings::max_paused`].
pub const DEFAULT_MAX_PAUSED: u32 = 100;

impl Default for BreakpointSettings {
    fn default() -> Self {
        BreakpointSettings {
            max_paused: DEFAULT_MAX_PAUSED,
            auto_release_secs: 0,
        }
    }
}

#[cfg(test)]
mod limits_tests {
    use super::*;

    #[test]
    fn limits_are_bounded_by_default() {
        let s = Settings::default();
        assert_eq!(s.capture.max_exchanges, 200_000);
        assert_eq!(s.capture.spill_max_bytes, 4 << 30);
        assert_eq!(s.capture.event_queue_bytes, 64 << 20);
        assert_eq!(s.proxy.response_timeout_ms, 120_000);
        assert_eq!(s.proxy.body_idle_timeout_ms, 300_000);
        assert_eq!(s.proxy.rule_body_budget_bytes, 1 << 30);
        assert_eq!(s.breakpoints.max_paused, 100);
        assert_eq!(s.breakpoints.auto_release_secs, 0);
        assert!(s.sessions.autosave);
        assert_eq!(s.sessions.autosave_max_exchanges, 20_000);
        assert_eq!(s.sessions.autosave_max_bytes, 256 << 20);
    }

    #[test]
    fn limits_round_trip_and_partial_files_get_defaults() {
        // A file without these settings loads with the bounded defaults.
        let old: Settings =
            serde_json::from_str(r#"{"capture":{"maxBodyBytes":1024},"proxy":{"port":9000}}"#)
                .unwrap();
        assert_eq!(old.capture.max_body_bytes, 1024);
        assert_eq!(old.capture.spill_max_bytes, DEFAULT_SPILL_MAX_BYTES);
        assert_eq!(old.proxy.body_idle_timeout_ms, DEFAULT_BODY_IDLE_TIMEOUT_MS);
        assert_eq!(old.breakpoints, BreakpointSettings::default());

        let json = serde_json::to_value(Settings::default()).unwrap();
        assert_eq!(json["capture"]["spillMaxBytes"], 4u64 << 30);
        assert_eq!(json["capture"]["eventQueueBytes"], 64u64 << 20);
        assert_eq!(json["proxy"]["bodyIdleTimeoutMs"], 300_000);
        assert_eq!(json["proxy"]["ruleBodyBudgetBytes"], 1u64 << 30);
        assert_eq!(
            json["breakpoints"],
            serde_json::json!({"maxPaused": 100, "autoReleaseSecs": 0})
        );
        // Unlimited stays expressible.
        let unlimited: CaptureSettings =
            serde_json::from_str(r#"{"maxExchanges":0,"spillMaxBytes":0}"#).unwrap();
        assert_eq!(unlimited.max_exchanges, 0);
        assert_eq!(unlimited.spill_max_bytes, 0);
    }
}

#[cfg(test)]
mod http3_tests {
    use super::*;

    #[test]
    fn http3_settings_default_and_round_trip() {
        // A file without the setting loads with HTTP/3 off.
        let old: ProxySettings = serde_json::from_str(r#"{"port":9000}"#).unwrap();
        assert_eq!(old.http3, Http3Settings::default());
        assert_eq!(old.http3.upstream, Http3UpstreamMode::Off);
        assert!(!old.http3.listen);
        assert_eq!(old.http3.listen_port, 0);

        let json = serde_json::to_value(ProxySettings::default()).unwrap();
        assert_eq!(
            json["http3"],
            serde_json::json!({"upstream": "off", "listen": false, "listenPort": 0})
        );
        let parsed: ProxySettings = serde_json::from_str(
            r#"{"http3":{"upstream":"alt-svc","listen":true,"listenPort":8443}}"#,
        )
        .unwrap();
        assert_eq!(parsed.http3.upstream, Http3UpstreamMode::AltSvc);
        assert!(parsed.http3.listen);
        assert_eq!(parsed.http3.listen_port, 8443);
        let partial: Http3Settings = serde_json::from_str(r#"{"upstream":"prefer"}"#).unwrap();
        assert_eq!(partial.upstream, Http3UpstreamMode::Prefer);
        assert!(!partial.listen);
    }
}
