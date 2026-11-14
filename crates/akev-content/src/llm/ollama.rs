//! Ollama's native API (`/api/chat`, `/api/generate`): streamed by default as NDJSON, one
//! JSON object per line, the last one (`done: true`) carrying the counts and durations Ollama
//! measured itself.

use serde_json::{Map, Value};

use super::openai::{chat_message, chat_tools};
use super::{
    LlmApi, LlmMessage, LlmPart, LlmRequest, LlmUsage, ParsedRequest, ParsedResponse, StreamEvent,
    Warnings, error_from_value, hoist_system, image_data, push_text, push_thinking, round2, str_of,
    u64_of, usage_or_none,
};

/// Top-level fields plus the flattened `options`.
fn params(v: &Value, skip: &[&str]) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(Value::Object(o)) = v.get("options") {
        out.extend(o.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if k != "options" && !skip.contains(&k.as_str()) {
                out.insert(k.clone(), val.clone());
            }
        }
    }
    out
}

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
            params: params(v, &["model", "messages", "tools", "stream"]),
        },
        // Ollama streams unless told not to.
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(true),
        model: str_of(v, "model").map(str::to_string),
    }
}

pub(super) fn generate_request(v: &Value, _w: &mut Warnings) -> ParsedRequest {
    let mut content = Vec::new();
    push_text(&mut content, str_of(v, "prompt").unwrap_or(""));
    for img in v
        .get("images")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(s) = img.as_str() {
            content.push(image_data(None, s));
        }
    }
    let messages = if content.is_empty() {
        Vec::new()
    } else {
        vec![LlmMessage {
            role: "user".into(),
            name: None,
            content,
        }]
    };
    ParsedRequest {
        request: LlmRequest {
            system: str_of(v, "system")
                .filter(|s| !s.is_empty())
                .map(str::to_string),
            messages,
            tools: Vec::new(),
            params: params(v, &["model", "prompt", "system", "images", "stream"]),
        },
        stream: v.get("stream").and_then(Value::as_bool).unwrap_or(true),
        model: str_of(v, "model").map(str::to_string),
    }
}

/// Merge the response lines (a non-streamed response is one line).
pub(super) fn chunks(api: LlmApi, lines: &[Value], w: &mut Warnings) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut content: Vec<LlmPart> = Vec::new();
    let mut role = None;
    for line in lines {
        if line.get("error").is_some()
            && let Some(e) = error_from_value(line)
        {
            out.error = Some(e);
            continue;
        }
        if let Some(m) = str_of(line, "model") {
            out.response.model = Some(m.to_string());
        }
        match api {
            LlmApi::OllamaGenerate => {
                if let Some(t) = str_of(line, "thinking") {
                    push_thinking(&mut content, t);
                }
                if let Some(t) = str_of(line, "response") {
                    push_text(&mut content, t);
                }
            }
            _ => {
                if let Some(m) = line.get("message") {
                    if let Some(r) = str_of(m, "role") {
                        role = Some(r.to_string());
                    }
                    let msg = chat_message(m, "assistant", w);
                    for part in msg.content {
                        match part {
                            LlmPart::Text { text } => push_text(&mut content, &text),
                            LlmPart::Thinking {
                                text,
                                redacted: false,
                            } => push_thinking(&mut content, &text),
                            other => content.push(other),
                        }
                    }
                }
            }
        }
        if line.get("done").and_then(Value::as_bool) == Some(true) {
            out.complete = true;
            out.response.stop_reason = str_of(line, "done_reason")
                .map(str::to_string)
                .or(Some("stop".into()));
            let input = u64_of(line, "prompt_eval_count");
            let output = u64_of(line, "eval_count");
            out.response.usage = usage_or_none(LlmUsage {
                input_tokens: input,
                output_tokens: output,
                total_tokens: match (input, output) {
                    (Some(i), Some(o)) => Some(i + o),
                    _ => None,
                },
                ..LlmUsage::default()
            });
            if let (Some(n), Some(ns)) = (output, u64_of(line, "eval_duration"))
                && ns > 0
                && n > 0
            {
                out.server_tps = Some(round2(n as f64 / (ns as f64 / 1e9)));
            }
        }
    }
    if !content.is_empty() || role.is_some() {
        out.response.messages = vec![LlmMessage {
            role: role.unwrap_or_else(|| "assistant".into()),
            name: None,
            content,
        }];
    }
    out
}

/// The same lines delivered as server-sent events.
pub(super) fn sse(api: LlmApi, events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut lines = Vec::new();
    let mut first = None;
    for e in events {
        let Ok(line) = serde_json::from_str::<Value>(e.data.trim()) else {
            continue;
        };
        let text = line
            .get("message")
            .and_then(|m| str_of(m, "content"))
            .or_else(|| str_of(&line, "response"))
            .unwrap_or("");
        if first.is_none() && !text.is_empty() {
            first = e.at_ms;
        }
        lines.push(line);
    }
    let mut out = chunks(api, &lines, w);
    out.first_token_ms = first;
    out
}
