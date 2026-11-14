//! End-to-end tests: a real Akev instance (ephemeral, random ports), a local upstream HTTP
//! server, and clients sending traffic through the proxy. Assertions go through the public
//! REST API, the WebSocket stream and the MCP endpoint, exactly like the UI and agents do.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::routing::{any, get, post};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
    proxied: reqwest::Client,
    upstream: SocketAddr,
}

async fn upstream_server() -> SocketAddr {
    let app = Router::new()
        .route(
            "/hello",
            get(|| async { axum::Json(json!({ "message": "hello world" })) }),
        )
        .route("/echo", post(|body: String| async move { body }))
        .route(
            "/status/{code}",
            get(
                |axum::extract::Path(code): axum::extract::Path<u16>| async move {
                    (
                        http::StatusCode::from_u16(code).unwrap_or(http::StatusCode::OK),
                        format!("status {code}"),
                    )
                },
            ),
        )
        .fallback(any(|uri: http::Uri| async move {
            format!("upstream saw {}", uri.path())
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

async fn setup() -> Env {
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    })
    .await
    .expect("akev starts");
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    let upstream = upstream_server().await;
    Env {
        handle,
        api,
        proxied,
        upstream,
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    fn upstream_url(&self, path: &str) -> String {
        format!("http://{}{path}", self.upstream)
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.api
            .get(self.url(path))
            .bearer_auth(self.handle.token())
            .send()
            .await
            .unwrap()
    }

    async fn get_json(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert!(r.status().is_success(), "GET {path} → {}", r.status());
        r.json().await.unwrap()
    }

    async fn send(&self, method: reqwest::Method, path: &str, body: Value) -> reqwest::Response {
        self.api
            .request(method, self.url(path))
            .bearer_auth(self.handle.token())
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    /// Poll the exchange list until one matches `pred`.
    async fn wait_for_exchange(&self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let list = self.get_json("/api/exchanges").await;
            if let Some(found) = list.as_array().unwrap().iter().find(|s| pred(s)) {
                return found.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting for exchange; have {list}"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn ws(
        &self,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let url = format!(
            "ws://{}/api/stream?token={}",
            self.handle.api_addr(),
            self.handle.token()
        );
        let (ws, _) = tokio_tungstenite::connect_async(url)
            .await
            .expect("websocket connects");
        ws
    }
}

fn is_complete(s: &Value) -> bool {
    s["state"] == "complete"
}

async fn next_json<S>(ws: &mut S) -> Value
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let msg = tokio::time::timeout(TIMEOUT, ws.next())
            .await
            .expect("stream message in time")
            .expect("open")
            .unwrap();
        if let Message::Text(t) = msg {
            return serde_json::from_str(t.as_str()).unwrap();
        }
    }
}

#[tokio::test]
async fn auth_and_origin_rules() {
    let env = setup().await;
    let r = env.api.get(env.url("/api/status")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");

    let status = env.get_json("/api/status").await;
    assert_eq!(status["proxy"]["port"], env.handle.proxy_addr().port());
    assert_eq!(status["apiUrl"], env.handle.api_url());
    assert!(status["ca"]["sha256Fingerprint"].is_string());

    let r = env
        .api
        .get(env.url("/api/status"))
        .bearer_auth(env.handle.token())
        .header("origin", "https://evil.example")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 403);

    let r = env
        .api
        .get(env.url(&format!("/api/status?token={}", env.handle.token())))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);

    // `GET /?token=` serves the UI, which keeps the token in (port-isolated) web storage.
    // The token never goes into a cookie: cookies aren't isolated by port, so every other
    // service on 127.0.0.1 (and Akev's own proxy port, which records it) would receive it.
    let no_redirect = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let token = env.handle.token().to_string();
    let r = no_redirect.get(env.handle.ui_url()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    assert_eq!(r.headers()["cache-control"], "no-store");
    assert_eq!(r.headers()["referrer-policy"], "no-referrer");
    let cookies: Vec<String> = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert!(cookies.iter().all(|c| !c.contains(&token)), "{cookies:?}");
    let r = no_redirect
        .get(env.url("/api/status"))
        .header("cookie", format!("akev_token={token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    // A wrong token is dropped from the URL.
    let r = no_redirect
        .get(env.url("/?token=wrong"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 303);
    assert_eq!(r.headers()["location"], "/");
    assert!(r.headers().get("set-cookie").is_none());

    // The UI itself is public.
    let r = env.api.get(env.url("/")).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert!(
        r.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    env.handle.shutdown().await;
}

#[tokio::test]
async fn captures_traffic_with_details_bodies_and_snippets() {
    let env = setup().await;
    let resp = env
        .proxied
        .get(env.upstream_url("/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.json::<Value>().await.unwrap()["message"],
        "hello world"
    );

    let s = env
        .wait_for_exchange(|s| s["path"] == "/hello" && is_complete(s))
        .await;
    assert_eq!(s["status"], 200);
    let id = s["id"].as_u64().unwrap();

    let detail = env.get_json(&format!("/api/exchanges/{id}")).await;
    assert_eq!(detail["request"]["method"], "GET");
    assert_eq!(detail["response"]["status"], 200);

    let body = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert_eq!(body["kind"], "json");
    assert!(body["text"].as_str().unwrap().contains("hello world"));
    assert!(body["wireSize"].as_u64().unwrap() > 0);
    assert_eq!(body["captureTruncated"], false);

    let raw = env
        .get(&format!(
            "/api/exchanges/{id}/body/response/raw?decoded=1&download=1"
        ))
        .await;
    assert_eq!(raw.status(), 200);
    assert!(
        raw.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("attachment")
    );
    assert!(
        raw.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("sandbox")
    );
    assert!(raw.text().await.unwrap().contains("hello world"));

    let snippet = env
        .get_json(&format!("/api/exchanges/{id}/snippet?target=curl"))
        .await;
    assert!(
        snippet["code"]
            .as_str()
            .unwrap()
            .contains(&env.upstream_url("/hello"))
    );

    let editable = env
        .get_json(&format!("/api/exchanges/{id}/editable/request"))
        .await;
    assert_eq!(editable["method"], "GET");

    // Filtering through the REST API.
    let filtered = env
        .get_json("/api/exchanges?filter=status%3A200%20path%3A%2Fhello")
        .await;
    assert!(filtered.as_array().unwrap().iter().any(|e| e["id"] == id));
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/filter/query",
            json!({ "query": "status:404" }),
        )
        .await;
    let ids: Value = r.json().await.unwrap();
    assert!(!ids["ids"].as_array().unwrap().iter().any(|v| v == id));
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/filter/validate",
            json!({ "query": "status:" }),
        )
        .await;
    let v: Value = r.json().await.unwrap();
    assert!(v["ok"].is_boolean());
    let r = env.get("/api/exchanges?filter=%28%28").await;
    assert_eq!(r.status(), 400);

    // Pin + note, then delete.
    let r = env
        .send(
            reqwest::Method::PATCH,
            "/api/exchanges",
            json!({ "ids": [id], "pinned": true, "note": "look" }),
        )
        .await;
    assert_eq!(r.status(), 204);
    let detail = env.get_json(&format!("/api/exchanges/{id}")).await;
    assert_eq!(detail["summary"]["pinned"], true);
    assert_eq!(detail["note"], "look");
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/exchanges/delete",
            json!({ "ids": [id] }),
        )
        .await;
    assert_eq!(r.status(), 204);
    assert_eq!(env.get(&format!("/api/exchanges/{id}")).await.status(), 404);
    env.handle.shutdown().await;
}

/// A HAR with `n` minimal entries.
fn har_with(n: usize) -> Vec<u8> {
    let entries: Vec<Value> = (0..n)
        .map(|i| {
            json!({
                "startedDateTime": "2026-01-02T03:04:05.000Z",
                "time": 1,
                "request": {"method": "GET", "url": format!("http://paged.example/{i}"),
                    "httpVersion": "HTTP/1.1", "headers": [], "queryString": [], "cookies": [],
                    "headersSize": -1, "bodySize": 0},
                "response": {"status": 200, "statusText": "OK", "httpVersion": "HTTP/1.1",
                    "headers": [], "cookies": [], "content": {"size": 0, "mimeType": "text/plain"},
                    "redirectURL": "", "headersSize": -1, "bodySize": 0},
                "cache": {},
                "timings": {"send": 0, "wait": 1, "receive": 0}
            })
        })
        .collect();
    serde_json::to_vec(&json!({"log": {"version": "1.2", "creator": {"name": "t", "version": "1"}, "entries": entries}}))
        .unwrap()
}

#[tokio::test]
async fn websocket_hello_is_paged_newest_first() {
    let env = setup().await;
    let n = 6_500;
    let r = env
        .api
        .post(env.url("/api/import/har"))
        .bearer_auth(env.handle.token())
        .body(har_with(n))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let total = env.get_json("/api/status").await["stats"]["exchanges"]
        .as_u64()
        .unwrap() as usize;
    assert!(total >= n);

    let mut ws = env.ws().await;
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    let first = hello["summaries"].as_array().unwrap();
    assert_eq!(first.len(), 2_000, "only the newest page in the hello");
    assert_eq!(hello["remaining"], total - 2_000);
    let ids = |v: &[Value]| {
        v.iter()
            .map(|s| s["id"].as_u64().unwrap())
            .collect::<Vec<_>>()
    };
    let mut seen = ids(first);
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "id order");
    let mut oldest = seen[0];
    let mut remaining = hello["remaining"].as_u64().unwrap();
    let mut chunks = 0;
    while remaining > 0 {
        let m = next_json(&mut ws).await;
        if m["type"] != "hello-more" {
            continue;
        }
        chunks += 1;
        let chunk = ids(m["summaries"].as_array().unwrap());
        assert!(chunk.len() <= 5_000);
        assert!(chunk.windows(2).all(|w| w[0] < w[1]));
        assert!(
            chunk.last().is_some_and(|last| *last < oldest),
            "each chunk is older than everything before it"
        );
        oldest = chunk[0];
        seen.extend(chunk);
        remaining = m["remaining"].as_u64().unwrap();
    }
    assert_eq!(chunks, 1, "4,500 older summaries fit one chunk");
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), total, "every exchange arrives exactly once");

    // The new limits are reported with the other counters.
    let stats = env.get_json("/api/status").await["stats"].clone();
    for key in [
        "evicted",
        "droppedBodies",
        "diskDroppedBodies",
        "passedThrough",
        "autoReleased",
        "spillDiskBytes",
    ] {
        assert!(stats[key].is_u64(), "{key} in {stats}");
    }
    env.handle.shutdown().await;
}

#[tokio::test]
async fn websocket_stream_hello_batches_and_server_side_filter() {
    let env = setup().await;
    let mut ws = env.ws().await;
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    assert!(hello["summaries"].is_array());
    assert!(hello["rules"]["items"].is_array());
    assert_eq!(
        hello["status"]["proxy"]["port"],
        env.handle.proxy_addr().port()
    );

    ws.send(Message::Text(json!({ "type": "ping" }).to_string().into()))
        .await
        .unwrap();
    ws.send(Message::Text(
        json!({ "type": "set-filter", "query": "path:/match", "seq": 1 })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();

    let mut got_pong = false;
    let mut got_filter_result = false;
    while !(got_pong && got_filter_result) {
        let m = next_json(&mut ws).await;
        match m["type"].as_str() {
            Some("pong") => got_pong = true,
            Some("filter-result") => {
                assert_eq!(m["seq"], 1);
                assert_eq!(m["ids"], json!([]));
                got_filter_result = true;
            }
            _ => {}
        }
    }

    env.proxied
        .get(env.upstream_url("/match"))
        .send()
        .await
        .unwrap();
    env.proxied
        .get(env.upstream_url("/other"))
        .send()
        .await
        .unwrap();

    let mut seen_paths = std::collections::HashMap::new();
    let mut filtered_in = std::collections::HashSet::new();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no filtered delta for /match"
        );
        let m = next_json(&mut ws).await;
        if m["type"] != "exchanges" {
            continue;
        }
        // Each id at most once per batch.
        let mut ids: Vec<u64> = m["added"]
            .as_array()
            .unwrap()
            .iter()
            .chain(m["updated"].as_array().unwrap())
            .map(|s| s["id"].as_u64().unwrap())
            .collect();
        let before = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(before, ids.len(), "duplicate id in one batch: {m}");
        for s in m["added"]
            .as_array()
            .unwrap()
            .iter()
            .chain(m["updated"].as_array().unwrap())
        {
            seen_paths.insert(
                s["id"].as_u64().unwrap(),
                s["path"].as_str().unwrap().to_string(),
            );
        }
        let filtered = &m["filtered"];
        assert!(
            filtered.is_object(),
            "batches carry `filtered` once a filter is set"
        );
        for id in filtered["add"].as_array().unwrap() {
            filtered_in.insert(id.as_u64().unwrap());
        }
        let match_id = seen_paths
            .iter()
            .find(|(_, p)| p.as_str() == "/match")
            .map(|(id, _)| *id);
        let other_id = seen_paths
            .iter()
            .find(|(_, p)| p.as_str() == "/other")
            .map(|(id, _)| *id);
        if let (Some(mi), Some(oi)) = (match_id, other_id)
            && filtered_in.contains(&mi)
        {
            assert!(!filtered_in.contains(&oi));
            break;
        }
    }

    // Invalid filter keeps the previous one and reports an error.
    ws.send(Message::Text(
        json!({ "type": "set-filter", "query": "((", "seq": 2 })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "filter-result" {
            assert_eq!(m["seq"], 2);
            assert!(m["error"]["message"].is_string());
            break;
        }
    }

    // Clearing is broadcast.
    let r = env
        .send(reqwest::Method::POST, "/api/exchanges/clear", json!({}))
        .await;
    assert_eq!(r.status(), 204);
    loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "cleared" {
            break;
        }
    }
    env.handle.shutdown().await;
}

fn breakpoint_rules(path: &str) -> Value {
    json!({
        "version": 1,
        "items": [{
            "type": "rule",
            "id": "bp-1",
            "matchers": [{ "type": "path", "pattern": { "op": "equals", "value": path } }],
            "action": { "type": "breakpoint", "request": true, "response": false }
        }]
    })
}

/// With recording paused, a breakpoint still holds matching requests. They aren't stored, so
/// the pause itself must carry everything a client needs to show, edit and resume them.
#[tokio::test]
async fn unrecorded_requests_still_pause_and_resume() {
    let env = setup().await;
    let r = env
        .send(reqwest::Method::PUT, "/api/rules", breakpoint_rules("/bp"))
        .await;
    assert_eq!(r.status(), 200);
    let r = env
        .send(
            reqwest::Method::PUT,
            "/api/capture",
            json!({ "recording": false }),
        )
        .await;
    assert_eq!(r.status(), 200);

    let client = env.proxied.clone();
    let url = env.upstream_url("/bp");
    let pending =
        tokio::spawn(async move { client.get(url).send().await.unwrap().text().await.unwrap() });
    let paused = loop {
        let list = env.get_json("/api/breakpoints").await;
        if let Some(p) = list.as_array().and_then(|l| l.first()) {
            break p.clone();
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    assert_eq!(paused["stage"], "request");
    assert_eq!(paused["request"]["url"], env.upstream_url("/bp"));
    assert!(paused["pausedAt"].as_i64().unwrap() > 0, "{paused}");
    let id = paused["id"].as_u64().unwrap();
    let r = env.get(&format!("/api/exchanges/{id}")).await;
    assert_eq!(r.status(), 404, "not recorded");

    let mut request = paused["request"].clone();
    request["url"] = json!(env.upstream_url("/edited"));
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/breakpoints/{id}/resume"),
            json!({ "action": "continue", "request": request }),
        )
        .await;
    assert_eq!(r.status(), 204);
    let body = tokio::time::timeout(TIMEOUT, pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body, "upstream saw /edited");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn breakpoint_pause_and_resume() {
    let env = setup().await;
    let mut ws = env.ws().await;
    let _hello = next_json(&mut ws).await;
    let r = env
        .send(reqwest::Method::PUT, "/api/rules", breakpoint_rules("/bp"))
        .await;
    assert_eq!(r.status(), 200);
    let errors: Value = r.json().await.unwrap();
    assert_eq!(errors["errors"], json!([]));

    let client = env.proxied.clone();
    let url = env.upstream_url("/bp");
    let pending =
        tokio::spawn(async move { client.get(url).send().await.unwrap().text().await.unwrap() });

    let paused = loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "paused" {
            break m["exchange"].clone();
        }
    };
    assert_eq!(paused["stage"], "request");
    assert!(paused["pausedAt"].as_i64().unwrap() > 0, "{paused}");
    let id = paused["id"].as_u64().unwrap();
    let list = env.get_json("/api/breakpoints").await;
    assert_eq!(list[0]["id"], id);

    // Edit the URL while paused: the upstream sees the new path.
    let mut request = paused["request"].clone();
    request["url"] = json!(env.upstream_url("/edited"));
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/breakpoints/{id}/resume"),
            json!({ "action": "continue", "request": request }),
        )
        .await;
    assert_eq!(r.status(), 204);
    let body = tokio::time::timeout(TIMEOUT, pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body, "upstream saw /edited");
    loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "resumed" {
            assert_eq!(m["id"], id);
            break;
        }
    }
    assert_eq!(env.get_json("/api/breakpoints").await, json!([]));
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/breakpoints/{id}/resume"),
            json!({}),
        )
        .await;
    assert_eq!(r.status(), 404);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn response_breakpoint_edit_and_resume_all() {
    use base64::Engine;
    let b64 = |s: &str| base64::engine::general_purpose::STANDARD.encode(s);
    let env = setup().await;
    let mut ws = env.ws().await;
    let _hello = next_json(&mut ws).await;
    let rules = json!({
        "version": 1,
        "items": [{
            "type": "rule",
            "id": "bp-res",
            "matchers": [{ "type": "path", "pattern": { "op": "equals", "value": "/echo" } }],
            "action": { "type": "breakpoint", "request": false, "response": true }
        }]
    });
    let r = env.send(reqwest::Method::PUT, "/api/rules", rules).await;
    assert_eq!(r.status(), 200);

    // 1. Edit the response while paused at the response stage.
    let client = env.proxied.clone();
    let url = env.upstream_url("/echo");
    let pending = tokio::spawn(async move {
        let r = client.post(url).body("ping").send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    });
    let paused = loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "paused" {
            break m["exchange"].clone();
        }
    };
    assert_eq!(paused["stage"], "response");
    assert_eq!(paused["response"]["body"], b64("ping"));
    assert_eq!(paused["request"]["method"], "POST");
    let id = paused["id"].as_u64().unwrap();
    let mut response = paused["response"].clone();
    response["status"] = json!(201);
    response["statusMessage"] = json!("Created");
    response["body"] = json!(b64("edited"));
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/breakpoints/{id}/resume"),
            json!({ "action": "continue", "response": response }),
        )
        .await;
    assert_eq!(r.status(), 204);
    let (status, body) = tokio::time::timeout(TIMEOUT, pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((status, body.as_str()), (201, "edited"));

    // 2. `respond` is a request-stage action only; resume-all continues unchanged.
    let client = env.proxied.clone();
    let url = env.upstream_url("/echo");
    let pending = tokio::spawn(async move {
        let r = client.post(url).body("pong").send().await.unwrap();
        (r.status().as_u16(), r.text().await.unwrap())
    });
    let id = loop {
        let m = next_json(&mut ws).await;
        if m["type"] == "paused" {
            break m["exchange"]["id"].as_u64().unwrap();
        }
    };
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/breakpoints/{id}/resume"),
            json!({ "action": "respond", "response": { "status": 200, "statusMessage": "OK", "headers": [], "body": "" } }),
        )
        .await;
    assert_eq!(r.status(), 400, "respond is invalid at the response stage");
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/breakpoints/resume-all",
            json!({}),
        )
        .await;
    assert_eq!(r.status(), 204);
    let (status, body) = tokio::time::timeout(TIMEOUT, pending)
        .await
        .unwrap()
        .unwrap();
    assert_eq!((status, body.as_str()), (200, "pong"));
    assert_eq!(env.get_json("/api/breakpoints").await, json!([]));
    env.handle.shutdown().await;
}

/// Read stream messages until one of type `ty` satisfies `pred`.
async fn wait_message<S>(ws: &mut S, ty: &str, pred: impl Fn(&Value) -> bool) -> Value
where
    S: futures::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        assert!(
            tokio::time::Instant::now() < deadline,
            "no `{ty}` message arrived"
        );
        let m = next_json(ws).await;
        if m["type"] == ty && pred(&m) {
            return m;
        }
    }
}

#[tokio::test]
async fn stream_broadcasts_control_messages() {
    let env = setup().await;
    let mut ws = env.ws().await;
    let hello = next_json(&mut ws).await;
    assert_eq!(hello["type"], "hello");
    // The initial snapshot is followed by the recording state.
    wait_message(&mut ws, "capture", |m| m["recording"] == true).await;

    let rules = json!({
        "version": 1,
        "items": [{
            "type": "rule", "id": "count-me",
            "matchers": [{ "type": "path", "pattern": { "op": "equals", "value": "/counted" } }],
            "action": { "type": "mock", "status": 200, "headers": [], "body": { "type": "text", "text": "ok" } }
        }]
    });
    env.send(reqwest::Method::PUT, "/api/rules", rules).await;
    wait_message(&mut ws, "rules", |m| {
        m["rules"]["items"][0]["id"] == "count-me"
    })
    .await;

    env.proxied
        .get(env.upstream_url("/counted"))
        .send()
        .await
        .unwrap();
    wait_message(&mut ws, "rule-stats", |m| {
        m["stats"]
            .as_array()
            .into_iter()
            .flatten()
            .any(|s| s["ruleId"] == "count-me" && s["matches"].as_u64() >= Some(1))
    })
    .await;

    let r = env
        .send(
            reqwest::Method::PATCH,
            "/api/settings/ui",
            json!({ "layout": "wide" }),
        )
        .await;
    assert_eq!(r.status(), 204);
    wait_message(&mut ws, "settings", |m| {
        m["settings"]["ui"]["layout"] == "wide"
    })
    .await;
    // A PUT without `ui` keeps the UI preferences.
    let mut settings = env.get_json("/api/settings").await;
    settings.as_object_mut().unwrap().remove("ui");
    settings["capture"]["ignoreHosts"] = json!(["noise.example"]);
    let r = env
        .send(reqwest::Method::PUT, "/api/settings", settings)
        .await;
    assert_eq!(r.status(), 200);
    let applied = wait_message(&mut ws, "settings", |m| {
        m["settings"]["capture"]["ignoreHosts"][0] == "noise.example"
    })
    .await;
    assert_eq!(applied["settings"]["ui"]["layout"], "wide");

    env.send(
        reqwest::Method::PUT,
        "/api/capture",
        json!({ "recording": false }),
    )
    .await;
    wait_message(&mut ws, "capture", |m| m["recording"] == false).await;

    // Periodic stats (every 2 s).
    let stats = wait_message(&mut ws, "stats", |_| true).await;
    assert!(stats["stats"]["exchanges"].is_number());
    env.handle.shutdown().await;
}

#[tokio::test]
async fn busy_proxy_port_falls_back_with_a_notice() {
    // Occupy a port; the proxy documents falling back to the next free one.
    let blocker = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let busy = blocker.local_addr().unwrap().port();
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(busy),
        api_port: 0,
        ..pro::options()
    })
    .await
    .expect("akev starts even though the preferred port is busy");
    let actual = handle.proxy_addr().port();
    assert_ne!(actual, busy);
    let url = format!(
        "ws://{}/api/stream?token={}",
        handle.api_addr(),
        handle.token()
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let notice = wait_message(&mut ws, "notice", |m| {
        m["message"]
            .as_str()
            .is_some_and(|t| t.contains(&busy.to_string()))
    })
    .await;
    assert_eq!(notice["level"], "warning");
    assert!(
        notice["message"]
            .as_str()
            .unwrap()
            .contains(&actual.to_string())
    );
    drop(blocker);
    handle.shutdown().await;
}

#[tokio::test]
async fn state_persists_across_restarts_and_corrupt_files_recover() {
    let dir = tempfile::tempdir().unwrap();
    let opts = || AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    };
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let call = |h: &AkevHandle, method: reqwest::Method, path: &str| {
        api.request(method, format!("{}{path}", h.api_url()))
            .bearer_auth(h.token())
    };

    let first = start(opts()).await.unwrap();
    assert!(dir.path().join("daemon.json").exists());
    let r = call(&first, reqwest::Method::PATCH, "/api/settings/ui")
        .json(&json!({ "theme": "oled" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 204);
    let rules = json!({"version": 1, "items": [{"type": "rule", "id": "keep-me", "matchers": [], "action": {"type": "passthrough"}}]});
    let r = call(&first, reqwest::Method::PUT, "/api/rules")
        .json(&rules)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    first.shutdown().await;
    assert!(!dir.path().join("daemon.json").exists());

    let second = start(opts()).await.unwrap();
    let settings: Value = call(&second, reqwest::Method::GET, "/api/settings")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["ui"]["theme"], "oled");
    let loaded: Value = call(&second, reqwest::Method::GET, "/api/rules")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(loaded["items"][0]["id"], "keep-me");
    second.shutdown().await;

    std::fs::write(dir.path().join("settings.json"), b"{ definitely not json").unwrap();
    let third = start(opts()).await.unwrap();
    let settings: Value = call(&third, reqwest::Method::GET, "/api/settings")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(settings["ui"].is_null(), "defaults after corruption");
    assert_eq!(
        std::fs::read(dir.path().join("settings.json.bak")).unwrap(),
        b"{ definitely not json"
    );
    third.shutdown().await;
}

#[tokio::test]
async fn rules_put_serves_mock_and_counts_matches() {
    let env = setup().await;
    let rules = json!({
        "version": 1,
        "items": [{
            "type": "rule",
            "id": "mock-1",
            "title": "Teapot",
            "matchers": [{ "type": "path", "pattern": { "op": "equals", "value": "/mocked" } }],
            "action": { "type": "mock", "status": 418, "headers": [["content-type", "text/plain"]], "body": { "type": "text", "text": "mocked!" } }
        }]
    });
    let r = env
        .send(reqwest::Method::PUT, "/api/rules", rules.clone())
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(env.get_json("/api/rules").await["items"][0]["id"], "mock-1");

    let resp = env
        .proxied
        .get(env.upstream_url("/mocked"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 418);
    assert_eq!(resp.text().await.unwrap(), "mocked!");

    let s = env
        .wait_for_exchange(|s| s["path"] == "/mocked" && is_complete(s))
        .await;
    assert_eq!(s["mocked"], true);
    assert_eq!(s["rule"], "mock-1");

    let stats = env.get_json("/api/rules/stats").await;
    let m = stats
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["ruleId"] == "mock-1")
        .unwrap();
    assert!(m["matches"].as_u64().unwrap() >= 1);

    // Dry run doesn't count.
    let r = env
        .send(reqwest::Method::POST, "/api/rules/test", json!({ "request": { "method": "GET", "url": env.upstream_url("/mocked"), "headers": [], "body": "" } }))
        .await;
    let test: Value = r.json().await.unwrap();
    assert_eq!(test["terminalRule"], "mock-1");
    assert_eq!(test["action"]["type"], "mock");

    let export = env.get("/api/rules/export").await;
    assert!(
        export.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("akev-rules.json")
    );
    let imported = env
        .send(reqwest::Method::POST, "/api/rules/import", rules)
        .await;
    assert_eq!(imported.status(), 200);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn har_and_session_export_import() {
    let env = setup().await;
    env.proxied
        .get(env.upstream_url("/hello"))
        .send()
        .await
        .unwrap();
    env.proxied
        .post(env.upstream_url("/echo"))
        .body("ping")
        .send()
        .await
        .unwrap();
    env.wait_for_exchange(|s| s["path"] == "/echo" && is_complete(s))
        .await;

    let har = env.get("/api/export/har?filter=path%3A%2Fecho").await;
    assert!(
        har.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains(".har")
    );
    let har: Value = har.json().await.unwrap();
    let entries = har["log"]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1);
    assert!(
        entries[0]["request"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/echo")
    );

    // Streamed (no Content-Length up front), byte for byte the pretty-printed export.
    let all = env.get("/api/export/har").await;
    assert_eq!(all.status(), 200);
    assert!(all.headers().get("content-length").is_none());
    let all = all.bytes().await.unwrap();
    let expected = serde_json::to_vec_pretty(&env.handle.store().export_har(None)).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&all),
        String::from_utf8_lossy(&expected)
    );
    let all: Value = serde_json::from_slice(&all).unwrap();
    assert!(all["log"]["entries"].as_array().unwrap().len() >= 2);

    // MCP export to a file is streamed into it (in the exports folder), with the same bytes
    // (nothing here is secret, so redaction changes nothing).
    let path = env.handle.data_dir().join("exports").join("mcp.har");
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"akev_export_har","arguments":{"path": "mcp.har"}
    }});
    let out = env.handle.mcp(call.to_string().as_bytes()).await.unwrap();
    assert_ne!(out["result"]["isError"], true, "{out}");
    assert_eq!(std::fs::read(&path).unwrap(), expected);
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    let info: Value = serde_json::from_str(text).unwrap();
    assert_eq!(info["bytes"], expected.len());
    assert_eq!(
        info["exchanges"],
        all["log"]["entries"].as_array().unwrap().len()
    );

    let har_bytes = serde_json::to_vec(&har).unwrap();
    let r = env
        .api
        .post(env.url("/api/import/har"))
        .bearer_auth(env.handle.token())
        .body(har_bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let ids: Value = r.json().await.unwrap();
    assert_eq!(ids["ids"].as_array().unwrap().len(), 1);

    let session = env.get("/api/export/session").await.bytes().await.unwrap();
    assert!(!session.is_empty());
    let r = env
        .api
        .post(env.url("/api/import/session"))
        .bearer_auth(env.handle.token())
        .body(session)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn compose_send_replay_and_curl_import() {
    let env = setup().await;
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/send",
            json!({ "request": { "method": "POST", "url": env.upstream_url("/echo"), "headers": [["content-type", "text/plain"]], "body": "aGVsbG8=" } }),
        )
        .await;
    assert_eq!(r.status(), 200);
    let sent: Value = r.json().await.unwrap();
    assert_eq!(sent["response"]["status"], 200);
    assert_eq!(sent["body"]["text"], "hello");
    let id = sent["id"].as_u64().unwrap();

    env.wait_for_exchange(|s| s["id"] == id && is_complete(s))
        .await;
    let r = env
        .send(
            reqwest::Method::POST,
            &format!("/api/exchanges/{id}/replay"),
            json!({}),
        )
        .await;
    assert_eq!(r.status(), 200);
    let replayed: Value = r.json().await.unwrap();
    assert_ne!(replayed["id"], id);
    assert_eq!(replayed["response"]["status"], 200);

    let r = env
        .send(
            reqwest::Method::POST,
            "/api/import/curl",
            json!({ "command": "curl -X PUT https://example.com/x -H 'X-A: 1' -d 'b'" }),
        )
        .await;
    let req: Value = r.json().await.unwrap();
    assert_eq!(req["method"], "PUT");
    assert_eq!(req["url"], "https://example.com/x");

    let targets = env.get_json("/api/codegen/targets").await;
    assert!(targets.as_array().unwrap().len() >= 10);
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/codegen",
            json!({ "request": req, "target": "python-requests" }),
        )
        .await;
    let code: Value = r.json().await.unwrap();
    assert_eq!(code["language"], "python");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn compose_reports_whether_the_exchange_was_recorded() {
    let env = setup().await;
    let send = |path: &str| {
        env.send(
            reqwest::Method::POST,
            "/api/send",
            json!({ "request": { "method": "GET", "url": env.upstream_url(path), "headers": [], "body": "" } }),
        )
    };
    let recorded: Value = send("/hello").await.json().await.unwrap();
    assert_eq!(recorded["recorded"], true, "{recorded}");
    let id = recorded["id"].as_u64().unwrap();
    assert_eq!(env.get(&format!("/api/exchanges/{id}")).await.status(), 200);

    let r = env
        .send(
            reqwest::Method::PUT,
            "/api/capture",
            json!({ "recording": false }),
        )
        .await;
    assert_eq!(r.status(), 200);
    let paused: Value = send("/hello").await.json().await.unwrap();
    assert_eq!(paused["response"]["status"], 200);
    assert_eq!(paused["recorded"], false, "{paused}");
    let paused_id = paused["id"].as_u64().unwrap();
    assert_eq!(
        env.get(&format!("/api/exchanges/{paused_id}"))
            .await
            .status(),
        404,
        "the id of an unrecorded exchange can't be opened"
    );
    // Replays report it too.
    let replayed: Value = env
        .send(
            reqwest::Method::POST,
            &format!("/api/exchanges/{id}/replay"),
            json!({}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(replayed["recorded"], false, "{replayed}");
    // So does MCP, so agents don't try to open it.
    let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"akev_send_request","arguments":{"url": env.upstream_url("/hello")}}});
    let out = env.handle.mcp(rpc.to_string().as_bytes()).await.unwrap();
    let text: Value =
        serde_json::from_str(out["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text["recorded"], false, "{text}");

    env.send(
        reqwest::Method::PUT,
        "/api/capture",
        json!({ "recording": true }),
    )
    .await;
    let resumed: Value = send("/hello").await.json().await.unwrap();
    assert_eq!(resumed["recorded"], true, "{resumed}");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn mcp_cannot_launch_arbitrary_programs_without_opt_in() {
    let env = setup().await;
    let call = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{
        "name":"akev_activate_interceptor",
        "arguments":{"id":"electron","options":{"path":"/bin/true","args":["--version"]}}
    }});
    let out = env.handle.mcp(call.to_string().as_bytes()).await.unwrap();
    assert_eq!(out["result"]["isError"], true, "{out}");
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("--mcp-allow-launch"), "{text}");
    // Over HTTP too.
    let out: Value = env
        .api
        .post(env.url("/mcp"))
        .bearer_auth(env.handle.token())
        .json(&call)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(out["result"]["isError"], true, "{out}");
    let tools = env
        .handle
        .mcp(br#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#)
        .await
        .unwrap();
    let activate = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == "akev_activate_interceptor")
        .unwrap()
        .clone();
    assert_eq!(activate["annotations"]["destructiveHint"], true);
    env.handle.shutdown().await;
}

/// A port that was free a moment ago.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

#[tokio::test]
async fn proxy_overrides_survive_settings_changes_and_ca_regeneration() {
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let call = |h: &AkevHandle, method: reqwest::Method, path: &str| {
        api.request(method, format!("{}{path}", h.api_url()))
            .bearer_auth(h.token())
    };

    // `--proxy-port P`: changing only the bind address keeps listening on P. Under a parallel
    // test run another test can grab P (0.0.0.0:P) in the instant the proxy rebinds, which makes
    // the proxy fall back to the next port; retry with a fresh port so only a real regression
    // (three misses in a row) fails the test.
    let mut attempt = 0;
    let (port, handle, settings) = loop {
        attempt += 1;
        let port = free_port();
        let handle = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(port),
            api_port: 0,
            ..pro::options()
        })
        .await
        .unwrap();
        assert_eq!(handle.proxy_addr().port(), port);
        let mut settings: Value = call(&handle, reqwest::Method::GET, "/api/settings")
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_ne!(settings["proxy"]["port"], port, "the override isn't stored");
        settings["proxy"]["bindAddress"] = json!("0.0.0.0");
        let r = call(&handle, reqwest::Method::PUT, "/api/settings")
            .json(&settings)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 200);
        if handle.proxy_addr().port() == port || attempt >= 3 {
            break (port, handle, settings);
        }
        handle.shutdown().await;
    };
    assert_eq!(
        handle.proxy_addr().port(),
        port,
        "the proxy kept port {port}"
    );
    assert!(handle.proxy_addr().ip().is_unspecified());
    let status = handle.status();
    assert_eq!(status.proxy.port, port);
    assert_eq!(status.proxy.bind_address, "0.0.0.0");
    let stored: Value = call(&handle, reqwest::Method::GET, "/api/settings")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(stored["proxy"]["port"], settings["proxy"]["port"]);
    let upstream = upstream_server().await;
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{port}")).unwrap())
        .build()
        .unwrap();
    let resp = proxied
        .get(format!("http://{upstream}/hello"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    handle.shutdown().await;

    // `--proxy-bind ADDR`: reported as is, and kept by CA regeneration and port changes.
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        proxy_bind: Some("0.0.0.0".into()),
        api_port: 0,
        ..pro::options()
    })
    .await
    .unwrap();
    assert!(handle.proxy_addr().ip().is_unspecified());
    assert_eq!(handle.status().proxy.bind_address, "0.0.0.0");
    let before = handle.proxy_addr();
    let r = call(&handle, reqwest::Method::POST, "/api/ca/regenerate")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(handle.proxy_addr(), before, "CA regeneration kept {before}");
    assert_eq!(handle.status().proxy.bind_address, "0.0.0.0");
    let mut settings: Value = call(&handle, reqwest::Method::GET, "/api/settings")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(settings["proxy"]["bindAddress"], "127.0.0.1", "not stored");
    let new_port = free_port();
    settings["proxy"]["port"] = json!(new_port);
    let r = call(&handle, reqwest::Method::PUT, "/api/settings")
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(handle.proxy_addr().port(), new_port);
    assert!(
        handle.proxy_addr().ip().is_unspecified(),
        "a port change kept the --proxy-bind override"
    );
    // Explicitly changing the bind address applies it.
    settings["proxy"]["bindAddress"] = json!("0.0.0.0");
    call(&handle, reqwest::Method::PUT, "/api/settings")
        .json(&settings)
        .send()
        .await
        .unwrap();
    settings["proxy"]["bindAddress"] = json!("127.0.0.1");
    let r = call(&handle, reqwest::Method::PUT, "/api/settings")
        .json(&settings)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(
        handle.proxy_addr(),
        SocketAddr::from(([127, 0, 0, 1], new_port))
    );
    assert_eq!(handle.status().proxy.bind_address, "127.0.0.1");
    handle.shutdown().await;
}

#[tokio::test]
async fn network_exposed_ui_uses_a_ui_only_session_cookie() {
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        api_bind: "0.0.0.0".into(),
        ..pro::options()
    })
    .await
    .unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let base = handle.api_url();
    let token = handle.token().to_string();
    // Everything needs authentication on a network bind.
    let r = client.get(format!("{base}/")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    let r = client.get(handle.ui_url()).send().await.unwrap();
    assert_eq!(r.status(), 200);
    let cookies: Vec<String> = r
        .headers()
        .get_all("set-cookie")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let name = format!("akev_ui_{}", handle.api_addr().port());
    let session = cookies
        .iter()
        .find(|c| c.starts_with(&format!("{name}=")))
        .unwrap_or_else(|| panic!("no UI session cookie in {cookies:?}"));
    assert!(session.contains("HttpOnly") && session.contains("SameSite=Strict"));
    assert!(
        !session.contains(&token),
        "the UI cookie is not the API token"
    );
    let pair = session.split(';').next().unwrap().to_string();
    // It loads the UI...
    let r = client
        .get(format!("{base}/traffic"))
        .header("cookie", &pair)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    // ... but is no API credential.
    for path in ["/api/status", "/api/exchanges"] {
        let r = client
            .get(format!("{base}{path}"))
            .header("cookie", &pair)
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 401, "{path}");
    }
    handle.shutdown().await;
}

#[tokio::test]
async fn ui_launcher_page_is_removed_on_shutdown() {
    let dir = tempfile::tempdir().unwrap();
    let handle = start(AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    })
    .await
    .unwrap();
    let path = handle.write_ui_launcher().unwrap();
    assert_eq!(
        path,
        std::path::absolute(dir.path().join(akev_api::UI_LAUNCHER_FILE)).unwrap()
    );
    let html = std::fs::read_to_string(&path).unwrap();
    assert!(html.contains(handle.token()));
    let target = akev_api::ui_launcher_url(&path);
    assert!(!target.contains(handle.token()));
    handle.shutdown().await;
    assert!(
        !path.exists(),
        "the launcher (holding the token) is removed"
    );
}

#[tokio::test]
async fn settings_capture_toggle_and_ca() {
    let env = setup().await;
    let settings = env.get_json("/api/settings").await;
    assert!(settings["proxy"]["port"].is_number());
    let r = env
        .send(
            reqwest::Method::PATCH,
            "/api/settings/ui",
            json!({ "theme": "dark" }),
        )
        .await;
    assert_eq!(r.status(), 204);
    assert_eq!(env.get_json("/api/settings").await["ui"]["theme"], "dark");

    let mut updated = env.get_json("/api/settings").await;
    updated["capture"]["ignoreHosts"] = json!(["ignored.example"]);
    let r = env
        .send(reqwest::Method::PUT, "/api/settings", updated)
        .await;
    assert_eq!(r.status(), 200);
    let bad = env
        .send(
            reqwest::Method::PUT,
            "/api/settings",
            json!({ "proxy": { "bindAddress": "not-an-ip" } }),
        )
        .await;
    assert_eq!(bad.status(), 400);

    let r = env
        .send(
            reqwest::Method::PUT,
            "/api/capture",
            json!({ "recording": false }),
        )
        .await;
    assert_eq!(r.json::<Value>().await.unwrap()["recording"], false);
    env.proxied
        .get(env.upstream_url("/not-recorded"))
        .send()
        .await
        .unwrap();
    let r = env
        .send(
            reqwest::Method::PUT,
            "/api/capture",
            json!({ "recording": true }),
        )
        .await;
    assert_eq!(r.json::<Value>().await.unwrap()["recording"], true);
    env.proxied
        .get(env.upstream_url("/recorded"))
        .send()
        .await
        .unwrap();
    env.wait_for_exchange(|s| s["path"] == "/recorded" && is_complete(s))
        .await;
    let list = env.get_json("/api/exchanges").await;
    assert!(
        !list
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["path"] == "/not-recorded")
    );

    let ca = env.get_json("/api/ca").await;
    assert!(ca["daysRemaining"].as_i64().unwrap() > 0);
    let pem = env.get("/api/ca/cert.pem").await;
    assert_eq!(pem.headers()["content-type"], "application/x-pem-file");
    assert!(pem.text().await.unwrap().contains("BEGIN CERTIFICATE"));
    let der = env.get("/api/ca/cert.der").await;
    assert_eq!(der.headers()["content-type"], "application/x-x509-ca-cert");

    // Trust in the operating system's stores: reported, never changed by this test (an
    // unknown store is refused before anything runs).
    let trust = env.get_json("/api/ca/trust").await;
    assert!(trust["stores"].is_array(), "{trust}");
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/ca/trust",
            json!({ "store": "nope" }),
        )
        .await;
    assert_eq!(r.status(), 409);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["code"], "trust-unavailable");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn mcp_endpoint() {
    let env = setup().await;
    let rpc = |body: Value| {
        env.api
            .post(env.url("/mcp"))
            .bearer_auth(env.handle.token())
            .header("accept", "application/json, text/event-stream")
            .json(&body)
            .send()
    };
    let init: Value = rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"e2e","version":"1"}}}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(init["result"]["protocolVersion"], "2025-06-18");
    let r = rpc(json!({"jsonrpc":"2.0","method":"notifications/initialized"}))
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let tools: Value = rpc(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let names: Vec<&str> = tools["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"akev_list_exchanges"));

    env.proxied
        .get(env.upstream_url("/hello"))
        .send()
        .await
        .unwrap();
    env.wait_for_exchange(|s| s["path"] == "/hello" && is_complete(s))
        .await;
    let listed: Value = rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"akev_list_exchanges","arguments":{"filter":"path:/hello"}}}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = listed["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(parsed["total"], 1);
    let id = parsed["exchanges"][0]["id"].as_u64().unwrap();
    let detail: Value = rpc(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"akev_get_exchange","arguments":{"id": id}}}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(
        detail["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("hello world")
    );

    let mock: Value = rpc(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"akev_add_mock","arguments":{"urlPattern":"/from-mcp","status":202,"body":{"ok":true}}}}))
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(mock["result"].get("isError").is_none(), "{mock}");
    let resp = env
        .proxied
        .get(env.upstream_url("/from-mcp"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 202);

    // MCP requires the token too.
    let r = env
        .api
        .post(env.url("/mcp"))
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn shutdown_via_api_resolves_wait() {
    let env = setup().await;
    let r = env
        .send(reqwest::Method::POST, "/api/shutdown", json!({}))
        .await;
    assert_eq!(r.status(), 204);
    tokio::time::timeout(TIMEOUT, env.handle.wait())
        .await
        .expect("wait() resolves after POST /api/shutdown");
    env.handle.shutdown().await;
    // The API is gone afterwards.
    assert!(
        env.api
            .get(env.url("/api/status"))
            .timeout(Duration::from_secs(2))
            .send()
            .await
            .is_err()
    );
}

#[tokio::test]
async fn stream_state_converges_with_rest_under_load() {
    let env = setup().await;
    let mut ws = env.ws().await;
    let hello = next_json(&mut ws).await;
    let mut state: std::collections::BTreeMap<u64, Value> = hello["summaries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_u64().unwrap(), s.clone()))
        .collect();

    let requests = (0..50).map(|i| {
        let client = env.proxied.clone();
        let url = env.upstream_url(&format!("/load/{i}"));
        async move { client.get(url).send().await.unwrap().status() }
    });
    for status in futures::future::join_all(requests).await {
        assert_eq!(status, 200);
    }

    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let done = state
            .values()
            .filter(|s| s["path"].as_str().unwrap_or("").starts_with("/load/") && is_complete(s))
            .count();
        if done == 50 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "stream delivered {done}/50 completed exchanges"
        );
        let m = next_json(&mut ws).await;
        match m["type"].as_str() {
            Some("hello") => {
                // Resync after lag: replace the whole state.
                state = m["summaries"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|s| (s["id"].as_u64().unwrap(), s.clone()))
                    .collect();
            }
            Some("exchanges") => {
                for s in m["added"].as_array().unwrap() {
                    let id = s["id"].as_u64().unwrap();
                    assert!(
                        !state.contains_key(&id),
                        "`added` for an id the client already has: {id}"
                    );
                    state.insert(id, s.clone());
                }
                for s in m["updated"].as_array().unwrap() {
                    state.insert(s["id"].as_u64().unwrap(), s.clone());
                }
            }
            Some("removed") => {
                for id in m["ids"].as_array().unwrap() {
                    state.remove(&id.as_u64().unwrap());
                }
            }
            Some("cleared") => state.clear(),
            _ => {}
        }
    }
    let rest = env.get_json("/api/exchanges").await;
    let rest: std::collections::BTreeMap<u64, Value> = rest
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_u64().unwrap(), s.clone()))
        .collect();
    assert_eq!(
        rest.keys().collect::<Vec<_>>(),
        state.keys().collect::<Vec<_>>()
    );
    for (id, s) in &rest {
        assert_eq!(s["status"], state[id]["status"], "status of {id}");
        assert_eq!(s["state"], state[id]["state"], "state of {id}");
    }
    env.handle.shutdown().await;
}

#[tokio::test]
async fn settings_port_change_rebinds_proxy() {
    let env = setup().await;
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let new_port = free.local_addr().unwrap().port();
    drop(free);
    let mut settings = env.get_json("/api/settings").await;
    settings["proxy"]["port"] = json!(new_port);
    let r = env
        .send(reqwest::Method::PUT, "/api/settings", settings)
        .await;
    assert_eq!(r.status(), 200, "{}", r.text().await.unwrap_or_default());
    assert_eq!(env.handle.proxy_addr().port(), new_port);
    let status = env.get_json("/api/status").await;
    assert_eq!(status["proxy"]["port"], new_port);

    let moved = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://127.0.0.1:{new_port}")).unwrap())
        .build()
        .unwrap();
    let resp = moved
        .get(env.upstream_url("/after-move"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    env.wait_for_exchange(|s| s["path"] == "/after-move" && is_complete(s))
        .await;
    env.handle.shutdown().await;
}

#[tokio::test]
async fn mcp_search_and_wait_for() {
    let env = setup().await;
    let call = |name: &'static str, arguments: Value| {
        let api = env.api.clone();
        let url = env.url("/mcp");
        let token = env.handle.token().to_string();
        async move {
            let v: Value = api
                .post(url)
                .bearer_auth(token)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name": name, "arguments": arguments}}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let text = v["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            (
                v["result"]["isError"] == true,
                serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text)),
            )
        }
    };
    let waiter = tokio::spawn(call(
        "akev_wait_for",
        json!({ "filter": "path:/later", "timeoutMs": 8000 }),
    ));
    tokio::time::sleep(Duration::from_millis(200)).await;
    env.proxied
        .get(env.upstream_url("/later"))
        .send()
        .await
        .unwrap();
    let (is_error, waited) = tokio::time::timeout(TIMEOUT, waiter)
        .await
        .unwrap()
        .unwrap();
    assert!(!is_error, "{waited}");
    assert_eq!(waited["matched"], true);
    assert!(
        waited["exchange"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/later")
    );

    env.proxied
        .get(env.upstream_url("/hello"))
        .send()
        .await
        .unwrap();
    env.wait_for_exchange(|s| s["path"] == "/hello" && is_complete(s))
        .await;
    let (is_error, found) = call("akev_search", json!({ "query": "HELLO WORLD" })).await;
    assert!(!is_error, "{found}");
    let results = found["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert!(
        results[0]["matches"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["in"] == "response-body")
    );

    let (is_error, body) = call(
        "akev_get_body",
        json!({ "id": results[0]["id"], "offset": 2, "maxChars": 5 }),
    )
    .await;
    assert!(!is_error, "{body}");
    assert_eq!(body["returned"], 5);
    assert_eq!(body["truncated"], true);

    let (is_error, msg) = call("akev_get_exchange", json!({ "id": 999_999 })).await;
    assert!(is_error);
    assert!(msg.as_str().unwrap().contains("not found"));
    env.handle.shutdown().await;
}

/// Compiling rules runs script rules' top-level code (up to the syntax-check time limit each),
/// so it runs on a blocking thread: the API keeps answering meanwhile, even on a
/// single-threaded runtime like this test's. Covers PUT /api/rules, the MCP rule tools and
/// dry runs with inline rules.
#[tokio::test]
async fn rule_compilation_does_not_block_the_api() {
    let env = setup().await;
    let slow = |id: &str| {
        json!({
            "type": "rule",
            "id": id,
            "matchers": [{ "type": "path", "pattern": { "op": "equals", "value": "/never" } }],
            "action": { "type": "script", "source": "while (true) {}\nfunction onRequest(req) {}" }
        })
    };
    let rules = json!({ "version": 1, "items": [slow("slow-1"), slow("slow-2")] });

    // Time a status request issued while the slow request is in flight. The clock starts
    // before the sleep: a blocked runtime also delays the end of the sleep.
    async fn status_latency(env: &Env) -> Duration {
        let started = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(100)).await;
        env.get_json("/api/status").await;
        started.elapsed().saturating_sub(Duration::from_millis(100))
    }
    let limit = Duration::from_millis(400);

    let (put, latency) = tokio::join!(
        env.send(reqwest::Method::PUT, "/api/rules", rules.clone()),
        status_latency(&env)
    );
    assert_eq!(put.status(), 200);
    assert!(
        latency < limit,
        "PUT /api/rules blocked the API for {latency:?}"
    );

    let dry = json!({
        "request": { "method": "GET", "url": env.upstream_url("/x"), "headers": [], "body": "" },
        "rules": rules.clone(),
        "runScripts": false
    });
    let (test, latency) = tokio::join!(
        env.send(reqwest::Method::POST, "/api/rules/test", dry),
        status_latency(&env)
    );
    assert_eq!(test.status(), 200);
    assert!(latency < limit, "dry run blocked the API for {latency:?}");

    let call = json!({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": { "name": "akev_set_rules", "arguments": { "rules": rules } }
    });
    let mcp = async {
        env.api
            .post(env.url("/mcp"))
            .bearer_auth(env.handle.token())
            .header("accept", "application/json, text/event-stream")
            .json(&call)
            .send()
            .await
            .unwrap()
    };
    let (mcp, latency) = tokio::join!(mcp, status_latency(&env));
    let mcp: Value = mcp.json().await.unwrap();
    assert_ne!(mcp["result"]["isError"], true, "{mcp}");
    assert!(
        latency < limit,
        "akev_set_rules blocked the API for {latency:?}"
    );
    env.handle.shutdown().await;
}

/// The existing-terminal one-liner sends the token as a Bearer header (curl config on stdin),
/// never in the URL, and the API's setup endpoint accepts it.
#[tokio::test]
async fn existing_terminal_one_liner_sends_the_token_in_a_header() {
    let env = setup().await;
    let list = env.get_json("/api/interceptors").await;
    let terminal = list
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "existing-terminal")
        .expect("existing-terminal interceptor");
    let shells = terminal["metadata"]["shells"].as_object().unwrap();
    for (name, shell) in shells {
        let command = shell["command"].as_str().unwrap();
        assert!(!command.contains("token="), "{name}: {command}");
        assert!(command.contains(env.handle.token()), "{name}: {command}");
    }
    let r = env
        .api
        .get(env.url("/api/setup/terminal?shell=bash"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);
    let r = env.get("/api/setup/terminal?shell=bash").await;
    assert_eq!(r.status(), 200);

    // Must actually run: on Windows `bash` is usually System32's WSL launcher, which exists
    // (and fails) even when no Linux distribution is installed.
    let have = |cmd: &str, expect: &str| {
        std::process::Command::new(cmd)
            .arg("--version")
            .output()
            .is_ok_and(|o| {
                o.status.success() && String::from_utf8_lossy(&o.stdout).contains(expect)
            })
    };
    let bash = shells.get("bash").and_then(|s| s["command"].as_str());
    if let (Some(bash), true) = (bash, have("bash", "bash") && have("curl", "curl")) {
        let script = format!("{bash}; printf 'proxy=%s' \"$HTTP_PROXY\"");
        // The API runs on this (single-threaded) runtime: run bash off it.
        let out = tokio::task::spawn_blocking(move || {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(script)
                .env_remove("HTTP_PROXY")
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        let port = env.handle.proxy_addr().port();
        assert!(
            stdout.contains(&format!("proxy=http://127.0.0.1:{port}")),
            "{stdout}\n{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    env.handle.shutdown().await;
}

/// Every HTML document of the UI (the token login, `/`, SPA routes) carries the strict CSP:
/// only bundled scripts run and the page can only connect back to this daemon.
#[tokio::test]
async fn ui_documents_carry_a_strict_content_security_policy() {
    let env = setup().await;
    let host = env
        .handle
        .api_url()
        .trim_start_matches("http://")
        .to_string();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    for url in [env.handle.ui_url(), env.url("/"), env.url("/traffic/42")] {
        let r = client.get(&url).send().await.unwrap();
        assert_eq!(r.status(), 200, "{url}");
        let csp = r.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .to_string();
        for directive in [
            "default-src 'self'",
            "script-src 'self';",
            "object-src 'none'",
            "base-uri 'none'",
            "frame-ancestors 'self'",
        ] {
            assert!(csp.contains(directive), "{url}: {directive} missing: {csp}");
        }
        assert!(
            csp.contains(&format!("connect-src 'self' ws://{host} wss://{host};")),
            "{url}: {csp}"
        );
        assert!(!csp.contains("unsafe-eval"), "{csp}");
        assert!(!csp.contains("ipc:"), "not the desktop shell: {csp}");
    }
    // Static assets don't need one (CSP only applies to documents and workers).
    env.handle.shutdown().await;
}
