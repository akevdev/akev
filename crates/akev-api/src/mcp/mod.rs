//! Model Context Protocol server (`POST /mcp`, Streamable HTTP transport with JSON responses,
//! JSON-RPC 2.0). Lets AI agents inspect, search, replay and mock traffic.
//!
//! Supported methods: `initialize`, `ping`, `tools/list`, `tools/call`, and notifications
//! (`notifications/initialized`, `notifications/cancelled`, ...), which are acknowledged with
//! `202 Accepted`. JSON-RPC batches (protocol ≤ 2025-03-26) are accepted too. There is no
//! server → client stream, so `GET /mcp` answers `405` as the transport spec requires.

pub mod files;
mod redact;
pub mod tools;

use crate::app::App;
use async_trait::async_trait;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::StatusCode;
use http::header::{self, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use std::sync::Arc;

/// Protocol revision we implement (and answer with when the client asks for an unknown one).
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// Revisions we accept.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// JSON-RPC: invalid JSON.
pub const PARSE_ERROR: i64 = -32700;
/// JSON-RPC: not a valid request object.
pub const INVALID_REQUEST: i64 = -32600;
/// JSON-RPC: unknown method.
pub const METHOD_NOT_FOUND: i64 = -32601;
/// JSON-RPC: invalid parameters (also used for unknown tools, per the MCP spec).
pub const INVALID_PARAMS: i64 = -32602;
/// JSON-RPC: internal error.
pub const INTERNAL_ERROR: i64 = -32603;

/// Result of a tool call (rendered as one text content block).
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    /// The text content returned to the agent.
    pub text: String,
    /// Report the call as failed (`isError: true`) so the agent can correct itself.
    pub is_error: bool,
}

impl ToolOutput {
    /// A successful result.
    pub fn ok(text: impl Into<String>) -> Self {
        ToolOutput {
            text: text.into(),
            is_error: false,
        }
    }

    /// A failed call with an explanation.
    pub fn error(text: impl Into<String>) -> Self {
        ToolOutput {
            text: text.into(),
            is_error: true,
        }
    }

    /// A successful result rendered as pretty-printed JSON.
    pub fn json(value: &Value) -> Self {
        Self::ok(serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string()))
    }
}

/// Why a tool call couldn't be dispatched at all (→ JSON-RPC error rather than `isError`).
#[derive(Debug, Clone, PartialEq)]
pub enum CallError {
    /// No tool with this name.
    UnknownTool(String),
}

/// Executes tools; implemented by the application (and by fakes in tests).
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Tool definitions for `tools/list`.
    fn definitions(&self) -> Vec<Value>;
    /// Run a tool with its (object) arguments.
    async fn call(&self, name: &str, arguments: Value) -> Result<ToolOutput, CallError>;
}

fn rpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } })
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

const INSTRUCTIONS: &str = "Akev is an HTTP(S) debugging proxy. Use akev_status to learn the proxy address \
and CA certificate path (route a program's traffic through Akev by setting HTTP_PROXY/HTTPS_PROXY and trusting the CA). \
Use akev_list_exchanges / akev_search with the filter language (see akev_filter_help) to find traffic, \
akev_get_exchange for headers and decoded bodies, akev_wait_for to block until a matching request happens, \
akev_replay / akev_send_request to (re)send requests, and akev_add_mock / akev_set_rules to mock or rewrite traffic.";

/// Negotiate the protocol version: echo a supported requested version, else ours.
pub fn negotiate_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_VERSIONS.iter().find(|v| **v == r).copied())
        .unwrap_or(PROTOCOL_VERSION)
}

/// Handle one JSON-RPC message. Returns `None` for notifications and client responses.
pub async fn handle_message(exec: &dyn ToolExecutor, msg: Value) -> Option<Value> {
    let Value::Object(obj) = msg else {
        return Some(rpc_error(
            Value::Null,
            INVALID_REQUEST,
            "expected a JSON-RPC object",
        ));
    };
    let id = obj.get("id").cloned();
    let Some(method) = obj.get("method").and_then(Value::as_str) else {
        // A response to a server request (we send none) or garbage.
        if obj.contains_key("result") || obj.contains_key("error") {
            return None;
        }
        return Some(rpc_error(
            id.unwrap_or(Value::Null),
            INVALID_REQUEST,
            "missing method",
        ));
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return id.map(|id| rpc_error(id, INVALID_REQUEST, "jsonrpc must be \"2.0\""));
    }
    // Notifications (no id) get no response.
    let id = id?;
    if method.starts_with("notifications/") {
        return None;
    }
    let params = obj.get("params").cloned().unwrap_or(Value::Null);
    Some(match method {
        "initialize" => {
            let requested = params.get("protocolVersion").and_then(Value::as_str);
            rpc_result(
                id,
                json!({
                    "protocolVersion": negotiate_version(requested),
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": "akev", "title": "Akev", "version": akev_core::VERSION },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => rpc_result(id, json!({})),
        "tools/list" => rpc_result(id, json!({ "tools": exec.definitions() })),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Some(rpc_error(id, INVALID_PARAMS, "missing tool name"));
            };
            let arguments = match params.get("arguments") {
                None | Some(Value::Null) => json!({}),
                Some(v @ Value::Object(_)) => v.clone(),
                Some(_) => {
                    return Some(rpc_error(
                        id,
                        INVALID_PARAMS,
                        "tool arguments must be an object",
                    ));
                }
            };
            match exec.call(name, arguments).await {
                Ok(out) => {
                    let mut result = json!({ "content": [{ "type": "text", "text": out.text }] });
                    if out.is_error {
                        result["isError"] = Value::Bool(true);
                    }
                    rpc_result(id, result)
                }
                Err(CallError::UnknownTool(name)) => {
                    rpc_error(id, INVALID_PARAMS, format!("unknown tool: {name}"))
                }
            }
        }
        "resources/list" => rpc_result(id, json!({ "resources": [] })),
        "resources/templates/list" => rpc_result(id, json!({ "resourceTemplates": [] })),
        "prompts/list" => rpc_result(id, json!({ "prompts": [] })),
        other => rpc_error(id, METHOD_NOT_FOUND, format!("method not found: {other}")),
    })
}

/// Handle an HTTP body: a single message or a batch. `None` = nothing to answer (202).
pub async fn handle_payload(exec: &dyn ToolExecutor, body: &[u8]) -> Option<Value> {
    let value: Value = match serde_json::from_slice(body) {
        Ok(v) => v,
        Err(e) => {
            return Some(rpc_error(
                Value::Null,
                PARSE_ERROR,
                format!("parse error: {e}"),
            ));
        }
    };
    match value {
        Value::Array(items) => {
            if items.is_empty() {
                return Some(rpc_error(Value::Null, INVALID_REQUEST, "empty batch"));
            }
            let mut out = Vec::new();
            for item in items {
                if let Some(resp) = handle_message(exec, item).await {
                    out.push(resp);
                }
            }
            (!out.is_empty()).then_some(Value::Array(out))
        }
        other => handle_message(exec, other).await,
    }
}

/// `POST /mcp`
pub(crate) async fn http_handler(
    State(app): State<Arc<App>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(version) = headers
        .get("mcp-protocol-version")
        .and_then(|v| v.to_str().ok())
        && !SUPPORTED_VERSIONS.contains(&version)
    {
        let err = rpc_error(
            Value::Null,
            INVALID_REQUEST,
            format!("unsupported MCP protocol version {version}"),
        );
        return (StatusCode::BAD_REQUEST, axum::Json(err)).into_response();
    }
    let exec = tools::AppTools::new(app);
    match handle_payload(&exec, &body).await {
        Some(resp) => {
            let mut r = axum::Json(resp).into_response();
            r.headers_mut()
                .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
            r
        }
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// `GET /mcp`, `DELETE /mcp`: no server-initiated stream / sessions.
pub(crate) async fn method_not_allowed() -> Response {
    let mut r = (
        StatusCode::METHOD_NOT_ALLOWED,
        "this MCP endpoint only accepts POST",
    )
        .into_response();
    r.headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;

    #[async_trait]
    impl ToolExecutor for Fake {
        fn definitions(&self) -> Vec<Value> {
            vec![json!({"name": "echo", "description": "echo", "inputSchema": {"type": "object"}})]
        }
        async fn call(&self, name: &str, arguments: Value) -> Result<ToolOutput, CallError> {
            match name {
                "echo" => Ok(ToolOutput::json(&arguments)),
                "fail" => Ok(ToolOutput::error("boom")),
                other => Err(CallError::UnknownTool(other.to_string())),
            }
        }
    }

    async fn call(v: Value) -> Option<Value> {
        handle_message(&Fake, v).await
    }

    #[tokio::test]
    async fn initialize_negotiates_version() {
        let r = call(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}))
            .await
            .unwrap();
        assert_eq!(r["id"], 1);
        assert_eq!(r["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(r["result"]["serverInfo"]["name"], "akev");
        assert!(r["result"]["capabilities"]["tools"].is_object());
        let r = call(json!({"jsonrpc":"2.0","id":"x","method":"initialize","params":{"protocolVersion":"1999-01-01"}}))
            .await
            .unwrap();
        assert_eq!(r["id"], "x");
        assert_eq!(r["result"]["protocolVersion"], PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn notifications_get_no_response() {
        assert_eq!(
            call(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await,
            None
        );
        assert_eq!(
            call(
                json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}})
            )
            .await,
            None
        );
        // A response object from the client.
        assert_eq!(
            call(json!({"jsonrpc":"2.0","id":5,"result":{}})).await,
            None
        );
        assert_eq!(
            handle_payload(
                &Fake,
                br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#
            )
            .await,
            None
        );
    }

    #[tokio::test]
    async fn ping_and_list() {
        let r = call(json!({"jsonrpc":"2.0","id":2,"method":"ping"}))
            .await
            .unwrap();
        assert_eq!(r["result"], json!({}));
        let r = call(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}))
            .await
            .unwrap();
        assert_eq!(r["result"]["tools"][0]["name"], "echo");
    }

    #[tokio::test]
    async fn tool_calls() {
        let r = call(json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"a":1}}}))
            .await
            .unwrap();
        assert_eq!(r["result"]["content"][0]["type"], "text");
        let text = r["result"]["content"][0]["text"].as_str().unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(text).unwrap(),
            json!({"a": 1})
        );
        assert!(r["result"].get("isError").is_none());

        let r =
            call(json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"fail"}}))
                .await
                .unwrap();
        assert_eq!(r["result"]["isError"], true);
        assert_eq!(r["result"]["content"][0]["text"], "boom");

        let r =
            call(json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"nope"}}))
                .await
                .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = call(json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{}}))
            .await
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
        let r = call(json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"echo","arguments":[1]}}))
            .await
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_PARAMS);
    }

    #[tokio::test]
    async fn errors() {
        let r = call(json!({"jsonrpc":"2.0","id":9,"method":"bogus/method"}))
            .await
            .unwrap();
        assert_eq!(r["error"]["code"], METHOD_NOT_FOUND);
        let r = call(json!([1, 2])).await.unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = call(json!({"jsonrpc":"1.0","id":10,"method":"ping"}))
            .await
            .unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
        let r = handle_payload(&Fake, b"{not json").await.unwrap();
        assert_eq!(r["error"]["code"], PARSE_ERROR);
        assert_eq!(r["id"], Value::Null);
        let r = handle_payload(&Fake, b"[]").await.unwrap();
        assert_eq!(r["error"]["code"], INVALID_REQUEST);
    }

    #[tokio::test]
    async fn batches() {
        let r = handle_payload(
            &Fake,
            br#"[{"jsonrpc":"2.0","id":1,"method":"ping"},{"jsonrpc":"2.0","method":"notifications/initialized"},{"jsonrpc":"2.0","id":2,"method":"tools/list"}]"#,
        )
        .await
        .unwrap();
        let arr = r.as_array().unwrap();
        assert_eq!(arr.len(), 2);
        assert_eq!(arr[0]["id"], 1);
        assert_eq!(arr[1]["id"], 2);
    }
}
