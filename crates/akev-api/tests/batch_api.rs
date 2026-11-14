//! Batched per-row data (`POST /api/exchanges/headers`, `/timings`) and repeated sends
//! (`POST /api/send/repeat`) against a real Akev instance and a local upstream server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::routing::get;
use futures::StreamExt;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

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
            get(|| async {
                (
                    [
                        ("content-type", "application/json"),
                        ("cache-control", "no-cache"),
                    ],
                    r#"{"message":"hello"}"#,
                )
            }),
        )
        .route(
            "/created",
            get(|| async { (http::StatusCode::CREATED, "made") }),
        )
        .route(
            "/slow",
            get(|| async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                "slow"
            }),
        );
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
    Env {
        handle,
        api,
        proxied,
        upstream: upstream_server().await,
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    fn upstream_url(&self, path: &str) -> String {
        format!("http://{}{path}", self.upstream)
    }

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.api
            .post(self.url(path))
            .bearer_auth(self.handle.token())
            .json(&body)
            .send()
            .await
            .unwrap()
    }

    async fn exchanges(&self) -> Vec<Value> {
        self.api
            .get(self.url("/api/exchanges"))
            .bearer_auth(self.handle.token())
            .send()
            .await
            .unwrap()
            .json::<Vec<Value>>()
            .await
            .unwrap()
    }

    async fn wait_for(&self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            if let Some(found) = self.exchanges().await.into_iter().find(|s| pred(s)) {
                return found;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn lines(text: &str) -> Vec<Value> {
    text.lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[tokio::test]
async fn header_values_and_timings_for_many_rows() {
    let env = setup().await;
    let r = env
        .proxied
        .get(env.upstream_url("/hello"))
        .header("X-Request-Id", "req-1")
        .header("X-Multi", "a")
        .header("X-Multi", "b")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 200);
    let s = env
        .wait_for(|s| s["path"] == "/hello" && s["state"] == "complete")
        .await;
    let id = s["id"].as_u64().unwrap();

    let r = env
        .post(
            "/api/exchanges/headers",
            json!({ "ids": [id, 999_999], "request": ["x-request-id", "X-Multi", "x-missing"], "response": ["Cache-Control", "content-type"] }),
        )
        .await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    assert_eq!(
        v,
        json!({ "exchanges": [{
            "id": id,
            "request": { "x-multi": "a, b", "x-request-id": "req-1" },
            "response": { "cache-control": "no-cache", "content-type": "application/json" },
        }] })
    );

    // Bad header names and oversized batches are refused.
    let r = env
        .post(
            "/api/exchanges/headers",
            json!({ "ids": [id], "request": ["bad name"] }),
        )
        .await;
    assert_eq!(r.status(), 400);
    let too_many: Vec<u64> = (0..5_001).collect();
    let r = env
        .post("/api/exchanges/timings", json!({ "ids": too_many }))
        .await;
    assert_eq!(r.status(), 400);

    let r = env
        .post("/api/exchanges/timings", json!({ "ids": [999_999, id] }))
        .await;
    assert_eq!(r.status(), 200);
    let v: Value = r.json().await.unwrap();
    let timings = v["timings"].as_array().unwrap();
    assert_eq!(timings.len(), 1);
    assert_eq!(timings[0]["id"], id);
    assert!(timings[0]["start"].as_i64().unwrap() > 0);
    assert!(timings[0]["responseBody"].as_f64().is_some());
    env.handle.shutdown().await;
}

#[tokio::test]
async fn repeat_streams_one_line_per_request() {
    let env = setup().await;
    let r = env
        .post(
            "/api/send/repeat",
            json!({ "request": { "method": "GET", "url": env.upstream_url("/created"), "headers": [], "body": "" }, "count": 5, "concurrency": 2 }),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(
        r.headers()["content-type"],
        "application/x-ndjson",
        "streamed"
    );
    let out = lines(&r.text().await.unwrap());
    assert_eq!(out.len(), 6);
    let mut indices: Vec<u64> = out[..5]
        .iter()
        .map(|l| l["index"].as_u64().unwrap())
        .collect();
    indices.sort_unstable();
    assert_eq!(indices, [0, 1, 2, 3, 4]);
    for l in &out[..5] {
        assert_eq!(l["status"], 201);
        assert!(l["durationMs"].as_f64().unwrap() >= 0.0);
        assert!(l["id"].as_u64().is_some());
        assert!(l.get("error").is_none());
    }
    assert_eq!(out[5], json!({ "done": true, "sent": 5 }));

    // Resend a captured exchange by id.
    let first = out[0]["id"].as_u64().unwrap();
    env.wait_for(|s| s["id"] == first && s["state"] == "complete")
        .await;
    let r = env
        .post(
            "/api/send/repeat",
            json!({ "exchangeId": first, "count": 2, "delayMs": 10 }),
        )
        .await;
    let out = lines(&r.text().await.unwrap());
    assert_eq!(out.len(), 3);
    assert_eq!(out[0]["status"], 201);
    assert_eq!(out[2]["sent"], 2);

    // Failures are reported per request.
    let r = env
        .post(
            "/api/send/repeat",
            json!({ "request": { "method": "GET", "url": "http://127.0.0.1:9/", "headers": [], "body": "" }, "count": 1 }),
        )
        .await;
    let out = lines(&r.text().await.unwrap());
    assert_eq!(out[0]["error"], "connection-refused");
    assert!(out[0].get("status").is_none());

    for (body, why) in [
        (json!({ "exchangeId": first, "count": 0 }), "count"),
        (json!({ "exchangeId": first, "count": 10_001 }), "count"),
        (
            json!({ "exchangeId": first, "count": 2, "concurrency": 65 }),
            "concurrency",
        ),
        (
            json!({ "exchangeId": first, "count": 2, "delayMs": 60_001 }),
            "delay",
        ),
        (json!({ "count": 2 }), "no request"),
        (json!({ "exchangeId": 999_999, "count": 2 }), "unknown id"),
    ] {
        let r = env.post("/api/send/repeat", body).await;
        assert!(r.status().is_client_error(), "{why}: {}", r.status());
    }
    env.handle.shutdown().await;
}

#[tokio::test]
async fn closing_the_stream_cancels_the_run() {
    let env = setup().await;
    let slow = || async {
        env.exchanges()
            .await
            .iter()
            .filter(|s| s["path"] == "/slow")
            .count()
    };
    let r = env
        .post(
            "/api/send/repeat",
            json!({ "request": { "method": "GET", "url": env.upstream_url("/slow"), "headers": [], "body": "" }, "count": 100, "concurrency": 2 }),
        )
        .await;
    let mut body = r.bytes_stream();
    let mut seen = Vec::new();
    while !seen.ends_with(b"\n") {
        seen.extend_from_slice(&body.next().await.unwrap().unwrap());
    }
    drop(body);
    // In-flight requests are cancelled and no new ones start.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let after = slow().await;
    assert!(after <= 6, "{after} requests after cancelling");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(slow().await, after);
    env.handle.shutdown().await;
}
