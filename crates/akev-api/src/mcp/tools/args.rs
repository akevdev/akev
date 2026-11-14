//! Reading and validating tool arguments.

use super::*;

pub(super) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

pub(super) fn arg_bool(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(default)
}

pub(super) fn arg_u64(args: &Value, key: &str, default: u64, max: u64) -> Result<u64, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
            .map(|n| n.min(max))
            .ok_or_else(|| format!("`{key}` must be a non-negative integer")),
    }
}

pub(super) fn arg_id(args: &Value, key: &str) -> Result<ExchangeId, String> {
    let v = args
        .get(key)
        .ok_or_else(|| format!("`{key}` is required"))?;
    v.as_u64()
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().trim_start_matches('#').parse().ok())
        })
        .map(ExchangeId)
        .ok_or_else(|| format!("`{key}` must be an exchange id (integer)"))
}

pub(super) fn arg_headers(args: &Value, key: &str) -> Result<Option<Headers>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => serde_json::from_value::<Headers>(v.clone())
            .map(Some)
            .map_err(|e| {
                format!("`{key}` must be an object of header values or [[name, value], ...]: {e}")
            }),
    }
}

pub(super) fn parse_filter(args: &Value) -> Result<akev_filter::Filter, String> {
    let q = arg_str(args, "filter").unwrap_or("");
    akev_filter::Filter::parse(q).map_err(|e| {
        format!(
            "invalid filter at {}..{}: {}. Call akev_filter_help for the syntax.",
            e.start, e.end, e.message
        )
    })
}

/// Body from `body` / `bodyBase64` / `json` arguments. Returns (body, is_json).
pub(super) fn arg_body(args: &Value) -> Result<Option<(Bytes, bool)>, String> {
    if let Some(b64) = arg_str(args, "bodyBase64") {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64.trim())
            .map_err(|e| format!("`bodyBase64` is not valid base64: {e}"))?;
        return Ok(Some((Bytes::from(bytes), false)));
    }
    if let Some(v) = args.get("json").filter(|v| !v.is_null()) {
        let bytes = serde_json::to_vec(v).map_err(|e| e.to_string())?;
        return Ok(Some((Bytes::from(bytes), true)));
    }
    match args.get("body") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) => Ok(Some((Bytes::from(s.clone()), false))),
        Some(other) => Ok(Some((
            Bytes::from(serde_json::to_vec(other).map_err(|e| e.to_string())?),
            true,
        ))),
    }
}

/// Truncate to at most `max` chars (on a char boundary). Returns (text, truncated).
pub fn truncate_chars(s: &str, max: usize) -> (&str, bool) {
    match s.char_indices().nth(max) {
        Some((idx, _)) => (&s[..idx], true),
        None => (s, false),
    }
}

/// Char-based slice `[offset, offset+len)`.
pub(super) fn slice_chars(s: &str, offset: usize, len: usize) -> &str {
    let start = s
        .char_indices()
        .nth(offset)
        .map(|(i, _)| i)
        .unwrap_or(s.len());
    let rest = &s[start..];
    truncate_chars(rest, len).0
}
