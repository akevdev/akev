//! `akev capture`: headless, ephemeral capture for CI pipelines.

use crate::style::{self, Paint};
use crate::{GlobalArgs, shutdown_gracefully, shutdown_signal};
use akev_api::AkevOptions;
use akev_core::{ExchangeKind, ExchangeState, ExchangeSummary};
use akev_store::StoreEvent;
use anyhow::Context;
use clap::Args;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Args, Debug, Clone)]
pub struct CaptureArgs {
    /// HAR file to write when the capture ends.
    #[arg(short, long, value_name = "FILE")]
    pub output: PathBuf,

    /// Proxy port (default 8000; the next free port is used if busy; 0 = random).
    #[arg(long, value_name = "PORT")]
    pub proxy_port: Option<u16>,

    /// API port for the embedded instance (default: random).
    #[arg(long, value_name = "PORT")]
    pub api_port: Option<u16>,

    /// Rules file to apply (mocks, rewrites...).
    #[arg(long, value_name = "FILE")]
    pub rules: Option<PathBuf>,

    /// Only write exchanges matching this filter to the HAR.
    #[arg(long, value_name = "QUERY")]
    pub filter: Option<String>,

    /// Stop automatically after this many seconds.
    #[arg(long, value_name = "SECS")]
    pub duration: Option<u64>,

    /// Don't print a line per exchange.
    #[arg(short, long)]
    pub quiet: bool,

    /// Also write the CA certificate (PEM) here, for clients started some other way. Each
    /// capture makes a fresh CA, so `akev ca path` doesn't point at it.
    #[arg(long, value_name = "FILE")]
    pub ca_out: Option<PathBuf>,

    /// Run this command through the proxy (proxy and CA variables set for it), stop when it
    /// exits, and exit with its status: `akev capture -o t.har -- npm test`.
    #[arg(last = true, value_name = "COMMAND")]
    pub command: Vec<String>,
}

/// Whether an exchange finished (completed, aborted, or a finished stream/tunnel row).
pub(crate) fn is_done(s: &ExchangeSummary) -> bool {
    !matches!(
        s.state,
        ExchangeState::Requesting
            | ExchangeState::Waiting
            | ExchangeState::Responding
            | ExchangeState::Paused
    )
}

/// One line per completed exchange: `GET    200   85ms  https://example.com/path`.
pub fn summary_line(p: &Paint, s: &ExchangeSummary) -> String {
    let default_port = matches!(
        (s.scheme.as_str(), s.port),
        ("http" | "ws", 80) | ("https" | "wss", 443)
    );
    let host = if s.host.contains(':') {
        format!("[{}]", s.host)
    } else {
        s.host.clone()
    };
    let url = if default_port || s.port == 0 {
        format!("{}://{}{}", s.scheme, host, s.path)
    } else {
        format!("{}://{}:{}{}", s.scheme, host, s.port, s.path)
    };
    let status_text = match (s.status, s.kind, s.error) {
        (Some(code), _, _) => code.to_string(),
        (None, ExchangeKind::TlsFailure, _) => "TLS".into(),
        (None, ExchangeKind::Tunnel, _) => "TUN".into(),
        (None, _, Some(_)) => "ERR".into(),
        (None, _, None) => "---".into(),
    };
    let duration = s
        .duration
        .map(style::duration_ms)
        .unwrap_or_else(|| "-".into());
    let mut line = format!(
        "{} {} {} {}",
        p.bold(&format!("{:<7}", s.method)),
        p.status(s.status, &format!("{status_text:>3}")),
        p.dim(&format!("{duration:>7}")),
        url
    );
    if let Some(err) = s.error {
        let name = serde_json::to_value(err)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_default();
        line.push_str(&p.red(&format!("  ({name})")));
    }
    if s.mocked {
        line.push_str(&p.magenta("  [mocked]"));
    }
    line
}

pub async fn run(args: CaptureArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let filter = match args
        .filter
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty())
    {
        Some(q) => Some(akev_filter::Filter::parse(q).map_err(|e| {
            anyhow::anyhow!("invalid --filter at {}..{}: {}", e.start, e.end, e.message)
        })?),
        None => None,
    };
    if let Some(parent) = args.output.parent().filter(|p| !p.as_os_str().is_empty())
        && !parent.is_dir()
    {
        anyhow::bail!("output directory {} does not exist", parent.display());
    }
    let handle = akev_api::start(AkevOptions {
        data_dir: global.data_dir.clone(),
        proxy_port: args.proxy_port,
        api_port: args.api_port.unwrap_or(0),
        token: global.token.clone(),
        ephemeral: true,
        rules_file: args.rules.clone(),
        license: global.license.clone(),
        trial_file: akev_api::default_trial_file(),
        ..AkevOptions::default()
    })
    .await?;

    let p = Paint::stderr();
    let proxy = handle
        .status()
        .proxy
        .addresses
        .first()
        .cloned()
        .unwrap_or_else(|| handle.proxy_addr().to_string());
    let ca = handle.ca_path().display().to_string();
    if let Some(out) = &args.ca_out {
        std::fs::copy(handle.ca_path(), out)
            .with_context(|| format!("failed to write the CA certificate to {}", out.display()))?;
    }
    eprintln_safe!(
        "{} capturing on {}",
        p.accent("▲ akev"),
        p.bold(&format!("http://{proxy}"))
    );
    eprintln_safe!("  CA certificate: {ca}");
    eprintln_safe!(
        "  {}",
        p.dim(&format!(
            "export HTTP_PROXY=http://{proxy} HTTPS_PROXY=http://{proxy} SSL_CERT_FILE={ca} NODE_EXTRA_CA_CERTS={ca} REQUESTS_CA_BUNDLE={ca}"
        ))
    );
    // `-- COMMAND`: run it with the proxy variables; its exit ends the capture.
    let mut child = match args.command.split_first() {
        Some((program, rest)) => {
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(rest).envs(handle.proxy_env_vars());
            Some(
                cmd.spawn()
                    .with_context(|| format!("failed to start {program}"))?,
            )
        }
        None => None,
    };
    let mut child_status: Option<std::process::ExitStatus> = None;
    match args.duration {
        _ if child.is_some() => eprintln_safe!(
            "  {}",
            p.dim(&format!(
                "running {}; writing {} when it exits",
                args.command.join(" "),
                args.output.display()
            ))
        ),
        Some(secs) => eprintln_safe!(
            "  {}",
            p.dim(&format!(
                "stopping after {secs}s (or Ctrl+C); writing {}",
                args.output.display()
            ))
        ),
        None => eprintln_safe!(
            "  {}",
            p.dim(&format!(
                "press Ctrl+C to stop and write {}",
                args.output.display()
            ))
        ),
    }

    let mut rx = handle.store().subscribe();
    let mut printed: HashSet<u64> = HashSet::new();
    let deadline = async {
        match args.duration {
            Some(secs) => tokio::time::sleep(Duration::from_secs(secs)).await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = shutdown_signal() => break,
            _ = &mut deadline => break,
            _ = handle.wait() => break,
            status = async {
                match child.as_mut() {
                    Some(c) => c.wait().await,
                    None => std::future::pending().await,
                }
            } => {
                child_status = status.ok();
                break;
            }
            ev = rx.recv() => match ev {
                Ok(StoreEvent::Added(s) | StoreEvent::Updated(s)) => {
                    if is_done(&s) && printed.insert(s.id.0) && !args.quiet {
                        eprintln_safe!("{}", summary_line(&p, &s));
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                    tracing::debug!("skipped {n} live events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    }

    // Stop recording new exchanges and give in-flight ones a moment to complete.
    handle.set_recording(false);
    for _ in 0..20 {
        if handle.store().summaries().iter().all(is_done) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let store = handle.store().clone();
    let total = store.stats().exchanges;
    let output = args.output.clone();
    let written = tokio::task::spawn_blocking(move || -> anyhow::Result<usize> {
        let ids = filter.as_ref().map(|f| store.query_ids(f));
        write_har_file(&store, ids.as_deref(), &output)
    })
    .await
    .context("HAR export task failed")??;
    eprintln_safe!(
        "{} wrote {} exchange(s) to {}{}",
        p.green("✓"),
        written,
        args.output.display(),
        if args.filter.is_some() {
            format!(" ({total} captured)")
        } else {
            String::new()
        }
    );
    shutdown_gracefully(&handle).await;
    if let Some(mut c) = child
        && child_status.is_none()
    {
        // Stopped early (Ctrl+C, --duration): don't leave the command running.
        let _ = c.kill().await;
    }
    Ok(match child_status {
        Some(status) if !status.success() => ExitCode::from(
            status
                .code()
                .and_then(|c| u8::try_from(c).ok())
                .unwrap_or(1),
        ),
        _ => ExitCode::SUCCESS,
    })
}

/// Write the HAR export of `ids` (all when `None`) to `output` through a sibling temp file,
/// streamed one exchange at a time so memory stays flat however much was captured. Returns
/// the number of exchanges written. On failure the temp file is removed and `output` is left
/// as it was.
fn write_har_file(
    store: &akev_store::Store,
    ids: Option<&[akev_core::ExchangeId]>,
    output: &Path,
) -> anyhow::Result<usize> {
    let tmp = output.with_extension("har.tmp");
    let streamed = (|| -> anyhow::Result<usize> {
        let file = std::fs::File::create(&tmp)
            .with_context(|| format!("failed to create {}", tmp.display()))?;
        let mut out = std::io::BufWriter::new(file);
        let count = store
            .write_har(&mut out, ids)
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        out.into_inner()
            .map_err(|e| e.into_error())
            .and_then(|f| f.sync_all())
            .with_context(|| format!("failed to write {}", tmp.display()))?;
        Ok(count)
    })();
    let count = match streamed {
        Ok(count) => count,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&tmp, output) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("failed to write {}", output.display()));
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::*;

    #[test]
    fn har_file_is_streamed_export() {
        let store = akev_store::Store::new(Default::default(), None);
        let har = serde_json::json!({ "log": { "version": "1.2", "creator": { "name": "t", "version": "1" }, "entries": [
            { "startedDateTime": "2026-01-01T00:00:00.000Z", "time": 5,
              "request": { "method": "POST", "url": "https://example.com/a", "httpVersion": "HTTP/1.1",
                "headers": [{ "name": "Content-Type", "value": "text/plain" }], "queryString": [], "cookies": [],
                "headersSize": -1, "bodySize": 5, "postData": { "mimeType": "text/plain", "text": "hello" } },
              "response": { "status": 200, "statusText": "OK", "httpVersion": "HTTP/1.1",
                "headers": [{ "name": "Content-Type", "value": "application/json" }], "cookies": [],
                "content": { "size": 11, "mimeType": "application/json", "text": "{\"ok\":true}" },
                "redirectURL": "", "headersSize": -1, "bodySize": 11 },
              "cache": {}, "timings": { "send": 0, "wait": 5, "receive": 0 } }
        ] } });
        let ids = store
            .import_har(&serde_json::to_vec(&har).unwrap())
            .unwrap();
        assert_eq!(ids.len(), 1);
        let dir = tempfile::tempdir().unwrap();
        let output = dir.path().join("out.har");
        assert_eq!(write_har_file(&store, None, &output).unwrap(), 1);
        let expected = serde_json::to_vec_pretty(&store.export_har(None)).unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), expected);
        assert!(!output.with_extension("har.tmp").exists());

        // A failed write leaves no temp file and doesn't touch an existing output.
        let missing = dir.path().join("missing").join("out.har");
        assert!(write_har_file(&store, None, &missing).is_err());
        assert!(!missing.with_extension("har.tmp").exists());
    }

    fn summary() -> ExchangeSummary {
        ExchangeSummary {
            id: ExchangeId(1),
            kind: ExchangeKind::Http,
            state: ExchangeState::Complete,
            started_at: 0.0,
            method: "GET".into(),
            scheme: "https".into(),
            host: "example.com".into(),
            port: 443,
            path: "/a?b=1".into(),
            http_version: HttpVersion::H2,
            status: Some(200),
            content_kind: ContentKind::Json,
            content_type: None,
            request_size: 0,
            response_size: 10,
            duration: Some(85.2),
            error: None,
            paused: None,
            source: None,
            rule: None,
            modified: false,
            mocked: false,
            pinned: false,
            color: None,
            message_count: 0,
            has_note: false,
            llm: None,
        }
    }

    #[test]
    fn lines() {
        let p = Paint::plain();
        assert_eq!(
            summary_line(&p, &summary()),
            "GET     200    85ms https://example.com/a?b=1"
        );
        let mut s = summary();
        s.status = None;
        s.error = Some(ErrorKind::ConnectionRefused);
        s.port = 8443;
        s.duration = Some(1500.0);
        assert_eq!(
            summary_line(&p, &s),
            "GET     ERR   1.50s https://example.com:8443/a?b=1  (connection-refused)"
        );
        let mut s = summary();
        s.mocked = true;
        assert!(summary_line(&p, &s).ends_with("[mocked]"));
    }

    #[test]
    fn completion() {
        let mut s = summary();
        assert!(is_done(&s));
        s.state = ExchangeState::Waiting;
        assert!(!is_done(&s));
        s.state = ExchangeState::Aborted;
        assert!(is_done(&s));
    }
}
