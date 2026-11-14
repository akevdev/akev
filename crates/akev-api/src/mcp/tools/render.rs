//! How exchanges are shown to agents: compact rows and decoded, truncated bodies.

use super::*;

/// Absolute URL of an exchange from its summary (default ports omitted, IPv6 bracketed).
pub fn summary_url(s: &ExchangeSummary) -> String {
    let default_port = matches!(
        (s.scheme.as_str(), s.port),
        ("http" | "ws", 80) | ("https" | "wss", 443)
    );
    let host = if s.host.contains(':') {
        format!("[{}]", s.host)
    } else {
        s.host.clone()
    };
    if default_port || s.port == 0 {
        format!("{}://{}{}", s.scheme, host, s.path)
    } else {
        format!("{}://{}:{}{}", s.scheme, host, s.port, s.path)
    }
}

pub(super) fn iso_ms(ms: f64) -> String {
    UnixMicros((ms * 1000.0) as i64).to_rfc3339()
}

pub(super) fn to_value<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

/// Compact row for lists (defaults omitted to save tokens).
pub fn row(s: &ExchangeSummary) -> Value {
    let mut m = Map::new();
    m.insert("id".into(), json!(s.id.0));
    m.insert("method".into(), json!(s.method));
    m.insert("url".into(), json!(summary_url(s)));
    if let Some(status) = s.status {
        m.insert("status".into(), json!(status));
    }
    m.insert("state".into(), to_value(&s.state));
    if s.kind != ExchangeKind::Http {
        m.insert("kind".into(), to_value(&s.kind));
    }
    if let Some(d) = s.duration {
        m.insert("durationMs".into(), json!((d * 10.0).round() / 10.0));
    }
    if s.request_size > 0 {
        m.insert("requestSize".into(), json!(s.request_size));
    }
    if s.response_size > 0 {
        m.insert("responseSize".into(), json!(s.response_size));
    }
    if let Some(ct) = &s.content_type {
        m.insert("contentType".into(), json!(ct));
    }
    if let Some(e) = &s.error {
        m.insert("error".into(), to_value(e));
    }
    if let Some(src) = &s.source {
        m.insert("source".into(), json!(src));
    }
    if let Some(rule) = &s.rule {
        m.insert("rule".into(), json!(rule));
    }
    for (flag, on) in [
        ("mocked", s.mocked),
        ("modified", s.modified),
        ("pinned", s.pinned),
    ] {
        if on {
            m.insert(flag.into(), json!(true));
        }
    }
    if let Some(p) = &s.paused {
        m.insert("paused".into(), to_value(p));
    }
    if s.message_count > 0 {
        m.insert("messages".into(), json!(s.message_count));
    }
    if let Some(llm) = &s.llm {
        m.insert("llm".into(), to_value(llm));
    }
    m.insert("startedAt".into(), json!(iso_ms(s.started_at)));
    Value::Object(m)
}

/// Agent-friendly rendering of a body view (`redact`: form bodies get the query-string rules
/// before truncation; everything else is swept by [`redact::json`]).
pub(super) fn agent_body(
    view: &ApiBodyView,
    content_type: Option<&str>,
    max_chars: usize,
    redact: bool,
) -> Value {
    let v = &view.view;
    let mut m = Map::new();
    m.insert("kind".into(), to_value(&v.kind));
    if let Some(ct) = content_type {
        m.insert("contentType".into(), json!(ct));
    }
    m.insert("size".into(), json!(v.size));
    if view.wire_size as usize != v.size {
        m.insert("wireSize".into(), json!(view.wire_size));
    }
    if !view.encodings.is_empty() {
        m.insert("encodings".into(), json!(view.encodings));
    }
    if view.capture_truncated {
        m.insert("captureTruncated".into(), json!(true));
    }
    if let Some(err) = &v.decode_error {
        m.insert("decodeError".into(), json!(err));
    }
    if v.size == 0 {
        return Value::Object(m);
    }
    let text = match (&v.formatted, &v.text) {
        (Some(f), _) if f.chars().count() <= max_chars => Some(f.as_str()),
        (_, Some(t)) => Some(t.as_str()),
        (Some(f), None) => Some(f.as_str()),
        _ => None,
    };
    let text = text.map(|t| {
        if redact {
            redact::body_text(t, content_type)
        } else {
            std::borrow::Cow::Borrowed(t)
        }
    });
    let text = text.as_deref();
    if let Some(t) = text {
        let (shown, truncated) = truncate_chars(t, max_chars);
        m.insert("text".into(), json!(shown));
        if truncated || v.truncated {
            m.insert("truncated".into(), json!(true));
            m.insert("totalChars".into(), json!(t.chars().count()));
            m.insert(
                "hint".into(),
                json!("use akev_get_body with an offset to read the rest"),
            );
        }
    }
    if let Some(structured) = &v.structured {
        let rendered = serde_json::to_string(structured).unwrap_or_default();
        if text.is_none() || rendered.len() <= max_chars {
            if rendered.chars().count() <= max_chars {
                m.insert("structured".into(), structured.clone());
            } else {
                m.insert(
                    "structured".into(),
                    json!(truncate_chars(&rendered, max_chars).0),
                );
                m.insert("structuredTruncated".into(), json!(true));
            }
        }
    }
    if let Some(schema) = &v.schema {
        // The protobuf schema that decoded the body (typed decode of protobuf bodies; gRPC
        // messages carry theirs in `structured`).
        let rendered = to_value(schema);
        if serde_json::to_string(&rendered).map_or(0, |s| s.chars().count()) <= max_chars {
            m.insert("schema".into(), rendered);
        } else {
            m.insert(
                "schema".into(),
                json!({ "messageType": schema.message_type, "truncated": true }),
            );
        }
    }
    if text.is_none() && v.structured.is_none() {
        m.insert("binary".into(), json!(true));
        m.insert(
            "hint".into(),
            json!("binary body: use akev_get_body with format=base64"),
        );
    }
    Value::Object(m)
}
