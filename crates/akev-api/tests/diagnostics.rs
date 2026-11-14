//! `GET /api/diagnostics` and the one-instance-per-data-directory lock, on real instances.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::{AkevOptions, observability, start};
use serde_json::Value;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const TOKEN: &str = "diagnostics-test-token-0123456789abcdef";

fn options() -> AkevOptions {
    AkevOptions {
        proxy_port: Some(0),
        api_port: 0,
        ..AkevOptions::default()
    }
}

#[tokio::test]
async fn diagnostics_describe_the_instance_without_secrets() {
    // What the binaries install: recent warnings feed the diagnostics.
    let _ = tracing_subscriber::registry()
        .with(observability::RecentEvents.with_filter(LevelFilter::WARN))
        .try_init();
    let handle = start(AkevOptions {
        ephemeral: true,
        token: Some(TOKEN.into()),
        ..options()
    })
    .await
    .unwrap();
    tracing::warn!(target: "akev_test", "upstream refused GET /x?token={TOKEN} (test warning)");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = format!("{}/api/diagnostics", handle.api_url());
    let denied = client.get(&url).send().await.unwrap();
    assert_eq!(denied.status(), 401);

    let resp = client.get(&url).bearer_auth(TOKEN).send().await.unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    let text = resp.text().await.unwrap();
    assert!(!text.contains(TOKEN), "the token leaked: {text}");
    let d: Value = serde_json::from_str(&text).unwrap();

    assert_eq!(d["version"], akev_core::VERSION);
    assert_eq!(d["releaseDate"], akev_core::RELEASE_DATE);
    assert_eq!(d["arch"], std::env::consts::ARCH);
    assert!(d["os"].as_str().is_some_and(|s| !s.is_empty()));
    assert!(
        ["linux", "macos", "windows"].contains(&d["platform"].as_str().unwrap()),
        "{d}"
    );
    assert_eq!(d["desktop"], false);
    assert!(d["uptimeSecs"].is_u64());
    assert_eq!(d["repository"], "https://github.com/akevdev/akev");
    assert!(d["stats"]["exchanges"].is_u64());
    assert_eq!(
        d["settings"]["proxy"]["port"],
        u64::from(handle.proxy_addr().port())
    );
    assert!(d["settings"]["capture"]["maxExchanges"].is_u64());
    assert!(d["settings"].get("ui").is_none());
    assert!(d["crashReports"].as_array().is_some_and(Vec::is_empty));
    assert!(d.get("logFile").is_some(), "logFile is always present: {d}");

    let warnings = d["recentWarnings"].as_array().unwrap();
    let ours = warnings
        .iter()
        .find(|w| {
            w["message"]
                .as_str()
                .is_some_and(|m| m.contains("test warning"))
        })
        .unwrap_or_else(|| panic!("the warning is listed: {d}"));
    assert_eq!(ours["level"], "WARN");
    assert_eq!(ours["target"], "akev_test");
    assert!(ours["at"].as_str().is_some_and(|s| s.contains('T')));
    handle.shutdown().await;
}

#[tokio::test]
async fn crash_reports_are_listed_newest_first() {
    let dir = tempfile::tempdir().unwrap();
    let crashes = dir.path().join(observability::CRASH_DIR);
    std::fs::create_dir_all(&crashes).unwrap();
    std::fs::write(crashes.join("crash-20260101-000000.txt"), "old").unwrap();
    std::fs::write(crashes.join("crash-20260202-000000.txt"), "new").unwrap();
    std::fs::write(crashes.join("notes.txt"), "not a report").unwrap();
    let handle = start(AkevOptions {
        ephemeral: true,
        data_dir: Some(dir.path().to_path_buf()),
        token: Some(TOKEN.into()),
        ..options()
    })
    .await
    .unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let d: Value = client
        .get(format!("{}/api/diagnostics", handle.api_url()))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        d["crashReports"],
        serde_json::json!(["crash-20260202-000000.txt", "crash-20260101-000000.txt"])
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn a_second_instance_on_the_same_data_dir_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let persistent = || AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        ..options()
    };
    let first = start(persistent()).await.unwrap();
    let instance = std::fs::read_to_string(dir.path().join("instance.json")).unwrap();
    let instance: Value = serde_json::from_str(&instance).unwrap();
    assert_eq!(instance["pid"], std::process::id());
    assert_eq!(instance["apiUrl"], first.api_url());

    let err = match start(persistent()).await {
        Ok(second) => {
            second.shutdown().await;
            panic!("two instances shared a data directory");
        }
        Err(e) => format!("{e:#}"),
    };
    assert!(err.contains("already using the data directory"), "{err}");
    assert!(
        err.contains(&format!("pid {}", std::process::id())),
        "{err}"
    );
    assert!(err.contains(&first.api_url()), "{err}");

    // Ephemeral sessions don't save anything there, so they don't need the lock.
    let ephemeral = start(AkevOptions {
        ephemeral: true,
        ..persistent()
    })
    .await
    .unwrap();
    ephemeral.shutdown().await;

    // Released after shutdown.
    first.shutdown().await;
    drop(first);
    assert!(!dir.path().join("instance.json").exists());
    let again = start(persistent()).await.unwrap();
    again.shutdown().await;
}
