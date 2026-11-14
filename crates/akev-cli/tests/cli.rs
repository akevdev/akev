//! Integration tests driving the real `akev` binary against an in-process daemon (discovered
//! through `daemon.json` in a temporary data directory), plus a headless `akev capture` run.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::{AkevHandle, AkevOptions};
use serde_json::{Value, json};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_akev");
const TIMEOUT: Duration = Duration::from_secs(15);

async fn daemon(dir: &Path) -> AkevHandle {
    akev_api::start(AkevOptions {
        data_dir: Some(dir.to_path_buf()),
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..AkevOptions::default()
    })
    .await
    .expect("daemon starts")
}

fn akev(dir: &Path) -> Command {
    let mut c = Command::new(BIN);
    c.arg("--data-dir").arg(dir);
    // Never inherit a developer's settings (their shell or the repository's .env).
    c.env_remove("AKEV_URL")
        .env_remove("AKEV_TOKEN")
        .env_remove("AKEV_DATA_DIR")
        .env_remove("AKEV_LICENSE")
        .env("NO_COLOR", "1");
    c
}

/// A tiny upstream answering `GET /<anything>` with 200 and the path.
async fn upstream() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = axum::Router::new()
        .fallback(|uri: http::Uri| async move { format!("upstream saw {}", uri.path()) });
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[tokio::test]
async fn status_reports_running_daemon_and_stop_shuts_it_down() {
    let dir = tempfile::tempdir().unwrap();
    let out = akev(dir.path())
        .args(["status", "--json"])
        .output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "not running yet");
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["running"], false);

    let handle = daemon(dir.path()).await;
    let out = akev(dir.path())
        .args(["status", "--json"])
        .output()
        .await
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(v["running"], true);
    assert_eq!(v["apiUrl"], handle.api_url());
    assert_eq!(v["proxy"]["port"], handle.proxy_addr().port());

    let out = akev(dir.path())
        .args(["ca", "path"])
        .output()
        .await
        .unwrap();
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        handle.ca_path().display().to_string()
    );

    let stop = tokio::spawn({
        let mut cmd = akev(dir.path());
        async move { cmd.arg("stop").output().await.unwrap() }
    });
    tokio::time::timeout(TIMEOUT, handle.wait())
        .await
        .expect("`akev stop` requests shutdown");
    handle.shutdown().await;
    let out = tokio::time::timeout(TIMEOUT, stop).await.unwrap().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!dir.path().join("daemon.json").exists());
}

#[tokio::test]
async fn mcp_stdio_bridge_forwards_to_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let handle = daemon(dir.path()).await;
    let mut child = akev(dir.path())
        .arg("mcp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"cli-test","version":"1"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"akev_status","arguments":{}}}),
    ];
    for r in &requests {
        stdin.write_all(format!("{r}\n").as_bytes()).await.unwrap();
    }
    stdin.flush().await.unwrap();
    let mut responses = std::collections::HashMap::new();
    while responses.len() < 3 {
        let line = tokio::time::timeout(TIMEOUT, lines.next_line())
            .await
            .expect("MCP response in time")
            .unwrap()
            .expect("bridge still running");
        let v: Value = serde_json::from_str(&line).unwrap();
        responses.insert(v["id"].as_u64().unwrap(), v);
    }
    assert_eq!(responses[&1]["result"]["protocolVersion"], "2025-06-18");
    assert!(
        responses[&2]["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .any(|t| t["name"] == "akev_get_exchange")
    );
    let status: Value = serde_json::from_str(
        responses[&3]["result"]["content"][0]["text"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(status["apiUrl"], handle.api_url());
    drop(stdin); // EOF ends the bridge
    let exit = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success());
    handle.shutdown().await;
}

#[tokio::test]
async fn capture_prints_exchanges_and_writes_filtered_har() {
    let dir = tempfile::tempdir().unwrap();
    let har = dir.path().join("out.har");
    let origin = upstream().await;
    let mut child = akev(&dir.path().join("data"))
        .args([
            "capture",
            "--proxy-port",
            "0",
            "--duration",
            "4",
            "--filter",
        ])
        .arg("path:/keep")
        .arg("-o")
        .arg(&har)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap()).lines();
    let proxy = loop {
        let line = tokio::time::timeout(TIMEOUT, stderr.next_line())
            .await
            .expect("capture banner")
            .unwrap()
            .expect("capture still running");
        if let Some(addr) = line.split("capturing on ").nth(1) {
            break addr.trim().to_string();
        }
    };
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(&proxy).unwrap())
        .build()
        .unwrap();
    for path in ["/keep", "/drop"] {
        let r = client
            .get(format!("http://{origin}{path}"))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
    }
    let mut log = Vec::new();
    while let Ok(Some(line)) = tokio::time::timeout(TIMEOUT, stderr.next_line())
        .await
        .unwrap_or(Ok(None))
    {
        log.push(line);
    }
    let exit = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success(), "{log:?}");
    assert!(
        log.iter()
            .any(|l| l.starts_with("GET") && l.contains(" 200 ") && l.ends_with("/keep")),
        "one line per completed exchange: {log:?}"
    );
    let har: Value = serde_json::from_slice(&std::fs::read(&har).unwrap()).unwrap();
    let entries = har["log"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "--filter applies to the HAR");
    assert!(
        entries[0]["request"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/keep")
    );
}

/// `akev capture -- COMMAND`: the command runs through the proxy, its exit ends the capture
/// and becomes the exit status, and `--ca-out` writes the capture's own CA.
#[cfg(unix)]
#[tokio::test]
async fn capture_runs_a_command_through_the_proxy() {
    let dir = tempfile::tempdir().unwrap();
    let har = dir.path().join("out.har");
    let ca = dir.path().join("ca.pem");
    // The command reports the proxy it was given, then fails on purpose.
    let script = format!(
        "printf '%s' \"$HTTPS_PROXY\" > {}; exit 7",
        dir.path().join("proxy.txt").display()
    );
    let output = akev(&dir.path().join("data"))
        .args(["capture", "--proxy-port", "0", "-q", "-o"])
        .arg(&har)
        .arg("--ca-out")
        .arg(&ca)
        .args(["--", "sh", "-c", &script])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output();
    let output = tokio::time::timeout(TIMEOUT, output)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(7), "the command's exit status");
    let proxy = std::fs::read_to_string(dir.path().join("proxy.txt")).unwrap();
    assert!(proxy.starts_with("http://127.0.0.1:"), "{proxy}");
    assert!(
        std::fs::read_to_string(&ca)
            .unwrap()
            .contains("BEGIN CERTIFICATE")
    );
    let har: Value = serde_json::from_slice(&std::fs::read(&har).unwrap()).unwrap();
    assert!(har["log"]["entries"].is_array());
}

/// A fake `xdg-open` (first in `PATH`) recording what it was asked to open, one per line.
#[cfg(target_os = "linux")]
fn fake_opener(root: &Path) -> (std::ffi::OsString, std::path::PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let bin = root.join("fakebin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = root.join("opened.log");
    let script = bin.join("xdg-open");
    std::fs::write(
        &script,
        format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n", log.display()),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut path = bin.into_os_string();
    if let Some(rest) = std::env::var_os("PATH") {
        path.push(":");
        path.push(rest);
    }
    (path, log)
}

/// Wait until `file` exists and is non-empty, then return its contents.
#[cfg(target_os = "linux")]
async fn wait_for_content(file: &Path) -> String {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Ok(s) = std::fs::read_to_string(file)
            && !s.trim().is_empty()
        {
            return s;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{} never appeared",
            file.display()
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Wait for `daemon.json` in `dir` written by process `pid`.
#[cfg(unix)]
async fn wait_for_daemon(dir: &Path, pid: u32) -> akev_api::DaemonInfo {
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        if let Some(info) = akev_api::DaemonInfo::read(dir).filter(|i| i.pid == pid) {
            return info;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "daemon.json never appeared"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The browser launcher must only ever see the path of the private launcher page.
#[cfg(target_os = "linux")]
fn assert_opened_launcher_page(opened: &str, token: &str) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    assert!(
        !opened.contains(token),
        "the API token was passed on the browser launcher's command line: {opened}"
    );
    let target = opened.lines().next().unwrap().trim();
    let page = url::Url::parse(target)
        .ok()
        .and_then(|u| u.to_file_path().ok())
        .unwrap_or_else(|| panic!("expected a file URL, got {target}"));
    let html = std::fs::read_to_string(&page).unwrap();
    assert!(
        html.contains(&akev_api::ui_url_for("", token)[2..]),
        "{html}"
    );
    let mode = std::fs::metadata(&page).unwrap().permissions().mode();
    assert_eq!(mode & 0o077, 0, "the launcher page is private");
    page
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn start_opens_the_ui_without_the_token_on_a_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let (path, log) = fake_opener(dir.path());
    let mut child = akev(&data)
        .args([
            "start",
            "--ephemeral",
            "--api-port",
            "0",
            "--proxy-port",
            "0",
        ])
        .env("PATH", path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let info = wait_for_daemon(&data, child.id().unwrap()).await;
    let opened = wait_for_content(&log).await;
    let page = assert_opened_launcher_page(&opened, &info.token);
    assert_eq!(page.parent().unwrap(), std::path::absolute(&data).unwrap());

    let r = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/api/shutdown", info.api_url))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let exit = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success());
    assert!(!page.exists(), "the launcher page is removed on shutdown");
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn reopening_a_running_daemon_keeps_the_token_off_command_lines() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let handle = daemon(&data).await;
    let (path, log) = fake_opener(dir.path());
    let out = akev(&data).env("PATH", path).output().await.unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stdout).contains("already running"));
    let opened = wait_for_content(&log).await;
    let page = assert_opened_launcher_page(&opened, handle.token());
    handle.shutdown().await;
    assert!(
        !page.exists(),
        "the daemon removes the launcher on shutdown"
    );
}

/// Closing the terminal (SIGHUP) must run the graceful shutdown (restore the system proxy,
/// stop launched clients, remove daemon.json) instead of killing the process — even though
/// the terminal is gone and writing to it fails.
#[cfg(unix)]
#[tokio::test]
async fn hangup_runs_the_graceful_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let mut cmd = akev(&data);
    cmd.args([
        "start",
        "--ephemeral",
        "--no-open",
        "--api-port",
        "0",
        "--proxy-port",
        "0",
    ])
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true);
    // Don't inherit an ignored SIGHUP (e.g. the tests running under `nohup`).
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    let pid = child.id().unwrap();
    wait_for_daemon(&data, pid).await;
    // The terminal goes away: every later write to stdout/stderr fails.
    drop(child.stdout.take());
    drop(child.stderr.take());
    let sent = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    let exit = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .expect("akev exits after SIGHUP")
        .unwrap();
    assert_eq!(
        exit.code(),
        Some(0),
        "graceful exit, not killed by the signal: {exit:?}"
    );
    assert!(
        akev_api::DaemonInfo::read(&data).is_none(),
        "the graceful shutdown ran (daemon.json removed)"
    );
}

/// `nohup akev start &` keeps running when the terminal closes.
#[cfg(unix)]
#[tokio::test]
async fn hangup_stays_ignored_under_nohup() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let mut cmd = akev(&data);
    cmd.args([
        "start",
        "--ephemeral",
        "--no-open",
        "--api-port",
        "0",
        "--proxy-port",
        "0",
    ])
    .stdout(Stdio::null())
    .stderr(Stdio::null())
    .kill_on_drop(true);
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe {
        cmd.pre_exec(|| {
            libc::signal(libc::SIGHUP, libc::SIG_IGN);
            Ok(())
        });
    }
    let mut child = cmd.spawn().unwrap();
    let pid = child.id().unwrap();
    let info = wait_for_daemon(&data, pid).await;
    let sent = std::process::Command::new("kill")
        .args(["-HUP", &pid.to_string()])
        .status()
        .unwrap();
    assert!(sent.success());
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        child.try_wait().unwrap().is_none(),
        "an ignored SIGHUP doesn't stop Akev"
    );
    let r = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(format!("{}/api/shutdown", info.api_url))
        .bearer_auth(&info.token)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let exit = tokio::time::timeout(TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(exit.success());
}
