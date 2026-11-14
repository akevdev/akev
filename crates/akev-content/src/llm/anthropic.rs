//! Anthropic Messages API (`/v1/messages`, also Claude on Vertex AI): `system` + `messages`
//! of typed content blocks; streamed as `message_start` / `content_block_*` /
//! `message_delta` / `message_stop` events.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    LlmError, LlmMessage, LlmPart, LlmRequest, LlmResponse, LlmTool, LlmUsage, ParsedRequest,
    ParsedResponse, StreamEvent, Warnings, error_from_value, file_part, image_data, image_url,
    params_except, parse_arguments, plain_text, push_text, push_thinking, str_of, u64_of,
    usage_or_none,
};

pub(super) fn request(v: &Value, w: &mut Warnings) -> ParsedRequest {
    let system = v
        .get("system")
        .and_then(plain_text)
        .filter(|s| !s.is_empty());
    let messages = v
        .get("messages")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|m| LlmMessage {
            role: str_of(m, "role").unwrap_or("user").to_string(),
            name: None,
            content: content(m.get("content"), w),
        })
        .collect();
    let tools = v
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| {
            let kind = str_of(t, "type").filter(|k| *k != "custom");
            LlmTool {
                name: str_of(t, "name")
                    .or(kind)
                    .unwrap_or("(unnamed)")
                    .to_string(),
                description: str_of(t, "description").map(str::to_string),
                schema: t.get("input_schema").cloned(),
                kind: kind.map(str::to_string),
            }
        })
        .collect();
    ParsedRequest {
        request: LlmRequest {
            system,
            messages,
            tools,
            params: params_except(v, &["model", "system", "messages", "tools", "stream"]),
        },
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        model: str_of(v, "model").map(str::to_string),
    }
}

/// Message content: a string or an array of content blocks.
fn content(c: Option<&Value>, w: &mut Warnings) -> Vec<LlmPart> {
    match c {
        Some(Value::String(s)) if !s.is_empty() => vec![LlmPart::Text { text: s.clone() }],
        Some(Value::Array(blocks)) => blocks.iter().filter_map(|b| block(b, w)).collect(),
        _ => Vec::new(),
    }
}

fn block(b: &Value, w: &mut Warnings) -> Option<LlmPart> {
    let kind = str_of(b, "type").unwrap_or("");
    Some(match kind {
        "text" => LlmPart::Text {
            text: str_of(b, "text").unwrap_or("").to_string(),
        },
        "image" => {
            let src = b.get("source").unwrap_or(&Value::Null);
            match str_of(src, "type") {
                Some("base64") => {
                    image_data(str_of(src, "media_type"), str_of(src, "data").unwrap_or(""))
                }
                Some("url") => image_url(str_of(src, "url").unwrap_or("")),
                _ => file_part("image", None, str_of(src, "file_id"), None, None),
            }
        }
        "document" => {
            let src = b.get("source").unwrap_or(&Value::Null);
            let base64 = (str_of(src, "type") == Some("base64"))
                .then(|| str_of(src, "data"))
                .flatten();
            let media_type = str_of(src, "media_type").or(match str_of(src, "type") {
                Some("text") => Some("text/plain"),
                _ => None,
            });
            file_part(
                "document",
                media_type,
                str_of(b, "title").or_else(|| str_of(src, "file_id")),
                str_of(src, "url"),
                base64,
            )
        }
        "tool_use" | "server_tool_use" | "mcp_tool_use" => LlmPart::ToolCall {
            id: str_of(b, "id").map(str::to_string),
            name: str_of(b, "name").unwrap_or("(unnamed)").to_string(),
            arguments: b.get("input").cloned().unwrap_or(Value::Object(Map::new())),
        },
        "tool_result" | "mcp_tool_result" => LlmPart::ToolResult {
            id: str_of(b, "tool_use_id").map(str::to_string),
            name: None,
            content: content(b.get("content"), w),
            is_error: b.get("is_error").and_then(Value::as_bool).unwrap_or(false),
        },
        k if k.ends_with("_tool_result") => LlmPart::ToolResult {
            id: str_of(b, "tool_use_id").map(str::to_string),
            name: None,
            content: vec![LlmPart::Other {
                label: k.to_string(),
                value: b.get("content").cloned().unwrap_or(Value::Null),
            }],
            is_error: false,
        },
        "thinking" => LlmPart::Thinking {
            text: str_of(b, "thinking").unwrap_or("").to_string(),
            redacted: false,
        },
        "redacted_thinking" => LlmPart::Thinking {
            text: String::new(),
            redacted: true,
        },
        "" => return None,
        other => LlmPart::Other {
            label: other.to_string(),
            value: b.clone(),
        },
    })
}

/// Usage; `input_tokens` becomes the total prompt size (uncached + cache reads + writes).
fn usage(u: Option<&Value>) -> Option<LlmUsage> {
    let u = u.filter(|u| u.is_object())?;
    let uncached = u64_of(u, "input_tokens");
    let read = u64_of(u, "cache_read_input_tokens");
    let write = u64_of(u, "cache_creation_input_tokens");
    let input = match (uncached, read, write) {
        (None, None, None) => None,
        (a, b, c) => Some(a.unwrap_or(0) + b.unwrap_or(0) + c.unwrap_or(0)),
    };
    let output = u64_of(u, "output_tokens");
    usage_or_none(LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: match (input, output) {
            (Some(i), Some(o)) => Some(i + o),
            _ => None,
        },
        cache_read_tokens: read.filter(|n| *n > 0),
        cache_write_tokens: write.filter(|n| *n > 0),
        reasoning_tokens: None,
    })
}

pub(super) fn response(v: &Value, w: &mut Warnings) -> ParsedResponse {
    if (str_of(v, "type") == Some("error") || v.get("content").is_none())
        && let Some(e) = error_from_value(v)
    {
        return ParsedResponse {
            error: Some(e),
            ..ParsedResponse::default()
        };
    }
    ParsedResponse {
        response: LlmResponse {
            id: str_of(v, "id").map(str::to_string),
            model: str_of(v, "model").map(str::to_string),
            messages: vec![LlmMessage {
                role: str_of(v, "role").unwrap_or("assistant").to_string(),
                name: None,
                content: content(v.get("content"), w),
            }],
            stop_reason: str_of(v, "stop_reason").map(str::to_string),
            usage: usage(v.get("usage")),
        },
        ..ParsedResponse::default()
    }
}

#[derive(Default)]
struct BlockAcc {
    start: Value,
    text: String,
    json: String,
    thinking: String,
}

/// Shallow merge of usage objects (later non-null values win).
fn merge_usage(into: &mut Map<String, Value>, from: &Value) {
    if let Some(obj) = from.as_object() {
        for (k, v) in obj {
            if !v.is_null() {
                into.insert(k.clone(), v.clone());
            }
        }
    }
}

pub(super) fn stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut blocks: BTreeMap<u64, BlockAcc> = BTreeMap::new();
    let mut usage_raw = Map::new();
    let mut role = None;
    let mut bad = 0usize;
    for ev in events {
        let data = ev.data.trim();
        if data.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            bad += 1;
            continue;
        };
        let kind = str_of(&v, "type").or(ev.event.as_deref()).unwrap_or("");
        let index = u64_of(&v, "index").unwrap_or(0);
        let mut produced = false;
        match kind {
            "message_start" => {
                let m = v.get("message").unwrap_or(&Value::Null);
                out.response.id = str_of(m, "id").map(str::to_string);
                out.response.model = str_of(m, "model").map(str::to_string);
                role = str_of(m, "role").map(str::to_string);
                if let Some(u) = m.get("usage") {
                    merge_usage(&mut usage_raw, u);
                }
            }
            "content_block_start" => {
                let start = v.get("content_block").cloned().unwrap_or(Value::Null);
                let text = str_of(&start, "text").unwrap_or("").to_string();
                let thinking = str_of(&start, "thinking").unwrap_or("").to_string();
                produced = !text.is_empty() || !thinking.is_empty();
                blocks.insert(
                    index,
                    BlockAcc {
                        start,
                        text,
                        json: String::new(),
                        thinking,
                    },
                );
            }
            "content_block_delta" => {
                let d = v.get("delta").unwrap_or(&Value::Null);
                let acc = blocks.entry(index).or_default();
                match str_of(d, "type").unwrap_or("") {
                    "text_delta" => {
                        let t = str_of(d, "text").unwrap_or("");
                        acc.text.push_str(t);
                        produced = !t.is_empty();
                    }
                    "input_json_delta" => {
                        let t = str_of(d, "partial_json").unwrap_or("");
                        acc.json.push_str(t);
                        produced = !t.is_empty();
                    }
                    "thinking_delta" => {
                        let t = str_of(d, "thinking").unwrap_or("");
                        acc.thinking.push_str(t);
                        produced = !t.is_empty();
                    }
                    _ => {}
                }
            }
            "message_delta" => {
                if let Some(r) = v.get("delta").and_then(|d| str_of(d, "stop_reason")) {
                    out.response.stop_reason = Some(r.to_string());
                }
                if let Some(u) = v.get("usage") {
                    merge_usage(&mut usage_raw, u);
                }
            }
            "message_stop" => out.complete = true,
            "error" => {
                out.error = error_from_value(&v).or_else(|| {
                    Some(LlmError {
                        message: "Stream error".into(),
                        ..LlmError::default()
                    })
                });
            }
            _ => {}
        }
        if produced && out.first_token_ms.is_none() {
            out.first_token_ms = ev.at_ms;
        }
    }
    if bad > 0 {
        w.push(format!("Skipped {bad} malformed stream events"));
    }
    let mut parts = Vec::new();
    for acc in blocks.into_values() {
        let kind = str_of(&acc.start, "type").unwrap_or("text");
        match kind {
            "text" => push_text(&mut parts, &acc.text),
            "thinking" => push_thinking(&mut parts, &acc.thinking),
            "tool_use" | "server_tool_use" | "mcp_tool_use" => {
                let name = str_of(&acc.start, "name")
                    .unwrap_or("(unnamed)")
                    .to_string();
                let arguments = if acc.json.trim().is_empty() {
                    acc.start
                        .get("input")
                        .cloned()
                        .unwrap_or(Value::Object(Map::new()))
                } else {
                    parse_arguments(&acc.json, &name, w)
                };
                parts.push(LlmPart::ToolCall {
                    id: str_of(&acc.start, "id").map(str::to_string),
                    name,
                    arguments,
                });
            }
            _ => {
                if let Some(p) = block(&acc.start, w) {
                    parts.push(p);
                }
            }
        }
    }
    if !parts.is_empty() {
        out.response.messages = vec![LlmMessage {
            role: role.unwrap_or_else(|| "assistant".into()),
            name: None,
            content: parts,
        }];
    }
    out.response.usage = usage(Some(&Value::Object(usage_raw)));
    out
}
