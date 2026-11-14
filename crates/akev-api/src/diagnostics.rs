//! `GET /api/diagnostics`: what a bug report needs, without secrets or captured traffic.
//!
//! Version, OS, uptime, counters, a summary of the settings (booleans, counts, ports and
//! limits: no token, no upstream proxy URL, no certificate paths or passphrases, no DNS
//! servers, no UI preferences), the last warnings and errors (scrubbed, see
//! [`crate::observability`]), the names of crash reports and the log file's location (home
//! directory shown as `~`). The UI's Settings → About → "Copy diagnostics" and "Report a
//! problem" show it to the user before anything leaves the computer.

use crate::app::{App, StatusStats};
use crate::error::ApiResult;
use crate::observability::{self, RecentWarning};
use akev_core::settings::Settings;
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// Response of `GET /api/diagnostics`.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub version: &'static str,
    pub release_date: &'static str,
    /// Long OS name and version, e.g. `Linux (Fedora Linux 45)`.
    pub os: String,
    pub arch: &'static str,
    /// `linux`, `macos` or `windows`.
    pub platform: &'static str,
    pub desktop: bool,
    pub uptime_secs: u64,
    /// Where issues are reported.
    pub repository: &'static str,
    pub stats: StatusStats,
    /// Settings summary without secrets.
    pub settings: Value,
    pub recent_warnings: Vec<RecentWarning>,
    /// Crash report file names in `<data dir>/crashes`, newest first.
    pub crash_reports: Vec<String>,
    /// This process's log file (`~` for the home directory), if it writes one.
    pub log_file: Option<String>,
}

/// `GET /api/diagnostics`
pub async fn get(State(app): State<Arc<App>>) -> ApiResult<Json<Diagnostics>> {
    // Reads the OS release and lists the crash directory: off the async workers.
    let out = tokio::task::spawn_blocking(move || collect(&app))
        .await
        .map_err(|e| crate::error::ApiError::internal(format!("diagnostics task failed: {e}")))?;
    Ok(Json(out))
}

fn collect(app: &App) -> Diagnostics {
    let settings = app.settings();
    let proxy_port = app.proxy().local_addr().port();
    let uptime_ms = (crate::app::now_ms() - app.started_at).max(0.0);
    Diagnostics {
        version: akev_core::VERSION,
        release_date: akev_core::RELEASE_DATE,
        os: sysinfo::System::long_os_version().unwrap_or_else(|| std::env::consts::OS.into()),
        arch: std::env::consts::ARCH,
        platform: crate::app::platform(),
        desktop: app.desktop,
        uptime_secs: (uptime_ms / 1000.0) as u64,
        repository: env!("CARGO_PKG_REPOSITORY"),
        stats: app.stats(),
        settings: settings_summary(&settings, proxy_port),
        recent_warnings: observability::recent_warnings(),
        crash_reports: observability::crash_reports(&app.data_dir.join(observability::CRASH_DIR)),
        log_file: observability::log_file_path()
            .map(|p| observability::tilde_home(&p.display().to_string())),
    }
}

/// Bind addresses that say nothing about the user's network.
fn describe_bind(bind: &str) -> &str {
    match bind.trim() {
        b @ ("127.0.0.1" | "::1" | "[::1]" | "localhost" | "0.0.0.0" | "::" | "[::]") => b,
        _ => "other",
    }
}

fn kebab<T: Serialize>(value: &T) -> Value {
    serde_json::to_value(value).unwrap_or(Value::Null)
}

/// The settings that help diagnose problems, without secrets: no token, upstream proxy URL,
/// certificate paths or passphrases, DNS servers, host lists or UI preferences — only their
/// counts and switches.
pub fn settings_summary(s: &Settings, proxy_port: u16) -> Value {
    let p = &s.proxy;
    let c = &s.capture;
    json!({
        "proxy": {
            "port": proxy_port,
            "configuredPort": p.port,
            "bindAddress": describe_bind(&p.bind_address),
            "http2": p.http2,
            "socks": p.socks,
            "tlsPassthroughHosts": p.tls_passthrough.len(),
            "ignoreUpstreamCertErrorHosts": p.ignore_upstream_cert_errors.len(),
            "trustedCas": p.trusted_ca_paths.len(),
            "clientCertificates": p.client_certificates.len(),
            "tlsProfile": p.tls_profile.profile.as_str(),
            "tlsProfileOverrides": p.tls_profile.overrides.len(),
            "upstreamProxy": kebab(&p.upstream_proxy.mode),
            "upstreamProxyBypassHosts": p.upstream_proxy.no_proxy.len(),
            "dnsOverrides": p.dns_overrides.len(),
            "dnsOverHttps": p.doh_url.as_deref().is_some_and(|u| !u.trim().is_empty()),
            "connectTimeoutMs": p.connect_timeout_ms,
            "responseTimeoutMs": p.response_timeout_ms,
            "bodyIdleTimeoutMs": p.body_idle_timeout_ms,
            "ruleBodyBudgetBytes": p.rule_body_budget_bytes,
            "preserveHeaderCase": p.preserve_header_case,
            "wireguard": p.wireguard.enabled,
            "transparent": p.transparent.enabled,
            "http3Upstream": kebab(&p.http3.upstream),
            "http3Listen": p.http3.listen,
            "clientPolicy": kebab(&p.access.policy),
            "allowedClients": p.access.allow.len(),
            "blockedClients": p.access.block.len(),
            "proxyCredentials": p.access.credentials().is_some(),
            "allowLoopbackTargets": p.access.allow_loopback_targets,
            "trustProxyProtocol": p.access.trust_proxy_protocol,
        },
        "capture": {
            "maxBodyBytes": c.max_body_bytes,
            "memoryBudgetBytes": c.memory_budget_bytes,
            "maxExchanges": c.max_exchanges,
            "spillMaxBytes": c.spill_max_bytes,
            "eventQueueBytes": c.event_queue_bytes,
            "recordTunnels": c.record_tunnels,
            "recordTlsFailures": c.record_tls_failures,
            "ignoredHosts": c.ignore_hosts.len(),
            "detectProcesses": c.detect_processes,
        },
        "sessions": {
            "autosave": s.sessions.autosave,
            "autosaveIntervalSecs": s.sessions.autosave_interval_secs,
            "restoreOnStartup": s.sessions.restore_on_startup,
            "autosaveMaxExchanges": s.sessions.autosave_max_exchanges,
            "autosaveMaxBytes": s.sessions.autosave_max_bytes,
        },
        "breakpoints": {
            "maxPaused": s.breakpoints.max_paused,
            "autoReleaseSecs": s.breakpoints.auto_release_secs,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::settings::{ClientCertificate, DnsOverride, UpstreamProxyMode};

    #[test]
    fn settings_summary_has_no_secrets() {
        let mut s = Settings::default();
        s.proxy.bind_address = "192.168.1.20".into();
        s.proxy.upstream_proxy.mode = UpstreamProxyMode::Manual;
        s.proxy.upstream_proxy.url = "http://user:hunter2@proxy.corp:3128".into();
        s.proxy.client_certificates.push(ClientCertificate {
            host: "*.corp".into(),
            path: "/home/alice/secret/client.p12".into(),
            passphrase: Some("p4ssphrase".into()),
        });
        s.proxy.dns_overrides.push(DnsOverride {
            host: "api.corp".into(),
            address: "10.0.0.5".into(),
        });
        s.proxy.doh_url = Some("https://user:pw@doh.example/dns-query".into());
        s.capture.ignore_hosts.push("tracker.example".into());
        s.proxy.access.username = "phone".into();
        s.proxy.access.password = "pr0xy-pw".into();
        s.proxy.access.block.push("192.168.1.99".into());
        s.ui = json!({"theme": "dark", "secretish": "value-from-ui"});

        let summary = settings_summary(&s, 8001);
        let text = summary.to_string();
        for secret in [
            "hunter2",
            "proxy.corp",
            "client.p12",
            "p4ssphrase",
            "10.0.0.5",
            "doh.example",
            "tracker.example",
            "192.168.1.20",
            "value-from-ui",
            "dark",
            "pr0xy-pw",
            "192.168.1.99",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        assert_eq!(summary["proxy"]["port"], 8001);
        assert_eq!(summary["proxy"]["bindAddress"], "other");
        assert_eq!(summary["proxy"]["upstreamProxy"], "manual");
        assert_eq!(summary["proxy"]["clientCertificates"], 1);
        assert_eq!(summary["proxy"]["dnsOverHttps"], true);
        assert_eq!(summary["proxy"]["proxyCredentials"], true);
        assert_eq!(summary["proxy"]["blockedClients"], 1);
        assert_eq!(summary["proxy"]["clientPolicy"], "private");
        assert_eq!(summary["capture"]["ignoredHosts"], 1);
        assert_eq!(summary["capture"]["maxExchanges"], s.capture.max_exchanges);
        assert_eq!(summary["sessions"]["autosave"], s.sessions.autosave);
        assert_eq!(
            summary["breakpoints"]["maxPaused"],
            s.breakpoints.max_paused
        );
        assert!(summary.get("ui").is_none());
    }

    #[test]
    fn loopback_and_wildcard_binds_are_shown() {
        assert_eq!(describe_bind("127.0.0.1"), "127.0.0.1");
        assert_eq!(describe_bind("0.0.0.0"), "0.0.0.0");
        assert_eq!(describe_bind("::"), "::");
        assert_eq!(describe_bind("10.1.2.3"), "other");
    }
}
