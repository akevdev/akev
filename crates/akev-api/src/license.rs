//! Akev Pro licences: offline verification of licence keys, the 14-day trial, and the
//! entitlement checks the routes and the MCP tools use.
//!
//! ## Key format
//!
//! `AKEV1.<payload>.<signature>`: `payload` is the base64url (unpadded) UTF-8 JSON
//! `{"v":1,"id","email","name"?,"plan":"pro","issued":"YYYY-MM-DD","updatesUntil":"YYYY-MM-DD"}`
//! and `signature` the base64url Ed25519 signature over the ASCII text `AKEV1.<payload>`. Keys
//! are issued by the website's licensing service (`akev-website/licensing`).
//!
//! A key never expires at run time: it covers every build released (see
//! [`akev_core::RELEASE_DATE`]) up to `updatesUntil`. Nothing is sent anywhere.
//!
//! An optional `"seats": N` (team purchases) is shown to the user and not enforced.
//!
//! ## Revocation
//!
//! Refunded and charged-back licences are listed by id in `revoked.txt`, which the release
//! script regenerates from Stripe and which is compiled in. Like everything else here, it
//! works offline: a revoked key keeps working in the builds released before it was revoked.
//!
//! ## State
//!
//! `license.json` in the data directory (0600): `{key, trialStartedAt}`. Not part of
//! `settings.json`, so settings exports never contain the key. `--license` / `AKEV_LICENSE`
//! supply a key for one run without storing it (ephemeral runs, CI, containers).
//!
//! The trial is once per computer: the CLI and the desktop app record its start in a per-user
//! file ([`default_trial_file`]) that every data directory shares, so a new `--data-dir`
//! doesn't start it over. Ephemeral sessions can use a running trial but never start one.

use std::path::{Path, PathBuf};

use akev_core::license::{
    Feature, LicenseInfo, LicenseState, LicenseStatus, Plan, TRIAL_DAYS, is_iso_date,
};
use akev_rules::LockedActions;
use base64::Engine;
use base64::alphabet::URL_SAFE;
use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
use http::StatusCode;
use parking_lot::RwLock;
use ring::signature::{ED25519, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::error::ApiError;
use crate::persist;

/// File in the data directory holding the licence key and the trial start.
pub const LICENSE_FILE: &str = "license.json";

/// Prefix (and signature domain) of licence keys.
const KEY_PREFIX: &str = "AKEV1.";

/// Ed25519 public keys (base64url, 32 bytes) whose signatures Akev accepts.
///
/// Generate the production key pair with `bun licensing/cli.ts keygen` in the website
/// repository, keep the seed secret (licensing service only) and paste the public key here.
/// Development builds can add one with the build-time environment variable
/// `AKEV_LICENSE_PUBLIC_KEY` (comma-separated).
///
/// Production key (since 2026-11-14). Keys are only ever added: a key leaving this list would
/// lock out every licence it signed. The `canary_key_verifies_in_release_builds` test fails the
/// release if this list can't verify a key signed by the production signer.
const PUBLIC_KEYS: &[&str] = &["m375AjbQpWNMBFg2Jqyoyj3KG2TJj2TsFJN3EzDR9pQ"];

/// Licence ids revoked after a refund or a chargeback (one per line, `#` comments).
const REVOKED: &str = include_str!("revoked.txt");

/// Where to write about a licence problem.
pub const SUPPORT_EMAIL: &str = "support@akev.dev";

/// File (in the per-user config directory) recording when the trial started on this computer.
pub const TRIAL_FILE: &str = "trial.json";

const TRIAL_MS: f64 = TRIAL_DAYS as f64 * 24.0 * 60.0 * 60.0 * 1000.0;

/// Where to buy Pro (shown in error messages).
pub const PRICING_URL: &str = "https://akev.dev/pricing/";

/// base64url that accepts keys with or without `=` padding.
const B64: GeneralPurpose = GeneralPurpose::new(
    &URL_SAFE,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

/// The public keys built into this binary.
pub fn builtin_public_keys() -> Vec<String> {
    PUBLIC_KEYS
        .iter()
        .copied()
        .chain(
            option_env!("AKEV_LICENSE_PUBLIC_KEY")
                .into_iter()
                .flat_map(|s| s.split(',')),
        )
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .collect()
}

/// The licence ids revoked in this build.
pub fn builtin_revoked() -> Vec<String> {
    REVOKED
        .lines()
        .map(|l| l.split('#').next().unwrap_or_default().trim())
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// The per-user trial file: `<config dir>/akev/trial.json` (shared by all data directories).
pub fn default_trial_file() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("akev").join(TRIAL_FILE))
}

/// Decode base64url public keys, skipping (and logging) malformed ones.
fn decode_public_keys(keys: &[String]) -> Vec<[u8; 32]> {
    keys.iter()
        .filter_map(|k| {
            match B64
                .decode(k.trim())
                .ok()
                .and_then(|b| <[u8; 32]>::try_from(b).ok())
            {
                Some(key) => Some(key),
                None => {
                    tracing::warn!("ignoring malformed license public key {k:?}");
                    None
                }
            }
        })
        .collect()
}

/// The signed payload of a licence key.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Payload {
    v: u32,
    id: String,
    email: String,
    #[serde(default)]
    name: Option<String>,
    plan: String,
    issued: String,
    updates_until: String,
    /// People the licence is for (team purchases); absent means one.
    #[serde(default)]
    seats: Option<u32>,
}

/// Remove the whitespace pasting tends to add (line breaks in emails, trailing newlines).
pub fn normalize_key(key: &str) -> String {
    key.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Verify a licence key against `keys` (raw Ed25519 public keys) and the ids in `revoked`.
/// The error is a sentence for the user.
pub fn verify(key: &str, keys: &[[u8; 32]], revoked: &[String]) -> Result<LicenseInfo, String> {
    let key = normalize_key(key);
    if key.is_empty() {
        return Err("the license key is empty".into());
    }
    let Some((signed, signature)) = key.rsplit_once('.') else {
        return Err("this is not an Akev license key (it should start with AKEV1.)".into());
    };
    let Some(payload) = signed.strip_prefix(KEY_PREFIX) else {
        return Err("this is not an Akev license key (it should start with AKEV1.)".into());
    };
    let damaged = || "the license key is damaged; copy it again from the email".to_string();
    let signature = B64.decode(signature).map_err(|_| damaged())?;
    let payload = B64.decode(payload).map_err(|_| damaged())?;
    if keys.is_empty() {
        return Err(
            "this build of Akev can't verify license keys (it has no license public key)".into(),
        );
    }
    let genuine = keys.iter().any(|public| {
        UnparsedPublicKey::new(&ED25519, public)
            .verify(signed.as_bytes(), &signature)
            .is_ok()
    });
    if !genuine {
        return Err(
            "the license key's signature is not valid; copy the whole key again from the email"
                .into(),
        );
    }
    let p: Payload = serde_json::from_slice(&payload).map_err(|_| {
        "the license key's contents can't be read; this version of Akev may be too old for it"
            .to_string()
    })?;
    if p.v != 1 {
        return Err(format!(
            "the license key has format version {}; update Akev to use it",
            p.v
        ));
    }
    if p.plan != "pro" {
        return Err(format!("unknown plan {:?} in the license key", p.plan));
    }
    if !is_iso_date(&p.issued) || !is_iso_date(&p.updates_until) {
        return Err("the license key has invalid dates".into());
    }
    if p.seats == Some(0) {
        return Err("the license key has no seats".into());
    }
    if revoked.contains(&p.id) {
        return Err(format!(
            "this license key was revoked after a refund or a chargeback; if that's a mistake, write to {SUPPORT_EMAIL}"
        ));
    }
    Ok(LicenseInfo {
        id: p.id,
        email: p.email,
        name: p.name.filter(|n| !n.trim().is_empty()),
        issued: p.issued,
        updates_until: p.updates_until,
        seats: p.seats.filter(|&n| n > 1),
    })
}

/// `license.json`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct LicenseFile {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<String>,
    /// When the trial was started (unix ms).
    #[serde(skip_serializing_if = "Option::is_none")]
    trial_started_at: Option<f64>,
}

#[derive(Debug)]
struct Inner {
    /// What `license.json` holds.
    stored: LicenseFile,
    /// `--license` / `AKEV_LICENSE` for this run (takes precedence; never stored).
    run_key: Option<String>,
    /// Verification result of the key in effect.
    verified: Option<Result<LicenseInfo, String>>,
}

impl Inner {
    fn key(&self) -> Option<&str> {
        self.run_key.as_deref().or(self.stored.key.as_deref())
    }
}

/// `trial.json` in the per-user config directory.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct TrialFile {
    /// When the trial was started on this computer (unix ms).
    started_at: Option<f64>,
}

/// The licence state of a running instance.
#[derive(Debug)]
pub struct LicenseManager {
    keys: Vec<[u8; 32]>,
    revoked: Vec<String>,
    release_date: String,
    /// `license.json` (None for ephemeral sessions: nothing is stored).
    file: Option<PathBuf>,
    /// The per-user trial file (None: the trial is recorded in `license.json` only).
    trial_file: Option<PathBuf>,
    inner: RwLock<Inner>,
}

impl LicenseManager {
    /// Load `license.json` from `data_dir` when `persist`, and use `run_key` (when given) for
    /// this run. `public_keys` are base64url Ed25519 keys (normally
    /// [`builtin_public_keys`]). `trial_file` is the per-user trial record (normally
    /// [`default_trial_file`]); without one the trial lives in `license.json`.
    pub fn load(
        data_dir: &Path,
        persist: bool,
        run_key: Option<String>,
        public_keys: &[String],
        trial_file: Option<PathBuf>,
    ) -> LicenseManager {
        let file = persist.then(|| data_dir.join(LICENSE_FILE));
        let mut stored = match &file {
            Some(path) => persist::load_json_or_default::<LicenseFile>(path).0,
            None => LicenseFile::default(),
        };
        // The earliest start wins: a trial started on this computer covers every data dir.
        if let Some(path) = &trial_file {
            let shared = persist::load_json_or_default::<TrialFile>(path).0;
            stored.trial_started_at = match (stored.trial_started_at, shared.started_at) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            };
        }
        let manager = LicenseManager {
            keys: decode_public_keys(public_keys),
            revoked: builtin_revoked(),
            release_date: akev_core::RELEASE_DATE.to_string(),
            file,
            trial_file,
            inner: RwLock::new(Inner {
                stored,
                run_key: run_key.map(|k| normalize_key(&k)).filter(|k| !k.is_empty()),
                verified: None,
            }),
        };
        manager.reverify();
        if let Some(Err(e)) = &manager.inner.read().verified {
            tracing::warn!("license key not accepted: {e}");
        }
        manager
    }

    #[cfg(test)]
    fn with_release_date(mut self, date: &str) -> Self {
        self.release_date = date.to_string();
        self
    }

    #[cfg(test)]
    fn with_revoked(mut self, ids: &[&str]) -> Self {
        self.revoked = ids.iter().map(|s| s.to_string()).collect();
        self.reverify();
        self
    }

    fn reverify(&self) {
        let mut inner = self.inner.write();
        inner.verified = inner.key().map(|k| verify(k, &self.keys, &self.revoked));
    }

    /// Whether this instance may start the trial: only a persistent session (not an ephemeral
    /// one, as CI runs and throwaway containers are) may.
    fn can_record_trial(&self) -> bool {
        self.file.is_some()
    }

    /// The status now.
    pub fn status(&self) -> LicenseStatus {
        self.status_at(now_ms())
    }

    fn status_at(&self, now: f64) -> LicenseStatus {
        let inner = self.inner.read();
        let trial_ends_at = inner.stored.trial_started_at.map(|t| t + TRIAL_MS);
        let trial_running = trial_ends_at.is_some_and(|end| now < end);
        let (license, error) = match &inner.verified {
            Some(Ok(info)) => (Some(info.clone()), None),
            Some(Err(e)) => (None, Some(e.clone())),
            None => (None, None),
        };
        let covers = license
            .as_ref()
            .is_some_and(|l| self.release_date.as_str() <= l.updates_until.as_str());
        let state = if covers {
            LicenseState::Active
        } else if trial_running {
            LicenseState::Trial
        } else if license.is_some() {
            LicenseState::UpdatesEnded
        } else if error.is_some() {
            LicenseState::Invalid
        } else if trial_ends_at.is_some() {
            LicenseState::TrialEnded
        } else {
            LicenseState::None
        };
        let pro = matches!(state, LicenseState::Active | LicenseState::Trial);
        LicenseStatus {
            plan: if pro { Plan::Pro } else { Plan::Free },
            state,
            features: if pro {
                Feature::ALL.to_vec()
            } else {
                Vec::new()
            },
            license,
            trial_ends_at,
            trial_available: trial_ends_at.is_none() && !covers && self.can_record_trial(),
            error,
            release_date: self.release_date.clone(),
        }
    }

    /// Whether `feature` may be used now.
    pub fn has(&self, feature: Feature) -> bool {
        self.status().has(feature)
    }

    /// `Ok` when `feature` may be used, else a 402 `pro-required` error naming it.
    pub fn require(&self, feature: Feature) -> Result<(), ApiError> {
        if self.has(feature) {
            Ok(())
        } else {
            Err(pro_required(feature))
        }
    }

    /// End of a running trial (unix ms).
    pub fn trial_running_until(&self) -> Option<f64> {
        let status = self.status();
        (status.state == LicenseState::Trial)
            .then_some(status.trial_ends_at)
            .flatten()
    }

    /// Store `key` (after verifying it) in place of the current one. A genuine key whose
    /// updates ended before this build is stored too (it reports `updates-ended`).
    /// Writes `license.json`: call off the async workers.
    pub fn set_key(&self, key: &str) -> Result<LicenseStatus, ApiError> {
        let key = normalize_key(key);
        verify(&key, &self.keys, &self.revoked).map_err(|e| {
            ApiError::new(StatusCode::BAD_REQUEST, "invalid-license", capitalize(&e))
        })?;
        self.update(|inner| {
            inner.run_key = None;
            inner.stored.key = Some(key);
        })?;
        Ok(self.status())
    }

    /// Remove the licence key (stored and `--license`). Writes `license.json`.
    pub fn remove_key(&self) -> Result<LicenseStatus, ApiError> {
        self.update(|inner| {
            inner.run_key = None;
            inner.stored.key = None;
        })?;
        Ok(self.status())
    }

    /// Start the trial (once per computer). Writes the trial file and `license.json`.
    pub fn start_trial(&self) -> Result<LicenseStatus, ApiError> {
        if !self.can_record_trial() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "trial-unavailable",
                "The trial can't be started in an ephemeral session; start it once from a normal Akev run.",
            ));
        }
        if !self.status().trial_available {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "trial-unavailable",
                "The trial was already used on this computer, or a license already covers this version.",
            ));
        }
        let now = now_ms();
        if let Some(path) = &self.trial_file {
            if let Some(dir) = path.parent() {
                persist::ensure_dir(dir)
                    .map_err(|e| ApiError::internal(format!("could not record the trial: {e}")))?;
            }
            let record = TrialFile {
                started_at: Some(now),
            };
            persist::write_json_atomic(path, &record, 0o600).map_err(|e| {
                tracing::warn!("failed to write {}: {e}", path.display());
                ApiError::internal(format!("could not record the trial: {e}"))
            })?;
        }
        self.update(|inner| inner.stored.trial_started_at = Some(now))?;
        Ok(self.status())
    }

    fn update(&self, edit: impl FnOnce(&mut Inner)) -> Result<(), ApiError> {
        let mut inner = self.inner.write();
        let mut next = Inner {
            stored: inner.stored.clone(),
            run_key: inner.run_key.clone(),
            verified: None,
        };
        edit(&mut next);
        if next.stored != inner.stored
            && let Some(path) = &self.file
        {
            persist::write_json_atomic(path, &next.stored, 0o600).map_err(|e| {
                tracing::warn!("failed to write {}: {e}", path.display());
                ApiError::internal(format!("could not save the license: {e}"))
            })?;
        }
        next.verified = next.key().map(|k| verify(k, &self.keys, &self.revoked));
        *inner = next;
        Ok(())
    }

    /// Rule action types that may not run without Pro, for the rules engine.
    pub fn locked_actions(&self) -> LockedActions {
        let status = self.status();
        let mut locked = LockedActions::new();
        if !status.has(Feature::Scripting) {
            locked.insert(
                "script",
                "Inactive: script rules need Akev Pro. The rule is saved and runs again once Pro is active.".to_string(),
            );
        }
        if !status.has(Feature::TlsImpersonation) {
            locked.insert(
                "tls-profile",
                "Inactive: TLS fingerprint rules need Akev Pro. Matching requests use the native TLS stack until Pro is active.".to_string(),
            );
        }
        locked
    }
}

/// HTTP 402 `pro-required` for `feature` (`{"error": {…, "feature": "<id>"}}`).
pub fn pro_required(feature: Feature) -> ApiError {
    let mut e = ApiError::new(
        StatusCode::PAYMENT_REQUIRED,
        "pro-required",
        pro_required_message(feature),
    );
    e.extra = Some(json!({ "feature": feature.id() }));
    e
}

/// "<Feature> needs Akev Pro. …" — shared by the API and the MCP tools.
pub fn pro_required_message(feature: Feature) -> String {
    format!(
        "{} needs Akev Pro. Start the free {TRIAL_DAYS}-day trial or enter a license key in Settings → License ({PRICING_URL}).",
        feature.label()
    )
}

/// Which Pro features `new` turns on compared with `old` (settings changes that need Pro).
/// Settings that were already on are not reported, so other edits keep working without a
/// licence.
pub fn features_turned_on(
    old: &akev_core::settings::Settings,
    new: &akev_core::settings::Settings,
) -> Vec<Feature> {
    let mut out = Vec::new();
    if new.proxy.wireguard.enabled && !old.proxy.wireguard.enabled {
        out.push(Feature::Wireguard);
    }
    if new.proxy.transparent.enabled && !old.proxy.transparent.enabled {
        out.push(Feature::Transparent);
    }
    if adds_impersonation(&old.proxy.tls_profile, &new.proxy.tls_profile) {
        out.push(Feature::TlsImpersonation);
    }
    out
}

/// Whether `new` assigns a browser profile that `old` didn't have: a different non-native
/// default, or a non-native per-host override that wasn't there (same host and profile).
/// Removing or reordering existing ones, or going back to native, never counts.
fn adds_impersonation(
    old: &akev_core::settings::TlsProfileSettings,
    new: &akev_core::settings::TlsProfileSettings,
) -> bool {
    use akev_core::settings::TlsProfile;
    let new_default = new.profile != TlsProfile::Native && new.profile != old.profile;
    let new_override = new.overrides.iter().any(|o| {
        o.profile != TlsProfile::Native
            && !old
                .overrides
                .iter()
                .any(|p| p.host == o.host && p.profile == o.profile)
    });
    new_default || new_override
}

/// The proxy settings that take effect: Pro features without an entitlement are switched off
/// (the stored settings keep them).
pub fn effective_proxy(
    proxy: &akev_core::settings::ProxySettings,
    status: &LicenseStatus,
) -> akev_core::settings::ProxySettings {
    let mut p = proxy.clone();
    if !status.has(Feature::Wireguard) {
        p.wireguard.enabled = false;
    }
    if !status.has(Feature::Transparent) {
        p.transparent.enabled = false;
    }
    if !status.has(Feature::TlsImpersonation) {
        p.tls_profile = Default::default();
    }
    p
}

/// The Pro feature an interceptor needs, by id.
pub fn interceptor_feature(id: &str) -> Option<Feature> {
    match id {
        "wireguard" => Some(Feature::Wireguard),
        "jvm" => Some(Feature::JvmAttach),
        akev_intercept::transparent::ID => Some(Feature::Transparent),
        _ => None,
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

use akev_core::time::now_ms;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    /// Seed 0x00..0x1f; the same vector is checked by the website's signer tests.
    pub(crate) const TEST_PUBLIC_KEY: &str = "A6EHv_POEL4dcN0Y50vAmWfk1jCbpQ1fHdyGZBJVMbg";
    pub(crate) const TEST_KEY: &str = "AKEV1.eyJ2IjoxLCJpZCI6InRlc3QtMDAwMSIsImVtYWlsIjoiZGV2QGV4YW1wbGUuY29tIiwicGxhbiI6InBybyIsImlzc3VlZCI6IjIwMjYtMTEtMTEiLCJ1cGRhdGVzVW50aWwiOiIyMDI3LTExLTExIn0.hrkZEADnIPSegwrMgdBQ2n-Hs8jsBsFrvIsDF3UmcJdt74GW6-NOg2b83cL6GxLmRODv-Hkb0_UdQQ99LWRJBQ";

    fn test_seed() -> [u8; 32] {
        std::array::from_fn(|i| i as u8)
    }

    /// Sign `payload` (JSON text) with the test seed.
    pub(crate) fn sign(payload: &str) -> String {
        let pair = Ed25519KeyPair::from_seed_unchecked(&test_seed()).unwrap();
        let signed = format!("{KEY_PREFIX}{}", B64_NO_PAD.encode(payload));
        let sig = pair.sign(signed.as_bytes());
        format!("{signed}.{}", B64_NO_PAD.encode(sig.as_ref()))
    }

    const B64_NO_PAD: GeneralPurpose =
        GeneralPurpose::new(&URL_SAFE, base64::engine::general_purpose::NO_PAD);

    fn keys() -> Vec<[u8; 32]> {
        decode_public_keys(&[TEST_PUBLIC_KEY.to_string()])
    }

    fn manager(dir: &Path) -> LicenseManager {
        LicenseManager::load(dir, true, None, &[TEST_PUBLIC_KEY.to_string()], None)
            .with_release_date("2027-01-01")
    }

    #[test]
    fn test_public_key_matches_the_seed() {
        let pair = Ed25519KeyPair::from_seed_unchecked(&test_seed()).unwrap();
        assert_eq!(
            B64_NO_PAD.encode(pair.public_key().as_ref()),
            TEST_PUBLIC_KEY
        );
    }

    #[test]
    fn verifies_the_shared_vector() {
        let info = verify(TEST_KEY, &keys(), &[]).unwrap();
        assert_eq!(info.id, "test-0001");
        assert_eq!(info.email, "dev@example.com");
        assert_eq!(info.name, None);
        assert_eq!(info.issued, "2026-11-11");
        assert_eq!(info.updates_until, "2027-11-11");
        // Ed25519 is deterministic: signing the same payload reproduces the key.
        assert_eq!(
            sign(
                r#"{"v":1,"id":"test-0001","email":"dev@example.com","plan":"pro","issued":"2026-11-11","updatesUntil":"2027-11-11"}"#
            ),
            TEST_KEY
        );
    }

    #[test]
    fn tolerates_whitespace_and_padding() {
        let wrapped = format!("  {}\n{}\n", &TEST_KEY[..40], &TEST_KEY[40..]);
        assert!(verify(&wrapped, &keys(), &[]).is_ok());
    }

    #[test]
    fn rejects_tampering_and_other_keys() {
        let (signed, sig) = TEST_KEY.rsplit_once('.').unwrap();
        let forged_payload = B64_NO_PAD.encode(
            r#"{"v":1,"id":"test-0001","email":"dev@example.com","plan":"pro","issued":"2026-11-11","updatesUntil":"2099-11-11"}"#,
        );
        let forged = format!("{KEY_PREFIX}{forged_payload}.{sig}");
        assert!(
            verify(&forged, &keys(), &[])
                .unwrap_err()
                .contains("signature")
        );
        let mut bad_sig = B64.decode(sig).unwrap();
        bad_sig[0] ^= 1;
        let tampered = format!("{signed}.{}", B64_NO_PAD.encode(bad_sig));
        assert!(verify(&tampered, &keys(), &[]).is_err());
        let other = Ed25519KeyPair::from_seed_unchecked(&[7u8; 32]).unwrap();
        let other_key: [u8; 32] = other.public_key().as_ref().try_into().unwrap();
        assert!(verify(TEST_KEY, &[other_key], &[]).is_err());
        assert!(
            verify(TEST_KEY, &[], &[])
                .unwrap_err()
                .contains("can't verify")
        );
        assert!(
            verify("hello", &keys(), &[])
                .unwrap_err()
                .contains("AKEV1.")
        );
        assert!(
            verify("AKEV1.!!!.???", &keys(), &[])
                .unwrap_err()
                .contains("damaged")
        );
    }

    #[test]
    fn rejects_unknown_versions_and_plans() {
        let v2 = sign(
            r#"{"v":2,"id":"a","email":"e","plan":"pro","issued":"2026-11-11","updatesUntil":"2027-11-11"}"#,
        );
        assert!(verify(&v2, &keys(), &[]).unwrap_err().contains("version 2"));
        let team = sign(
            r#"{"v":1,"id":"a","email":"e","plan":"team","issued":"2026-11-11","updatesUntil":"2027-11-11"}"#,
        );
        assert!(verify(&team, &keys(), &[]).unwrap_err().contains("plan"));
        let dates = sign(
            r#"{"v":1,"id":"a","email":"e","plan":"pro","issued":"soon","updatesUntil":"2027-11-11"}"#,
        );
        assert!(verify(&dates, &keys(), &[]).unwrap_err().contains("dates"));
    }

    #[test]
    fn free_by_default_with_trial_available() {
        let dir = tempfile::tempdir().unwrap();
        let s = manager(dir.path()).status();
        assert_eq!(s.plan, Plan::Free);
        assert_eq!(s.state, LicenseState::None);
        assert!(s.features.is_empty());
        assert!(s.trial_available);
    }

    #[test]
    fn a_key_activates_pro_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        let s = m.set_key(&format!("{TEST_KEY}\n")).unwrap();
        assert_eq!(s.state, LicenseState::Active);
        assert_eq!(s.features, Feature::ALL.to_vec());
        assert!(!s.trial_available);
        assert!(m.has(Feature::Wireguard));
        assert!(m.locked_actions().is_empty());
        let reloaded = manager(dir.path());
        assert_eq!(reloaded.status().state, LicenseState::Active);
        let s = reloaded.remove_key().unwrap();
        assert_eq!(s.state, LicenseState::None);
        assert_eq!(manager(dir.path()).status().state, LicenseState::None);
    }

    #[test]
    fn invalid_keys_are_refused_and_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        let e = m.set_key("AKEV1.nope.nope").unwrap_err();
        assert_eq!(e.status, StatusCode::BAD_REQUEST);
        assert_eq!(e.code, "invalid-license");
        assert!(!dir.path().join(LICENSE_FILE).exists());
    }

    #[test]
    fn builds_after_the_updates_year_run_as_free() {
        let dir = tempfile::tempdir().unwrap();
        let m = LicenseManager::load(
            dir.path(),
            true,
            Some(TEST_KEY.into()),
            &[TEST_PUBLIC_KEY.into()],
            None,
        )
        .with_release_date("2027-11-12");
        let s = m.status();
        assert_eq!(s.state, LicenseState::UpdatesEnded);
        assert_eq!(s.plan, Plan::Free);
        assert_eq!(s.license.unwrap().updates_until, "2027-11-11");
        assert!(s.trial_available);
        let same_day = LicenseManager::load(
            dir.path(),
            true,
            Some(TEST_KEY.into()),
            &[TEST_PUBLIC_KEY.into()],
            None,
        )
        .with_release_date("2027-11-11");
        assert_eq!(same_day.status().state, LicenseState::Active);
    }

    #[test]
    fn run_keys_are_not_stored() {
        let dir = tempfile::tempdir().unwrap();
        let m = LicenseManager::load(
            dir.path(),
            true,
            Some(TEST_KEY.into()),
            &[TEST_PUBLIC_KEY.into()],
            None,
        )
        .with_release_date("2027-01-01");
        assert_eq!(m.status().state, LicenseState::Active);
        assert!(!dir.path().join(LICENSE_FILE).exists());
        let bad = LicenseManager::load(
            dir.path(),
            true,
            Some("AKEV1.x.y".into()),
            &[TEST_PUBLIC_KEY.into()],
            None,
        );
        let s = bad.status();
        assert_eq!(s.state, LicenseState::Invalid);
        assert!(s.error.is_some());
    }

    #[test]
    fn trial_runs_once() {
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path());
        let s = m.start_trial().unwrap();
        assert_eq!(s.state, LicenseState::Trial);
        assert_eq!(s.plan, Plan::Pro);
        assert!(!s.trial_available);
        assert!(m.trial_running_until().is_some());
        assert_eq!(m.start_trial().unwrap_err().code, "trial-unavailable");
        let start = m.inner.read().stored.trial_started_at.unwrap();
        let ended = m.status_at(start + TRIAL_MS + 1.0);
        assert_eq!(ended.state, LicenseState::TrialEnded);
        assert!(ended.features.is_empty());
        assert!(!ended.trial_available);
        // Survives a restart.
        assert_eq!(manager(dir.path()).status().state, LicenseState::Trial);
    }

    /// Signed by the production signer for "Release canary", with updates that ended on
    /// 2026-01-01, before any Akev release: it proves this build trusts the production key
    /// without unlocking anything.
    const CANARY_KEY: &str = "AKEV1.eyJ2IjoxLCJpZCI6ImNhbmFyeS0wMDAxIiwiZW1haWwiOiJjYW5hcnlAYWtldi5kZXYiLCJuYW1lIjoiUmVsZWFzZSBjYW5hcnkiLCJwbGFuIjoicHJvIiwiaXNzdWVkIjoiMjAyNS0wMS0wMSIsInVwZGF0ZXNVbnRpbCI6IjIwMjYtMDEtMDEifQ.4QgU8nJKPyEsUPk7hMIetB8w0syUQ-bROgt15T6KYfzWv8sxy1VAlQi1sDxZQcSuy72TogKB_3y6LYWQEDKkDw";

    /// Release gate: a build that can't verify production keys would refuse every buyer.
    #[test]
    fn canary_key_verifies_in_release_builds() {
        let keys = decode_public_keys(&builtin_public_keys());
        let info = verify(CANARY_KEY, &keys, &builtin_revoked())
            .expect("this build must trust the production license key (PUBLIC_KEYS)");
        assert_eq!(info.id, "canary-0001");
        assert_eq!(info.updates_until, "2026-01-01");
        let dir = tempfile::tempdir().unwrap();
        let m = LicenseManager::load(
            dir.path(),
            false,
            Some(CANARY_KEY.into()),
            &builtin_public_keys(),
            None,
        );
        let status = m.status();
        assert_eq!(status.state, LicenseState::UpdatesEnded);
        assert_eq!(status.plan, Plan::Free);
    }

    #[test]
    fn the_revocation_list_parses() {
        for id in builtin_revoked() {
            assert!(!id.contains(char::is_whitespace), "{id:?}");
        }
        assert!(!builtin_revoked().iter().any(|id| id == "canary-0001"));
    }

    #[test]
    fn revoked_keys_are_refused() {
        let err = verify(TEST_KEY, &keys(), &["test-0001".to_string()]).unwrap_err();
        assert!(err.contains("revoked"), "{err}");
        assert!(verify(TEST_KEY, &keys(), &["other".to_string()]).is_ok());
        let dir = tempfile::tempdir().unwrap();
        let m = manager(dir.path()).with_revoked(&["test-0001"]);
        assert_eq!(m.set_key(TEST_KEY).unwrap_err().code, "invalid-license");
        let stored = LicenseManager::load(
            dir.path(),
            true,
            Some(TEST_KEY.into()),
            &[TEST_PUBLIC_KEY.into()],
            None,
        )
        .with_release_date("2027-01-01")
        .with_revoked(&["test-0001"]);
        assert_eq!(stored.status().state, LicenseState::Invalid);
    }

    #[test]
    fn team_keys_carry_seats() {
        let team = sign(
            r#"{"v":1,"id":"t","email":"e@x","plan":"pro","issued":"2026-11-11","updatesUntil":"2027-11-11","seats":5}"#,
        );
        assert_eq!(verify(&team, &keys(), &[]).unwrap().seats, Some(5));
        let one = sign(
            r#"{"v":1,"id":"t","email":"e@x","plan":"pro","issued":"2026-11-11","updatesUntil":"2027-11-11","seats":1}"#,
        );
        assert_eq!(verify(&one, &keys(), &[]).unwrap().seats, None);
        let none = sign(
            r#"{"v":1,"id":"t","email":"e@x","plan":"pro","issued":"2026-11-11","updatesUntil":"2027-11-11","seats":0}"#,
        );
        assert!(verify(&none, &keys(), &[]).unwrap_err().contains("seats"));
        assert_eq!(verify(TEST_KEY, &keys(), &[]).unwrap().seats, None);
    }

    #[test]
    fn the_trial_is_once_per_computer_not_per_data_dir() {
        let home = tempfile::tempdir().unwrap();
        let trial = home.path().join("config").join(TRIAL_FILE);
        let load = |dir: &Path, persist: bool| {
            LicenseManager::load(
                dir,
                persist,
                None,
                &[TEST_PUBLIC_KEY.to_string()],
                Some(trial.clone()),
            )
            .with_release_date("2027-01-01")
        };
        let first = tempfile::tempdir().unwrap();
        let s = load(first.path(), true).start_trial().unwrap();
        assert_eq!(s.state, LicenseState::Trial);
        assert!(trial.exists());
        // A fresh data directory sees the running trial and can't start another.
        let second = tempfile::tempdir().unwrap();
        let m = load(second.path(), true);
        assert_eq!(m.status().state, LicenseState::Trial);
        assert!(!m.status().trial_available);
        // So does an ephemeral session.
        let eph = tempfile::tempdir().unwrap();
        assert_eq!(load(eph.path(), false).status().state, LicenseState::Trial);
    }

    #[test]
    fn ephemeral_sessions_cannot_start_the_trial() {
        let dir = tempfile::tempdir().unwrap();
        let m = LicenseManager::load(
            dir.path(),
            false,
            None,
            &[TEST_PUBLIC_KEY.to_string()],
            None,
        );
        assert!(!m.status().trial_available);
        let e = m.start_trial().unwrap_err();
        assert_eq!(e.code, "trial-unavailable");
        assert!(e.message.contains("ephemeral"));
        // Not with a per-user trial file either (a running one is used, see above).
        let home = tempfile::tempdir().unwrap();
        let trial = home.path().join(TRIAL_FILE);
        let m = LicenseManager::load(
            dir.path(),
            false,
            None,
            &[TEST_PUBLIC_KEY.to_string()],
            Some(trial.clone()),
        );
        assert!(!m.status().trial_available);
        assert!(m.start_trial().is_err());
        assert!(!trial.exists());
    }

    #[test]
    fn locked_actions_without_pro() {
        let dir = tempfile::tempdir().unwrap();
        let locked = manager(dir.path()).locked_actions();
        assert!(locked["script"].contains("Pro"));
        assert!(locked.contains_key("tls-profile"));
    }

    #[test]
    fn pro_required_error_shape() {
        let e = pro_required(Feature::Wireguard);
        assert_eq!(e.status, StatusCode::PAYMENT_REQUIRED);
        let body = e.body();
        assert_eq!(body["error"]["code"], "pro-required");
        assert_eq!(body["error"]["feature"], "wireguard");
    }

    #[test]
    fn settings_changes_that_need_pro() {
        use akev_core::settings::{Settings, TlsProfile, TlsProfileOverride};
        let old = Settings::default();
        let mut new = old.clone();
        assert!(features_turned_on(&old, &new).is_empty());
        new.proxy.wireguard.enabled = true;
        new.proxy.transparent.enabled = true;
        new.proxy.tls_profile.overrides.push(TlsProfileOverride {
            host: "*.example.com".into(),
            profile: TlsProfile::Chrome,
        });
        assert_eq!(
            features_turned_on(&old, &new),
            vec![
                Feature::Wireguard,
                Feature::Transparent,
                Feature::TlsImpersonation
            ]
        );
        // Already on: editing something else doesn't need Pro.
        let mut edited = new.clone();
        edited.proxy.port = 9000;
        assert!(features_turned_on(&new, &edited).is_empty());
        // Turning impersonation off never needs Pro.
        let mut off = new.clone();
        off.proxy.tls_profile = Default::default();
        assert!(!features_turned_on(&new, &off).contains(&Feature::TlsImpersonation));
        // Removing one of several browser overrides doesn't either; changing one does.
        let mut two = new.clone();
        two.proxy.tls_profile.overrides.push(TlsProfileOverride {
            host: "api.test".into(),
            profile: TlsProfile::Firefox,
        });
        let mut one = two.clone();
        one.proxy.tls_profile.overrides.remove(0);
        assert!(features_turned_on(&two, &one).is_empty());
        let mut changed = one.clone();
        changed.proxy.tls_profile.overrides[0].profile = TlsProfile::Safari;
        assert_eq!(
            features_turned_on(&one, &changed),
            vec![Feature::TlsImpersonation]
        );
    }

    #[test]
    fn effective_proxy_masks_pro_features() {
        use akev_core::settings::{ProxySettings, TlsProfile};
        let dir = tempfile::tempdir().unwrap();
        let free = manager(dir.path()).status();
        let mut p = ProxySettings::default();
        p.wireguard.enabled = true;
        p.transparent.enabled = true;
        p.tls_profile.profile = TlsProfile::Chrome;
        let eff = effective_proxy(&p, &free);
        assert!(!eff.wireguard.enabled && !eff.transparent.enabled);
        assert_eq!(eff.tls_profile.profile, TlsProfile::Native);
        let m = manager(dir.path());
        m.set_key(TEST_KEY).unwrap();
        assert_eq!(effective_proxy(&p, &m.status()), p);
    }
}
