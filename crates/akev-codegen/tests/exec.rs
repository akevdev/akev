//! End-to-end checks: run generated snippets with the real tools against a local capture server
//! and compare what arrives (method, target, headers, body bytes) with the request.
//!
//! These need local tools and loopback networking, so they're `#[ignore]`d by default:
//! `cargo test -p akev-codegen --test exec -- --ignored`. Targets whose tool (or library:
//! requests, httpx) is missing are skipped.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_codegen::{SnippetRequest, Target, generate};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::time::Duration;

#[derive(Debug)]
struct Captured {
    method: String,
    target: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

/// Accept one HTTP/1.1 request on a fresh port, reply `200 ok`, and hand back what was sent.
fn capture_server() -> (u16, mpsc::Receiver<Captured>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        stream.set_read_timeout(Some(Duration::from_secs(30))).ok();
        let mut reader = BufReader::new(stream.try_clone().expect("clone"));
        let mut line = String::new();
        reader.read_line(&mut line).expect("request line");
        let mut parts = line.trim_end().splitn(3, ' ');
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let mut headers = Vec::new();
        loop {
            let mut raw = Vec::new();
            reader.read_until(b'\n', &mut raw).expect("header");
            let text = String::from_utf8_lossy(&raw)
                .trim_end_matches(['\r', '\n'])
                .to_string();
            if text.is_empty() {
                break;
            }
            if let Some((n, v)) = text.split_once(':') {
                headers.push((n.trim().to_string(), v.trim().to_string()));
            }
        }
        let header = |name: &str| {
            headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.clone())
        };
        let mut stream = stream;
        if header("expect").is_some_and(|v| v.eq_ignore_ascii_case("100-continue")) {
            stream.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").ok();
        }
        let mut body = Vec::new();
        if header("transfer-encoding").is_some_and(|v| v.to_ascii_lowercase().contains("chunked")) {
            loop {
                let mut size = String::new();
                reader.read_line(&mut size).expect("chunk size");
                let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or("0"), 16)
                    .unwrap_or(0);
                let mut chunk = vec![0; n + 2];
                reader.read_exact(&mut chunk).expect("chunk");
                if n == 0 {
                    break;
                }
                body.extend_from_slice(&chunk[..n]);
            }
        } else if let Some(len) = header("content-length").and_then(|v| v.parse::<usize>().ok()) {
            body.resize(len, 0);
            reader.read_exact(&mut body).expect("body");
        }
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok").ok();
        tx.send(Captured {
            method,
            target,
            headers,
            body,
        })
        .ok();
    });
    (port, rx)
}

/// Clients that can't send non-ASCII header values at all (the snippet carries a note).
const ASCII_HEADERS_ONLY: [Target; 3] =
    [Target::JavaHttpClient, Target::Powershell, Target::DartHttp];

/// Clients that always send the method upper-cased (the snippet carries a note).
const UPPERCASE_METHODS: [Target; 2] = [Target::PythonRequests, Target::PythonHttpx];

/// Clients that always derive `Host` from the URL (the snippet carries a note).
const URL_HOST_ONLY: [Target; 2] = [Target::JsFetch, Target::NodeFetch];

/// A fixture pointed at the capture server (plain HTTP), minus what `target` documents it
/// can't send.
fn localize(req: &SnippetRequest, port: u16, target: Target) -> SnippetRequest {
    let rest = req
        .url
        .split_once("://")
        .map_or(req.url.as_str(), |(_, r)| r);
    let path = rest.find(['/', '?']).map_or("/", |i| &rest[i..]);
    let mut req = req.clone();
    req.url = format!("http://127.0.0.1:{port}{path}");
    if ASCII_HEADERS_ONLY.contains(&target) {
        req.headers.0.retain(|(_, v)| v.is_ascii());
    }
    if URL_HOST_ONLY.contains(&target) {
        req.headers
            .0
            .retain(|(n, _)| !n.eq_ignore_ascii_case("host"));
    }
    if target == Target::Wget {
        // Wget2 ignores empty-valued headers (the snippet carries a note).
        req.headers.0.retain(|(_, v)| !v.trim().is_empty());
    }
    req
}

fn merged(headers: impl Iterator<Item = (String, String)>) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (n, v) in headers {
        let lower = n.to_ascii_lowercase();
        match out.iter_mut().find(|(m, _)| *m == lower) {
            Some((_, existing)) => {
                existing.push_str(if lower == "cookie" { "; " } else { ", " });
                existing.push_str(&v);
            }
            None => out.push((lower, v)),
        }
    }
    out
}

/// Compare what arrived with what the snippet promised to send.
fn verify(name: &str, target: Target, req: &SnippetRequest, got: &Captured) -> Result<(), String> {
    let method = match req.method.to_ascii_uppercase().as_str() {
        m @ ("GET" | "POST" | "PUT" | "DELETE" | "HEAD" | "OPTIONS" | "PATCH") => m.to_string(),
        m if UPPERCASE_METHODS.contains(&target) => m.to_string(),
        _ => req.method.clone(),
    };
    if got.method != method {
        return Err(format!("method {} != {method}", got.method));
    }
    let expected_target = &req.url[req.url.find("//").map_or(0, |i| i + 2)..];
    let expected_target = &expected_target[expected_target.find('/').unwrap_or(0)..];
    // Clients normalise dot segments and unusual characters differently; compare simple URLs.
    let simple =
        !expected_target.contains(['[', '{', '\'', '"', '$']) && !expected_target.contains("/..");
    if simple && got.target != expected_target {
        return Err(format!("target {} != {expected_target}", got.target));
    }
    const SKIP: [&str; 13] = [
        "content-length",
        "content-encoding",
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "te",
        "trailer",
        "upgrade",
        "expect",
        "accept-encoding",
        "host",
        "proxy-authorization",
    ];
    let expected = merged(
        req.headers
            .iter()
            .filter(|(n, _)| {
                !n.starts_with(':') && !SKIP.contains(&n.to_ascii_lowercase().as_str())
            })
            .map(|(n, v)| (n.into(), v.into())),
    );
    let received = merged(got.headers.iter().cloned());
    for (name, value) in &expected {
        // Map-based clients join repeated headers; compare joined values.
        let got_value = received
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str());
        if got_value != Some(value.trim()) {
            return Err(format!(
                "header {name}: got {got_value:?}, expected {value:?}"
            ));
        }
    }
    if let Some(host) = req
        .headers
        .get("host")
        .filter(|h| !h.starts_with("127.0.0.1"))
    {
        let got_host = received
            .iter()
            .find(|(n, _)| n == "host")
            .map(|(_, v)| v.as_str());
        if got_host != Some(host) {
            return Err(format!("Host: got {got_host:?}, expected {host:?}"));
        }
    }
    let body = req.body.as_deref().unwrap_or_default();
    // Native-JSON snippets re-serialise the body: compare JSON values, not bytes.
    let is_json = req
        .headers
        .get("content-type")
        .is_some_and(|ct| ct.contains("json"));
    let same_json = || {
        let a: Option<serde_json::Value> = serde_json::from_slice(&got.body).ok();
        let b: Option<serde_json::Value> = serde_json::from_slice(body).ok();
        a.is_some() && a == b
    };
    if got.body != body && !(is_json && same_json()) {
        return Err(format!(
            "body differs for {name}/{}:\n got {:?}\nwant {:?}",
            target.id(),
            String::from_utf8_lossy(&got.body),
            String::from_utf8_lossy(body)
        ));
    }
    Ok(())
}

/// Run every fixture through `target` with `run(snippet_path)`.
fn exec_target(target: Target, file: &str, run: impl Fn(&Path) -> Command) {
    let mut failures = Vec::new();
    for case in common::cases() {
        let dir = common::temp_dir(&format!("exec-{}-{}", target.id(), case.name));
        let (port, rx) = capture_server();
        let req = localize(&case.request, port, target);
        let code = generate(target, &req);
        let path = dir.join(file);
        std::fs::write(&path, &code).expect("write");
        let mut cmd = run(&path);
        cmd.current_dir(&dir);
        let output = cmd.output().expect("spawn");
        match rx.recv_timeout(Duration::from_secs(60)) {
            Ok(got) => {
                if let Err(e) = verify(case.name, target, &req, &got) {
                    failures.push(format!("{}/{}: {e}\n{code}\n", case.name, target.id()));
                }
            }
            Err(_) => failures.push(format!(
                "{}/{}: no request received\nstdout: {}\nstderr: {}\n{code}\n",
                case.name,
                target.id(),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )),
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_curl() {
    if !common::have("curl", &["--version"]) {
        return eprintln!("skipping: curl");
    }
    exec_target(Target::Curl, "snippet.sh", |p| {
        let mut c = Command::new("bash");
        c.arg(p);
        c
    });
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_wget() {
    if !common::have("wget", &["--version"]) {
        return eprintln!("skipping: wget");
    }
    exec_target(Target::Wget, "snippet.sh", |p| {
        let mut c = Command::new("bash");
        c.arg(p);
        c
    });
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_httpie() {
    if !common::have("http", &["--version"]) {
        return eprintln!("skipping: httpie");
    }
    exec_target(Target::Httpie, "snippet.sh", |p| {
        let mut c = Command::new("bash");
        c.arg(p);
        c
    });
}

fn python(p: &Path) -> Command {
    let mut c = Command::new("python3");
    c.arg(p);
    c
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_python_requests() {
    if !common::have("python3", &["-c", "import requests"]) {
        return eprintln!("skipping: requests");
    }
    exec_target(Target::PythonRequests, "snippet.py", python);
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_python_httpx() {
    if !common::have("python3", &["-c", "import httpx"]) {
        return eprintln!("skipping: httpx");
    }
    exec_target(Target::PythonHttpx, "snippet.py", python);
}

fn node(p: &Path) -> Command {
    let mut c = Command::new("node");
    c.arg(p);
    c
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_node_fetch() {
    if !common::have(
        "node",
        &["-e", "if (typeof fetch !== 'function') process.exit(1)"],
    ) {
        return eprintln!("skipping: node fetch");
    }
    exec_target(Target::NodeFetch, "snippet.mjs", node);
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_js_fetch_in_node() {
    if !common::have(
        "node",
        &["-e", "if (typeof fetch !== 'function') process.exit(1)"],
    ) {
        return eprintln!("skipping: node fetch");
    }
    exec_target(Target::JsFetch, "snippet.mjs", node);
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_java() {
    if !common::have("java", &["-version"]) {
        return eprintln!("skipping: java");
    }
    exec_target(Target::JavaHttpClient, "Main.java", |p| {
        let mut c = Command::new("java");
        c.arg(p);
        c
    });
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_php() {
    if !common::have(
        "php",
        &["-r", "exit(function_exists('curl_init') ? 0 : 1);"],
    ) {
        return eprintln!("skipping: php-curl");
    }
    exec_target(Target::PhpCurl, "snippet.php", |p| {
        let mut c = Command::new("php");
        c.arg(p);
        c
    });
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_ruby() {
    if !common::have("ruby", &["-e", "require 'net/http'"]) {
        return eprintln!("skipping: ruby");
    }
    exec_target(Target::RubyNetHttp, "snippet.rb", |p| {
        let mut c = Command::new("ruby");
        c.arg(p);
        c
    });
}

#[test]
#[ignore = "runs local tools against a loopback server"]
fn exec_go() {
    if !common::have("go", &["version"]) {
        return eprintln!("skipping: go");
    }
    exec_target(Target::GoNetHttp, "main.go", |p| {
        let mut c = Command::new("go");
        c.arg("run").arg(p);
        c
    });
}
