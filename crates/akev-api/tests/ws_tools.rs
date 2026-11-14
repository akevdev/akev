//! End-to-end WebSocket tools through the public API: injecting messages into a live
//! WebSocket and `ws-breakpoint` rules (pause → edit / drop / forward), with the live stream's
//! `message-paused` / `message-resumed` notifications.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::{AkevHandle, AkevOptions, start};
use futures::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;

const TIMEOUT: Duration = Duration::from_secs(10);

type Ws<S> = tokio_tungstenite::WebSocketStream<S>;

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
}

impl Env {
    async fn start() -> Env {
        let handle = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
        .expect("akev starts");
        let api = reqwest::Client::builder().no_proxy().build().unwrap();
        Env { handle, api }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    async fn get(&self, path: &str) -> Value {
        let r = self
            .api
            .get(self.url(path))
            .bearer_auth(self.handle.token())
            .send()
            .await
            .unwrap();
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

    async fn post(&self, path: &str, body: Value) -> reqwest::Response {
        self.send(reqwest::Method::POST, path, body).await
    }

    async fn stream(&self) -> Ws<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>> {
        let url = format!(
            "ws://{}/api/stream?token={}",
            self.handle.api_addr(),
            self.handle.token()
        );
        tokio_tungstenite::connect_async(url).await.unwrap().0
    }

    async fn wait_for_exchange(&self, pred: impl Fn(&Value) -> bool) -> Value {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let list = self.get("/api/exchanges").await;
            if let Some(found) = list.as_array().unwrap().iter().find(|s| pred(s)) {
                return found.clone();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no exchange: {list}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    /// A WebSocket through the proxy to `upstream`.
    async fn ws_client(&self, upstream: SocketAddr, path: &str) -> Ws<tokio::net::TcpStream> {
        let mut tcp = tokio::net::TcpStream::connect(self.handle.proxy_addr())
            .await
            .unwrap();
        tcp.write_all(
            format!("CONNECT {upstream} HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes(),
        )
        .await
        .unwrap();
        let mut buf = Vec::new();
        while !buf.ends_with(b"\r\n\r\n") {
            let mut b = [0u8; 1];
            assert_eq!(tcp.read(&mut b).await.unwrap(), 1);
            buf.push(b[0]);
        }
        assert!(buf.starts_with(b"HTTP/1.1 200"));
        tokio_tungstenite::client_async(format!("ws://{upstream}{path}"), tcp)
            .await
            .expect("ws handshake")
            .0
    }
}

/// WebSocket echo server.
async fn echo_upstream() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(s).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_close() {
                        break;
                    }
                    if (msg.is_text() || msg.is_binary()) && ws.send(msg).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

/// Next stream message of type `ty` matching `pred`.
async fn stream_message<S>(ws: &mut S, ty: &str, pred: impl Fn(&Value) -> bool) -> Value
where
    S: StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    tokio::time::timeout(TIMEOUT, async {
        loop {
            let Some(Ok(Message::Text(t))) = ws.next().await else {
                continue;
            };
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == ty && pred(&v) {
                return v;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {ty} stream message"))
}

async fn recv(ws: &mut Ws<tokio::net::TcpStream>) -> Message {
    tokio::time::timeout(TIMEOUT, ws.next())
        .await
        .expect("message arrives")
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn inject_and_message_breakpoints_over_the_api() {
    let env = Env::start().await;
    let upstream = echo_upstream().await;
    let mut stream = env.stream().await;
    let hello = stream_message(&mut stream, "hello", |_| true).await;
    assert_eq!(hello["pausedMessages"], json!([]));

    let rules = json!({"items": [{
        "type": "rule", "id": "wsb", "matchers": [],
        "action": {"type": "ws-breakpoint", "direction": "sent", "pattern": {"op": "contains", "value": "pause"}}
    }]});
    let r = env.send(reqwest::Method::PUT, "/api/rules", rules).await;
    assert!(r.status().is_success());
    let errors: Value = r.json().await.unwrap();
    assert_eq!(errors["errors"], json!([]));

    let mut ws = env.ws_client(upstream, "/socket").await;
    let summary = env
        .wait_for_exchange(|s| s["path"] == "/socket" && s["state"] == "streaming")
        .await;
    let id = summary["id"].as_u64().unwrap();
    let messages_path = format!("/api/exchanges/{id}/messages");

    // ---- inject
    let r = env
        .post(
            &messages_path,
            json!({"direction": "received", "text": "hello client"}),
        )
        .await;
    assert_eq!(r.status(), 202);
    let first: Value = r.json().await.unwrap();
    assert!(first["seq"].is_u64(), "{first}");
    assert_eq!(recv(&mut ws).await, Message::text("hello client"));
    let r = env
        .post(
            &messages_path,
            json!({"direction": "sent", "kind": "binary", "base64": "3q2+7w=="}),
        )
        .await;
    assert_eq!(r.status(), 202);
    assert_eq!(
        recv(&mut ws).await,
        Message::binary(vec![0xde, 0xad, 0xbe, 0xef])
    );

    // ---- pause, edit, forward
    ws.send(Message::text("pause me")).await.unwrap();
    let paused = stream_message(&mut stream, "message-paused", |_| true).await;
    let msg = &paused["message"];
    assert_eq!(msg["id"], json!(id));
    assert_eq!(msg["direction"], "sent");
    assert_eq!(msg["kind"], "text");
    assert_eq!(msg["payload"], "cGF1c2UgbWU="); // "pause me"
    assert_eq!(msg["rule"], "wsb");
    let pause_id = msg["pauseId"].as_u64().unwrap();
    let listed = env.get("/api/breakpoints/messages").await;
    assert_eq!(listed[0]["pauseId"], json!(pause_id));
    let r = env
        .post(
            &format!("/api/breakpoints/messages/{pause_id}/resume"),
            json!({"action": "forward", "text": "edited"}),
        )
        .await;
    assert_eq!(r.status(), 204);
    assert_eq!(recv(&mut ws).await, Message::text("edited"));
    let resumed = stream_message(&mut stream, "message-resumed", |v| {
        v["pauseId"] == json!(pause_id)
    })
    .await;
    assert_eq!(resumed["id"], json!(id));
    assert_eq!(env.get("/api/breakpoints/messages").await, json!([]));

    // ---- pause, drop (resume-all for this exchange)
    ws.send(Message::text("pause again")).await.unwrap();
    let paused = stream_message(&mut stream, "message-paused", |_| true).await;
    let r = env
        .post(
            "/api/breakpoints/messages/resume-all",
            json!({"id": id, "action": "drop"}),
        )
        .await;
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<Value>().await.unwrap(), json!({"resumed": 1}));
    stream_message(&mut stream, "message-resumed", |v| {
        v["pauseId"] == paused["message"]["pauseId"]
    })
    .await;
    ws.send(Message::text("plain")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("plain"));

    // ---- errors
    let r = env
        .post(
            "/api/breakpoints/messages/999999/resume",
            json!({"action": "forward"}),
        )
        .await;
    assert_eq!(r.status(), 404);
    let r = env
        .post(
            &messages_path,
            json!({"direction": "sideways", "text": "x"}),
        )
        .await;
    assert_eq!(r.status(), 400);
    let r = env.post(&messages_path, json!({"direction": "sent"})).await;
    assert_eq!(r.status(), 400, "a payload is required");
    let r = env
        .post(
            &messages_path,
            json!({"direction": "sent", "kind": "text", "base64": "/w=="}),
        )
        .await;
    assert_eq!(r.status(), 400, "text must be UTF-8");
    let r = env
        .post(
            "/api/exchanges/987654321/messages",
            json!({"direction": "sent", "text": "x"}),
        )
        .await;
    assert_eq!(r.status(), 404);
    let r = env
        .post(
            "/api/breakpoints/messages/resume-all",
            json!({"action": "explode"}),
        )
        .await;
    assert_eq!(r.status(), 400);

    // ---- what was recorded
    let recorded = env.get(&messages_path).await;
    let rows: Vec<(String, Value, bool)> = recorded
        .as_array()
        .unwrap()
        .iter()
        .map(|m| {
            (
                m["direction"].as_str().unwrap().to_string(),
                if m["text"].is_null() {
                    m["base64"].clone()
                } else {
                    m["text"].clone()
                },
                m["modified"].as_bool().unwrap_or(false),
            )
        })
        .collect();
    let expect = |d: &str, v: Value, modified: bool| (d.to_string(), v, modified);
    assert_eq!(
        rows,
        vec![
            expect("received", json!("hello client"), true),
            expect("sent", json!("3q2+7w=="), true),
            expect("received", json!("3q2+7w=="), false),
            expect("sent", json!("edited"), true),
            expect("received", json!("edited"), false),
            expect("sent", json!("plain"), false),
            expect("received", json!("plain"), false),
        ]
    );
    let detail = env.get(&format!("/api/exchanges/{id}")).await;
    assert_eq!(detail["summary"]["modified"], json!(true));
    assert!(
        detail["matchedRules"]
            .as_array()
            .unwrap()
            .contains(&json!("wsb"))
    );

    // ---- closed: 409
    ws.close(None).await.unwrap();
    while let Some(Ok(_)) = ws.next().await {}
    env.wait_for_exchange(|s| s["id"] == json!(id) && s["state"] != "streaming")
        .await;
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let r = env
            .post(
                &messages_path,
                json!({"direction": "received", "text": "late"}),
            )
            .await;
        if r.status() == 409 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "still open: {}",
            r.status()
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    }
    env.handle.shutdown().await;
}

#[tokio::test]
async fn http_exchanges_are_not_websockets_and_shutdown_releases_pauses() {
    let env = Env::start().await;
    let upstream = echo_upstream().await;
    // Any HTTP exchange: injection is a conflict.
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", env.handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    let _ = proxied
        .get(format!("http://{upstream}/plain-http"))
        .send()
        .await;
    let http = env.wait_for_exchange(|s| s["path"] == "/plain-http").await;
    let r = env
        .post(
            &format!("/api/exchanges/{}/messages", http["id"]),
            json!({"direction": "sent", "text": "x"}),
        )
        .await;
    assert_eq!(r.status(), 409);

    // A paused message is released by resume-all (also used on shutdown).
    let rules =
        json!({"items": [{"type": "rule", "id": "all", "action": {"type": "ws-breakpoint"}}]});
    env.send(reqwest::Method::PUT, "/api/rules", rules).await;
    let mut ws = env.ws_client(upstream, "/held").await;
    ws.send(Message::text("held")).await.unwrap();
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    while env.get("/api/breakpoints/messages").await == json!([]) {
        assert!(tokio::time::Instant::now() < deadline, "message paused");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let r = env.post("/api/breakpoints/resume-all", json!({})).await;
    assert_eq!(r.status(), 204);
    // The echo comes back... and is paused in turn (both directions), then released again.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let listed = env.get("/api/breakpoints/messages").await;
        if listed[0]["direction"] == "received" {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "echo paused: {listed}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    env.post("/api/breakpoints/messages/resume-all", json!({}))
        .await;
    assert_eq!(recv(&mut ws).await, Message::text("held"));
    env.handle.shutdown().await;
}
