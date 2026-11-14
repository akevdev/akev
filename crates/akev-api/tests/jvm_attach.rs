//! JVM attach end to end, against a real JDK and a real (in-process) Akev.
//!
//! A Java program (`tests/fixtures/jvm/AkevJvmTarget.java`) serves HTTPS itself and keeps calling it
//! with the JVM's default clients — `HttpURLConnection`, a `java.net.http.HttpClient` created
//! per request and one created at startup — plus a loopback URL and an untrusted server. It
//! starts out talking to its origin directly. Activating the `jvm` interceptor through the REST
//! API attaches the Akev agent: the requests then flow through Akev's proxy, decrypted
//! (MITM) and recorded, while loopback stays direct and the untrusted server stays untrusted.
//! Deactivating restores direct connections.
//!
//! Skipped (with a note) without `java`, `javac` and `keytool`, or with a Java older than 11.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const HOST: &str = "jvm-origin.akev.test";
const TARGET_SRC: &str = include_str!("fixtures/jvm/AkevJvmTarget.java");
const WAIT: Duration = Duration::from_secs(60);
const KINDS: [&str; 3] = ["huc", "jhc-new", "jhc-early"];

fn find_tool(name: &str) -> Option<PathBuf> {
    let exe = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    if let Some(home) = std::env::var_os("JAVA_HOME").filter(|h| !h.is_empty()) {
        let p = PathBuf::from(home).join("bin").join(&exe);
        if p.is_file() {
            return Some(p);
        }
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|d| d.join(&exe))
        .find(|p| p.is_file())
}

fn keytool(keytool: &Path, dir: &Path, args: &[&str]) {
    let out = Command::new(keytool)
        .current_dir(dir)
        .args(args)
        .args(["-storepass", "changeit"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "keytool {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

/// One line of the target's output: `OK|ERR <kind> <seq> <detail>`.
#[derive(Debug, Clone)]
struct Line {
    ok: bool,
    kind: String,
    seq: u64,
    detail: String,
}

fn parse(line: &str) -> Option<Line> {
    let mut parts = line.splitn(4, ' ');
    let ok = match parts.next()? {
        "OK" => true,
        "ERR" => false,
        _ => return None,
    };
    Some(Line {
        ok,
        kind: parts.next()?.to_string(),
        seq: parts.next()?.parse().ok()?,
        detail: parts.next().unwrap_or_default().to_string(),
    })
}

struct Target {
    child: Child,
    lines: Arc<Mutex<Vec<String>>>,
    errors: Arc<Mutex<Vec<String>>>,
}

impl Drop for Target {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Target {
    fn results(&self) -> Vec<Line> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|l| parse(l))
            .collect()
    }

    fn max_seq(&self) -> u64 {
        self.results().iter().map(|l| l.seq).max().unwrap_or(0)
    }

    fn dump(&self) -> String {
        let lines = self.lines.lock().unwrap();
        let tail = lines.len().saturating_sub(12);
        format!(
            "stdout tail:\n{}\nstderr:\n{}",
            lines[tail..].join("\n"),
            self.errors.lock().unwrap().join("\n")
        )
    }

    /// Wait until every kind in `kinds` has a result with `seq > after` matching `pred`.
    async fn wait_round(
        &self,
        what: &str,
        after: u64,
        kinds: &[&str],
        pred: impl Fn(&Line) -> bool,
    ) {
        let deadline = Instant::now() + WAIT;
        loop {
            let results = self.results();
            let done = kinds.iter().all(|k| {
                results
                    .iter()
                    .any(|l| l.kind == *k && l.seq > after && pred(l))
            });
            if done {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}\n{}",
                self.dump()
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

struct Api {
    handle: AkevHandle,
    client: reqwest::Client,
}

impl Api {
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut req = self
            .client
            .request(method, format!("{}{path}", self.handle.api_url()))
            .bearer_auth(self.handle.token());
        if let Some(b) = body {
            req = req.json(&b);
        }
        req.send().await.unwrap()
    }

    async fn get(&self, path: &str) -> Value {
        let r = self.call(reqwest::Method::GET, path, None).await;
        assert!(r.status().is_success(), "GET {path} → {}", r.status());
        r.json().await.unwrap()
    }

    /// Recorded exchanges to the origin: (kind, seq, summary).
    async fn origin_exchanges(&self) -> Vec<(String, u64, Value)> {
        let list = self.get("/api/exchanges").await;
        list.as_array()
            .unwrap()
            .iter()
            .filter(|s| s["host"] == HOST)
            .filter_map(|s| {
                let path = s["path"].as_str()?;
                let (kind, query) = path.trim_start_matches('/').split_once("?seq=")?;
                Some((kind.to_string(), query.parse().ok()?, s.clone()))
            })
            .collect()
    }

    async fn jvm(&self) -> Value {
        let list = self.get("/api/interceptors").await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|i| i["id"] == "jvm")
            .cloned()
            .expect("jvm interceptor listed")
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn attach_routes_a_running_jvm_through_akev_and_restore_sends_it_direct() {
    let (Some(java), Some(javac), Some(keytool_bin)) =
        (find_tool("java"), find_tool("javac"), find_tool("keytool"))
    else {
        eprintln!("skipping: no JDK (java, javac, keytool) found");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();

    // Origin certificate (trusted by the target through javax.net.ssl.trustStore) and an
    // untrusted one, both valid for the origin name and 127.0.0.1.
    for (store, alias) in [("origin.p12", "origin"), ("untrusted.p12", "untrusted")] {
        keytool(
            &keytool_bin,
            d,
            &[
                "-genkeypair",
                "-alias",
                alias,
                "-keyalg",
                "EC",
                // Java 8's keytool has no -groupname; 256-bit EC is secp256r1 everywhere.
                "-keysize",
                "256",
                "-validity",
                "2",
                "-dname",
                &format!("CN={HOST}"),
                "-ext",
                &format!("SAN=dns:{HOST},ip:127.0.0.1"),
                "-keystore",
                store,
                "-storetype",
                "PKCS12",
            ],
        );
    }
    keytool(
        &keytool_bin,
        d,
        &[
            "-exportcert",
            "-rfc",
            "-alias",
            "origin",
            "-keystore",
            "origin.p12",
            "-file",
            "origin.pem",
        ],
    );
    keytool(
        &keytool_bin,
        d,
        &[
            "-importcert",
            "-noprompt",
            "-alias",
            "origin",
            "-file",
            "origin.pem",
            "-keystore",
            "trust.p12",
            "-storetype",
            "PKCS12",
        ],
    );
    // The target resolves the origin name itself (direct mode) from this hosts file.
    std::fs::write(d.join("hosts"), format!("127.0.0.1 {HOST} localhost\n")).unwrap();
    std::fs::write(d.join("AkevJvmTarget.java"), TARGET_SRC).unwrap();
    // Java 8's javac doesn't create the output directory.
    std::fs::create_dir_all(d.join("classes")).unwrap();
    let out = Command::new(&javac)
        .current_dir(d)
        .args(["-nowarn", "-d", "classes", "AkevJvmTarget.java"])
        .output()
        .unwrap();
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        if err.contains("java.net.http") {
            eprintln!("skipping: Java 11+ is needed (java.net.http): {err}");
            return;
        }
        panic!("javac failed: {err}");
    }

    // Akev, resolving the origin name to the target's server and accepting its certificate.
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    })
    .await
    .expect("akev starts");
    let api = Api {
        handle,
        client: reqwest::Client::builder().no_proxy().build().unwrap(),
    };
    let mut settings = api.get("/api/settings").await;
    settings.as_object_mut().unwrap().remove("ui");
    settings["proxy"]["dnsOverrides"] = json!([{ "host": HOST, "address": "127.0.0.1" }]);
    settings["proxy"]["ignoreUpstreamCertErrors"] = json!([HOST]);
    let r = api
        .call(reqwest::Method::PUT, "/api/settings", Some(settings))
        .await;
    assert_eq!(r.status(), 200);

    let mut child = Command::new(&java)
        .current_dir(d)
        .arg(format!(
            "-Djdk.net.hosts.file={}",
            d.join("hosts").display()
        ))
        .arg(format!(
            "-Djavax.net.ssl.trustStore={}",
            d.join("trust.p12").display()
        ))
        .arg("-Djavax.net.ssl.trustStorePassword=changeit")
        .args([
            "-cp",
            "classes",
            "AkevJvmTarget",
            "origin.p12",
            "untrusted.p12",
            HOST,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let lines = Arc::new(Mutex::new(Vec::new()));
    let errors = Arc::new(Mutex::new(Vec::new()));
    for (stream, sink) in [
        (
            Box::new(child.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            lines.clone(),
        ),
        (Box::new(child.stderr.take().unwrap()), errors.clone()),
    ] {
        std::thread::spawn(move || {
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                sink.lock().unwrap().push(line);
            }
        });
    }
    let target = Target {
        child,
        lines,
        errors,
    };
    let pid = target.child.id();

    // ---- Direct: everything works without Akev, nothing is recorded. ----
    target
        .wait_round(
            "direct requests",
            0,
            &["huc", "jhc-new", "jhc-early", "loopback"],
            |l| l.ok,
        )
        .await;
    target
        .wait_round("the untrusted server", 0, &["untrusted"], |l| !l.ok)
        .await;
    assert!(
        api.origin_exchanges().await.is_empty(),
        "nothing goes through Akev before attaching"
    );

    // ---- Attach through the REST API. ----
    let jvm = api.jvm().await;
    assert_eq!(jvm["available"], true, "{jvm}");
    assert_eq!(jvm["needsConfig"], true);
    let listed = jvm["metadata"]["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pid"] == pid)
        .cloned()
        .unwrap_or_else(|| panic!("target {pid} not listed: {}", jvm["metadata"]));
    assert_eq!(listed["name"], "AkevJvmTarget");
    assert_eq!(listed["attached"], false);

    let r = api
        .call(
            reqwest::Method::POST,
            "/api/interceptors/jvm/activate",
            Some(json!({ "options": { "pid": pid } })),
        )
        .await;
    assert_eq!(r.status(), 200);
    let result: Value = r.json().await.unwrap();
    assert_eq!(result["success"], true, "{result}\n{}", target.dump());
    assert!(
        result["message"]
            .as_str()
            .unwrap()
            .starts_with("Attached to AkevJvmTarget"),
        "{result}"
    );
    assert_eq!(
        result["metadata"]["agent"]["existingDefaultContext"], "patched",
        "{result}"
    );
    assert_eq!(
        result["metadata"]["agent"]["proxyProperties"], "set",
        "{result}"
    );
    // The request in flight while attaching may still go direct: judge from the next ones.
    let attached_at = target.max_seq() + 1;

    // Requests succeed through Akev (the target trusts the Akev CA) and are recorded,
    // decrypted, for every client — including the HttpClient created before the attach.
    target
        .wait_round("intercepted requests", attached_at, &KINDS, |l| {
            l.ok && l.detail.starts_with("200 origin ok")
        })
        .await;
    let deadline = Instant::now() + WAIT;
    let recorded = loop {
        let ex = api.origin_exchanges().await;
        let complete = |k: &str| {
            ex.iter()
                .find(|(kind, seq, s)| kind == k && *seq > attached_at && s["status"] == 200)
                .cloned()
        };
        if let (Some(a), Some(b), Some(c)) =
            (complete("huc"), complete("jhc-new"), complete("jhc-early"))
        {
            break vec![a, b, c];
        }
        assert!(
            Instant::now() < deadline,
            "exchanges not recorded: {ex:?}\n{}",
            target.dump()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    for (kind, seq, summary) in &recorded {
        assert_eq!(summary["scheme"], "https", "{summary}");
        let id = summary["id"].as_u64().unwrap();
        let body = api.get(&format!("/api/exchanges/{id}/body/response")).await;
        assert_eq!(
            body["text"],
            format!("origin ok /{kind}?seq={seq}"),
            "the decrypted response body is recorded"
        );
    }
    let all = api.origin_exchanges().await;
    assert!(
        all.iter().all(|(k, _, _)| KINDS.contains(&k.as_str())),
        "loopback and untrusted requests stay out of Akev: {all:?}"
    );
    let now = target.max_seq();
    target
        .wait_round("loopback while attached", now, &["loopback"], |l| l.ok)
        .await;
    target
        .wait_round("untrusted while attached", now, &["untrusted"], |l| {
            !l.ok && l.detail.contains("SSLHandshakeException")
        })
        .await;
    let jvm = api.jvm().await;
    assert_eq!(jvm["active"], true);
    let me = jvm["metadata"]["processes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["pid"] == pid)
        .cloned()
        .unwrap();
    assert_eq!(me["active"], true);
    assert_eq!(me["attachedPort"], api.handle.proxy_addr().port());

    // ---- Restore: direct again. ----
    let r = api
        .call(
            reqwest::Method::POST,
            "/api/interceptors/jvm/deactivate",
            Some(json!({ "options": { "pid": pid } })),
        )
        .await;
    assert_eq!(r.status(), 204, "{}", r.text().await.unwrap_or_default());
    let restored_at = target.max_seq() + 1;
    target
        .wait_round(
            "direct requests after restoring",
            restored_at + 2,
            &KINDS,
            |l| l.ok && l.detail.starts_with("200 origin ok"),
        )
        .await;
    let late: Vec<_> = api
        .origin_exchanges()
        .await
        .into_iter()
        .filter(|(_, seq, _)| *seq > restored_at)
        .map(|(k, s, _)| (k, s))
        .collect();
    assert!(
        late.is_empty(),
        "requests after restoring went through Akev: {late:?}"
    );
    let jvm = api.jvm().await;
    assert_eq!(jvm["active"], false);
    let errors = target.errors.lock().unwrap().join("\n");
    assert!(
        errors.contains("[Akev] HTTP(S) traffic of this JVM now goes through Akev"),
        "{errors}"
    );
    assert!(errors.contains("[Akev] Interception removed"), "{errors}");

    // ---- Quitting Akev restores attached JVMs (they'd fail against a dead proxy otherwise). ----
    let r = api
        .call(
            reqwest::Method::POST,
            "/api/interceptors/jvm/activate",
            Some(json!({ "options": { "pid": pid } })),
        )
        .await;
    let result: Value = r.json().await.unwrap();
    assert_eq!(result["success"], true, "{result}");
    let reattached_at = target.max_seq() + 1;
    target
        .wait_round("intercepted again", reattached_at, &["huc"], |l| l.ok)
        .await;
    api.handle.shutdown().await;
    let stopped_at = target.max_seq() + 1;
    target
        .wait_round("direct after Akev quit", stopped_at + 1, &KINDS, |l| {
            l.ok && l.detail.starts_with("200 origin ok")
        })
        .await;
    drop(target);
}
