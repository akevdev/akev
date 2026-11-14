//! Tools that control the app: breakpoints, recording, interceptors, filter help.

use super::*;

pub(super) fn list_breakpoints(app: &Arc<App>) -> ToolResult {
    let paused: Vec<Value> = app
        .breakpoints
        .list()
        .into_iter()
        .map(|p| {
            let preview = |b: &Bytes| match std::str::from_utf8(b) {
                Ok(t) => json!(truncate_chars(t, 2000).0),
                Err(_) => json!(format!("<{} bytes of binary data>", b.len())),
            };
            let mut v = json!({
                "id": p.id.0,
                "stage": p.stage,
                "request": { "method": p.request.method, "url": p.request.url, "headers": p.request.headers, "body": preview(&p.request.body) },
            });
            if let Some(r) = &p.response {
                v["response"] = json!({ "status": r.status, "statusMessage": r.status_message, "headers": r.headers, "body": preview(&r.body) });
            }
            v
        })
        .collect();
    Ok(Out::Json(json!({ "paused": paused })))
}

pub(super) fn resume_breakpoint(app: &Arc<App>, args: &Value) -> ToolResult {
    let id = arg_id(args, "id")?;
    let action = arg_str(args, "action").unwrap_or("continue");
    let paused = app
        .breakpoints
        .list()
        .into_iter()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("exchange {id} is not paused (see akev_list_breakpoints)"))?;
    let payload = match (paused.stage, action) {
        (BreakpointStage::Request, "continue") => {
            let mut req = paused.request;
            apply_request_edits(&mut req, args)?;
            to_value(&RequestResume::Continue { request: req })
        }
        (BreakpointStage::Request, "respond") => {
            let mut headers = arg_headers(args, "setHeaders")?.unwrap_or_default();
            let body = match arg_body(args)? {
                Some((b, is_json)) => {
                    if is_json && !headers.contains("content-type") {
                        headers.append("Content-Type", "application/json");
                    }
                    b
                }
                None => Bytes::new(),
            };
            let status = arg_u64(args, "status", 200, 999)? as u16;
            let status_message = arg_str(args, "statusMessage")
                .map(str::to_string)
                .unwrap_or_else(|| akev_core::canonical_reason(status).to_string());
            to_value(&RequestResume::Respond {
                response: EditableResponse {
                    status,
                    status_message,
                    headers,
                    body,
                },
            })
        }
        (BreakpointStage::Response, "continue") => {
            let mut res = paused.response.ok_or("paused response is missing")?;
            if args.get("status").is_some() {
                res.status = arg_u64(args, "status", res.status as u64, 999)? as u16;
            }
            if let Some(m) = arg_str(args, "statusMessage") {
                res.status_message = m.to_string();
            }
            apply_header_edits(&mut res.headers, args)?;
            if let Some((b, _)) = arg_body(args)? {
                res.body = b;
            }
            to_value(&ResponseResume::Continue { response: res })
        }
        (_, "close") => json!({ "action": "close" }),
        (_, "reset") => json!({ "action": "reset" }),
        (stage, other) => {
            return Err(format!(
                "action {other:?} is not valid at the {stage:?} stage"
            ));
        }
    };
    app.breakpoints
        .resume(id, payload)
        .map_err(|e| e.to_string())?;
    Ok(Out::Json(json!({ "resumed": id.0, "action": action })))
}

pub(super) fn set_recording(app: &Arc<App>, args: &Value) -> ToolResult {
    let on = args
        .get("recording")
        .and_then(Value::as_bool)
        .ok_or("`recording` (boolean) is required")?;
    app.set_recording(on);
    Ok(Out::Json(
        json!({ "recording": app.recording.is_recording() }),
    ))
}

pub(super) async fn list_interceptors(app: &Arc<App>) -> ToolResult {
    let list = app.refresh_interceptors(false).await;
    let rows: Vec<Value> = list
        .iter()
        .map(|i| {
            let mut v = json!({
                "id": i.id,
                "name": i.name,
                "category": i.category,
                "available": i.available,
                "active": i.active,
            });
            if i.needs_config {
                v["needsConfig"] = json!(true);
            }
            if let Some(r) = &i.unavailable_reason {
                v["unavailableReason"] = json!(r);
            }
            if !i.supported {
                v["supported"] = json!(false);
            }
            v
        })
        .collect();
    Ok(Out::Json(json!({ "interceptors": rows })))
}

/// Interceptors that start an arbitrary local executable chosen by the caller.
pub(super) const PROGRAM_LAUNCHING_INTERCEPTORS: &[&str] = &["electron"];

/// Why MCP may not activate interceptor `id` (`None` = allowed). Agents read captured,
/// possibly attacker-controlled traffic, so starting arbitrary programs needs the user's
/// explicit opt-in (`AkevOptions::mcp_allow_launch`).
pub(super) fn mcp_launch_refusal(id: &str, launch_allowed: bool) -> Option<String> {
    (!launch_allowed && PROGRAM_LAUNCHING_INTERCEPTORS.contains(&id)).then(|| {
        format!(
            "The `{id}` interceptor starts an arbitrary program, which AI agents may not do unless the user allows it. \
             Ask the user to launch the app from Akev's Connect page, or to restart Akev with `akev start --mcp-allow-launch` \
             (`akev mcp --allow-launch` for an embedded instance) to allow it over MCP."
        )
    })
}

pub(super) async fn activate_interceptor(app: &Arc<App>, args: &Value) -> ToolResult {
    let id = arg_str(args, "id").ok_or("`id` is required")?;
    if let Some(refusal) = mcp_launch_refusal(id, app.mcp_allow_launch) {
        return Err(refusal);
    }
    let options = args.get("options").cloned().unwrap_or(Value::Null);
    let result = crate::routes::misc::activate_interceptor(app, id, options)
        .await
        .map_err(|e| e.message)?;
    Ok(Out::Json(to_value(&result)))
}

pub(super) async fn deactivate_interceptor(app: &Arc<App>, args: &Value) -> ToolResult {
    let id = arg_str(args, "id").ok_or("`id` is required")?;
    let options = args.get("options").cloned().unwrap_or(Value::Null);
    crate::routes::misc::deactivate_interceptor(app, id, options)
        .await
        .map_err(|e| e.message)?;
    Ok(Out::Json(json!({ "deactivated": id })))
}

pub(super) fn filter_help() -> Out {
    let mut text = String::from(FILTER_SYNTAX);
    text.push_str("\n\nFields and flags:\n");
    for doc in akev_filter::field_docs() {
        text.push_str("- ");
        text.push_str(doc.name);
        if !doc.aliases.is_empty() {
            text.push_str(&format!(" (aliases: {})", doc.aliases.join(", ")));
        }
        text.push_str(": ");
        text.push_str(doc.description);
        if !doc.examples.is_empty() {
            text.push_str(&format!(" — e.g. {}", doc.examples.join(", ")));
        }
        text.push('\n');
    }
    Out::Text(text)
}
