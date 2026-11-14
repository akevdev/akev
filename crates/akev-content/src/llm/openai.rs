//! OpenAI Chat Completions (`/chat/completions`) and legacy Completions (`/completions`),
//! as spoken by OpenAI, Azure OpenAI and every OpenAI-compatible server.

use std::collections::BTreeMap;

use serde_json::Value;

use super::{
    LlmMessage, LlmPart, LlmRequest, LlmResponse, LlmTool, LlmUsage, ParsedRequest, ParsedResponse,
    StreamEvent, Warnings, error_from_value, file_part, hoist_system, image_url, params_except,
    parse_arguments, push_text, push_thinking, str_of, u64_of, usage_or_none,
};

// ---------------------------------------------------------------------------------------------
// Chat Completions: request

pub(super) fn chat_request(v: &Value, w: &mut Warnings) -> ParsedRequest {
    let mut messages: Vec<LlmMessage> = v
        .get("messages")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|m| chat_message(m, "user", w)).collect())
        .unwrap_or_default();
    let system = hoist_system(&mut messages);
    ParsedRequest {
        request: LlmRequest {
            system,
            messages,
            tools: chat_tools(v),
            params: params_except(v, &["model", "messages", "tools", "functions", "stream"]),
        },
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        model: str_of(v, "model").map(str::to_string),
    }
}

/// Tool definitions: `tools: [{type: "function", function: {…}}]` (plus `custom` tools) and
/// the legacy `functions: [{name, description, parameters}]`.
pub(super) fn chat_tools(v: &Value) -> Vec<LlmTool> {
    let mut out = Vec::new();
    for t in v
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let kind = str_of(t, "type").unwrap_or("function");
        let def = t.get(kind).filter(|d| d.is_object()).unwrap_or(t);
        match kind {
            "function" | "custom" => out.push(LlmTool {
                name: str_of(def, "name").unwrap_or("(unnamed)").to_string(),
                description: str_of(def, "description").map(str::to_string),
                schema: def.get("parameters").or_else(|| def.get("format")).cloned(),
                kind: (kind != "function").then(|| kind.to_string()),
            }),
            other => out.push(LlmTool {
                name: str_of(def, "name").unwrap_or(other).to_string(),
                description: str_of(def, "description").map(str::to_string),
                schema: None,
                kind: Some(other.to_string()),
            }),
        }
    }
    for f in v
        .get("functions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        out.push(LlmTool {
            name: str_of(f, "name").unwrap_or("(unnamed)").to_string(),
            description: str_of(f, "description").map(str::to_string),
            schema: f.get("parameters").cloned(),
            kind: None,
        });
    }
    out
}

/// One chat message (request messages and response `choices[].message`).
pub(super) fn chat_message(m: &Value, default_role: &str, w: &mut Warnings) -> LlmMessage {
    let raw_role = str_of(m, "role").unwrap_or(default_role);
    let role = if raw_role == "function" {
        "tool"
    } else {
        raw_role
    };
    let mut content = Vec::new();
    for key in ["reasoning_content", "reasoning", "thinking"] {
        if let Some(t) = str_of(m, key) {
            push_thinking(&mut content, t);
        }
    }
    if role == "tool" {
        let inner = chat_content(m.get("content"));
        content.push(LlmPart::ToolResult {
            id: str_of(m, "tool_call_id").map(str::to_string),
            name: str_of(m, "name")
                .or_else(|| str_of(m, "tool_name"))
                .map(str::to_string),
            content: inner,
            is_error: false,
        });
        return LlmMessage {
            role: role.to_string(),
            name: None,
            content,
        };
    }
    content.extend(chat_content(m.get("content")));
    if let Some(r) = str_of(m, "refusal").filter(|r| !r.is_empty()) {
        content.push(LlmPart::Refusal {
            text: r.to_string(),
        });
    }
    for tc in m
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let f = tc.get("function").unwrap_or(tc);
        let name = str_of(f, "name").unwrap_or("(unnamed)").to_string();
        let arguments = match f.get("arguments").or_else(|| f.get("input")) {
            Some(Value::String(s)) => parse_arguments(s, &name, w),
            Some(other) => other.clone(),
            None => Value::Object(serde_json::Map::new()),
        };
        content.push(LlmPart::ToolCall {
            id: str_of(tc, "id").map(str::to_string),
            name,
            arguments,
        });
    }
    if let Some(fc) = m.get("function_call").filter(|f| f.is_object()) {
        let name = str_of(fc, "name").unwrap_or("(unnamed)").to_string();
        let arguments = match fc.get("arguments") {
            Some(Value::String(s)) => parse_arguments(s, &name, w),
            Some(other) => other.clone(),
            None => Value::Object(serde_json::Map::new()),
        };
        content.push(LlmPart::ToolCall {
            id: None,
            name,
            arguments,
        });
    }
    // Ollama's OpenAI layer (and some others) send images next to the text.
    for img in m
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(s) = img.as_str() {
            content.push(super::image_data(None, s));
        }
    }
    LlmMessage {
        role: role.to_string(),
        name: str_of(m, "name").map(str::to_string),
        content,
    }
}

/// Message content: a string or an array of typed parts.
pub(super) fn chat_content(c: Option<&Value>) -> Vec<LlmPart> {
    match c {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::String(s)) if s.is_empty() => Vec::new(),
        Some(Value::String(s)) => vec![LlmPart::Text { text: s.clone() }],
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for p in parts {
                match chat_part(p) {
                    Some(LlmPart::Text { text }) => {
                        // Adjacent text parts read as one paragraph flow.
                        if let Some(LlmPart::Text { text: last }) = out.last_mut() {
                            last.push_str("\n\n");
                            last.push_str(&text);
                        } else {
                            out.push(LlmPart::Text { text });
                        }
                    }
                    Some(part) => out.push(part),
                    None => {}
                }
            }
            out
        }
        Some(other) => vec![LlmPart::Other {
            label: "content".into(),
            value: other.clone(),
        }],
    }
}

fn chat_part(p: &Value) -> Option<LlmPart> {
    if let Value::String(s) = p {
        return Some(LlmPart::Text { text: s.clone() });
    }
    let kind = str_of(p, "type").unwrap_or("");
    Some(match kind {
        "text" | "input_text" | "output_text" => LlmPart::Text {
            text: str_of(p, "text").unwrap_or("").to_string(),
        },
        "image_url" | "input_image" => {
            let url = match p.get("image_url") {
                Some(Value::String(s)) => s.as_str(),
                Some(obj) => str_of(obj, "url").unwrap_or(""),
                None => "",
            };
            image_url(url)
        }
        "input_audio" => {
            let a = p.get("input_audio").unwrap_or(p);
            let format = str_of(a, "format").map(|f| format!("audio/{f}"));
            file_part("audio", format.as_deref(), None, None, str_of(a, "data"))
        }
        "file" | "input_file" => {
            let f = p.get("file").unwrap_or(p);
            let data = str_of(f, "file_data");
            let media_type = data
                .and_then(|d| d.strip_prefix("data:"))
                .and_then(|d| d.split_once(';'))
                .map(|(m, _)| m);
            let base64 = data.and_then(|d| d.split_once(',').map(|(_, b)| b));
            file_part(
                "file",
                media_type,
                str_of(f, "filename").or_else(|| str_of(f, "file_id")),
                str_of(f, "file_url"),
                base64,
            )
        }
        "refusal" => LlmPart::Refusal {
            text: str_of(p, "refusal").unwrap_or("").to_string(),
        },
        "" => return None,
        other => LlmPart::Other {
            label: other.to_string(),
            value: p.clone(),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// Chat Completions: response

pub(super) fn chat_usage(u: Option<&Value>) -> Option<LlmUsage> {
    let u = u.filter(|u| u.is_object())?;
    let cached = u
        .get("prompt_tokens_details")
        .and_then(|d| u64_of(d, "cached_tokens"))
        .or_else(|| u64_of(u, "prompt_cache_hit_tokens"));
    usage_or_none(LlmUsage {
        input_tokens: u64_of(u, "prompt_tokens"),
        output_tokens: u64_of(u, "completion_tokens"),
        total_tokens: u64_of(u, "total_tokens"),
        cache_read_tokens: cached.filter(|n| *n > 0),
        cache_write_tokens: None,
        reasoning_tokens: u
            .get("completion_tokens_details")
            .and_then(|d| u64_of(d, "reasoning_tokens"))
            .filter(|n| *n > 0),
    })
}

pub(super) fn chat_response(v: &Value, w: &mut Warnings) -> ParsedResponse {
    let choices = v.get("choices").and_then(Value::as_array);
    if choices.is_none()
        && let Some(e) = error_from_value(v)
    {
        return ParsedResponse {
            error: Some(e),
            ..ParsedResponse::default()
        };
    }
    let mut messages = Vec::new();
    let mut stop = None;
    for c in choices.into_iter().flatten() {
        if let Some(m) = c.get("message") {
            messages.push(chat_message(m, "assistant", w));
        }
        if stop.is_none() {
            stop = str_of(c, "finish_reason").map(str::to_string);
        }
    }
    ParsedResponse {
        response: LlmResponse {
            id: str_of(v, "id").map(str::to_string),
            model: str_of(v, "model").map(str::to_string),
            messages,
            stop_reason: stop,
            usage: chat_usage(v.get("usage")),
        },
        ..ParsedResponse::default()
    }
}

// ---------------------------------------------------------------------------------------------
// Chat Completions: stream

#[derive(Default)]
struct ToolAcc {
    id: Option<String>,
    name: String,
    arguments: String,
    /// Arguments sent as a JSON object instead of string fragments (some servers).
    object: Option<Value>,
}

#[derive(Default)]
struct ChoiceAcc {
    role: Option<String>,
    content: Vec<LlmPart>,
    refusal: String,
    tools: BTreeMap<u64, ToolAcc>,
    finish: Option<String>,
}

pub(super) fn chat_stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut choices: BTreeMap<u64, ChoiceAcc> = BTreeMap::new();
    let mut done = false;
    let mut bad = 0usize;
    for ev in events {
        let data = ev.data.trim();
        if data.is_empty() {
            continue;
        }
        if data == "[DONE]" {
            done = true;
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            bad += 1;
            continue;
        };
        if v.get("choices").is_none()
            && v.get("error").is_some()
            && let Some(e) = error_from_value(&v)
        {
            out.error = Some(e);
            continue;
        }
        let r = &mut out.response;
        if r.id.is_none() {
            r.id = str_of(&v, "id")
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        if r.model.is_none() {
            r.model = str_of(&v, "model")
                .filter(|s| !s.is_empty())
                .map(str::to_string);
        }
        if let Some(u) = chat_usage(v.get("usage")) {
            r.usage = Some(u);
        }
        let mut produced = false;
        for (pos, c) in v
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let idx = u64_of(c, "index").unwrap_or(pos as u64);
            let acc = choices.entry(idx).or_default();
            // Some servers put the whole message in `message` instead of `delta`.
            let delta = c.get("delta").or_else(|| c.get("message"));
            if let Some(d) = delta {
                if let Some(role) = str_of(d, "role") {
                    acc.role = Some(role.to_string());
                }
                for key in ["reasoning_content", "reasoning", "thinking"] {
                    if let Some(t) = str_of(d, key).filter(|t| !t.is_empty()) {
                        push_thinking(&mut acc.content, t);
                        produced = true;
                    }
                }
                if let Some(t) = str_of(d, "content").filter(|t| !t.is_empty()) {
                    push_text(&mut acc.content, t);
                    produced = true;
                }
                if let Some(t) = str_of(d, "refusal").filter(|t| !t.is_empty()) {
                    acc.refusal.push_str(t);
                    produced = true;
                }
                let calls = d.get("tool_calls").and_then(Value::as_array);
                for (tpos, tc) in calls.into_iter().flatten().enumerate() {
                    let tidx = u64_of(tc, "index").unwrap_or(tpos as u64);
                    let t = acc.tools.entry(tidx).or_default();
                    if let Some(id) = str_of(tc, "id").filter(|s| !s.is_empty()) {
                        t.id = Some(id.to_string());
                    }
                    let f = tc.get("function").unwrap_or(tc);
                    if let Some(name) = str_of(f, "name").filter(|s| !s.is_empty())
                        && t.name.is_empty()
                    {
                        t.name = name.to_string();
                    }
                    match f.get("arguments") {
                        Some(Value::String(s)) => t.arguments.push_str(s),
                        Some(Value::Null) | None => {}
                        Some(other) => t.object = Some(other.clone()),
                    }
                    produced = true;
                }
                if let Some(fc) = d.get("function_call").filter(|f| f.is_object()) {
                    let t = acc.tools.entry(u64::MAX).or_default();
                    if let Some(name) = str_of(fc, "name").filter(|s| !s.is_empty()) {
                        t.name = name.to_string();
                    }
                    if let Some(a) = str_of(fc, "arguments") {
                        t.arguments.push_str(a);
                    }
                    produced = true;
                }
            }
            if let Some(f) = str_of(c, "finish_reason").filter(|s| !s.is_empty()) {
                acc.finish = Some(f.to_string());
            }
        }
        if produced && out.first_token_ms.is_none() {
            out.first_token_ms = ev.at_ms;
        }
    }
    if bad > 0 {
        w.push(format!("Skipped {bad} malformed stream events"));
    }
    let finished = choices.values().any(|c| c.finish.is_some());
    for acc in choices.values_mut() {
        let mut content = std::mem::take(&mut acc.content);
        if !acc.refusal.is_empty() {
            content.push(LlmPart::Refusal {
                text: std::mem::take(&mut acc.refusal),
            });
        }
        for t in std::mem::take(&mut acc.tools).into_values() {
            let name = if t.name.is_empty() {
                "(unnamed)".to_string()
            } else {
                t.name
            };
            let arguments = match t.object {
                Some(obj) => obj,
                None => parse_arguments(&t.arguments, &name, w),
            };
            content.push(LlmPart::ToolCall {
                id: t.id,
                name,
                arguments,
            });
        }
        out.response.messages.push(LlmMessage {
            role: acc.role.clone().unwrap_or_else(|| "assistant".into()),
            name: None,
            content,
        });
    }
    out.response.stop_reason = choices.values().find_map(|c| c.finish.clone());
    out.complete = done || finished;
    out
}

// ---------------------------------------------------------------------------------------------
// Legacy Completions

pub(super) fn completions_request(v: &Value, _w: &mut Warnings) -> ParsedRequest {
    let prompts: Vec<String> = match v.get("prompt") {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .map(|p| match p {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(other) => vec![other.to_string()],
    };
    let messages = prompts
        .into_iter()
        .map(|p| LlmMessage {
            role: "user".into(),
            name: None,
            content: vec![LlmPart::Text { text: p }],
        })
        .collect();
    ParsedRequest {
        request: LlmRequest {
            system: None,
            messages,
            tools: Vec::new(),
            params: params_except(v, &["model", "prompt", "stream"]),
        },
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(false),
        model: str_of(v, "model").map(str::to_string),
    }
}

pub(super) fn completions_response(v: &Value, _w: &mut Warnings) -> ParsedResponse {
    let choices = v.get("choices").and_then(Value::as_array);
    if choices.is_none()
        && let Some(e) = error_from_value(v)
    {
        return ParsedResponse {
            error: Some(e),
            ..ParsedResponse::default()
        };
    }
    let mut messages = Vec::new();
    let mut stop = None;
    for c in choices.into_iter().flatten() {
        let mut content = Vec::new();
        push_text(&mut content, str_of(c, "text").unwrap_or(""));
        messages.push(LlmMessage {
            role: "assistant".into(),
            name: None,
            content,
        });
        if stop.is_none() {
            stop = str_of(c, "finish_reason").map(str::to_string);
        }
    }
    ParsedResponse {
        response: LlmResponse {
            id: str_of(v, "id").map(str::to_string),
            model: str_of(v, "model").map(str::to_string),
            messages,
            stop_reason: stop,
            usage: chat_usage(v.get("usage")),
        },
        ..ParsedResponse::default()
    }
}

pub(super) fn completions_stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut texts: BTreeMap<u64, (String, Option<String>)> = BTreeMap::new();
    let mut done = false;
    let mut bad = 0usize;
    for ev in events {
        let data = ev.data.trim();
        if data == "[DONE]" {
            done = true;
            continue;
        }
        if data.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            bad += 1;
            continue;
        };
        if v.get("choices").is_none()
            && v.get("error").is_some()
            && let Some(e) = error_from_value(&v)
        {
            out.error = Some(e);
            continue;
        }
        if out.response.id.is_none() {
            out.response.id = str_of(&v, "id").map(str::to_string);
        }
        if out.response.model.is_none() {
            out.response.model = str_of(&v, "model").map(str::to_string);
        }
        if let Some(u) = chat_usage(v.get("usage")) {
            out.response.usage = Some(u);
        }
        for (pos, c) in v
            .get("choices")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let idx = u64_of(c, "index").unwrap_or(pos as u64);
            let entry = texts.entry(idx).or_default();
            if let Some(t) = str_of(c, "text").filter(|t| !t.is_empty()) {
                entry.0.push_str(t);
                if out.first_token_ms.is_none() {
                    out.first_token_ms = ev.at_ms;
                }
            }
            if let Some(f) = str_of(c, "finish_reason") {
                entry.1 = Some(f.to_string());
            }
        }
    }
    if bad > 0 {
        w.push(format!("Skipped {bad} malformed stream events"));
    }
    let finished = texts.values().any(|t| t.1.is_some());
    out.response.stop_reason = texts.values().find_map(|t| t.1.clone());
    out.response.messages = texts
        .into_values()
        .map(|(text, _)| {
            let mut content = Vec::new();
            push_text(&mut content, &text);
            LlmMessage {
                role: "assistant".into(),
                name: None,
                content,
            }
        })
        .collect();
    out.complete = done || finished;
    out
}
