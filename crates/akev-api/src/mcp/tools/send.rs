//! Tools that send requests: compose and replay.

use super::*;

pub(super) fn send_options(args: &Value) -> Result<SendOptions, String> {
    let mut o = SendOptions::default();
    o.apply_rules = arg_bool(args, "applyRules", o.apply_rules);
    o.follow_redirects = arg_bool(args, "followRedirects", o.follow_redirects);
    o.insecure = arg_bool(args, "insecure", o.insecure);
    if let Some(v) = arg_str(args, "httpVersion") {
        o.http_version = serde_json::from_value(Value::String(v.to_ascii_lowercase()))
            .map_err(|_| format!("httpVersion must be auto, http1, http2 or http3 (got {v:?})"))?;
    }
    o.timeout_ms = arg_u64(args, "timeoutMs", o.timeout_ms, 3_600_000)?.max(1);
    Ok(o)
}

pub(super) async fn send_and_render(
    app: &Arc<App>,
    request: EditableRequest,
    options: SendOptions,
    max_chars: usize,
    redact: bool,
) -> ToolResult {
    let out = crate::routes::compose::send_and_view(app, request, options)
        .await
        .map_err(|e| e.message)?;
    let mut v = json!({ "id": out.result.id.0, "timing": out.result.timing });
    if !out.recorded {
        v["recorded"] = json!(false);
        v["note"] = json!(
            "This exchange was not stored (recording is paused or its host is ignored), so its id can't be used with other tools."
        );
    }
    if let Some(head) = &out.result.response {
        v["status"] = json!(head.status);
        v["statusMessage"] = json!(head.status_message);
        v["httpVersion"] = to_value(&head.http_version);
        v["headers"] = to_value(&head.headers);
        if let Some(view) = &out.body {
            v["body"] = agent_body(view, head.headers.get("content-type"), max_chars, redact);
        }
    }
    if let Some(e) = &out.result.error {
        v["error"] = to_value(e);
    }
    Ok(Out::Json(v))
}

pub(super) async fn send_request(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let url = arg_str(args, "url").ok_or("`url` is required")?.to_string();
    let mut headers = arg_headers(args, "headers")?.unwrap_or_default();
    let body = arg_body(args)?;
    let method = arg_str(args, "method")
        .map(|m| m.trim().to_ascii_uppercase())
        .unwrap_or_else(|| {
            if body.is_some() {
                "POST".into()
            } else {
                "GET".into()
            }
        });
    let body = match body {
        Some((bytes, is_json)) => {
            if is_json && !headers.contains("content-type") {
                headers.append("Content-Type", "application/json");
            }
            bytes
        }
        None => Bytes::new(),
    };
    let max_chars = arg_u64(
        args,
        "maxBodyChars",
        DEFAULT_MAX_BODY_CHARS as u64,
        1_000_000,
    )? as usize;
    let request = EditableRequest {
        method,
        url,
        headers,
        body,
    };
    send_and_render(app, request, send_options(args)?, max_chars, redact).await
}

/// Apply agent-style edits (method/url/setHeaders/removeHeaders/body) to a request.
pub(super) fn apply_request_edits(req: &mut EditableRequest, m: &Value) -> Result<(), String> {
    if let Some(method) = arg_str(m, "method") {
        req.method = method.trim().to_ascii_uppercase();
    }
    if let Some(url) = arg_str(m, "url") {
        req.url = url.to_string();
    }
    apply_header_edits(&mut req.headers, m)?;
    if let Some((bytes, is_json)) = arg_body(m)? {
        if is_json && !req.headers.contains("content-type") {
            req.headers.append("Content-Type", "application/json");
        }
        req.body = bytes;
    }
    Ok(())
}

pub(super) fn apply_header_edits(headers: &mut Headers, m: &Value) -> Result<(), String> {
    if let Some(set) = arg_headers(m, "setHeaders")? {
        for (name, value) in set.0 {
            headers.set(&name, value);
        }
    }
    if let Some(remove) = m.get("removeHeaders").and_then(Value::as_array) {
        for name in remove.iter().filter_map(Value::as_str) {
            headers.remove(name);
        }
    }
    Ok(())
}

pub(super) async fn replay(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let id = arg_id(args, "id")?;
    let (mut req, _, truncated) = captured_request(app, id).map_err(|e| e.message)?;
    req.headers = strip_proxy_headers(req.headers);
    let mods = args.get("modifications").cloned().unwrap_or(Value::Null);
    let replaces_body = mods.get("body").is_some()
        || mods.get("bodyBase64").is_some()
        || mods.get("json").is_some();
    if truncated && !replaces_body {
        return Err(format!(
            "the request body of exchange {id} was only partially captured; provide modifications.body to replay it"
        ));
    }
    if mods.is_object() {
        apply_request_edits(&mut req, &mods)?;
    }
    let max_chars = arg_u64(
        args,
        "maxBodyChars",
        DEFAULT_MAX_BODY_CHARS as u64,
        1_000_000,
    )? as usize;
    let mut out = send_and_render(app, req, send_options(args)?, max_chars, redact).await?;
    if let Out::Json(v) = &mut out {
        v["replayOf"] = json!(id.0);
    }
    Ok(out)
}
