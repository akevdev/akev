//! Google Gemini `generateContent` / `streamGenerateContent` (AI Studio and Vertex AI):
//! `contents` of `parts` in, `candidates` out. Streams are SSE (`alt=sse`) or one JSON array,
//! each element a partial response whose parts carry the text increments. Both camelCase and
//! the REST API's snake_case field names are accepted.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::{
    LlmMessage, LlmPart, LlmRequest, LlmTool, LlmUsage, ParsedRequest, ParsedResponse, StreamEvent,
    Warnings, error_from_value, file_part, get_any, image_data, image_url, plain_text, push_text,
    push_thinking, str_of, u64_of, usage_or_none,
};

pub(super) fn request(v: &Value, _w: &mut Warnings) -> ParsedRequest {
    let system = get_any(v, &["systemInstruction", "system_instruction"]).and_then(|s| match s {
        Value::String(t) => Some(t.clone()),
        other => other.get("parts").and_then(plain_text),
    });
    let messages = v
        .get("contents")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|c| LlmMessage {
            role: role(str_of(c, "role").unwrap_or("user")),
            name: None,
            content: parts(c.get("parts")),
        })
        .collect();
    let mut tools = Vec::new();
    for t in v
        .get("tools")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(obj) = t.as_object() else { continue };
        for (key, val) in obj {
            if key == "functionDeclarations" || key == "function_declarations" {
                for f in val.as_array().into_iter().flatten() {
                    tools.push(LlmTool {
                        name: str_of(f, "name").unwrap_or("(unnamed)").to_string(),
                        description: str_of(f, "description").map(str::to_string),
                        schema: get_any(
                            f,
                            &[
                                "parameters",
                                "parametersJsonSchema",
                                "parameters_json_schema",
                            ],
                        )
                        .cloned(),
                        kind: None,
                    });
                }
            } else {
                tools.push(LlmTool {
                    name: key.clone(),
                    description: None,
                    schema: None,
                    kind: Some(key.clone()),
                });
            }
        }
    }
    let mut params = Map::new();
    if let Some(Value::Object(cfg)) = get_any(v, &["generationConfig", "generation_config"]) {
        params.extend(cfg.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    if let Some(obj) = v.as_object() {
        for (k, val) in obj {
            if !matches!(
                k.as_str(),
                "contents"
                    | "systemInstruction"
                    | "system_instruction"
                    | "tools"
                    | "generationConfig"
                    | "generation_config"
                    | "model"
            ) {
                params.insert(k.clone(), val.clone());
            }
        }
    }
    ParsedRequest {
        request: LlmRequest {
            system,
            messages,
            tools,
            params,
        },
        stream: false,
        model: str_of(v, "model").map(|m| m.trim_start_matches("models/").to_string()),
    }
}

fn role(r: &str) -> String {
    match r {
        "model" => "assistant",
        "function" | "tool" => "tool",
        "system" => "system",
        _ => "user",
    }
    .to_string()
}

fn parts(p: Option<&Value>) -> Vec<LlmPart> {
    let mut out = Vec::new();
    for part in p.and_then(Value::as_array).into_iter().flatten() {
        if let Some(text) = str_of(part, "text") {
            if part.get("thought").and_then(Value::as_bool) == Some(true) {
                push_thinking(&mut out, text);
            } else {
                push_text(&mut out, text);
            }
        } else if let Some(p) = part_of(part) {
            out.push(p);
        }
    }
    out
}

fn file_label(mime: &str) -> &'static str {
    if mime.starts_with("audio/") {
        "audio"
    } else if mime == "application/pdf" || mime.starts_with("text/") {
        "document"
    } else {
        "file"
    }
}

/// A non-text part.
fn part_of(part: &Value) -> Option<LlmPart> {
    if let Some(d) = get_any(part, &["inlineData", "inline_data"]) {
        let mime = str_of(d, "mimeType").or_else(|| str_of(d, "mime_type"));
        let data = str_of(d, "data").unwrap_or("");
        return Some(match mime {
            Some(m) if m.starts_with("image/") => image_data(Some(m), data),
            m => file_part(file_label(m.unwrap_or("")), m, None, None, Some(data)),
        });
    }
    if let Some(d) = get_any(part, &["fileData", "file_data"]) {
        let mime = str_of(d, "mimeType").or_else(|| str_of(d, "mime_type"));
        let uri = str_of(d, "fileUri").or_else(|| str_of(d, "file_uri"));
        return Some(match mime {
            Some(m) if m.starts_with("image/") => {
                let mut img = image_url(uri.unwrap_or(""));
                if let LlmPart::Image { media_type, .. } = &mut img {
                    *media_type = Some(m.to_string());
                }
                img
            }
            m => file_part(file_label(m.unwrap_or("")), m, None, uri, None),
        });
    }
    if let Some(f) = get_any(part, &["functionCall", "function_call"]) {
        return Some(LlmPart::ToolCall {
            id: str_of(f, "id").map(str::to_string),
            name: str_of(f, "name").unwrap_or("(unnamed)").to_string(),
            arguments: f.get("args").cloned().unwrap_or(Value::Object(Map::new())),
        });
    }
    if let Some(f) = get_any(part, &["functionResponse", "function_response"]) {
        let response = f.get("response").cloned().unwrap_or(Value::Null);
        let text = match &response {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        return Some(LlmPart::ToolResult {
            id: str_of(f, "id").map(str::to_string),
            name: str_of(f, "name").map(str::to_string),
            content: vec![LlmPart::Text { text }],
            is_error: false,
        });
    }
    if let Some(c) = get_any(part, &["executableCode", "executable_code"]) {
        return Some(LlmPart::ToolCall {
            id: None,
            name: "code_execution".into(),
            arguments: c.clone(),
        });
    }
    if let Some(r) = get_any(part, &["codeExecutionResult", "code_execution_result"]) {
        let outcome = str_of(r, "outcome").unwrap_or("");
        return Some(LlmPart::ToolResult {
            id: None,
            name: Some("code_execution".into()),
            content: str_of(r, "output")
                .map(|o| {
                    vec![LlmPart::Text {
                        text: o.to_string(),
                    }]
                })
                .unwrap_or_default(),
            is_error: !outcome.is_empty() && outcome != "OUTCOME_OK",
        });
    }
    let obj = part.as_object()?;
    let label = obj
        .keys()
        .find(|k| {
            !matches!(
                k.as_str(),
                "thought" | "thoughtSignature" | "thought_signature"
            )
        })?
        .clone();
    Some(LlmPart::Other {
        label,
        value: part.clone(),
    })
}

fn usage(u: Option<&Value>) -> Option<LlmUsage> {
    let u = u.filter(|u| u.is_object())?;
    let get = |a: &str, b: &str| u64_of(u, a).or_else(|| u64_of(u, b));
    let prompt = get("promptTokenCount", "prompt_token_count");
    let tool_prompt = get("toolUsePromptTokenCount", "tool_use_prompt_token_count");
    let candidates = get("candidatesTokenCount", "candidates_token_count");
    let thoughts = get("thoughtsTokenCount", "thoughts_token_count");
    let input = match (prompt, tool_prompt) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    let output = match (candidates, thoughts) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0) + b.unwrap_or(0)),
    };
    usage_or_none(LlmUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: get("totalTokenCount", "total_token_count"),
        cache_read_tokens: get("cachedContentTokenCount", "cached_content_token_count")
            .filter(|n| *n > 0),
        cache_write_tokens: None,
        reasoning_tokens: thoughts.filter(|n| *n > 0),
    })
}

#[derive(Default)]
struct CandidateAcc {
    role: Option<String>,
    content: Vec<LlmPart>,
    finish: Option<String>,
}

/// Merge response chunks (a whole response is a single chunk).
pub(super) fn chunks<'a>(
    items: impl Iterator<Item = (&'a Value, Option<f64>)>,
    w: &mut Warnings,
) -> ParsedResponse {
    let mut out = ParsedResponse::default();
    let mut candidates: BTreeMap<u64, CandidateAcc> = BTreeMap::new();
    for (chunk, at) in items {
        if chunk.get("candidates").is_none()
            && chunk.get("error").is_some()
            && let Some(e) = error_from_value(chunk)
        {
            out.error = Some(e);
            continue;
        }
        let r = &mut out.response;
        if r.id.is_none() {
            r.id = str_of(chunk, "responseId")
                .or_else(|| str_of(chunk, "response_id"))
                .map(str::to_string);
        }
        if let Some(m) = str_of(chunk, "modelVersion").or_else(|| str_of(chunk, "model_version")) {
            r.model = Some(m.to_string());
        }
        if let Some(u) = usage(get_any(chunk, &["usageMetadata", "usage_metadata"])) {
            r.usage = Some(u);
        }
        if let Some(reason) = get_any(chunk, &["promptFeedback", "prompt_feedback"])
            .and_then(|f| str_of(f, "blockReason").or_else(|| str_of(f, "block_reason")))
        {
            w.push(format!("The prompt was blocked ({reason})"));
            r.stop_reason = Some(reason.to_string());
            out.complete = true;
        }
        let mut produced = false;
        for (pos, c) in chunk
            .get("candidates")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let idx = u64_of(c, "index").unwrap_or(pos as u64);
            let acc = candidates.entry(idx).or_default();
            if let Some(content) = c.get("content") {
                if let Some(role) = str_of(content, "role") {
                    acc.role = Some(role.to_string());
                }
                for part in content
                    .get("parts")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(text) = str_of(part, "text") {
                        if part.get("thought").and_then(Value::as_bool) == Some(true) {
                            push_thinking(&mut acc.content, text);
                        } else {
                            push_text(&mut acc.content, text);
                        }
                        produced |= !text.is_empty();
                    } else if let Some(p) = part_of(part) {
                        acc.content.push(p);
                        produced = true;
                    }
                }
            }
            if let Some(f) = str_of(c, "finishReason").or_else(|| str_of(c, "finish_reason")) {
                acc.finish = Some(f.to_string());
            }
        }
        if produced && out.first_token_ms.is_none() {
            out.first_token_ms = at;
        }
    }
    if candidates.values().any(|c| c.finish.is_some()) {
        out.complete = true;
    }
    if out.response.stop_reason.is_none() {
        out.response.stop_reason = candidates.values().find_map(|c| c.finish.clone());
    }
    out.response.messages = candidates
        .into_values()
        .filter(|c| !c.content.is_empty() || c.role.is_some())
        .map(|c| LlmMessage {
            role: role(c.role.as_deref().unwrap_or("model")),
            name: None,
            content: c.content,
        })
        .collect();
    out
}

pub(super) fn stream(events: &[StreamEvent], w: &mut Warnings) -> ParsedResponse {
    let mut bad = 0usize;
    let parsed: Vec<(Value, Option<f64>)> = events
        .iter()
        .filter(|e| !e.data.trim().is_empty())
        .filter_map(|e| match serde_json::from_str::<Value>(e.data.trim()) {
            Ok(v) => Some((v, e.at_ms)),
            Err(_) => {
                bad += 1;
                None
            }
        })
        .collect();
    if bad > 0 {
        w.push(format!("Skipped {bad} malformed stream events"));
    }
    chunks(parsed.iter().map(|(v, at)| (v, *at)), w)
}
