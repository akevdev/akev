//! Update checks: compare this build with the newest release's `latest.json` (the manifest the
//! release workflow publishes for the desktop updater).
//!
//! This is the only request Akev makes on its own, and only when asked: the UI asks at most
//! once a day while `settings.updates.checkAutomatically` is on, and `akev update` or the
//! "Check now" button ask explicitly. Headless runs (CI, `akev capture`) never check. The request
//! carries nothing but the version in the User-Agent.

use std::time::{Duration, Instant};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// Where the newest release's manifest lives.
pub const MANIFEST_URL: &str = concat!(
    env!("CARGO_PKG_REPOSITORY"),
    "/releases/latest/download/latest.json"
);
/// The release page users download from.
pub const RELEASES_URL: &str = concat!(env!("CARGO_PKG_REPOSITORY"), "/releases/latest");

/// How old a result may be before an automatic check runs again.
const CHECK_EVERY: Duration = Duration::from_secs(24 * 60 * 60);
const TIMEOUT: Duration = Duration::from_secs(10);
/// The manifest is a few kilobytes; anything far bigger isn't it.
const MAX_MANIFEST: usize = 256 * 1024;

/// What `GET /api/update` reports.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateInfo {
    /// This build.
    pub current: String,
    /// The newest release, once checked.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// `latest` is newer than `current`.
    pub available: bool,
    /// Release notes of `latest` (Markdown).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_at: Option<String>,
    /// Where to download it.
    pub download_url: String,
    /// When the last check finished (unix ms); absent before the first one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checked_at: Option<f64>,
    /// Why the last check failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Deserialize)]
struct Manifest {
    version: String,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    pub_date: Option<String>,
}

/// Fetches and caches the newest release's version.
pub struct UpdateChecker {
    url: String,
    current: String,
    last: Mutex<Option<(Instant, UpdateInfo)>>,
    /// Serialises checks so concurrent callers share one request.
    running: tokio::sync::Mutex<()>,
}

impl UpdateChecker {
    /// Checks `url` for versions newer than this build.
    pub fn new(url: impl Into<String>) -> Self {
        Self::with_current(url, akev_core::VERSION)
    }

    fn with_current(url: impl Into<String>, current: &str) -> Self {
        UpdateChecker {
            url: url.into(),
            current: current.to_string(),
            last: Mutex::new(None),
            running: tokio::sync::Mutex::new(()),
        }
    }

    /// The last result, without checking.
    pub fn cached(&self) -> UpdateInfo {
        self.last
            .lock()
            .as_ref()
            .map(|(_, info)| info.clone())
            .unwrap_or_else(|| self.unchecked())
    }

    /// The last result, checking first when it's older than a day.
    pub async fn check_if_stale(&self) -> UpdateInfo {
        let fresh = self
            .last
            .lock()
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed() < CHECK_EVERY);
        if fresh {
            self.cached()
        } else {
            self.check().await
        }
    }

    /// Check now.
    pub async fn check(&self) -> UpdateInfo {
        let _one_at_a_time = self.running.lock().await;
        let info = match self.fetch().await {
            Ok(manifest) => {
                let latest = manifest.version.trim_start_matches('v').to_string();
                UpdateInfo {
                    available: is_newer(&latest, &self.current),
                    latest: Some(latest),
                    notes: manifest.notes.filter(|n| !n.trim().is_empty()),
                    published_at: manifest.pub_date,
                    checked_at: Some(now_ms()),
                    ..self.unchecked()
                }
            }
            Err(e) => {
                tracing::debug!(error = %e, "update check failed");
                UpdateInfo {
                    checked_at: Some(now_ms()),
                    error: Some(e),
                    // Keep what an earlier check found.
                    ..self.cached()
                }
            }
        };
        *self.last.lock() = Some((Instant::now(), info.clone()));
        info
    }

    fn unchecked(&self) -> UpdateInfo {
        UpdateInfo {
            current: self.current.clone(),
            latest: None,
            available: false,
            notes: None,
            published_at: None,
            download_url: RELEASES_URL.to_string(),
            checked_at: None,
            error: None,
        }
    }

    async fn fetch(&self) -> Result<Manifest, String> {
        let client = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .user_agent(format!("Akev/{}", self.current))
            .build()
            .map_err(|e| e.to_string())?;
        let response = client
            .get(&self.url)
            .send()
            .await
            .map_err(|e| format!("couldn't reach the release server: {e}"))?;
        if !response.status().is_success() {
            return Err(format!("the release server answered {}", response.status()));
        }
        if response
            .content_length()
            .is_some_and(|n| n > MAX_MANIFEST as u64)
        {
            return Err("the release manifest is unexpectedly large".into());
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("couldn't read the release manifest: {e}"))?;
        if bytes.len() > MAX_MANIFEST {
            return Err("the release manifest is unexpectedly large".into());
        }
        serde_json::from_slice(&bytes).map_err(|e| format!("unreadable release manifest: {e}"))
    }
}

/// Whether version `a` is newer than `b` (`major.minor.patch`, optionally `-N` for a
/// pre-release, which sorts before the release).
pub fn is_newer(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// `(major, minor, patch, is_release, pre)`: a release sorts after its pre-releases.
fn parse(v: &str) -> Option<(u64, u64, u64, bool, u64)> {
    let v = v.trim().trim_start_matches('v');
    let (core, pre) = match v.split_once('-') {
        Some((core, pre)) => (core, Some(pre.parse::<u64>().ok()?)),
        None => (v, None),
    };
    let mut parts = core.split('.').map(|p| p.parse::<u64>().ok());
    let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch, pre.is_none(), pre.unwrap_or(0)))
}

use akev_core::time::now_ms;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_order() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("v1.0.0", "0.9.9"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(
            is_newer("0.2.0", "0.2.0-3"),
            "a release beats its pre-releases"
        );
        assert!(is_newer("0.2.0-2", "0.2.0-1"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.2.0-1", "0.2.0"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert!(!is_newer("0.2", "0.1.0"));
        assert!(!is_newer("0.2.0.1", "0.1.0"));
    }

    async fn serve(body: &'static str, status: u16) -> String {
        use axum::http::StatusCode;
        let app = axum::Router::new().route(
            "/latest.json",
            axum::routing::get(move || async move {
                (StatusCode::from_u16(status).unwrap_or(StatusCode::OK), body)
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        format!("http://{addr}/latest.json")
    }

    #[tokio::test]
    async fn finds_a_newer_release() {
        let url = serve(
            r#"{"version":"0.3.0","notes":"- Faster.","pub_date":"2027-01-02T03:04:05Z","platforms":{}}"#,
            200,
        )
        .await;
        let checker = UpdateChecker::with_current(url, "0.2.0");
        assert_eq!(checker.cached().checked_at, None);
        let info = checker.check().await;
        assert!(info.available);
        assert_eq!(info.latest.as_deref(), Some("0.3.0"));
        assert_eq!(info.notes.as_deref(), Some("- Faster."));
        assert!(info.error.is_none());
        assert_eq!(checker.cached(), info);
        // Fresh: no second request needed.
        assert_eq!(checker.check_if_stale().await, info);
    }

    #[tokio::test]
    async fn up_to_date_and_failures() {
        let url = serve(r#"{"version":"v0.2.0"}"#, 200).await;
        let info = UpdateChecker::with_current(url, "0.2.0").check().await;
        assert!(!info.available);
        assert_eq!(info.latest.as_deref(), Some("0.2.0"));

        let url = serve("not found", 404).await;
        let info = UpdateChecker::with_current(url, "0.2.0").check().await;
        assert!(!info.available);
        assert!(info.error.unwrap().contains("404"));

        let info = UpdateChecker::with_current("http://127.0.0.1:9/latest.json", "0.2.0")
            .check()
            .await;
        assert!(info.error.is_some());
        assert!(info.checked_at.is_some());
    }
}
