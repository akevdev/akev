//! The panic hook writes a scrubbed crash report (its own test binary: the hook is global).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::observability;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const SECRET: &str = "crash-test-token-abcdef0123456789";

#[test]
fn a_panic_writes_a_crash_report_without_secrets() {
    let _ = tracing_subscriber::registry()
        .with(observability::RecentEvents)
        .try_init();
    let dir = tempfile::tempdir().unwrap();
    let crashes = dir.path().join(observability::CRASH_DIR);
    observability::install_panic_hook(crashes.clone());
    observability::register_secret(SECRET);
    tracing::info!(target: "akev_test", "serving GET /api/stream?token={SECRET}");
    tracing::warn!(target: "akev_test", "the last thing before the crash");

    let worker = std::thread::Builder::new()
        .name("akev-crash-test".into())
        .spawn(|| {
            panic!("boom with Authorization: Bearer {SECRET}");
        })
        .unwrap();
    assert!(worker.join().is_err());

    let reports = observability::crash_reports(&crashes);
    assert_eq!(reports.len(), 1, "{reports:?}");
    let report = std::fs::read_to_string(crashes.join(&reports[0])).unwrap();
    assert!(report.starts_with("Akev crash report"), "{report}");
    assert!(report.contains(&format!("Version: {}", akev_core::VERSION)));
    assert!(report.contains("Thread: akev-crash-test"), "{report}");
    assert!(
        report.contains("Panic: boom with Authorization: [redacted]"),
        "{report}"
    );
    assert!(report.contains("crash_report.rs"), "{report}");
    assert!(report.contains("Backtrace:"), "{report}");
    assert!(
        report.contains("serving GET /api/stream?token=[redacted]"),
        "{report}"
    );
    assert!(
        report.contains("the last thing before the crash"),
        "{report}"
    );
    assert!(!report.contains(SECRET), "the token leaked: {report}");
}
