//! `/api/exchanges*` — captured traffic.

use super::common::{Params, blocking, body_view_blocking, no_content, parse_id, parse_side};
use super::compose::{SendOutput, send_and_view};
use crate::app::App;
use crate::body;
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::{
    CapturedBody, EditableRequest, ExchangeDetail, ExchangeId, Headers, MessageKind, Side,
    StreamMessage, Timing,
};
use akev_proxy::SendOptions;
use akev_store::ExchangePatch;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use http::header::{self, HeaderValue};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

/// `GET /api/exchanges?filter=&limit=`
///
/// The snapshot and its serialization (100k+ rows) run on the blocking pool.
pub async fn list(State(app): State<Arc<App>>, params: Params) -> ApiResult<Response> {
    let limit = params.parse_num::<usize>("limit")?;
    let filter = match params.non_empty("filter") {
        Some(q) => Some(akev_filter::Filter::parse(q).map_err(|e| ApiError::invalid_filter(&e))?),
        None => None,
    };
    let bytes = blocking(move || {
        let summaries = match &filter {
            Some(filter) => app.store.query(filter, limit),
            // Only the newest `limit` are copied, not every summary.
            None => match limit {
                Some(limit) => app.store.newest_summaries(limit),
                None => app.store.summaries(),
            },
        };
        serde_json::to_vec(&summaries)
    })
    .await?
    .map_err(|e| ApiError::internal(format!("failed to serialize exchanges: {e}")))?;
    Ok(json_response(bytes))
}

/// A `200` JSON response from pre-serialized bytes.
pub fn json_response(bytes: Vec<u8>) -> Response {
    let mut resp = Response::new(axum::body::Body::from(bytes));
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    resp
}

/// `GET /api/exchanges/:id`
pub async fn detail(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
) -> ApiResult<Json<ExchangeDetail>> {
    let id = parse_id(&id)?;
    app.store.detail(id).map(Json).ok_or_else(|| not_found(id))
}

fn not_found(id: ExchangeId) -> ApiError {
    ApiError::not_found(format!("exchange {id} not found"))
}

/// The headers & body of one side of an exchange.
struct SideBody {
    headers: Headers,
    path: String,
    url: String,
    host: String,
    body: CapturedBody,
    side: Side,
}

fn side_body(app: &App, id: ExchangeId, side: Side, original: bool) -> ApiResult<SideBody> {
    let detail = app.store.detail(id).ok_or_else(|| not_found(id))?;
    let headers = match side {
        Side::Request => {
            let head = if original {
                detail.original_request.as_ref().unwrap_or(&detail.request)
            } else {
                &detail.request
            };
            head.headers.clone()
        }
        Side::Response => {
            let head = if original {
                detail
                    .original_response
                    .as_ref()
                    .or(detail.response.as_ref())
            } else {
                detail.response.as_ref()
            };
            head.ok_or_else(|| ApiError::not_found(format!("exchange {id} has no response (yet)")))?
                .headers
                .clone()
        }
    };
    let body = if original {
        app.store
            .original_body(id, side)
            .or_else(|| app.store.body(id, side))
    } else {
        app.store.body(id, side)
    };
    Ok(SideBody {
        headers,
        path: detail.request.path_only().to_string(),
        url: detail.request.url.clone(),
        host: detail.request.host.clone(),
        body: body.unwrap_or_default(),
        side,
    })
}

/// `GET /api/exchanges/:id/body/:side?original=1`
///
/// Built and serialized under a body-view permit (see [`body_view_blocking`]).
pub async fn body_view(
    State(app): State<Arc<App>>,
    Path((id, side)): Path<(String, String)>,
    params: Params,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let side = parse_side(&side)?;
    let sb = side_body(&app, id, side, params.flag("original"))?;
    let protos = app.protos.clone();
    let bytes = body_view_blocking(&app, move || {
        // A protobuf schema the user imported decodes protobuf / gRPC bodies with names.
        let plan = protos.plan(sb.side, &sb.url, &sb.path, sb.headers.get("content-type"));
        serde_json::to_vec(&body::body_view_schema(
            &sb.body,
            &sb.headers,
            &sb.path,
            plan.as_ref(),
        ))
    })
    .await?
    .map_err(|e| ApiError::internal(format!("failed to serialize the body view: {e}")))?;
    Ok(json_response(bytes))
}

/// `GET /api/exchanges/:id/body/:side/raw?decoded=1&download=1&original=1`
pub async fn raw_body(
    State(app): State<Arc<App>>,
    Path((id, side)): Path<(String, String)>,
    params: Params,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let side = parse_side(&side)?;
    let sb = side_body(&app, id, side, params.flag("original"))?;
    let encodings = sb.headers.content_encodings();
    let decoded = params.flag("decoded") && !encodings.is_empty();
    let bytes = if decoded {
        let (raw, headers) = (sb.body.bytes.clone(), sb.headers.clone());
        blocking(move || body::decode(&raw, &headers))
            .await?
            .map_err(|e| ApiError::unprocessable("decode-failed", e.to_string()))?
    } else {
        sb.body.bytes.clone()
    };
    let content_type = sb.headers.get("content-type").map(str::to_string);
    let mut resp = Response::new(axum::body::Body::from(bytes));
    let h = resp.headers_mut();
    let ct = content_type
        .as_deref()
        .unwrap_or("application/octet-stream");
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(ct).unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    // Captured content must never run with the API's origin (it could call the API).
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    if !decoded
        && !encodings.is_empty()
        && let Ok(v) = HeaderValue::from_str(&encodings.join(", "))
    {
        h.insert("x-akev-content-encoding", v);
    }
    if sb.body.truncated {
        h.insert("x-akev-capture-truncated", HeaderValue::from_static("1"));
    }
    if params.flag("download") {
        let mut name = body::download_filename(&sb.url, &sb.host, id.0, content_type.as_deref());
        if side == Side::Request {
            name = format!("request-{name}");
        }
        if let Ok(v) = HeaderValue::from_str(&body::content_disposition(&name)) {
            h.insert(header::CONTENT_DISPOSITION, v);
        }
    }
    Ok(resp)
}

/// `GET /api/exchanges/:id/llm` — the conversation view of an LLM API call (see
/// [`akev_content::llm`]); `404 not-llm` for other exchanges.
///
/// Built under a body-view permit: large prompts are parsed into JSON trees.
pub async fn llm(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Response> {
    app.license.require(akev_core::license::Feature::LlmLens)?;
    let id = parse_id(&id)?;
    if app.store.summary(id).is_none() {
        return Err(not_found(id));
    }
    let worker = app.clone();
    let out = body_view_blocking(&app, move || match crate::llm::conversation(&worker, id) {
        crate::llm::Lookup::Found(x) => serde_json::to_vec(&x)
            .map_err(|e| ApiError::internal(format!("failed to serialize the conversation: {e}"))),
        crate::llm::Lookup::NotLlm => Err(ApiError::new(
            http::StatusCode::NOT_FOUND,
            "not-llm",
            format!("exchange {id} is not a recognised LLM API call"),
        )),
        crate::llm::Lookup::NotFound => Err(not_found(id)),
    })
    .await??;
    Ok(json_response(out))
}

/// `StreamMessageWithPayload`
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageOut {
    #[serde(flatten)]
    pub message: StreamMessage,
    pub text: Option<String>,
    pub base64: Option<String>,
}

impl MessageOut {
    pub fn new(message: StreamMessage, payload: &[u8]) -> Self {
        let textual = matches!(message.kind, MessageKind::Text | MessageKind::Event);
        let (text, base64) = match (textual, std::str::from_utf8(payload)) {
            (true, Ok(s)) => (Some(s.to_string()), None),
            _ if payload.is_empty() => (None, None),
            _ => (
                None,
                Some(base64::engine::general_purpose::STANDARD.encode(payload)),
            ),
        };
        MessageOut {
            message,
            text,
            base64,
        }
    }
}

/// Messages per page when `limit` isn't given.
pub const MESSAGES_PAGE: usize = 1_000;
/// Largest `limit` accepted.
pub const MESSAGES_PAGE_MAX: usize = 10_000;

/// `GET /api/exchanges/:id/messages?after=<seq>&before=<seq>&limit=<n>&tail=<bool>`
///
/// Messages with `after < seq < before` (both optional) in ascending order, at most `limit`
/// (default [`MESSAGES_PAGE`], max [`MESSAGES_PAGE_MAX`]): the oldest of the range, or the
/// newest with `tail=true`. Page forward with `after=<last seq>` until a page comes back
/// short, or backward with `before=<first seq>&tail=true`.
pub async fn messages(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    params: Params,
) -> ApiResult<Json<Vec<MessageOut>>> {
    let id = parse_id(&id)?;
    let limit = params
        .parse_num::<usize>("limit")?
        .unwrap_or(MESSAGES_PAGE)
        .clamp(1, MESSAGES_PAGE_MAX);
    let tail = matches!(
        params
            .non_empty("tail")
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("1" | "true" | "yes")
    );
    let page = akev_store::MessagePage {
        after: params.parse_num::<u32>("after")?,
        before: params.parse_num::<u32>("before")?,
        limit: Some(limit),
        tail,
    };
    // Payloads may have to be read back from the spill file: off the async workers.
    let out = blocking(move || {
        app.store.messages_page(id, page).map(|messages| {
            messages
                .into_iter()
                .map(|m| MessageOut::new(m.message, &m.payload))
                .collect::<Vec<_>>()
        })
    })
    .await?
    .ok_or_else(|| not_found(id))?;
    Ok(Json(out))
}

/// Response of snippet / codegen endpoints.
#[derive(Serialize)]
pub struct SnippetOut {
    pub target: akev_codegen::Target,
    pub language: &'static str,
    pub code: String,
}

pub fn parse_target(s: &str) -> ApiResult<akev_codegen::Target> {
    serde_json::from_value(serde_json::Value::String(s.trim().to_ascii_lowercase())).map_err(|_| {
        ApiError::bad_request(format!(
            "unknown snippet target {s:?} (see GET /api/codegen/targets)"
        ))
    })
}

pub async fn generate_snippet(
    target: akev_codegen::Target,
    req: akev_codegen::SnippetRequest,
) -> ApiResult<SnippetOut> {
    let code = blocking(move || akev_codegen::generate(target, &req)).await?;
    let language = akev_codegen::targets()
        .into_iter()
        .find(|t| t.id == target)
        .map(|t| t.language)
        .unwrap_or("text");
    Ok(SnippetOut {
        target,
        language,
        code,
    })
}

/// The request as captured (original if a rule modified it), ready for codegen/replay.
pub fn captured_request(app: &App, id: ExchangeId) -> ApiResult<(EditableRequest, bool, bool)> {
    let detail = app.store.detail(id).ok_or_else(|| not_found(id))?;
    let head = detail.original_request.clone().unwrap_or(detail.request);
    let body = app
        .store
        .original_body(id, Side::Request)
        .or_else(|| app.store.body(id, Side::Request));
    let truncated = body.as_ref().is_some_and(|b| b.truncated);
    let (bytes, decoded) = match &body {
        Some(b) => body::decode_or_raw(&b.bytes, &head.headers),
        None => (bytes::Bytes::new(), true),
    };
    let req = EditableRequest {
        method: head.method,
        url: head.url,
        headers: head.headers,
        body: bytes,
    };
    Ok((req, decoded, truncated))
}

/// `GET /api/exchanges/:id/snippet?target=curl`
pub async fn snippet(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    params: Params,
) -> ApiResult<Json<SnippetOut>> {
    let id = parse_id(&id)?;
    let target = parse_target(params.non_empty("target").unwrap_or("curl"))?;
    let (req, decoded, _) = captured_request(&app, id)?;
    let snippet = body::snippet_request(
        &req.method,
        &req.url,
        &strip_proxy_headers(req.headers.clone()),
        &req.body,
        decoded,
    );
    generate_snippet(target, snippet).await.map(Json)
}

#[derive(Deserialize)]
pub struct IdsBody {
    pub ids: Vec<ExchangeId>,
}

/// Most exchanges one `POST /api/exchanges/headers` or `/timings` call may ask for.
pub const MAX_BATCH_IDS: usize = 5_000;
/// Most header names per side one `POST /api/exchanges/headers` call may ask for.
const MAX_HEADER_NAMES: usize = 32;

#[derive(Deserialize)]
pub struct HeaderValuesBody {
    pub ids: Vec<ExchangeId>,
    /// Request header names to read (case-insensitive).
    #[serde(default)]
    pub request: Vec<String>,
    /// Response header names to read (case-insensitive).
    #[serde(default)]
    pub response: Vec<String>,
}

/// The values of the asked-for headers of one exchange, by lowercase name. Headers it doesn't
/// have are left out; repeated headers are joined with `, `.
#[derive(Serialize, Debug, PartialEq)]
pub struct HeaderValuesOut {
    pub id: ExchangeId,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub request: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub response: BTreeMap<String, String>,
}

fn check_batch(ids: &[ExchangeId]) -> ApiResult<()> {
    if ids.len() > MAX_BATCH_IDS {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_BATCH_IDS} ids per request (got {})",
            ids.len()
        )));
    }
    Ok(())
}

fn header_names(names: Vec<String>) -> ApiResult<Vec<String>> {
    if names.len() > MAX_HEADER_NAMES {
        return Err(ApiError::bad_request(format!(
            "at most {MAX_HEADER_NAMES} header names per side"
        )));
    }
    let mut out: Vec<String> = Vec::with_capacity(names.len());
    for name in names {
        let name = name.trim().to_ascii_lowercase();
        let valid = !name.is_empty()
            && name.len() <= 128
            && name
                .bytes()
                .all(|b| b.is_ascii_graphic() && !b"()<>@,;\\\"/[]?={}".contains(&b));
        if !valid {
            return Err(ApiError::bad_request(format!(
                "{name:?} is not a valid header name"
            )));
        }
        if !out.contains(&name) {
            out.push(name);
        }
    }
    Ok(out)
}

fn pick_headers(headers: Option<&Headers>, names: &[String]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(headers) = headers else {
        return out;
    };
    for name in names {
        let values: Vec<&str> = headers.get_all(name).collect();
        if !values.is_empty() {
            out.insert(name.clone(), values.join(", "));
        }
    }
    out
}

/// `POST /api/exchanges/headers` — chosen request / response header values of many exchanges
/// at once (custom list columns fetch them for the rows on screen).
pub async fn header_values(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<HeaderValuesBody>,
) -> ApiResult<Response> {
    check_batch(&body.ids)?;
    let request = header_names(body.request)?;
    let response = header_names(body.response)?;
    let bytes = blocking(move || {
        let mut out = Vec::with_capacity(body.ids.len());
        app.store.visit_heads(&body.ids, |id, _, req, res| {
            out.push(HeaderValuesOut {
                id,
                request: pick_headers(Some(req), &request),
                response: pick_headers(res, &response),
            });
        });
        serde_json::to_vec(&serde_json::json!({ "exchanges": out }))
    })
    .await?
    .map_err(|e| ApiError::internal(format!("failed to serialize header values: {e}")))?;
    Ok(json_response(bytes))
}

/// One exchange's timing (`POST /api/exchanges/timings`).
#[derive(Serialize)]
pub struct TimingOut {
    pub id: ExchangeId,
    #[serde(flatten)]
    pub timing: Timing,
}

/// `POST /api/exchanges/timings` — the phase timings of many exchanges at once (the traffic
/// timeline fetches them for the rows on screen). Unknown ids are left out.
pub async fn timings(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<IdsBody>,
) -> ApiResult<Response> {
    check_batch(&body.ids)?;
    let bytes = blocking(move || {
        let mut out = Vec::with_capacity(body.ids.len());
        app.store.visit_heads(&body.ids, |id, timing, _, _| {
            out.push(TimingOut {
                id,
                timing: timing.clone(),
            });
        });
        serde_json::to_vec(&serde_json::json!({ "timings": out }))
    })
    .await?
    .map_err(|e| ApiError::internal(format!("failed to serialize timings: {e}")))?;
    Ok(json_response(bytes))
}

/// `POST /api/exchanges/delete`
pub async fn delete(State(app): State<Arc<App>>, JsonBody(body): JsonBody<IdsBody>) -> Response {
    app.store.delete(&body.ids);
    no_content()
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct ClearBody {
    pub keep_pinned: bool,
}

/// `POST /api/exchanges/clear`
pub async fn clear(State(app): State<Arc<App>>, JsonBody(body): JsonBody<ClearBody>) -> Response {
    app.store.clear(body.keep_pinned);
    no_content()
}

#[derive(Deserialize)]
pub struct PatchBody {
    pub ids: Vec<ExchangeId>,
    #[serde(flatten)]
    pub patch: ExchangePatch,
}

/// Named colour tags (plus `null` to clear). Hex colours (`#rgb`, `#rrggbb`, `#rrggbbaa`)
/// are accepted too, e.g. from a colour picker.
const COLORS: &[&str] = &[
    "red", "orange", "yellow", "green", "blue", "purple", "grey", "gray", "pink", "teal",
];

/// Whether `color` is a valid colour tag.
pub fn valid_color(color: &str) -> bool {
    if COLORS.contains(&color) {
        return true;
    }
    color.strip_prefix('#').is_some_and(|hex| {
        matches!(hex.len(), 3 | 4 | 6 | 8) && hex.bytes().all(|b| b.is_ascii_hexdigit())
    })
}

/// `PATCH /api/exchanges`
pub async fn patch(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<PatchBody>,
) -> ApiResult<Response> {
    let mut patch = body.patch;
    if let Some(Some(color)) = &patch.color {
        let normalized = color.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            patch.color = Some(None);
        } else if !valid_color(&normalized) {
            return Err(ApiError::bad_request(format!(
                "unknown colour {color:?} (one of {}, or a #rrggbb hex colour)",
                COLORS.join(", ")
            )));
        } else {
            patch.color = Some(Some(normalized));
        }
    }
    if let Some(Some(note)) = &patch.note
        && note.trim().is_empty()
    {
        patch.note = Some(None);
    }
    app.store.patch(&body.ids, &patch);
    Ok(no_content())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct ReplayBody {
    pub request: Option<EditableRequest>,
    pub options: Option<SendOptions>,
}

/// Remove headers that only make sense towards our proxy (or were HTTP/2 pseudo headers).
pub fn strip_proxy_headers(mut headers: Headers) -> Headers {
    headers.0.retain(|(name, _)| {
        !(name.starts_with(':')
            || name.eq_ignore_ascii_case("proxy-authorization")
            || name.eq_ignore_ascii_case("proxy-connection")
            || name.eq_ignore_ascii_case("transfer-encoding"))
    });
    headers
}

/// `POST /api/exchanges/:id/replay`
pub async fn replay(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<ReplayBody>,
) -> ApiResult<Json<SendOutput>> {
    let id = parse_id(&id)?;
    let request = match body.request {
        Some(r) => r,
        None => {
            let (mut req, _, truncated) = captured_request(&app, id)?;
            if truncated {
                return Err(ApiError::unprocessable(
                    "body-truncated",
                    "the request body was only partially captured; open it in Compose and provide the request explicitly",
                ));
            }
            req.headers = strip_proxy_headers(req.headers);
            req
        }
    };
    send_and_view(&app, request, body.options.unwrap_or_default())
        .await
        .map(Json)
}

/// `GET /api/exchanges/:id/editable/:side`
pub async fn editable(
    State(app): State<Arc<App>>,
    Path((id, side)): Path<(String, String)>,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let side = parse_side(&side)?;
    let detail = app.store.detail(id).ok_or_else(|| not_found(id))?;
    match side {
        Side::Request => {
            let body = app.store.body(id, Side::Request);
            Ok(Json(body::editable_request(&detail.request, body.as_ref())).into_response())
        }
        Side::Response => {
            let head = detail.response.as_ref().ok_or_else(|| {
                ApiError::not_found(format!("exchange {id} has no response (yet)"))
            })?;
            let body = app.store.body(id, Side::Response);
            Ok(Json(body::editable_response(head, body.as_ref())).into_response())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::{Direction, UnixMicros};

    fn msg(kind: MessageKind) -> StreamMessage {
        StreamMessage {
            seq: 3,
            direction: Direction::Sent,
            kind,
            at: UnixMicros(5),
            size: 2,
            event: None,
            event_id: None,
            modified: false,
        }
    }

    #[test]
    fn message_payload_encoding() {
        let v = serde_json::to_value(MessageOut::new(msg(MessageKind::Text), b"hi")).unwrap();
        assert_eq!(v["text"], "hi");
        assert_eq!(v["base64"], serde_json::Value::Null);
        assert_eq!(v["seq"], 3);
        assert_eq!(v["direction"], "sent");
        let v = serde_json::to_value(MessageOut::new(msg(MessageKind::Binary), b"hi")).unwrap();
        assert_eq!(v["text"], serde_json::Value::Null);
        assert_eq!(v["base64"], "aGk=");
        // Invalid UTF-8 in a text frame falls back to base64.
        let v =
            serde_json::to_value(MessageOut::new(msg(MessageKind::Text), &[0xff, 0xfe])).unwrap();
        assert_eq!(v["base64"], "//4=");
        let v = serde_json::to_value(MessageOut::new(msg(MessageKind::Ping), b"")).unwrap();
        assert_eq!(v["text"], serde_json::Value::Null);
        assert_eq!(v["base64"], serde_json::Value::Null);
    }

    #[test]
    fn patch_body_double_option() {
        let p: PatchBody =
            serde_json::from_str(r#"{"ids":[1,2],"color":null,"note":"x"}"#).unwrap();
        assert_eq!(p.ids, vec![ExchangeId(1), ExchangeId(2)]);
        assert_eq!(p.patch.color, Some(None));
        assert_eq!(p.patch.note, Some(Some("x".into())));
        assert_eq!(p.patch.pinned, None);
        let p: PatchBody = serde_json::from_str(r#"{"ids":[3],"pinned":true}"#).unwrap();
        assert_eq!(p.patch.pinned, Some(true));
        assert_eq!(p.patch.color, None);
        assert_eq!(p.patch.note, None);
    }

    #[test]
    fn proxy_headers_are_stripped() {
        let h = Headers(vec![
            (":authority".into(), "x".into()),
            ("Proxy-Authorization".into(), "Basic abc".into()),
            ("Proxy-Connection".into(), "keep-alive".into()),
            ("Transfer-Encoding".into(), "chunked".into()),
            ("Authorization".into(), "Bearer t".into()),
        ]);
        let out = strip_proxy_headers(h);
        assert_eq!(
            out.0,
            vec![("Authorization".to_string(), "Bearer t".to_string())]
        );
    }

    #[test]
    fn colours() {
        for ok in [
            "red",
            "grey",
            "gray",
            "#fff",
            "#00ff7f",
            "#00ff7f80",
            "#ABC",
        ] {
            assert!(
                valid_color(ok) || valid_color(&ok.to_ascii_lowercase()),
                "{ok}"
            );
        }
        for bad in [
            "", "#", "#12", "#12345", "#ggg", "url(x)", "red;", "magenta",
        ] {
            assert!(!valid_color(bad), "{bad}");
        }
    }

    #[test]
    fn targets_parse() {
        assert_eq!(parse_target("curl").unwrap(), akev_codegen::Target::Curl);
        assert_eq!(
            parse_target("Python-Requests").unwrap(),
            akev_codegen::Target::PythonRequests
        );
        assert!(parse_target("cobol").is_err());
    }
}
