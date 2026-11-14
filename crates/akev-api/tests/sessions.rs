//! End-to-end tests of saved sessions, autosave & restore, other tools' imports and the
//! explicit proxy rebind — through the public REST API and the live stream, like the UI.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::routing::get;
use futures::StreamExt;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TIMEOUT: Duration = Duration::from_secs(15);
const CHARLES: &[u8] = include_bytes!("../../akev-store/tests/fixtures/charles.chlsj");
const MITMPROXY: &[u8] = include_bytes!("../../akev-store/tests/fixtures/mitmproxy.flows");

async fn upstream_server() -> SocketAddr {
    let app = Router::new().route(
        "/item/{n}",
        get(
            |axum::extract::Path(n): axum::extract::Path<u32>| async move {
                axum::Json(json!({ "item": n }))
            },
        ),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

/// Poll `check` until it holds (or fail after [`TIMEOUT`]).
async fn wait_until<F, Fut>(what: &str, check: F)
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while !check().await {
        assert!(tokio::time::Instant::now() < deadline, "timed out: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
}

impl Env {
    async fn start(options: AkevOptions) -> Env {
        let handle = start(options).await.expect("akev starts");
        let api = reqwest::Client::builder().no_proxy().build().unwrap();
        Env { handle, api }
    }

    async fn ephemeral() -> Env {
        Env::start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
    }

    /// A persistent instance (settings saved) in `dir`.
    async fn persistent(dir: &Path) -> Env {
        Env::start(AkevOptions {
            data_dir: Some(dir.to_path_buf()),
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut req = self
            .api
            .request(method, self.url(path))
            .bearer_auth(self.handle.token());
        if let Some(body) = body {
            req = req.json(&body);
        }
        req.send().await.unwrap()
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let r = self.request(method.clone(), path, body).await;
        let status = r.status();
        let text = r.text().await.unwrap();
        assert!(status.is_success(), "{method} {path} → {status}: {text}");
        serde_json::from_str(&text).unwrap()
    }

    async fn get(&self, path: &str) -> Value {
        self.json(reqwest::Method::GET, path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> Value {
        self.json(reqwest::Method::POST, path, Some(body)).await
    }

    async fn post_bytes(&self, path: &str, bytes: &'static [u8]) -> reqwest::Response {
        self.api
            .post(self.url(path))
            .bearer_auth(self.handle.token())
            .body(bytes)
            .send()
            .await
            .unwrap()
    }

    /// Send `n` requests through the proxy and wait until they're stored.
    async fn traffic(&self, upstream: SocketAddr, from: u32, n: u32) {
        let proxied = reqwest::Client::builder()
            .proxy(reqwest::Proxy::all(format!("http://{}", self.handle.proxy_addr())).unwrap())
            .build()
            .unwrap();
        for i in from..from + n {
            let r = proxied
                .get(format!("http://{upstream}/item/{i}"))
                .send()
                .await
                .unwrap();
            assert_eq!(r.status(), 200);
        }
        wait_until("exchanges stored", || async {
            let list = self.get("/api/exchanges?filter=status:200").await;
            list.as_array().unwrap().len() as u32 >= from - 1 + n
        })
        .await;
    }

    async fn paths(&self) -> Vec<String> {
        self.get("/api/exchanges")
            .await
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["path"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    async fn enable_autosave(&self, interval: u64, restore: bool) {
        let mut settings = self.get("/api/settings").await;
        settings["sessions"]["autosave"] = json!(true);
        settings["sessions"]["autosaveIntervalSecs"] = json!(interval);
        settings["sessions"]["restoreOnStartup"] = json!(restore);
        self.json(reqwest::Method::PUT, "/api/settings", Some(settings))
            .await;
    }

    async fn session(&self, id: &str) -> Option<Value> {
        let list = self.get("/api/sessions").await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == id)
            .cloned()
    }
}

#[tokio::test]
async fn save_list_download_open_rename_delete() {
    let env = Env::ephemeral().await;
    let upstream = upstream_server().await;
    env.traffic(upstream, 1, 3).await;
    assert_eq!(env.get("/api/sessions").await, json!([]));

    let all = env
        .post("/api/sessions", json!({ "name": "Checkout bug" }))
        .await;
    assert_eq!(all["id"], "checkout-bug");
    assert_eq!(all["name"], "Checkout bug");
    assert_eq!(all["kind"], "saved");
    assert_eq!(all["exchanges"], 3);
    assert!(all["size"].as_u64().unwrap() > 0);
    assert!(all["createdAt"].as_f64().unwrap() > 0.0);

    // A subset via filter ∩ ids.
    let ids: Vec<u64> = env
        .get("/api/exchanges")
        .await
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .collect();
    let some = env
        .post(
            "/api/sessions",
            json!({ "name": "Two", "ids": [ids[0], ids[2]], "filter": "path:/item/3" }),
        )
        .await;
    assert_eq!(some["exchanges"], 1);
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/sessions",
            Some(json!({ "name": "  " })),
        )
        .await;
    assert_eq!(r.status(), 400);
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/sessions",
            Some(json!({ "name": "x", "ids": [] })),
        )
        .await;
    assert_eq!(r.status(), 400);

    let list = env.get("/api/sessions").await;
    assert_eq!(list.as_array().unwrap().len(), 2);

    // Download: the session file, named after the session, importable anywhere.
    let r = env
        .request(reqwest::Method::GET, "/api/sessions/checkout-bug", None)
        .await;
    assert_eq!(r.status(), 200);
    let disposition = r.headers()["content-disposition"]
        .to_str()
        .unwrap()
        .to_string();
    assert!(
        disposition.contains("filename*=UTF-8''Checkout%20bug%2Eakev"),
        "{disposition}"
    );
    assert!(r.headers().get("content-encoding").is_none());
    let file = r.bytes().await.unwrap();
    assert_eq!(file.len() as u64, all["size"].as_u64().unwrap());
    let other = Env::ephemeral().await;
    let imported = other
        .api
        .post(other.url("/api/import/session"))
        .bearer_auth(other.handle.token())
        .body(file.to_vec())
        .send()
        .await
        .unwrap();
    assert_eq!(imported.status(), 200);
    other.handle.shutdown().await;

    // Open (replace): the current traffic is swapped for the session's.
    env.traffic(upstream, 4, 1).await;
    assert_eq!(env.paths().await.len(), 4);
    let opened = env
        .post(
            "/api/sessions/checkout-bug/open",
            json!({ "mode": "replace" }),
        )
        .await;
    assert_eq!(opened["ids"].as_array().unwrap().len(), 3);
    assert_eq!(env.paths().await, ["/item/1", "/item/2", "/item/3"]);
    // Append (the default mode is replace, so it's explicit).
    env.post("/api/sessions/two/open", json!({ "mode": "append" }))
        .await;
    assert_eq!(env.paths().await.len(), 4);
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/sessions/nope/open",
            Some(json!({})),
        )
        .await;
    assert_eq!(r.status(), 404);

    // Rename keeps the id; delete removes it.
    let renamed = env
        .json(
            reqwest::Method::PUT,
            "/api/sessions/two",
            Some(json!({ "name": "Only #3" })),
        )
        .await;
    assert_eq!(renamed["id"], "two");
    assert_eq!(renamed["name"], "Only #3");
    let r = env
        .request(reqwest::Method::DELETE, "/api/sessions/two", None)
        .await;
    assert_eq!(r.status(), 204);
    let r = env
        .request(reqwest::Method::DELETE, "/api/sessions/two", None)
        .await;
    assert_eq!(r.status(), 404);
    let r = env
        .request(reqwest::Method::GET, "/api/sessions/..%2Fsettings", None)
        .await;
    assert_eq!(r.status(), 404);
    assert!(env.session("checkout-bug").await.is_some());
    env.handle.shutdown().await;
}

#[tokio::test]
async fn autosave_writes_periodically_and_on_shutdown_then_restores() {
    let dir = tempfile::tempdir().unwrap();
    let upstream = upstream_server().await;

    // Run 1: autosave every second.
    let env = Env::persistent(dir.path()).await;
    env.enable_autosave(1, true).await;
    env.traffic(upstream, 1, 3).await;
    wait_until("periodic autosave", || async {
        env.session("autosave")
            .await
            .is_some_and(|s| s["exchanges"] == 3)
    })
    .await;
    let autosave = env.session("autosave").await.unwrap();
    assert_eq!(autosave["kind"], "autosave");
    assert_eq!(autosave["name"], "Autosave");
    // Unchanged traffic isn't written again.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert_eq!(
        env.session("autosave").await.unwrap()["updatedAt"],
        autosave["updatedAt"]
    );
    // A slow interval: what's captured now is only saved by the shutdown autosave.
    env.enable_autosave(3600, true).await;
    env.traffic(upstream, 4, 2).await;
    env.handle.shutdown().await;
    assert!(dir.path().join("sessions/autosave.akev").is_file());

    // Run 2: restored in the background; restored traffic sorts before new traffic.
    let env = Env::persistent(dir.path()).await;
    let mut ws = {
        let url = format!(
            "ws://{}/api/stream?token={}",
            env.handle.api_addr(),
            env.handle.token()
        );
        tokio_tungstenite::connect_async(url).await.unwrap().0
    };
    wait_until("restored", || async {
        env.get("/api/status").await["restoredSession"]["exchanges"] == 5
    })
    .await;
    let status = env.get("/api/status").await;
    assert_eq!(status["restoredSession"]["name"], "Autosave");
    assert_eq!(status["restoredSession"]["id"], "autosave");
    assert!(status["restoredSession"]["savedAt"].as_f64().unwrap() > 0.0);
    assert_eq!(
        env.paths().await,
        ["/item/1", "/item/2", "/item/3", "/item/4", "/item/5"]
    );
    // The stream's hello (or a session-restored message) tells the UI.
    let mut told = false;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while !told && tokio::time::Instant::now() < deadline {
        let Ok(Some(Ok(Message::Text(t)))) =
            tokio::time::timeout(Duration::from_millis(500), ws.next()).await
        else {
            continue;
        };
        let m: Value = serde_json::from_str(t.as_str()).unwrap();
        told = (m["type"] == "hello" && m["status"]["restoredSession"]["exchanges"] == 5)
            || (m["type"] == "session-restored" && m["session"]["exchanges"] == 5);
    }
    assert!(told, "the stream announces the restore");
    env.traffic(upstream, 6, 1).await;
    assert_eq!(env.paths().await.last().unwrap(), "/item/6");
    env.handle.shutdown().await;

    // Run 3: restore off — the first autosave keeps the previous run's as "previous".
    let env = Env::persistent(dir.path()).await;
    let mut settings = env.get("/api/settings").await;
    assert_eq!(settings["sessions"]["autosave"], true);
    settings["sessions"]["restoreOnStartup"] = json!(false);
    settings["sessions"]["autosaveIntervalSecs"] = json!(1);
    env.json(reqwest::Method::PUT, "/api/settings", Some(settings))
        .await;
    env.handle.shutdown().await;
    let env = Env::persistent(dir.path()).await;
    assert!(
        env.get("/api/status")
            .await
            .get("restoredSession")
            .is_none()
    );
    assert!(env.paths().await.is_empty());
    env.traffic(upstream, 1, 1).await;
    wait_until("rotation + autosave", || async {
        env.session("autosave")
            .await
            .is_some_and(|s| s["exchanges"] == 1)
    })
    .await;
    let previous = env.session("autosave-previous").await.unwrap();
    assert_eq!(previous["exchanges"], 6);
    assert_eq!(previous["name"], "Autosave (previous run)");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn autosave_settings_are_validated() {
    let env = Env::ephemeral().await;
    let mut settings = env.get("/api/settings").await;
    assert_eq!(
        settings["sessions"],
        // On by default, bounded (see `docs/PRODUCT.md`).
        json!({
            "autosave": true,
            "autosaveIntervalSecs": 60,
            "restoreOnStartup": true,
            "autosaveMaxExchanges": 20000,
            "autosaveMaxBytes": 268435456u64,
        })
    );
    settings["sessions"]["autosaveIntervalSecs"] = json!(0);
    let r = env
        .request(reqwest::Method::PUT, "/api/settings", Some(settings))
        .await;
    assert_eq!(r.status(), 400);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn charles_and_mitmproxy_imports() {
    let env = Env::ephemeral().await;
    let r = env.post_bytes("/api/import/charles", CHARLES).await;
    assert_eq!(r.status(), 200);
    let ids: Value = r.json().await.unwrap();
    assert_eq!(ids["ids"].as_array().unwrap().len(), 7);
    let r = env.post_bytes("/api/import/mitmproxy", MITMPROXY).await;
    assert_eq!(r.status(), 200);
    let ids: Value = r.json().await.unwrap();
    let ws = ids["ids"].as_array().unwrap()[5].as_u64().unwrap();
    let messages = env.get(&format!("/api/exchanges/{ws}/messages")).await;
    assert_eq!(messages[1]["text"], "echo: hello");
    // Pages: the oldest, the newest (`tail`), and the ones before a sequence number.
    let first = env
        .get(&format!("/api/exchanges/{ws}/messages?limit=1"))
        .await;
    assert_eq!(first.as_array().unwrap().len(), 1);
    assert_eq!(first[0]["seq"], messages[0]["seq"]);
    let all = messages.as_array().unwrap();
    assert!(all.len() >= 2);
    let newest = env
        .get(&format!("/api/exchanges/{ws}/messages?limit=1&tail=1"))
        .await;
    assert_eq!(newest.as_array().unwrap().len(), 1);
    assert_eq!(newest[0], all[all.len() - 1]);
    let second_seq = all[1]["seq"].as_u64().unwrap();
    let before = env
        .get(&format!(
            "/api/exchanges/{ws}/messages?before={second_seq}&tail=true&limit=10"
        ))
        .await;
    assert_eq!(before.as_array().unwrap().len(), 1);
    assert_eq!(before[0], all[0]);
    let detail = env.get(&format!("/api/exchanges/{ws}")).await;
    assert_eq!(detail["summary"]["kind"], "web-socket");
    // Wrong formats are refused with a readable error.
    let r = env.post_bytes("/api/import/charles", MITMPROXY).await;
    assert_eq!(r.status(), 400);
    let body: Value = r.json().await.unwrap();
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Charles"),
        "{body}"
    );
    let r = env.post_bytes("/api/import/mitmproxy", CHARLES).await;
    assert_eq!(r.status(), 400);
    let r = env.post_bytes("/api/import/mitmproxy", b"").await;
    assert_eq!(r.status(), 400);
    assert_eq!(
        env.get("/api/exchanges").await.as_array().unwrap().len(),
        13
    );
    env.handle.shutdown().await;
}

#[tokio::test]
async fn rebind_moves_the_proxy_to_the_preferred_port_after_a_fallback() {
    // Occupy a port so the proxy falls back to another one.
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let wanted = blocker.local_addr().unwrap().port();
    let env = Env::start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(wanted),
        api_port: 0,
        ..AkevOptions::default()
    })
    .await;
    let fallback = env.handle.proxy_addr().port();
    assert_ne!(fallback, wanted, "the busy port can't be used");

    // Still busy: 409, and the proxy keeps listening where it was.
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/proxy/rebind",
            Some(json!({ "port": wanted })),
        )
        .await;
    assert_eq!(r.status(), 409);
    assert_eq!(env.handle.proxy_addr().port(), fallback);
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/proxy/rebind",
            Some(json!({ "port": 0 })),
        )
        .await;
    assert_eq!(r.status(), 400);
    let r = env
        .request(
            reqwest::Method::POST,
            "/api/proxy/rebind",
            Some(json!({ "port": wanted, "bindAddress": "not-an-ip" })),
        )
        .await;
    assert_eq!(r.status(), 400);

    // Freed: the explicit rebind always moves it, even though no setting changed.
    drop(blocker);
    let status = env
        .post(
            "/api/proxy/rebind",
            json!({ "port": wanted, "bindAddress": "127.0.0.1" }),
        )
        .await;
    assert_eq!(status["port"], wanted);
    assert_eq!(status["bindAddress"], "127.0.0.1");
    assert_eq!(env.handle.proxy_addr().port(), wanted);
    assert_eq!(env.get("/api/status").await["proxy"]["port"], wanted);
    // Traffic flows through the new port.
    let upstream = upstream_server().await;
    env.traffic(upstream, 1, 1).await;
    env.handle.shutdown().await;
}
