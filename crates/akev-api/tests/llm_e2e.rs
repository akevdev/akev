//! LLM traffic lens end to end: a fake OpenAI-compatible / Anthropic-compatible server streams
//! through a real Akev instance; the summary tag, the filter language,
//! `GET /api/exchanges/:id/llm` and the `akev_llm_conversation` MCP tool see the call.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::body::Body;
use axum::response::Response;
use axum::routing::{get, post};
use futures::StreamExt;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

/// A streamed body: each chunk after `gap`.
fn sse(chunks: Vec<String>, gap: Duration) -> Response {
    let stream = futures::stream::iter(chunks).then(move |c| async move {
        tokio::time::sleep(gap).await;
        Ok::<_, std::io::Error>(bytes::Bytes::from(c))
    });
    Response::builder()
        .header("content-type", "text/event-stream")
        .header("cache-control", "no-cache")
        .body(Body::from_stream(stream))
        .unwrap()
}

fn openai_chunk(delta: Value, finish: Option<&str>) -> String {
    let chunk = json!({
        "id": "chatcmpl-e2e", "object": "chat.completion.chunk", "model": "gpt-e2e-2026",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]
    });
    format!("data: {chunk}\n\n")
}

async fn chat(body: String) -> Response {
    let req: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if req["stream"] != true {
        return Response::builder()
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"id":"chatcmpl-1","model":"gpt-e2e-2026","choices":[{"index":0,"message":{"role":"assistant","content":"Plain answer."},"finish_reason":"stop"}],"usage":{"prompt_tokens":5,"completion_tokens":3,"total_tokens":8}}).to_string(),
            ))
            .unwrap();
    }
    let mut chunks = vec![openai_chunk(
        json!({"role": "assistant", "content": ""}),
        None,
    )];
    for piece in ["Hello", " from", " the", " proxy", "!"] {
        chunks.push(openai_chunk(json!({"content": piece}), None));
    }
    chunks.push(openai_chunk(
        json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function", "function": {"name": "lookup", "arguments": "{\"q\":"}}]}),
        None,
    ));
    chunks.push(openai_chunk(
        json!({"tool_calls": [{"index": 0, "function": {"arguments": "\"akev\"}"}}]}),
        None,
    ));
    chunks.push(openai_chunk(json!({}), Some("tool_calls")));
    chunks.push(format!(
        "data: {}\n\n",
        json!({"id":"chatcmpl-e2e","model":"gpt-e2e-2026","choices":[],"usage":{"prompt_tokens":21,"completion_tokens":12,"total_tokens":33}})
    ));
    chunks.push("data: [DONE]\n\n".into());
    sse(chunks, Duration::from_millis(15))
}

async fn messages(_body: String) -> Response {
    let ev = |name: &str, data: Value| format!("event: {name}\ndata: {data}\n\n");
    sse(
        vec![
            ev(
                "message_start",
                json!({"type":"message_start","message":{"id":"msg_e2e","type":"message","role":"assistant","model":"claude-e2e","content":[],"usage":{"input_tokens":10,"cache_read_input_tokens":90,"output_tokens":1}}}),
            ),
            ev(
                "content_block_start",
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}),
            ),
            ev(
                "content_block_delta",
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"Considering."}}),
            ),
            ev(
                "content_block_stop",
                json!({"type":"content_block_stop","index":0}),
            ),
            ev(
                "content_block_start",
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
            ),
            ev(
                "content_block_delta",
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Bonjour"}}),
            ),
            ev(
                "content_block_stop",
                json!({"type":"content_block_stop","index":1}),
            ),
            ev(
                "message_delta",
                json!({"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}),
            ),
            ev("message_stop", json!({"type":"message_stop"})),
        ],
        Duration::from_millis(5),
    )
}

async fn upstream() -> SocketAddr {
    let app = Router::new()
        .route("/v1/chat/completions", post(chat))
        .route("/v1/messages", post(messages))
        .route("/hello", get(|| async { "hi" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
    proxied: reqwest::Client,
    upstream: SocketAddr,
}

impl Env {
    async fn get(&self, path: &str) -> reqwest::Response {
        self.api
            .get(format!("{}{path}", self.handle.api_url()))
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

    async fn post_upstream(&self, path: &str, body: Value) -> String {
        self.proxied
            .post(format!("http://{}{path}", self.upstream))
            .json(&body)
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap()
    }

    /// The id of the first complete exchange to `path`.
    async fn complete(&self, path: &str) -> u64 {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let list = self.get_json("/api/exchanges").await;
            if let Some(s) = list
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["path"] == path && s["state"] == "complete")
            {
                return s["id"].as_u64().unwrap();
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "no exchange for {path}: {list}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn mcp(&self, name: &str, arguments: Value) -> Value {
        self.api
            .post(format!("{}/mcp", self.handle.api_url()))
            .bearer_auth(self.handle.token())
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}}))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
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
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    Env {
        handle,
        api: reqwest::Client::builder().no_proxy().build().unwrap(),
        proxied,
        upstream: upstream().await,
    }
}

#[tokio::test]
async fn llm_calls_are_recognised_reassembled_and_timed() {
    let env = setup().await;
    let text = env
        .post_upstream(
            "/v1/chat/completions",
            json!({
                "model": "gpt-e2e",
                "messages": [{"role": "system", "content": "Be nice."}, {"role": "user", "content": "Say hello"}],
                "tools": [{"type": "function", "function": {"name": "lookup", "parameters": {"type": "object"}}}],
                "temperature": 0.5,
                "stream": true
            }),
        )
        .await;
    assert!(text.contains("[DONE]"));
    let id = env.complete("/v1/chat/completions").await;
    let summary = env.get_json(&format!("/api/exchanges/{id}")).await["summary"].clone();
    assert_eq!(summary["kind"], "event-stream");
    assert_eq!(
        summary["llm"],
        json!({"provider": "openai-compatible", "model": "gpt-e2e"})
    );

    let llm = env.get_json(&format!("/api/exchanges/{id}/llm")).await;
    assert_eq!(llm["provider"], "openai-compatible");
    assert_eq!(llm["api"], "openai-chat");
    assert_eq!(llm["model"], "gpt-e2e-2026");
    assert_eq!(llm["stream"], true);
    assert_eq!(llm["request"]["system"], "Be nice.");
    assert_eq!(
        llm["request"]["messages"][0]["content"][0]["text"],
        "Say hello"
    );
    assert_eq!(llm["request"]["tools"][0]["name"], "lookup");
    assert_eq!(llm["request"]["params"]["temperature"], 0.5);
    let reply = &llm["response"]["messages"][0]["content"];
    assert_eq!(
        reply[0],
        json!({"type": "text", "text": "Hello from the proxy!"})
    );
    assert_eq!(reply[1]["type"], "tool-call");
    assert_eq!(reply[1]["arguments"], json!({"q": "akev"}));
    assert_eq!(llm["response"]["stopReason"], "tool_calls");
    assert_eq!(llm["response"]["usage"]["outputTokens"], 12);
    // Timed from the recorded SSE events: the first token came after the role-only chunk.
    let ttft = llm["timing"]["ttftMs"].as_f64().expect("ttft");
    let total = llm["timing"]["totalMs"].as_f64().expect("total");
    assert!(ttft >= 15.0 && ttft < total, "ttft {ttft} total {total}");
    assert!(llm["timing"]["tokensPerSecond"].as_f64().unwrap() > 0.0);
    assert_eq!(llm["timing"]["chunks"], 11);
    assert!(llm.get("warnings").is_none(), "{llm}");

    // Anthropic-compatible stream with thinking and cache reads.
    env.post_upstream(
        "/v1/messages",
        json!({"model": "claude-e2e", "max_tokens": 64, "messages": [{"role": "user", "content": "Salut"}], "stream": true}),
    )
    .await;
    let anthropic = env.complete("/v1/messages").await;
    let llm = env
        .get_json(&format!("/api/exchanges/{anthropic}/llm"))
        .await;
    assert_eq!(llm["provider"], "anthropic-compatible");
    let reply = &llm["response"]["messages"][0]["content"];
    assert_eq!(
        reply[0],
        json!({"type": "thinking", "text": "Considering."})
    );
    assert_eq!(reply[1]["text"], "Bonjour");
    assert_eq!(llm["response"]["usage"]["inputTokens"], 100);
    assert_eq!(llm["response"]["usage"]["cacheReadTokens"], 90);

    // Not streamed.
    env.post_upstream(
        "/v1/chat/completions?plain",
        json!({"model": "gpt-e2e", "messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    let plain = env.complete("/v1/chat/completions?plain").await;
    let llm = env.get_json(&format!("/api/exchanges/{plain}/llm")).await;
    assert_eq!(llm["stream"], false);
    assert_eq!(
        llm["response"]["messages"][0]["content"][0]["text"],
        "Plain answer."
    );
    assert!(llm["timing"].get("ttftMs").is_none());

    // Other exchanges.
    env.proxied
        .get(format!("http://{}/hello", env.upstream))
        .send()
        .await
        .unwrap();
    let hello = env.complete("/hello").await;
    let r = env.get(&format!("/api/exchanges/{hello}/llm")).await;
    assert_eq!(r.status(), 404);
    assert_eq!(r.json::<Value>().await.unwrap()["error"]["code"], "not-llm");
    let r = env.get("/api/exchanges/999999/llm").await;
    assert_eq!(
        r.json::<Value>().await.unwrap()["error"]["code"],
        "not-found"
    );

    // The filter language.
    let ids = |list: Value| -> Vec<u64> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_u64().unwrap())
            .collect()
    };
    assert_eq!(
        ids(env.get_json("/api/exchanges?filter=is:llm").await),
        [id, anthropic, plain]
    );
    assert_eq!(
        ids(env
            .get_json("/api/exchanges?filter=provider:anthropic-compatible")
            .await),
        [anthropic]
    );
    assert_eq!(
        ids(env.get_json("/api/exchanges?filter=model:claude").await),
        [anthropic]
    );

    // MCP.
    let out = env.mcp("akev_llm_conversation", json!({"id": id})).await;
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    let parsed: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        parsed["response"]["messages"][0]["content"][0]["text"],
        "Hello from the proxy!"
    );
    let out = env
        .mcp(
            "akev_llm_conversation",
            json!({"id": id, "maxTextChars": 100}),
        )
        .await;
    assert!(out["result"].get("isError").is_none(), "{out}");
    let out = env.mcp("akev_llm_conversation", json!({"id": hello})).await;
    assert_eq!(out["result"]["isError"], true);
    assert!(
        out["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("is:llm")
    );
    let listed = env
        .mcp("akev_list_exchanges", json!({"filter": "is:llm"}))
        .await;
    let rows: Value =
        serde_json::from_str(listed["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(rows["exchanges"][0]["llm"]["model"], "gpt-e2e");
    env.handle.shutdown().await;
}
