//! `GET /api/tls-profiles` — the upstream TLS fingerprint profiles and whether this build can
//! use them (browser profiles need the `impersonate` cargo feature).

use crate::app::App;
use akev_proxy::upstream::fingerprint::{self, ProfileInfo};
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use std::sync::Arc;

/// Response of `GET /api/tls-profiles`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsProfiles {
    /// Whether browser profiles work in this build.
    pub available: bool,
    /// The TLS library presenting browser fingerprints (`BoringSSL`), when compiled in.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend: Option<&'static str>,
    /// Every profile, in display order.
    pub profiles: Vec<ProfileInfo>,
    /// Whether this instance may use browser profiles (Akev Pro).
    pub licensed: bool,
}

/// The profile catalogue of this build.
pub fn catalog() -> TlsProfiles {
    TlsProfiles {
        available: fingerprint::available(),
        backend: fingerprint::backend(),
        profiles: fingerprint::catalog(),
        licensed: true,
    }
}

/// `GET /api/tls-profiles`
pub async fn list(State(app): State<Arc<App>>) -> Json<TlsProfiles> {
    Json(TlsProfiles {
        licensed: app
            .license
            .has(akev_core::license::Feature::TlsImpersonation),
        ..catalog()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalogue_shape() {
        let json = serde_json::to_value(catalog()).unwrap();
        // Impersonation is compiled into akev-proxy, whose feature can be enabled without
        // this crate's; this crate's feature always implies it.
        let available = fingerprint::available();
        if cfg!(feature = "impersonate") {
            assert!(available);
        }
        assert_eq!(json["available"], available);
        assert_eq!(json["backend"].is_string(), available);
        let profiles = json["profiles"].as_array().unwrap();
        assert_eq!(profiles.len(), 6);
        assert_eq!(profiles[0]["id"], "native");
        assert_eq!(profiles[0]["available"], true);
        assert_eq!(profiles[1]["id"], "chrome");
        assert_eq!(profiles[1]["ja4"], "t13d1516h2_8daaf6152771_d8a2da3f94cd");
        assert!(
            profiles[1]["http2"]
                .as_str()
                .unwrap()
                .contains("|15663105|")
        );
        assert_eq!(profiles[5]["id"], "mirror-client");
    }
}
