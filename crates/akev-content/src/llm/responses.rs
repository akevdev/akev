//! OpenAI Responses API (`/v1/responses`): `instructions` + `input` items in, `output` items
//! out; streamed as typed `response.*` events.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::openai::chat_content;
use super::{
    LlmError, LlmMessage, LlmPart, LlmRequest, LlmResponse, LlmTool, LlmUsage, ParsedRequest,
    ParsedResponse, StreamEvent, Warnings, error_from_value, hoist_system, join_system,
    params_except, parse_arguments, plain_text, scalar_string, str_of, u64_of, usage_or_none,
};

pub(super) fn request(v: &Value, w: &mut Warnings) -> ParsedRequest {
    let mut system = Vec::new();
    if let Some(i) = v.get("instructions").and_then(plain_text) {
        system.push(i);
    }
    let mut messages = match v.get("input") {
        Some(Value::String(s)) => vec![LlmMessage {
            role: "user".into(),
            name: None,
            content: vec![LlmPart::Text { text: s.clone() }],
        }],
        Some(Value::Array(items)) => items_to_messages(items, "user", w),
        _ => Vec::new(),
    };
    if let Some(s) = hoist_system(&mut messages) {
        system.push(s);
    }
    ParsedRequest {
        request: LlmRequest {
            system: join_system(system),
            messages,
            tools: tools(v),
            params: params_except(v, &["model", "input", "instructions", "tools", "stream"]),
        },
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        model: str_of(v, "model").map(str::to_string),
    }
}

fn tools(v: &Value) -> Vec<LlmTool> {
    v.get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|t| {
            let kind = str_of(t, "type").unwrap_or("function");
            match kind {
                "function" | "custom" => LlmTool {
                    name: str_of(t, "name").unwrap_or("(unnamed)").to_string(),
                    description: str_of(t, "description").map(str::to_string),
                    schema: t.get("parameters").or_else(|| t.get("format")).cloned(),
                    kind: (kind != "function").then(|| kind.to_string()),
                },
                other => LlmTool {
                    name: str_of(t, "name")
                        .or_else(|| str_of(t, "server_label"))
                        .unwrap_or(other)
                        .to_string(),
                    description: str_of(t, "description")
                        .or_else(|| str_of(t, "server_url"))
                        .map(str::to_string),
                    schema: None,
                    kind: Some(other.to_string()),
                },
            }
        })
        .collect()
}

/// Convert input/output items into messages. Consecutive assistant-side items (reasoning,
/// text, tool calls) form one assistant message, consecutive tool outputs one tool message.
fn items_to_messages(items: &[Value], default_role: &str, w: &mut Warnings) -> Vec<LlmMessage> {
    let mut out: Vec<LlmMessage> = Vec::new();
    for item in items {
        let Some((role, parts)) = item_parts(item, default_role, w) else {
            continue;
        };
        let merge = matches!(out.last(), Some(last) if last.role == role && (role == "assistant" || role == "tool"));
        if merge {
            if let Some(last) = out.last_mut() {
                last.content.extend(parts);
            }
        } else {
            out.push(LlmMessage {
                role,
                name: None,
                content: parts,
            });
        }
    }
    out
}

fn item_parts(
    item: &Value,
    default_role: &str,
    w: &mut Warnings,
) -> Option<(String, Vec<LlmPart>)> {
    if let Value::String(s) = item {
        return Some((
            default_role.to_string(),
            vec![LlmPart::Text { text: s.clone() }],
        ));
    }
    let kind = str_of(item, "type").unwrap_or("message");
    match kind {
        "message" => {
            let role = str_of(item, "role").unwrap_or(default_role).to_string();
            Some((role, chat_content(item.get("content"))))
        }
        "function_call" => {
            let name = str_of(item, "name").unwrap_or("(unnamed)").to_string();
            let arguments = match item.get("arguments") {
                Some(Value::String(s)) => parse_arguments(s, &name, w),
                Some(other) => other.clone(),
                None => Value::Object(Map::new()),
            };
            Some((
                "assistant".into(),
                vec![LlmPart::ToolCall {
                    id: str_of(item, "call_id")
                        .or_else(|| str_of(item, "id"))
                        .map(str::to_string),
                    name,
                    arguments,
                }],
            ))
        }
        "custom_tool_call" => Some((
            "assistant".into(),
            vec![LlmPart::ToolCall {
                id: str_of(item, "call_id").map(str::to_string),
                name: str_of(item, "name").unwrap_or("(unnamed)").to_string(),
                arguments: item.get("input").cloned().unwrap_or(Value::Null),
            }],
        )),
        "function_call_output" | "custom_tool_call_output" => {
            let content = match item.get("output") {
                Some(Value::String(s)) => vec![LlmPart::Text { text: s.clone() }],
                Some(arr @ Value::Array(_)) => chat_content(Some(arr)),
                Some(Value::Null) | None => Vec::new(),
                Some(other) => vec![LlmPart::Text {
                    text: other.to_string(),
                }],
            };
            Some((
                "tool".into(),
                vec![LlmPart::ToolResult {
                    id: str_of(item, "call_id").map(str::to_string),
                    name: None,
                    content,
                    is_error: false,
                }],
            ))
        }
        "reasoning" => {
            let mut texts: Vec<String> = item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|s| str_of(s, "text").map(str::to_string))
                .filter(|t| !t.is_empty())
                .collect();
            texts.extend(
                item.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|s| str_of(s, "text").map(str::to_string))
                    .filter(|t| !t.is_empty()),
            );
            let part = if texts.is_empty() {
                LlmPart::Thinking {
                    text: String::new(),
                    redacted: true,
                }
            } else {
                LlmPart::Thinking {
                    text: texts.join("\n\n"),
                    redacted: false,
                }
            };
            Some(("assistant".into(), vec![part]))
        }
        other if other.ends_with("_output") => Some((
            "tool".into(),
            vec![LlmPart::Other {
                label: other.to_string(),
                value: item.clone(),
            }],
        )),
        other => Some((
            "assistant".into(),
            vec![LlmPart::Other {
                label: other.to_string(),
                value: item.clone(),
            }],
        )),
    }
}

fn usage(u: Option<&Value>) -> Option<LlmUsage> {
    let u = u.filter(|u| u.is_object())?;
    usage_or_none(LlmUsage {
        input_tokens: u64_of(u, "input_tokens"),
        output_tokens: u64_of(u, "output_tokens"),
        total_tokens: u64_of(u, "total_tokens"),
        cache_read_tokens: u
            .get("input_tokens_details")
            .and_then(|d| u64_of(d, "cached_tokens"))
            .filter(|n| *n > 0),
        cache_write_tokens: None,
        reasoning_tokens: u
            .get("output_tokens_details")
            .and_then(|d| u64_of(d, "reasoning_tokens"))
            .filter(|n| *n > 0),
    })
}

fn stop_reason(v: &Value) -> Option<String> {
    v.get("incomplete_details")
        .and_then(|d| str_of(d, "reason"))
        .or_else(|| str_of(v, "status"))
        .map(str::to_string)
}

fn response_error(v: &Value) -> Option<LlmError> {
    let e = v.get("error").filter(|e| e.is_object())?;
    Some(LlmError {
        status: None,
        kind: str_of(e, "type").map(str::to_string),
        code: e.get("code").and_then(scalar_string),
        message: str_of(e, "message")
            .unwrap_or("The response failed")
            .to_string(),
    })
}

pub(super) fn response(v: &Value, w: &mut Warnings) -> ParsedResponse {
    if v.get("output").is_none()
        && let Some(e) = error_from_value(v)
    {
        return ParsedResponse {
            error: Some(e),
            ..ParsedResponse::default()
        };
    }
    let items: &[Value] = v
        .get("output")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    ParsedResponse {
        response: LlmResponse {
            id: str_of(v, "id").map(str::to_string),
            model: str_of(v, "model").map(str::to_string),
            messages: items_to_messages(items, "assistant", w),
            stop_reason: stop_reason(v),
            usage: usage(v.get("usage")),
        },
        error: response_error(v),
        ..ParsedResponse::default()
    }
}

/// The `index`-th element of `arr` (created as `fill` when missing).
fn slot(arr: &mut Value, index: usize, fill: Value) -> Option<&mut Value> {
    if !arr.is_array() {
        *arr = Value::Array(Vec::new());
    }
    let a = arr.as_array_mut()?;
    while a.len() <= index {
        a.push(fill.clone());
    }
    a.get_mut(index)
}

fn append_str(v: &mut Value, key: &str, delta: &str) {
    match v {
        Value::Object(obj) => match obj.get_mut(key) {
            Some(Value::String(s)) => s.push_str(delta),
            _ => {
                obj.insert(key.to_string(), Value::String(delta.to_string()));
            }
        },
        other => *other = json!({ key: delta }),
    }
}

pub(super) fn stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut items: BTreeMap<u64, Value> = BTreeMap::new();
    let mut last: Option<Value> = None;
    let mut bad = 0usize;
    for ev in events {
        let data = ev.data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            bad += 1;
            continue;
        };
        let kind = str_of(&v, "type")
            .or(ev.event.as_deref())
            .unwrap_or("")
            .to_string();
        let index = u64_of(&v, "output_index").unwrap_or(0);
        let mut produced = false;
        match kind.as_str() {
            "response.created" | "response.in_progress" | "response.queued" => {
                if let Some(r) = v.get("response") {
                    last = Some(r.clone());
                }
            }
            "response.completed" | "response.incomplete" | "response.failed" => {
                if let Some(r) = v.get("response") {
                    last = Some(r.clone());
                }
                out.complete = true;
            }
            "response.output_item.added" | "response.output_item.done" => {
                if let Some(item) = v.get("item") {
                    items.insert(index, item.clone());
                }
            }
            "response.output_text.delta" | "response.refusal.delta" => {
                let delta = str_of(&v, "delta").unwrap_or("");
                let ci = u64_of(&v, "content_index").unwrap_or(0) as usize;
                let (part_type, key) = if kind == "response.refusal.delta" {
                    ("refusal", "refusal")
                } else {
                    ("output_text", "text")
                };
                let item = items.entry(index).or_insert_with(
                    || json!({"type": "message", "role": "assistant", "content": []}),
                );
                let content = item
                    .as_object_mut()
                    .map(|o| o.entry("content").or_insert_with(|| json!([])));
                if let Some(part) = content.and_then(|c| slot(c, ci, json!({"type": part_type}))) {
                    append_str(part, key, delta);
                }
                produced = !delta.is_empty();
            }
            "response.function_call_arguments.delta" | "response.custom_tool_call_input.delta" => {
                let delta = str_of(&v, "delta").unwrap_or("");
                let (ty, key) = if kind.starts_with("response.custom") {
                    ("custom_tool_call", "input")
                } else {
                    ("function_call", "arguments")
                };
                let item = items.entry(index).or_insert_with(|| json!({"type": ty}));
                append_str(item, key, delta);
                produced = true;
            }
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                let delta = str_of(&v, "delta").unwrap_or("");
                let (key, idx_key, part_ty) = if kind == "response.reasoning_text.delta" {
                    ("content", "content_index", "reasoning_text")
                } else {
                    ("summary", "summary_index", "summary_text")
                };
                let si = u64_of(&v, idx_key).unwrap_or(0) as usize;
                let item = items
                    .entry(index)
                    .or_insert_with(|| json!({"type": "reasoning"}));
                if let Some(obj) = item.as_object_mut() {
                    let arr = obj.entry(key).or_insert_with(|| json!([]));
                    if let Some(part) = slot(arr, si, json!({"type": part_ty})) {
                        append_str(part, "text", delta);
                    }
                }
                produced = !delta.is_empty();
            }
            "error" => {
                out.error = error_from_value(&json!({ "error": v.get("error").unwrap_or(&v) }))
                    .or_else(|| {
                        Some(LlmError {
                            code: v.get("code").and_then(scalar_string),
                            message: str_of(&v, "message").unwrap_or("Stream error").to_string(),
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
    let mut base = last.unwrap_or_else(|| json!({}));
    let final_has_output = base
        .get("output")
        .and_then(Value::as_array)
        .is_some_and(|o| !o.is_empty());
    if !final_has_output && let Some(obj) = base.as_object_mut() {
        obj.insert("output".into(), Value::Array(items.into_values().collect()));
    }
    let parsed = response(&base, w);
    out.response = parsed.response;
    if out.error.is_none() {
        out.error = parsed.error;
    }
    out
}
