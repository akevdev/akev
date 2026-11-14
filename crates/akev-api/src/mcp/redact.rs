//! Secrets in MCP tool output: captured traffic is redacted by default before an AI agent
//! sees it (see `akev_core::redact`); a tool call may pass `reveal: true` only when the user
//! started Akev with `--mcp-reveal-secrets`.
//!
//! Tools redact what they know the shape of (header lists, URLs, bodies with their content
//! type, the text a search runs over) and [`json`] sweeps the whole result afterwards:
//! `[name, value]` header pairs, URLs, secret-named JSON fields (also inside decoded
//! protobuf / MessagePack / CBOR bodies) and token patterns in every string.

use akev_core::redact;
use serde_json::Value;
use std::borrow::Cow;

/// Tools whose output carries captured headers, URLs or bodies (they accept `reveal`).
pub const TOOLS: &[&str] = &[
    "akev_list_exchanges",
    "akev_get_exchange",
    "akev_get_body",
    "akev_search",
    "akev_wait_for",
    "akev_send_request",
    "akev_replay",
    "akev_export_har",
    "akev_snippet",
    "akev_llm_conversation",
    "akev_list_breakpoints",
];

/// Schema of the `reveal` argument.
pub fn reveal_property() -> Value {
    serde_json::json!({
        "type": "boolean",
        "description": "Show secrets (Authorization / Cookie / API-key headers, secret query parameters, tokens in bodies) instead of [redacted]. Only works when the user started Akev with --mcp-reveal-secrets; don't ask for it unless the user needs the actual values."
    })
}

/// Whether this call's output must be redacted. `Err` when the agent asked to reveal
/// secrets the user didn't allow.
pub fn wanted(args: &Value, reveal_allowed: bool) -> Result<bool, String> {
    let reveal = args.get("reveal").and_then(Value::as_bool).unwrap_or(false);
    if reveal && !reveal_allowed {
        return Err(
            "Secrets stay redacted: `reveal` only works when the user started Akev \
                    with --mcp-reveal-secrets (`akev start --mcp-reveal-secrets`, or \
                    `akev mcp --reveal-secrets` for an embedded instance). Call the tool again \
                    without `reveal`."
                .into(),
        );
    }
    Ok(!reveal)
}

fn looks_like_url(s: &str) -> bool {
    ["http://", "https://", "ws://", "wss://"]
        .iter()
        .any(|p| s.len() > p.len() && s[..p.len()].eq_ignore_ascii_case(p))
}

/// A body's text: form bodies get the query-string rules, others the token patterns.
pub fn body_text<'a>(text: &'a str, content_type: Option<&str>) -> Cow<'a, str> {
    let form = content_type
        .and_then(|c| c.split(';').next())
        .is_some_and(|c| {
            c.trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        });
    if !form {
        return redact::text(text);
    }
    // Formatted form bodies have one field per line.
    if !text.contains('\n') {
        return redact::query(text);
    }
    let lines: Vec<Cow<'_, str>> = text.split('\n').map(redact::query).collect();
    if lines.iter().all(|l| matches!(l, Cow::Borrowed(_))) {
        return Cow::Borrowed(text);
    }
    Cow::Owned(lines.join("\n"))
}

/// A string from tool output: URL rules for URLs, token patterns for everything.
pub fn string(s: &str) -> Cow<'_, str> {
    let url = if looks_like_url(s) && !s.contains(char::is_whitespace) {
        redact::url(s)
    } else {
        Cow::Borrowed(s)
    };
    match redact::text(&url) {
        Cow::Borrowed(_) => url,
        Cow::Owned(t) => Cow::Owned(t),
    }
}

/// Redact a JSON tool result in place.
pub fn json(value: &mut Value) {
    match value {
        Value::String(s) => {
            if let Cow::Owned(clean) = string(s) {
                *s = clean;
            }
        }
        Value::Array(items) => {
            // A `[name, value]` pair: a header, or a decoded form / query field.
            if let [Value::String(name), Value::String(v)] = items.as_mut_slice() {
                if let Some(clean) = redact::header_value(name, v) {
                    *v = clean;
                    return;
                }
                if !v.is_empty() && redact::is_secret_name(name) {
                    *v = redact::REDACTED.to_string();
                    return;
                }
            }
            items.iter_mut().for_each(json);
        }
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                match v {
                    Value::String(s) if !s.is_empty() && redact::is_secret_name(key) => {
                        *s = redact::REDACTED.to_string();
                    }
                    Value::String(s) if key == "path" => {
                        if let Cow::Owned(clean) = redact::path(s) {
                            *s = clean;
                        }
                        if let Cow::Owned(clean) = redact::text(s) {
                            *s = clean;
                        }
                    }
                    _ => json(v),
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn reveal_needs_the_users_opt_in() {
        assert_eq!(wanted(&json!({}), false), Ok(true));
        assert_eq!(wanted(&json!({"reveal": false}), true), Ok(true));
        assert_eq!(wanted(&json!({"reveal": true}), true), Ok(false));
        let err = wanted(&json!({"reveal": true}), false).unwrap_err();
        assert!(err.contains("--mcp-reveal-secrets"), "{err}");
    }

    #[test]
    fn json_results_are_swept() {
        let mut v = json!({
            "url": "https://api.test/v1?api_key=AIza&q=1",
            "request": {
                "headers": [["Authorization", "Bearer abcdefgh1234"], ["Accept", "*/*"]],
                "body": { "text": "{\"password\":\"hunter2\"}", "structured": { "session": { "accessToken": "t0k" }, "count": 3 } }
            },
            "summary": { "path": "/v1?token=abc" },
            "form": { "text": "user=me\npassword=pw", "structured": [["user", "me"], ["password", "pw"]] },
            "messages": [{ "text": "Bearer abcdefgh1234 sk-ant-api03-abcdefghijklmnopqrstu" }],
            "tokens": 12,
            "note": "ordinary text"
        });
        json(&mut v);
        assert_eq!(v["url"], "https://api.test/v1?api_key=[redacted]&q=1");
        assert_eq!(v["request"]["headers"][0][1], "Bearer [redacted]");
        assert_eq!(v["request"]["headers"][1][1], "*/*");
        assert_eq!(
            v["request"]["body"]["text"],
            "{\"password\":\"[redacted]\"}"
        );
        assert_eq!(
            v["request"]["body"]["structured"]["session"]["accessToken"],
            "[redacted]"
        );
        assert_eq!(v["request"]["body"]["structured"]["count"], 3);
        assert_eq!(v["summary"]["path"], "/v1?token=[redacted]");
        assert_eq!(v["form"]["structured"][0][1], "me");
        assert_eq!(v["form"]["structured"][1][1], "[redacted]");
        assert_eq!(
            body_text(
                "user=me\npassword=pw",
                Some("application/x-www-form-urlencoded")
            ),
            "user=me\npassword=[redacted]"
        );
        assert_eq!(
            body_text("a=1&secret=s", Some("application/x-www-form-urlencoded")),
            "a=1&secret=[redacted]"
        );
        assert_eq!(v["messages"][0]["text"], "Bearer [redacted] [redacted]");
        assert_eq!(v["tokens"], 12);
        assert_eq!(v["note"], "ordinary text");
    }
}
