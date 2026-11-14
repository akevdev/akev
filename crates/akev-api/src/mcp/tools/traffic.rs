//! Tools that read captured traffic: status, listing, details, bodies, search, waiting, export, snippets and LLM conversations.

use super::*;

pub(super) fn status(app: &Arc<App>) -> ToolResult {
    let status = app.status();
    let ctx = app.intercept_ctx();
    let env: Map<String, Value> = akev_intercept::proxy_env_vars(&ctx)
        .into_iter()
        .map(|(k, v)| (k, json!(v)))
        .collect();
    let proxy_url = format!("http://{}:{}", ctx.proxy_host, ctx.proxy_port);
    Ok(Out::Json(json!({
        "version": status.version,
        "proxy": {
            "url": proxy_url,
            "port": status.proxy.port,
            "bindAddress": status.proxy.bind_address,
            "addresses": status.proxy.addresses,
        },
        "ca": {
            "certificatePath": app.ca_path.display().to_string(),
            "subject": status.ca.subject,
            "sha256Fingerprint": status.ca.sha256_fingerprint,
            "notAfter": status.ca.not_after,
            "daysRemaining": status.ca.days_remaining,
            "expiry": status.ca.expiry,
        },
        "recording": app.recording.is_recording(),
        "files": {
            "exportsDir": app.mcp_files.exports_dir().display().to_string(),
            "allowedPaths": app.mcp_files.allowed_dirs(),
            "note": "akev_export_har writes files, and rules you create read them, only inside these folders.",
        },
        "stats": status.stats,
        "pausedAtBreakpoints": app.breakpoints.len(),
        "apiUrl": status.api_url,
        "dataDir": status.data_dir,
        "environment": env,
        "howTo": format!(
            "Run a command through Akev with the environment above, e.g. `HTTPS_PROXY={proxy_url} HTTP_PROXY={proxy_url} \
             SSL_CERT_FILE={ca} NODE_EXTRA_CA_CERTS={ca} <command>`, or `curl --proxy {proxy_url} --cacert {ca} https://...`.",
            ca = app.ca_path.display()
        ),
    })))
}

pub(super) async fn list_exchanges(app: &Arc<App>, args: &Value) -> ToolResult {
    let filter = parse_filter(args)?;
    let limit = arg_u64(args, "limit", 50, MAX_LIST_LIMIT)?.max(1) as usize;
    let a = app.clone();
    let matches = blocking(move || a.store.query(&filter, None))
        .await
        .map_err(|e| e.message)?;
    let total = matches.len();
    let rows: Vec<Value> = matches[total.saturating_sub(limit)..]
        .iter()
        .map(row)
        .collect();
    let mut out = json!({ "total": total, "returned": rows.len(), "exchanges": rows });
    if total > limit {
        out["note"] = json!(format!(
            "showing the {limit} most recent of {total} matches; refine the filter or raise limit"
        ));
    }
    Ok(Out::Json(out))
}

pub(super) async fn get_exchange(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let id = arg_id(args, "id")?;
    let include_bodies = arg_bool(args, "includeBodies", true);
    let max_chars = arg_u64(
        args,
        "maxBodyChars",
        DEFAULT_MAX_BODY_CHARS as u64,
        1_000_000,
    )? as usize;
    let detail = app
        .store
        .detail(id)
        .ok_or_else(|| format!("exchange {id} not found"))?;
    let mut out = Map::new();
    out.insert("id".into(), json!(id.0));
    out.insert("summary".into(), row(&detail.summary));
    let mut request = json!({
        "method": detail.request.method,
        "url": detail.request.url,
        "httpVersion": detail.request.http_version,
        "headers": detail.request.headers,
    });
    let mut response = detail.response.as_ref().map(|r| {
        json!({
            "status": r.status,
            "statusMessage": r.status_message,
            "httpVersion": r.http_version,
            "headers": r.headers,
        })
    });
    if include_bodies {
        let store = app.store.clone();
        let protos = app.protos.clone();
        let d = detail.clone();
        let (req_view, res_view) = body_view_blocking(app, move || {
            let path = d.request.path_only().to_string();
            // Imported protobuf schemas decode protobuf / gRPC bodies with field names.
            let view = |side: Side, b: &akev_core::CapturedBody, headers: &akev_core::Headers| {
                let plan = protos.plan(side, &d.request.url, &path, headers.get("content-type"));
                body::body_view_schema(b, headers, &path, plan.as_ref())
            };
            let req = store
                .body(id, Side::Request)
                .map(|b| view(Side::Request, &b, &d.request.headers));
            let res = match (&d.response, store.body(id, Side::Response)) {
                (Some(head), Some(b)) => Some(view(Side::Response, &b, &head.headers)),
                _ => None,
            };
            (req, res)
        })
        .await
        .map_err(|e| e.message)?;
        if let Some(v) = req_view.filter(|v| v.view.size > 0 || v.wire_size > 0) {
            request["body"] = agent_body(
                &v,
                detail.request.headers.get("content-type"),
                max_chars,
                redact,
            );
        }
        if let (Some(resp), Some(v), Some(head)) =
            (response.as_mut(), res_view, detail.response.as_ref())
        {
            resp["body"] = agent_body(&v, head.headers.get("content-type"), max_chars, redact);
        }
    }
    out.insert("request".into(), request);
    if let Some(r) = response {
        out.insert("response".into(), r);
    }
    if let Some(orig) = &detail.original_request {
        out.insert(
            "originalRequest".into(),
            json!({ "method": orig.method, "url": orig.url, "headers": orig.headers, "note": "as received from the client, before rules/breakpoints modified it" }),
        );
    }
    if let Some(orig) = &detail.original_response {
        out.insert(
            "originalResponse".into(),
            json!({ "status": orig.status, "headers": orig.headers, "note": "as received from upstream, before modification" }),
        );
    }
    out.insert("timing".into(), to_value(&detail.timing));
    out.insert("client".into(), to_value(&detail.client));
    if let Some(u) = &detail.upstream {
        out.insert("upstream".into(), to_value(u));
    }
    if let Some(e) = &detail.error {
        out.insert("error".into(), to_value(e));
    }
    if let Some(t) = &detail.tunnel {
        out.insert("tunnel".into(), to_value(t));
    }
    if !detail.matched_rules.is_empty() {
        out.insert("matchedRules".into(), json!(detail.matched_rules));
    }
    if let Some(n) = &detail.note {
        out.insert("note".into(), json!(n));
    }
    if detail.summary.message_count > 0 {
        let messages = app.store.messages(id);
        let total = messages.len();
        let shown: Vec<Value> = messages
            .iter()
            .take(50)
            .map(|m| {
                let mut v = to_value(&m.message);
                match std::str::from_utf8(&m.payload) {
                    Ok(t) => {
                        let (s, cut) = truncate_chars(t, 500);
                        v["text"] = json!(s);
                        if cut {
                            v["textTruncated"] = json!(true);
                        }
                    }
                    Err(_) => {
                        let prefix = &m.payload[..m.payload.len().min(256)];
                        v["base64"] =
                            json!(base64::engine::general_purpose::STANDARD.encode(prefix));
                    }
                }
                v
            })
            .collect();
        out.insert("messages".into(), json!({ "total": total, "shown": shown }));
    }
    Ok(Out::Json(Value::Object(out)))
}

pub(super) async fn get_body(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let id = arg_id(args, "id")?;
    let side = match arg_str(args, "side").unwrap_or("response") {
        "request" => Side::Request,
        "response" => Side::Response,
        other => return Err(format!("side must be request or response, not {other:?}")),
    };
    let format = arg_str(args, "format").unwrap_or("text").to_string();
    let offset = arg_u64(args, "offset", 0, u64::MAX)? as usize;
    let max = arg_u64(args, "maxChars", 50_000, 2_000_000)?.max(1) as usize;
    let original = arg_bool(args, "original", false);
    let detail = app
        .store
        .detail(id)
        .ok_or_else(|| format!("exchange {id} not found"))?;
    let headers = match side {
        Side::Request => if original {
            detail.original_request.as_ref().unwrap_or(&detail.request)
        } else {
            &detail.request
        }
        .headers
        .clone(),
        Side::Response => {
            let head = if original {
                detail
                    .original_response
                    .as_ref()
                    .or(detail.response.as_ref())
            } else {
                detail.response.as_ref()
            };
            head.ok_or_else(|| format!("exchange {id} has no response (yet)"))?
                .headers
                .clone()
        }
    };
    let captured = if original {
        app.store
            .original_body(id, side)
            .or_else(|| app.store.body(id, side))
    } else {
        app.store.body(id, side)
    };
    let captured = captured.unwrap_or_default();
    let path = detail.request.path_only().to_string();
    body_view_blocking(app, move || {
        let content_type = headers.get("content-type");
        if format == "base64" {
            let (bytes, decoded) = body::decode_or_raw(&captured.bytes, &headers);
            // Text bodies are redacted like the text formats (offsets count redacted bytes).
            let bytes: std::borrow::Cow<'_, [u8]> = match std::str::from_utf8(&bytes) {
                Ok(text) if redact => match redact::body_text(text, content_type) {
                    std::borrow::Cow::Owned(clean) => clean.into_bytes().into(),
                    std::borrow::Cow::Borrowed(_) => bytes.as_ref().into(),
                },
                _ => bytes.as_ref().into(),
            };
            let start = offset.min(bytes.len());
            let end = start.saturating_add(max).min(bytes.len());
            return Ok(Out::Json(json!({
                "id": id.0,
                "side": side,
                "totalBytes": bytes.len(),
                "offset": start,
                "returned": end - start,
                "truncated": end < bytes.len(),
                "decoded": decoded,
                "base64": base64::engine::general_purpose::STANDARD.encode(&bytes[start..end]),
            })));
        }
        let view = body::body_view(&captured, &headers, &path);
        let text = if format == "formatted" {
            view.view.formatted.as_ref().or(view.view.text.as_ref())
        } else {
            view.view.text.as_ref()
        };
        let text = text.map(|t| {
            if redact {
                redact::body_text(t, content_type)
            } else {
                std::borrow::Cow::Borrowed(t.as_str())
            }
        });
        let Some(text) = text.as_deref() else {
            return Err(match &view.view.decode_error {
                Some(e) => format!("body could not be decoded ({e}); use format=base64"),
                None if view.view.size == 0 => "the body is empty".to_string(),
                None => "binary body: use format=base64".to_string(),
            });
        };
        let total = text.chars().count();
        let slice = slice_chars(text, offset, max);
        let returned = slice.chars().count();
        Ok(Out::Json(json!({
            "id": id.0,
            "side": side,
            "kind": view.view.kind,
            "totalChars": total,
            "offset": offset.min(total),
            "returned": returned,
            "truncated": offset + returned < total || view.view.truncated,
            "captureTruncated": view.capture_truncated,
            "text": slice,
        })))
    })
    .await
    .map_err(|e| e.message)?
}

/// One search hit location with context.
pub(super) fn find_snippet(haystack: &str, needle_lower: &str) -> Option<String> {
    let lower = haystack.to_lowercase();
    let pos = lower.find(needle_lower)?;
    // Map the byte position in the lowercased string back approximately (same length for ASCII).
    let pos = pos.min(haystack.len());
    let mut start = pos.saturating_sub(80);
    while start > 0 && !haystack.is_char_boundary(start) {
        start -= 1;
    }
    let mut end = (pos + needle_lower.len() + 80).min(haystack.len());
    while end < haystack.len() && !haystack.is_char_boundary(end) {
        end += 1;
    }
    let mut s = String::new();
    if start > 0 {
        s.push('…');
    }
    s.push_str(&haystack[start..end].replace(['\n', '\r'], " "));
    if end < haystack.len() {
        s.push('…');
    }
    Some(s)
}

pub(super) async fn search(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let query = arg_str(args, "query")
        .map(str::trim)
        .filter(|q| !q.is_empty())
        .ok_or("`query` is required")?;
    let needle = query.to_lowercase();
    let filter = parse_filter(args)?;
    let limit = arg_u64(args, "limit", 20, 200)?.max(1) as usize;
    let a = app.clone();
    let result = blocking(move || {
        const SCAN_LIMIT: usize = 20_000;
        let candidates = a.store.query(&filter, None);
        let total = candidates.len();
        let mut hits = Vec::new();
        let mut scanned = 0usize;
        for s in candidates.iter().rev() {
            if hits.len() >= limit || scanned >= SCAN_LIMIT {
                break;
            }
            scanned += 1;
            let mut matches = Vec::new();
            // A redacted search runs over the redacted text: it must not reveal (even as a
            // yes/no) what a secret contains.
            let url = summary_url(s);
            let url = if redact {
                akev_core::redact::url(&url).into_owned()
            } else {
                url
            };
            if let Some(snip) = find_snippet(&url, &needle) {
                matches.push(json!({ "in": "url", "snippet": snip }));
            }
            if let Some(detail) = a.store.detail(s.id) {
                for (side, headers) in [
                    ("request-header", Some(&detail.request.headers)),
                    (
                        "response-header",
                        detail.response.as_ref().map(|r| &r.headers),
                    ),
                ] {
                    if let Some(headers) = headers {
                        for (name, value) in headers.iter() {
                            let redacted = redact
                                .then(|| akev_core::redact::header_value(name, value))
                                .flatten();
                            let line = format!("{name}: {}", redacted.as_deref().unwrap_or(value));
                            if let Some(snip) = find_snippet(&line, &needle) {
                                matches.push(json!({ "in": side, "snippet": snip }));
                                break;
                            }
                        }
                    }
                }
                let path = detail.request.path_only().to_string();
                for (side, label, headers) in [
                    (Side::Request, "request-body", Some(&detail.request.headers)),
                    (
                        Side::Response,
                        "response-body",
                        detail.response.as_ref().map(|r| &r.headers),
                    ),
                ] {
                    let (Some(headers), Some(b)) = (headers, a.store.body(s.id, side)) else {
                        continue;
                    };
                    if b.bytes.is_empty() {
                        continue;
                    }
                    let (decoded, _) = body::decode_or_raw(&b.bytes, headers);
                    let content_type = headers.get("content-type");
                    let text = body::text_if_textual(&decoded, content_type, &path).map(|t| {
                        if redact {
                            redact::body_text(&t, content_type).into_owned()
                        } else {
                            t.to_string()
                        }
                    });
                    if let Some(text) = text
                        && let Some(snip) = find_snippet(&text, &needle)
                    {
                        matches.push(json!({ "in": label, "snippet": snip }));
                    }
                }
            }
            if !matches.is_empty() {
                let mut r = row(s);
                r["matches"] = Value::Array(matches);
                hits.push(r);
            }
        }
        json!({
            "query": needle,
            "candidates": total,
            "scanned": scanned,
            "returned": hits.len(),
            "results": hits,
        })
    })
    .await
    .map_err(|e| e.message)?;
    Ok(Out::Json(result))
}

pub(super) fn is_done(s: &ExchangeSummary) -> bool {
    !matches!(
        s.state,
        ExchangeState::Requesting
            | ExchangeState::Waiting
            | ExchangeState::Responding
            | ExchangeState::Paused
    )
}

pub(super) async fn wait_for(app: &Arc<App>, args: &Value) -> ToolResult {
    let filter = parse_filter(args)?;
    let timeout = arg_u64(args, "timeoutMs", 30_000, 300_000)?.max(1);
    let include_existing = arg_bool(args, "includeExisting", false);
    let need_complete = arg_bool(args, "waitForCompletion", true);
    let mut rx = app.store.subscribe();
    if include_existing {
        let a = app.clone();
        let f = filter.clone();
        let existing = blocking(move || a.store.query(&f, None))
            .await
            .map_err(|e| e.message)?;
        if let Some(s) = existing.iter().rev().find(|s| !need_complete || is_done(s)) {
            return Ok(Out::Json(
                json!({ "matched": true, "existing": true, "exchange": row(s) }),
            ));
        }
    }
    let mut shutdown = app.shutdown_receiver();
    let deadline = tokio::time::sleep(Duration::from_millis(timeout));
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            _ = &mut deadline => {
                return Ok(Out::Json(json!({
                    "matched": false,
                    "timedOut": true,
                    "note": format!("no matching exchange within {timeout} ms"),
                })));
            }
            _ = shutdown.changed() => return Err("Akev is shutting down".into()),
            ev = rx.recv() => match ev {
                Ok(StoreEvent::Added(s) | StoreEvent::Updated(s)) => {
                    if (!need_complete || is_done(&s)) && matches_now(app, s.id, &filter).await {
                        return Ok(Out::Json(json!({ "matched": true, "exchange": row(&s) })));
                    }
                }
                Ok(_) => {}
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return Err("event stream closed".into()),
            }
        }
    }
}

/// Evaluate a filter against one stored exchange with the store's evaluator (consistent with
/// `akev_list_exchanges`); body filters run on the blocking pool.
pub(super) async fn matches_now(
    app: &Arc<App>,
    id: ExchangeId,
    filter: &akev_filter::Filter,
) -> bool {
    if !filter.needs_body() {
        return app.store.matches(id, filter) == Some(true);
    }
    let store = app.store.clone();
    let filter = filter.clone();
    blocking(move || store.matches(id, &filter) == Some(true))
        .await
        .unwrap_or(false)
}

pub(super) fn clear_traffic(app: &Arc<App>, args: &Value) -> ToolResult {
    let keep_pinned = arg_bool(args, "keepPinned", false);
    let before = app.store.stats().exchanges;
    app.store.clear(keep_pinned);
    Ok(Out::Json(
        json!({ "cleared": true, "exchangesBefore": before, "keptPinned": keep_pinned }),
    ))
}

pub(super) async fn export_har(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let options = akev_store::ExportOptions { redact };
    let mut ids: Option<Vec<ExchangeId>> = args.get("ids").and_then(Value::as_array).map(|list| {
        list.iter()
            .filter_map(Value::as_u64)
            .map(ExchangeId)
            .collect()
    });
    if arg_str(args, "filter").is_some_and(|f| !f.trim().is_empty()) {
        let filter = parse_filter(args)?;
        let a = app.clone();
        let matching = blocking(move || a.store.query_ids(&filter))
            .await
            .map_err(|e| e.message)?;
        ids = Some(match ids {
            None => matching,
            Some(wanted) => {
                let wanted: std::collections::HashSet<_> = wanted.into_iter().collect();
                matching
                    .into_iter()
                    .filter(|id| wanted.contains(id))
                    .collect()
            }
        });
    }
    if let Some(path) = arg_str(args, "path") {
        let path = app.mcp_files.export_path(path)?;
        if path.exists() && !arg_bool(args, "overwrite", false) {
            return Err(format!(
                "{} already exists (pass overwrite=true to replace it)",
                path.display()
            ));
        }
        // Streamed straight into the file: constant memory however large the export.
        let a = app.clone();
        let p = path.clone();
        let (count, len) = blocking(move || {
            crate::persist::write_atomic_with(&p, 0o644, |f| {
                let count = a
                    .store
                    .write_har_with(f, ids.as_deref(), options)
                    .map_err(std::io::Error::other)?;
                Ok((count, f.metadata()?.len()))
            })
        })
        .await
        .map_err(|e| e.message)?
        .map_err(|e| format!("failed to write {}: {e}", path.display()))?;
        return Ok(Out::Json(
            json!({ "path": path.display().to_string(), "bytes": len, "exchanges": count }),
        ));
    }
    let a = app.clone();
    let bytes = blocking(move || crate::routes::io::har_bytes(&a, ids.as_deref(), options))
        .await
        .map_err(|e| e.message)?
        .map_err(|e| e.message)?;
    let max = arg_u64(args, "maxChars", 100_000, 50_000_000)?.max(1000) as usize;
    let text = String::from_utf8_lossy(&bytes);
    let (shown, truncated) = truncate_chars(&text, max);
    if truncated {
        return Ok(Out::Text(format!(
            "{shown}\n\n[truncated: the HAR has {} characters; pass `path` to write it to a file]",
            text.chars().count()
        )));
    }
    Ok(Out::Text(shown.to_string()))
}

pub(super) async fn snippet(app: &Arc<App>, args: &Value, redact: bool) -> ToolResult {
    let id = arg_id(args, "id")?;
    let target = crate::routes::exchanges::parse_target(arg_str(args, "target").unwrap_or("curl"))
        .map_err(|e| e.message)?;
    let (mut req, decoded, _) = captured_request(app, id).map_err(|e| e.message)?;
    if redact {
        akev_core::redact::headers(&mut req.headers);
        req.url = akev_core::redact::url(&req.url).into_owned();
        if let Some(clean) = akev_core::redact::body(&req.body, req.headers.get("content-type")) {
            req.body = Bytes::from(clean);
        }
    }
    let snippet = body::snippet_request(
        &req.method,
        &req.url,
        &strip_proxy_headers(req.headers.clone()),
        &req.body,
        decoded,
    );
    let out = crate::routes::exchanges::generate_snippet(target, snippet)
        .await
        .map_err(|e| e.message)?;
    Ok(Out::Text(out.code))
}

pub(super) async fn llm_conversation(app: &Arc<App>, args: &Value) -> ToolResult {
    let id = arg_id(args, "id")?;
    let max_chars = arg_u64(
        args,
        "maxTextChars",
        DEFAULT_MAX_BODY_CHARS as u64,
        1_000_000,
    )? as usize;
    let worker = app.clone();
    let lookup = body_view_blocking(app, move || crate::llm::conversation(&worker, id))
        .await
        .map_err(|e| e.message)?;
    match lookup {
        crate::llm::Lookup::Found(mut x) => {
            akev_content::llm::compact(&mut x, max_chars.max(100));
            Ok(Out::Json(to_value(&x)))
        }
        crate::llm::Lookup::NotLlm => Err(format!(
            "exchange {id} is not a recognised LLM API call (OpenAI, Anthropic, Gemini, Ollama or an \
             OpenAI-compatible server); find LLM calls with akev_list_exchanges filter `is:llm`"
        )),
        crate::llm::Lookup::NotFound => Err(format!("exchange {id} not found")),
    }
}
