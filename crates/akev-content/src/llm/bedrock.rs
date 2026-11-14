//! Amazon Bedrock (`bedrock-runtime.<region>.amazonaws.com`):
//!
//! * the **Converse** API (`/model/{id}/converse`, `/model/{id}/converse-stream`): `system` +
//!   `messages` of content blocks (`text`, `image`, `document`, `toolUse`, `toolResult`,
//!   `reasoningContent`), `inferenceConfig`, `toolConfig`;
//! * **InvokeModel** with Anthropic models (`/model/anthropic.…/invoke`,
//!   `…/invoke-with-response-stream`), whose bodies are Anthropic Messages (the route maps
//!   them to [`super::LlmApi::AnthropicMessages`]).
//!
//! Streams use AWS's binary event-stream framing (`application/vnd.amazon.eventstream`), decoded
//! here into [`StreamEvent`]s: Converse events keep their `:event-type`; InvokeModel `chunk`
//! events carry the model's own JSON events, base64-encoded in `bytes`.

use std::collections::BTreeMap;

use base64::Engine;
use serde_json::{Map, Value};

use super::{
    LlmMessage, LlmPart, LlmRequest, LlmResponse, LlmTool, LlmUsage, ParsedRequest, ParsedResponse,
    StreamEvent, Warnings, file_part, image_data, params_except, parse_arguments, push_text,
    push_thinking, str_of, u64_of, usage_or_none,
};

/// The content type of AWS event streams.
pub(crate) const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

// `_w`: the same signature as the other APIs' parsers (Converse bodies need no warnings).
pub(super) fn request(v: &Value, _w: &mut Warnings) -> ParsedRequest {
    let system: Vec<String> = v
        .get("system")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|b| str_of(b, "text").map(str::to_string))
        .collect();
    let messages = v
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|m| LlmMessage {
            role: str_of(m, "role").unwrap_or("user").to_string(),
            name: None,
            content: content(m.get("content")),
        })
        .collect();
    let tools = v
        .pointer("/toolConfig/tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("toolSpec"))
        .map(|spec| LlmTool {
            name: str_of(spec, "name").unwrap_or("(unnamed)").to_string(),
            description: str_of(spec, "description").map(str::to_string),
            schema: spec.pointer("/inputSchema/json").cloned(),
            kind: None,
        })
        .collect();
    let mut params = params_except(v, &["system", "messages", "toolConfig", "inferenceConfig"]);
    if let Some(Value::Object(config)) = v.get("inferenceConfig") {
        for (k, val) in config {
            params.insert(k.clone(), val.clone());
        }
    }
    ParsedRequest {
        request: LlmRequest {
            system: (!system.is_empty()).then(|| system.join("\n\n")),
            messages,
            tools,
            params,
        },
        // Converse never says so in the body: the route knows from the path.
        stream: false,
        model: None,
    }
}

fn content(c: Option<&Value>) -> Vec<LlmPart> {
    c.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(block)
        .collect()
}

fn block(b: &Value) -> Option<LlmPart> {
    if let Some(text) = str_of(b, "text") {
        return Some(LlmPart::Text {
            text: text.to_string(),
        });
    }
    if let Some(img) = b.get("image") {
        let media = str_of(img, "format").map(|f| format!("image/{f}"));
        return Some(match img.pointer("/source/bytes").and_then(Value::as_str) {
            Some(data) => image_data(media.as_deref(), data),
            None => file_part(
                "image",
                media.as_deref(),
                None,
                img.pointer("/source/s3Location/uri")
                    .and_then(Value::as_str),
                None,
            ),
        });
    }
    if let Some(doc) = b.get("document") {
        return Some(file_part(
            "document",
            str_of(doc, "format"),
            str_of(doc, "name"),
            doc.pointer("/source/s3Location/uri")
                .and_then(Value::as_str),
            doc.pointer("/source/bytes").and_then(Value::as_str),
        ));
    }
    if let Some(tool) = b.get("toolUse") {
        return Some(LlmPart::ToolCall {
            id: str_of(tool, "toolUseId").map(str::to_string),
            name: str_of(tool, "name").unwrap_or("(unnamed)").to_string(),
            arguments: tool
                .get("input")
                .cloned()
                .unwrap_or(Value::Object(Map::new())),
        });
    }
    if let Some(result) = b.get("toolResult") {
        let content = result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| match c.get("json") {
                Some(json) => Some(LlmPart::Other {
                    label: "json".into(),
                    value: json.clone(),
                }),
                None => block(c),
            })
            .collect();
        return Some(LlmPart::ToolResult {
            id: str_of(result, "toolUseId").map(str::to_string),
            name: None,
            content,
            is_error: str_of(result, "status") == Some("error"),
        });
    }
    if let Some(reasoning) = b.get("reasoningContent") {
        return Some(match reasoning.get("redactedContent") {
            Some(_) => LlmPart::Thinking {
                text: String::new(),
                redacted: true,
            },
            None => LlmPart::Thinking {
                text: reasoning
                    .pointer("/reasoningText/text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                redacted: false,
            },
        });
    }
    let (label, value) = b.as_object()?.iter().next()?;
    Some(LlmPart::Other {
        label: label.clone(),
        value: value.clone(),
    })
}

fn usage(u: &Value) -> Option<LlmUsage> {
    usage_or_none(LlmUsage {
        input_tokens: u64_of(u, "inputTokens"),
        output_tokens: u64_of(u, "outputTokens"),
        total_tokens: u64_of(u, "totalTokens"),
        cache_read_tokens: u64_of(u, "cacheReadInputTokens"),
        cache_write_tokens: u64_of(u, "cacheWriteInputTokens"),
        reasoning_tokens: None,
    })
}

pub(super) fn response(v: &Value, _w: &mut Warnings) -> ParsedResponse {
    let message = v.pointer("/output/message");
    let messages = message
        .map(|m| LlmMessage {
            role: str_of(m, "role").unwrap_or("assistant").to_string(),
            name: None,
            content: content(m.get("content")),
        })
        .into_iter()
        .collect();
    ParsedResponse {
        response: LlmResponse {
            id: None,
            model: None,
            messages,
            stop_reason: str_of(v, "stopReason").map(str::to_string),
            usage: v.get("usage").and_then(usage),
        },
        complete: true,
        ..ParsedResponse::default()
    }
}

#[derive(Default)]
struct BlockAcc {
    text: String,
    reasoning: String,
    tool: Option<(Option<String>, String)>,
    tool_input: String,
}

/// ConverseStream events: `messageStart`, `contentBlockStart`/`Delta`/`Stop`, `messageStop`,
/// `metadata`.
pub(super) fn stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut blocks: BTreeMap<u64, BlockAcc> = BTreeMap::new();
    let mut role = None;
    for ev in events {
        let Ok(v) = serde_json::from_str::<Value>(ev.data.trim()) else {
            continue;
        };
        let index = u64_of(&v, "contentBlockIndex").unwrap_or(0);
        let mut produced = false;
        match ev.event.as_deref().unwrap_or("") {
            "messageStart" => role = str_of(&v, "role").map(str::to_string),
            "contentBlockStart" => {
                if let Some(t) = v.pointer("/start/toolUse") {
                    blocks.entry(index).or_default().tool = Some((
                        str_of(t, "toolUseId").map(str::to_string),
                        str_of(t, "name").unwrap_or("(unnamed)").to_string(),
                    ));
                }
            }
            "contentBlockDelta" => {
                let d = v.get("delta").unwrap_or(&Value::Null);
                let acc = blocks.entry(index).or_default();
                if let Some(t) = str_of(d, "text") {
                    acc.text.push_str(t);
                    produced = !t.is_empty();
                } else if let Some(t) = d.pointer("/toolUse/input").and_then(Value::as_str) {
                    acc.tool_input.push_str(t);
                    produced = !t.is_empty();
                } else if let Some(t) = d.pointer("/reasoningContent/text").and_then(Value::as_str)
                {
                    acc.reasoning.push_str(t);
                    produced = !t.is_empty();
                }
            }
            "messageStop" => {
                out.response.stop_reason = str_of(&v, "stopReason").map(str::to_string);
                out.complete = true;
            }
            "metadata" => out.response.usage = v.get("usage").and_then(usage),
            "exception" => out.error = super::error_from_value(&v),
            _ => {}
        }
        if produced && out.first_token_ms.is_none() {
            out.first_token_ms = ev.at_ms;
        }
    }
    let mut content = Vec::new();
    for acc in blocks.into_values() {
        push_thinking(&mut content, &acc.reasoning);
        push_text(&mut content, &acc.text);
        if let Some((id, name)) = acc.tool {
            let arguments = parse_arguments(&acc.tool_input, &name, w);
            content.push(LlmPart::ToolCall {
                id,
                name,
                arguments,
            });
        }
    }
    if !content.is_empty() || role.is_some() {
        out.response.messages.push(LlmMessage {
            role: role.unwrap_or_else(|| "assistant".into()),
            name: None,
            content,
        });
    }
    out
}

/// Decode an AWS event stream into events. InvokeModel's `chunk` events are unwrapped into
/// the model's own JSON events (their `type` becomes the event name); other events keep
/// `:event-type`. Exceptions become an `exception` event carrying `{"error": {…}}`. A
/// trailing partial frame is ignored (the stream is still arriving or was cut short).
pub(crate) fn event_stream(body: &[u8], w: &mut Warnings) -> Vec<StreamEvent> {
    let mut out = Vec::new();
    let mut rest = body;
    while rest.len() >= 16 {
        let total = u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) as usize;
        let headers_len = u32::from_be_bytes([rest[4], rest[5], rest[6], rest[7]]) as usize;
        if total < 16 + headers_len || total > (16 << 20) {
            w.push("The AWS event stream is malformed");
            break;
        }
        if rest.len() < total {
            break;
        }
        let headers = parse_headers(&rest[12..12 + headers_len]);
        let payload = &rest[12 + headers_len..total - 4];
        rest = &rest[total..];
        let header = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        let data = String::from_utf8_lossy(payload).into_owned();
        match header(":message-type") {
            Some("exception") | Some("error") => {
                let kind = header(":exception-type")
                    .or(header(":error-code"))
                    .unwrap_or("exception");
                let message = serde_json::from_str::<Value>(&data)
                    .ok()
                    .and_then(|v| str_of(&v, "message").map(str::to_string))
                    .unwrap_or(data);
                out.push(StreamEvent {
                    event: Some("exception".into()),
                    data: serde_json::json!({ "error": { "type": kind, "message": message } })
                        .to_string(),
                    at_ms: None,
                });
            }
            _ => {
                let event = header(":event-type").map(str::to_string);
                if event.as_deref() == Some("chunk")
                    && let Some(inner) = unwrap_chunk(&data)
                {
                    out.push(inner);
                    continue;
                }
                out.push(StreamEvent {
                    event,
                    data,
                    at_ms: None,
                });
            }
        }
    }
    out
}

/// InvokeModel stream chunk: `{"bytes": "<base64 JSON event>"}`.
fn unwrap_chunk(data: &str) -> Option<StreamEvent> {
    let v: Value = serde_json::from_str(data).ok()?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(str_of(&v, "bytes")?)
        .ok()?;
    let inner = String::from_utf8(bytes).ok()?;
    let kind = serde_json::from_str::<Value>(&inner)
        .ok()
        .and_then(|e| str_of(&e, "type").map(str::to_string));
    Some(StreamEvent {
        event: kind,
        data: inner,
        at_ms: None,
    })
}

/// Event-stream headers: string-valued ones (the only kind Bedrock's routing headers use);
/// other value types are skipped by their size.
fn parse_headers(mut b: &[u8]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    while let Some((&name_len, rest)) = b.split_first() {
        let name_len = name_len as usize;
        if rest.len() < name_len + 1 {
            break;
        }
        let name = String::from_utf8_lossy(&rest[..name_len]).into_owned();
        let kind = rest[name_len];
        let value = &rest[name_len + 1..];
        let (string, used) = match kind {
            0 | 1 => (None, 0),
            2 => (None, 1),
            3 => (None, 2),
            4 => (None, 4),
            5 | 8 => (None, 8),
            9 => (None, 16),
            6 | 7 => {
                if value.len() < 2 {
                    break;
                }
                let len = u16::from_be_bytes([value[0], value[1]]) as usize;
                if value.len() < 2 + len {
                    break;
                }
                let s =
                    (kind == 7).then(|| String::from_utf8_lossy(&value[2..2 + len]).into_owned());
                (s, 2 + len)
            }
            _ => break,
        };
        if value.len() < used {
            break;
        }
        if let Some(s) = string {
            out.push((name, s));
        }
        b = &value[used..];
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// One event-stream frame with string headers (CRCs left zero: they aren't checked).
    pub(crate) fn frame(headers: &[(&str, &str)], payload: &[u8]) -> Vec<u8> {
        let mut h = Vec::new();
        for (k, v) in headers {
            h.push(k.len() as u8);
            h.extend_from_slice(k.as_bytes());
            h.push(7);
            h.extend_from_slice(&(v.len() as u16).to_be_bytes());
            h.extend_from_slice(v.as_bytes());
        }
        let total = 16 + h.len() + payload.len();
        let mut out = Vec::new();
        out.extend_from_slice(&(total as u32).to_be_bytes());
        out.extend_from_slice(&(h.len() as u32).to_be_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&h);
        out.extend_from_slice(payload);
        out.extend_from_slice(&[0; 4]);
        out
    }

    fn event(kind: &str, json: &str) -> Vec<u8> {
        frame(
            &[
                (":event-type", kind),
                (":message-type", "event"),
                (":content-type", "application/json"),
            ],
            json.as_bytes(),
        )
    }

    #[test]
    fn converse_request_and_response() {
        let mut w = Warnings::default();
        let req: Value = serde_json::from_str(
            r#"{"system":[{"text":"Be brief."}],"messages":[{"role":"user","content":[{"text":"Hi"},{"image":{"format":"png","source":{"bytes":"iVBORw0KGgo="}}}]}],"inferenceConfig":{"maxTokens":256,"temperature":0.2},"toolConfig":{"tools":[{"toolSpec":{"name":"get_weather","description":"Weather","inputSchema":{"json":{"type":"object"}}}}]}}"#,
        )
        .unwrap();
        let p = request(&req, &mut w);
        assert_eq!(p.request.system.as_deref(), Some("Be brief."));
        assert_eq!(p.request.messages[0].content.len(), 2);
        assert!(matches!(
            p.request.messages[0].content[1],
            LlmPart::Image { .. }
        ));
        assert_eq!(p.request.tools[0].name, "get_weather");
        assert_eq!(p.request.params["maxTokens"], 256);

        let res: Value = serde_json::from_str(
            r#"{"output":{"message":{"role":"assistant","content":[{"text":"Sunny."},{"toolUse":{"toolUseId":"t1","name":"get_weather","input":{"city":"Paris"}}}]}},"stopReason":"tool_use","usage":{"inputTokens":12,"outputTokens":5,"totalTokens":17},"metrics":{"latencyMs":300}}"#,
        )
        .unwrap();
        let r = response(&res, &mut w);
        assert_eq!(r.response.stop_reason.as_deref(), Some("tool_use"));
        assert_eq!(r.response.usage.as_ref().unwrap().total_tokens, Some(17));
        assert!(matches!(
            &r.response.messages[0].content[1],
            LlmPart::ToolCall { name, .. } if name == "get_weather"
        ));
    }

    #[test]
    fn converse_stream_from_an_event_stream() {
        let mut body = Vec::new();
        body.extend(event("messageStart", r#"{"role":"assistant"}"#));
        body.extend(event(
            "contentBlockDelta",
            r#"{"contentBlockIndex":0,"delta":{"text":"Hel"}}"#,
        ));
        body.extend(event(
            "contentBlockDelta",
            r#"{"contentBlockIndex":0,"delta":{"text":"lo"}}"#,
        ));
        body.extend(event(
            "contentBlockStart",
            r#"{"contentBlockIndex":1,"start":{"toolUse":{"toolUseId":"t1","name":"f"}}}"#,
        ));
        body.extend(event(
            "contentBlockDelta",
            r#"{"contentBlockIndex":1,"delta":{"toolUse":{"input":"{\"a\":"}}}"#,
        ));
        body.extend(event(
            "contentBlockDelta",
            r#"{"contentBlockIndex":1,"delta":{"toolUse":{"input":"1}"}}}"#,
        ));
        body.extend(event("messageStop", r#"{"stopReason":"end_turn"}"#));
        body.extend(event(
            "metadata",
            r#"{"usage":{"inputTokens":3,"outputTokens":2,"totalTokens":5}}"#,
        ));
        // A partial frame at the end (still arriving) is ignored.
        body.extend_from_slice(&[0, 0, 1, 0, 0, 0]);
        let mut w = Warnings::default();
        let events = event_stream(&body, &mut w);
        assert_eq!(events.len(), 8);
        let r = stream(&events, &mut w);
        assert!(r.complete);
        let content = &r.response.messages[0].content;
        assert_eq!(
            content[0],
            LlmPart::Text {
                text: "Hello".into()
            }
        );
        assert!(matches!(&content[1], LlmPart::ToolCall { arguments, .. } if arguments["a"] == 1));
        assert_eq!(r.response.usage.unwrap().output_tokens, Some(2));
    }

    #[test]
    fn invoke_model_chunks_unwrap_to_model_events() {
        let inner =
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}}"#;
        let b64 = base64::engine::general_purpose::STANDARD.encode(inner);
        let body = event("chunk", &format!(r#"{{"bytes":"{b64}"}}"#));
        let mut w = Warnings::default();
        let events = event_stream(&body, &mut w);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(events[0].data, inner);
    }

    #[test]
    fn routes_and_parses_bedrock_calls_end_to_end() {
        use super::super::{LlmApi, LlmInput, parse, route};
        let host = "bedrock-runtime.us-east-1.amazonaws.com";
        let r = route(
            host,
            443,
            "/model/anthropic.claude-3-5-sonnet-20240620-v1%3A0/converse",
        )
        .unwrap();
        assert_eq!(
            (r.provider, r.api),
            ("aws-bedrock", LlmApi::BedrockConverse)
        );
        assert_eq!(
            r.path_model.as_deref(),
            Some("anthropic.claude-3-5-sonnet-20240620-v1:0")
        );
        assert!(
            route(host, 443, "/model/meta.llama3-70b/invoke").is_none(),
            "only Anthropic bodies are understood"
        );
        assert!(route("example.com", 443, "/model/x/converse").is_none());

        // InvokeModel streaming with Claude: Anthropic events inside AWS event-stream chunks.
        let chunk = |json: &str| {
            let b64 = base64::engine::general_purpose::STANDARD.encode(json);
            event("chunk", &format!(r#"{{"bytes":"{b64}"}}"#))
        };
        let mut body = Vec::new();
        body.extend(chunk(r#"{"type":"message_start","message":{"id":"m1","role":"assistant","usage":{"input_tokens":9}}}"#));
        body.extend(chunk(
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
        ));
        body.extend(chunk(r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Bonjour"}}"#));
        body.extend(chunk(r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}"#));
        body.extend(chunk(r#"{"type":"message_stop"}"#));
        let request = br#"{"anthropic_version":"bedrock-2023-05-31","max_tokens":100,"messages":[{"role":"user","content":"Say hi in French"}]}"#;
        let ex = parse(&LlmInput {
            host,
            port: 443,
            path: "/model/anthropic.claude-3-haiku-20240307-v1:0/invoke-with-response-stream",
            request_body: request,
            response_status: Some(200),
            response_content_type: Some("application/vnd.amazon.eventstream"),
            response_body: &body,
            finished: true,
            ..LlmInput::default()
        })
        .unwrap();
        assert_eq!(ex.provider, "aws-bedrock");
        assert_eq!(ex.api, LlmApi::AnthropicMessages);
        assert!(ex.stream);
        assert_eq!(
            ex.model.as_deref(),
            Some("anthropic.claude-3-haiku-20240307-v1:0")
        );
        let response = ex.response.unwrap();
        assert_eq!(
            response.messages[0].content[0],
            LlmPart::Text {
                text: "Bonjour".into()
            }
        );
        assert_eq!(response.usage.unwrap().output_tokens, Some(3));
        assert!(ex.warnings.is_empty(), "{:?}", ex.warnings);
    }

    #[test]
    fn exceptions_become_errors() {
        let body = frame(
            &[
                (":message-type", "exception"),
                (":exception-type", "throttlingException"),
            ],
            br#"{"message":"Too many requests"}"#,
        );
        let mut w = Warnings::default();
        let events = event_stream(&body, &mut w);
        let r = stream(&events, &mut w);
        let e = r.error.unwrap();
        assert!(e.message.contains("Too many requests"), "{e:?}");
    }
}
