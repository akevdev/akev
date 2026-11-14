//! Transparent capture: settings of the dedicated transparent listener, its live status and
//! the probe interface that lets the interceptors and the API read that status.
//!
//! The proxy owns the listener (`akev-proxy`'s `transparent` module): connections the kernel
//! redirected to it (nftables / iptables `REDIRECT`) are handled like a `CONNECT` tunnel to
//! their original destination. Akev itself never changes firewall rules; the
//! `transparent-linux` interceptor only generates the commands.

use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Default port of the transparent listener.
pub const DEFAULT_TRANSPARENT_PORT: u16 = 8001;

/// How long after the last redirected connection redirection still counts as active.
pub const REDIRECT_ACTIVE_WINDOW_MS: f64 = 10.0 * 60.0 * 1000.0;

/// `settings.proxy.transparent`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TransparentSettings {
    /// Run the transparent listener (off by default).
    pub enabled: bool,
    /// Port of the transparent listener; firewall redirect rules point here. When busy, the
    /// next free port (up to +100) is used and reported in the status. 0 = OS-assigned.
    pub port: u16,
    /// Firewall mark (`SO_MARK`) set on Akev's own upstream sockets so redirect rules can
    /// exclude them (`meta mark != MARK`). 0 = don't mark. Linux only; needs `CAP_NET_ADMIN`.
    pub mark: u32,
}

impl Default for TransparentSettings {
    fn default() -> Self {
        TransparentSettings {
            enabled: false,
            port: DEFAULT_TRANSPARENT_PORT,
            mark: 0,
        }
    }
}

/// One connection accepted on the transparent listener (newest first in
/// [`TransparentStatus::recent`]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransparentConnection {
    /// When it was accepted (unix ms).
    pub at: f64,
    /// Client address (`ip:port`).
    pub client: String,
    /// Where the client was really connecting to (`ip:port`), when the kernel redirected it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_destination: Option<String>,
    /// Local process that opened the connection, when detected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process: Option<String>,
}

/// Live status of the transparent listener (`GET /api/transparent` → `listener`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransparentStatus {
    /// `settings.proxy.transparent.enabled`.
    pub enabled: bool,
    /// The original destination of redirected connections can be read on this platform
    /// (Linux: `SO_ORIGINAL_DST` / `IP6T_SO_ORIGINAL_DST`).
    pub supported: bool,
    /// At least one listening socket is open.
    pub listening: bool,
    /// Port in use (`None` while not listening).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    /// Port asked for in the settings (differs from `port` after a busy-port fallback).
    pub requested_port: u16,
    /// Listening socket addresses.
    pub addresses: Vec<String>,
    /// Why the listener isn't running (bind failure...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Firewall mark set on upstream sockets (0 = none).
    pub mark: u32,
    /// Why upstream sockets can't be marked (missing `CAP_NET_ADMIN`, not Linux...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mark_error: Option<String>,
    /// Connections accepted since the listener started.
    pub connections: u64,
    /// ...of which the kernel had redirected (an original destination was found).
    pub redirected: u64,
    /// ...of which were made to the port directly (handled by `Host` / SNI like the main port).
    pub direct: u64,
    /// Connections refused because they came from Akev itself (the redirect rules don't
    /// exclude Akev's own traffic).
    pub loops: u64,
    /// Last redirected connection (unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_redirect_at: Option<f64>,
    /// The most recent connections, newest first.
    pub recent: Vec<TransparentConnection>,
}

impl TransparentStatus {
    /// Whether a redirected connection arrived within [`REDIRECT_ACTIVE_WINDOW_MS`] of `now_ms`.
    pub fn redirect_active(&self, now_ms: f64) -> bool {
        self.listening
            && self
                .last_redirect_at
                .is_some_and(|at| now_ms - at <= REDIRECT_ACTIVE_WINDOW_MS)
    }
}

/// Something that knows the live transparent listener status (the running proxy).
pub trait TransparentProbe: Send + Sync + 'static {
    /// Current status.
    fn transparent_status(&self) -> TransparentStatus;
}

/// A cloneable, `Debug` handle on a [`TransparentProbe`] (for contexts that derive `Debug`).
#[derive(Clone)]
pub struct TransparentProbeHandle(pub Arc<dyn TransparentProbe>);

impl TransparentProbeHandle {
    /// Wrap a probe.
    pub fn new(probe: impl TransparentProbe) -> Self {
        TransparentProbeHandle(Arc::new(probe))
    }

    /// Current status.
    pub fn status(&self) -> TransparentStatus {
        self.0.transparent_status()
    }
}

impl std::fmt::Debug for TransparentProbeHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TransparentProbeHandle")
    }
}

pub use crate::time::now_ms;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_defaults_and_partial_json() {
        let d = TransparentSettings::default();
        assert!(!d.enabled);
        assert_eq!(d.port, DEFAULT_TRANSPARENT_PORT);
        assert_eq!(d.mark, 0);
        let s: TransparentSettings = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
        assert_eq!(
            s,
            TransparentSettings {
                enabled: true,
                ..Default::default()
            }
        );
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"enabled": true, "port": 8001, "mark": 0})
        );
    }

    #[test]
    fn redirect_activity_window() {
        let mut s = TransparentStatus {
            listening: true,
            ..Default::default()
        };
        assert!(!s.redirect_active(1e12));
        s.last_redirect_at = Some(1e12);
        assert!(s.redirect_active(1e12 + 1000.0));
        assert!(!s.redirect_active(1e12 + REDIRECT_ACTIVE_WINDOW_MS + 1.0));
        s.listening = false;
        assert!(!s.redirect_active(1e12 + 1000.0));
    }

    #[test]
    fn status_serializes_camel_case_without_empty_options() {
        let s = TransparentStatus {
            enabled: true,
            supported: true,
            requested_port: 8001,
            recent: vec![TransparentConnection {
                at: 1.0,
                client: "127.0.0.1:5000".into(),
                original_destination: Some("93.184.216.34:443".into()),
                process: None,
            }],
            ..Default::default()
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["requestedPort"], 8001);
        assert!(v.get("port").is_none() && v.get("error").is_none());
        assert_eq!(v["recent"][0]["originalDestination"], "93.184.216.34:443");
        assert!(v["recent"][0].get("process").is_none());
        let back: TransparentStatus = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }

    struct Fixed;
    impl TransparentProbe for Fixed {
        fn transparent_status(&self) -> TransparentStatus {
            TransparentStatus {
                redirected: 3,
                ..Default::default()
            }
        }
    }

    #[test]
    fn probe_handle() {
        let h = TransparentProbeHandle::new(Fixed);
        assert_eq!(h.clone().status().redirected, 3);
        assert_eq!(format!("{h:?}"), "TransparentProbeHandle");
        assert!(now_ms() > 1.6e12);
    }
}
