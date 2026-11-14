//! Akev Pro: the features a licence (or the trial) unlocks and the licence status the API
//! reports (`GET /api/license`, `Status.license`).
//!
//! Licence keys are verified offline in `akev-api`; nothing here talks to a server. Licence
//! state never deletes or rewrites user data: without Pro, the features below stop running
//! (script rules are skipped, the WireGuard server is stopped, …) while their configuration
//! stays saved.

use serde::{Deserialize, Serialize};

/// Length of the Pro trial.
pub const TRIAL_DAYS: u32 = 14;

/// A feature that needs Akev Pro.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Feature {
    /// `script` rule actions (JavaScript).
    Scripting,
    /// Browser TLS fingerprints upstream (`proxy.tlsProfile`, `tls-profile` rule actions).
    TlsImpersonation,
    /// WireGuard mode (whole-device capture).
    Wireguard,
    /// Transparent capture (Linux).
    Transparent,
    /// Attaching to running JVMs (the `jvm` interceptor).
    JvmAttach,
    /// Imported protobuf schemas and URL mappings (schema-less decoding stays free).
    ProtoSchemas,
    /// The LLM conversation view of an exchange.
    LlmLens,
    /// MCP tools that change state (send, replay, rules, breakpoints, recording, …).
    McpWrite,
}

impl Feature {
    /// Every Pro feature.
    pub const ALL: [Feature; 8] = [
        Feature::Scripting,
        Feature::TlsImpersonation,
        Feature::Wireguard,
        Feature::Transparent,
        Feature::JvmAttach,
        Feature::ProtoSchemas,
        Feature::LlmLens,
        Feature::McpWrite,
    ];

    /// The wire id (`scripting`, `tls-impersonation`, …).
    pub fn id(self) -> &'static str {
        match self {
            Feature::Scripting => "scripting",
            Feature::TlsImpersonation => "tls-impersonation",
            Feature::Wireguard => "wireguard",
            Feature::Transparent => "transparent",
            Feature::JvmAttach => "jvm-attach",
            Feature::ProtoSchemas => "proto-schemas",
            Feature::LlmLens => "llm-lens",
            Feature::McpWrite => "mcp-write",
        }
    }

    /// Human-readable name, used in error messages.
    pub fn label(self) -> &'static str {
        match self {
            Feature::Scripting => "Script rules",
            Feature::TlsImpersonation => "TLS fingerprint impersonation",
            Feature::Wireguard => "WireGuard mode",
            Feature::Transparent => "Transparent capture",
            Feature::JvmAttach => "JVM attach",
            Feature::ProtoSchemas => "Protobuf schemas",
            Feature::LlmLens => "The LLM lens",
            Feature::McpWrite => "MCP tools that change state",
        }
    }
}

/// `free` or `pro`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Plan {
    Free,
    Pro,
}

/// Where the licence stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LicenseState {
    /// No licence key, trial not started.
    None,
    /// The Pro trial is running.
    Trial,
    /// The trial is over and there is no licence key.
    TrialEnded,
    /// A valid licence key covers this build.
    Active,
    /// The key is genuine, but this build was released after its year of updates ended.
    UpdatesEnded,
    /// The stored key can't be used (see `error`).
    Invalid,
}

/// The licensee and terms of a verified licence key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseInfo {
    /// Licence id (the purchase it came from).
    pub id: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Issue date, `YYYY-MM-DD`.
    pub issued: String,
    /// Builds released up to this date (`YYYY-MM-DD`) are covered.
    pub updates_until: String,
    /// How many people a team licence is for (absent: one person).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<u32>,
}

impl LicenseInfo {
    /// Who the licence is for, in one line: the name, or the email when there is none.
    pub fn licensee(&self) -> &str {
        self.name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or(&self.email)
    }
}

/// `GET /api/license`, `Status.license`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LicenseStatus {
    pub plan: Plan,
    pub state: LicenseState,
    /// The features this instance may use right now.
    pub features: Vec<Feature>,
    /// The verified licence (also for `updates-ended`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<LicenseInfo>,
    /// End of the trial (unix ms), once it was started.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_ends_at: Option<f64>,
    /// The trial can be started (never started, and no licence covers this build).
    pub trial_available: bool,
    /// Why the stored key can't be used (`invalid`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// This build's release date (`YYYY-MM-DD`), compared with `updatesUntil`.
    pub release_date: String,
}

impl LicenseStatus {
    pub fn has(&self, feature: Feature) -> bool {
        self.features.contains(&feature)
    }
}

/// Whether `s` is a `YYYY-MM-DD` date (digits in place; month 01-12, day 01-31).
pub fn is_iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let digits = |r: std::ops::Range<usize>| b[r].iter().all(u8::is_ascii_digit);
    if !(digits(0..4) && digits(5..7) && digits(8..10)) {
        return false;
    }
    let month = (b[5] - b'0') * 10 + (b[6] - b'0');
    let day = (b[8] - b'0') * 10 + (b[9] - b'0');
    (1..=12).contains(&month) && (1..=31).contains(&day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feature_ids_match_serde() {
        for f in Feature::ALL {
            assert_eq!(serde_json::to_value(f).unwrap(), f.id());
        }
    }

    #[test]
    fn iso_dates() {
        assert!(is_iso_date("2026-11-11"));
        assert!(is_iso_date("2028-02-29"));
        assert!(!is_iso_date("2026-13-01"));
        assert!(!is_iso_date("2026-1-01"));
        assert!(!is_iso_date("2026-11-11T00"));
        assert!(!is_iso_date("2026/11/11"));
    }

    #[test]
    fn status_shape() {
        let s = LicenseStatus {
            plan: Plan::Free,
            state: LicenseState::UpdatesEnded,
            features: vec![],
            license: None,
            trial_ends_at: None,
            trial_available: true,
            error: None,
            release_date: "2026-11-11".into(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["state"], "updates-ended");
        assert_eq!(v["trialAvailable"], true);
        assert_eq!(v["releaseDate"], "2026-11-11");
        assert!(v.get("license").is_none());
    }

    #[test]
    fn licensee_is_the_name_or_the_email() {
        let mut l = LicenseInfo {
            id: "AKV-1".into(),
            email: "ada@example.com".into(),
            name: Some("Ada Lovelace".into()),
            issued: "2026-11-05".into(),
            updates_until: "2027-11-05".into(),
            seats: None,
        };
        assert_eq!(l.licensee(), "Ada Lovelace");
        l.name = Some("  ".into());
        assert_eq!(l.licensee(), "ada@example.com");
        l.name = None;
        assert_eq!(l.licensee(), "ada@example.com");
    }
}
