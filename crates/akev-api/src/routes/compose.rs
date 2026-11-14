//! Compose: `/api/send`, `/api/import/curl`, `/api/codegen*`.

use super::common::body_view_blocking;
use super::exchanges::{SnippetOut, generate_snippet, parse_target};
use crate::app::App;
use crate::body::{self, ApiBodyView};
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::{EditableRequest, ExchangeId};
use akev_proxy::{SendOptions, SendResult};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// How long to wait for the store to ingest a sent exchange before reporting it as not
/// recorded (the store applies proxy events asynchronously).
const STORE_INGEST_WAIT: Duration = Duration::from_millis(300);

/// `SendResult & { body: BodyView, recorded: boolean }` (body is null when there was no
/// response).
#[derive(Serialize)]
pub struct SendOutput {
    #[serde(flatten)]
    pub result: SendResult,
    pub body: Option<ApiBodyView>,
    /// Whether exchange `id` is in the traffic list. `false` while recording is paused (or
    /// when the capture settings ignore the host): the id then can't be opened or restored.
    pub recorded: bool,
}

/// Whether the exchange made by a send ended up in the store.
async fn exchange_stored(app: &App, id: ExchangeId) -> bool {
    if !app.recording.is_recorded(id) {
        return false;
    }
    // Its first event was emitted before the request even left, so it's normally there.
    let deadline = Instant::now() + STORE_INGEST_WAIT;
    loop {
        if app.store.summary(id).is_some() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

/// Send a request through the proxy engine and build the response body view.
pub async fn send_and_view(
    app: &App,
    request: EditableRequest,
    options: SendOptions,
) -> ApiResult<SendOutput> {
    body::validate_send_request(&request).map_err(ApiError::bad_request)?;
    let path = body::request_head_from_editable(&request)
        .map(|h| h.path_only().to_string())
        .unwrap_or_default();
    let url = request.url.clone();
    let result = app.proxy().send(request, options).await;
    let view = match &result.response {
        Some(head) => {
            let (bytes, headers) = (result.body.clone(), head.headers.clone());
            let protos = app.protos.clone();
            Some(
                body_view_blocking(app, move || {
                    let plan = protos.plan(
                        akev_core::Side::Response,
                        &url,
                        &path,
                        headers.get("content-type"),
                    );
                    body::send_body_view(&bytes, &headers, &path, plan.as_ref())
                })
                .await?,
            )
        }
        None => None,
    };
    let recorded = exchange_stored(app, result.id).await;
    Ok(SendOutput {
        result,
        body: view,
        recorded,
    })
}

#[derive(Deserialize)]
pub struct SendBody {
    pub request: EditableRequest,
    #[serde(default)]
    pub options: Option<SendOptions>,
}

/// `POST /api/send`
pub async fn send(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<SendBody>,
) -> ApiResult<Json<SendOutput>> {
    send_and_view(&app, body.request, body.options.unwrap_or_default())
        .await
        .map(Json)
}

/// Most requests one `POST /api/send/repeat` run may send.
pub const MAX_REPEAT: u32 = 10_000;
/// Most requests one repeat run keeps in flight.
pub const MAX_REPEAT_CONCURRENCY: u32 = 64;
/// Longest pause between one worker's requests (ms).
pub const MAX_REPEAT_DELAY_MS: u64 = 60_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RepeatBody {
    /// The request to send. Without it, `exchangeId`'s request is resent as captured (like
    /// `POST /api/exchanges/:id/replay`).
    #[serde(default)]
    pub request: Option<EditableRequest>,
    #[serde(default)]
    pub exchange_id: Option<ExchangeId>,
    #[serde(default)]
    pub options: Option<SendOptions>,
    /// How many times to send it.
    pub count: u32,
    /// How many requests are in flight at once (default 1).
    #[serde(default)]
    pub concurrency: Option<u32>,
    /// Pause between the requests of each worker (ms).
    #[serde(default)]
    pub delay_ms: u64,
}

/// One line of the `POST /api/send/repeat` stream: a finished request, or the end.
#[derive(Serialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase", untagged)]
pub enum RepeatLine {
    #[serde(rename_all = "camelCase")]
    Result {
        /// 0-based position in the run (the order requests were started).
        index: u32,
        /// The exchange it made (in the traffic list unless recording is paused).
        id: ExchangeId,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<akev_core::ErrorKind>,
        /// From the request's start to the end of the response body (or the failure), ms.
        #[serde(skip_serializing_if = "Option::is_none")]
        duration_ms: Option<f64>,
    },
    Done {
        done: bool,
        sent: u32,
    },
}

impl RepeatLine {
    fn to_ndjson(&self) -> Bytes {
        let mut v = serde_json::to_vec(self).unwrap_or_default();
        v.push(b'\n');
        Bytes::from(v)
    }
}

/// The `POST /api/send/repeat` response body: lines from the workers. Dropping it (the client
/// went away or cancelled) aborts the run, in-flight requests included.
struct RepeatStream {
    rx: mpsc::Receiver<Bytes>,
    run: tokio::task::AbortHandle,
}

impl Drop for RepeatStream {
    fn drop(&mut self) {
        self.run.abort();
    }
}

impl futures::Stream for RepeatStream {
    type Item = Result<Bytes, std::convert::Infallible>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx).map(|line| line.map(Ok))
    }
}

/// Check the run's limits; returns the effective concurrency.
fn repeat_limits(body: &RepeatBody) -> ApiResult<u32> {
    if body.count == 0 || body.count > MAX_REPEAT {
        return Err(ApiError::bad_request(format!(
            "count must be between 1 and {MAX_REPEAT}"
        )));
    }
    let concurrency = body.concurrency.unwrap_or(1);
    if concurrency == 0 || concurrency > MAX_REPEAT_CONCURRENCY {
        return Err(ApiError::bad_request(format!(
            "concurrency must be between 1 and {MAX_REPEAT_CONCURRENCY}"
        )));
    }
    if body.delay_ms > MAX_REPEAT_DELAY_MS {
        return Err(ApiError::bad_request(format!(
            "delayMs must be at most {MAX_REPEAT_DELAY_MS}"
        )));
    }
    Ok(concurrency.min(body.count))
}

/// `POST /api/send/repeat` — send one request `count` times, `concurrency` at a time, each
/// worker pausing `delayMs` between its requests. Streams one JSON line per finished request
/// (`application/x-ndjson`) and a final `{"done":true,"sent":n}`; closing the connection
/// cancels the run.
pub async fn repeat(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<RepeatBody>,
) -> ApiResult<Response> {
    let concurrency = repeat_limits(&body)?;
    let request = match (body.request, body.exchange_id) {
        (Some(r), _) => r,
        (None, Some(id)) => replay_request(&app, id)?,
        (None, None) => {
            return Err(ApiError::bad_request(
                "provide the request, or the exchangeId to resend",
            ));
        }
    };
    body::validate_send_request(&request).map_err(ApiError::bad_request)?;
    let options = body.options.unwrap_or_default();
    let (count, delay) = (body.count, Duration::from_millis(body.delay_ms));

    let (tx, rx) = mpsc::channel::<Bytes>(256);
    let run = tokio::spawn(async move {
        let next = Arc::new(AtomicU32::new(0));
        let mut workers = tokio::task::JoinSet::new();
        for _ in 0..concurrency {
            let (app, request, options, next, tx) = (
                app.clone(),
                request.clone(),
                options.clone(),
                next.clone(),
                tx.clone(),
            );
            workers.spawn(async move {
                let mut first = true;
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= count {
                        break;
                    }
                    if !first && !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    first = false;
                    let result = app.proxy().send(request.clone(), options.clone()).await;
                    let line = RepeatLine::Result {
                        index,
                        id: result.id,
                        status: result.response.as_ref().map(|r| r.status),
                        error: result.error.as_ref().map(|e| e.kind),
                        duration_ms: result.timing.total(),
                    };
                    if tx.send(line.to_ndjson()).await.is_err() {
                        break;
                    }
                }
            });
        }
        while workers.join_next().await.is_some() {}
        let sent = next.load(Ordering::Relaxed).min(count);
        let _ = tx
            .send(RepeatLine::Done { done: true, sent }.to_ndjson())
            .await;
    });
    let stream = RepeatStream {
        rx,
        run: run.abort_handle(),
    };
    let mut resp = Response::new(axum::body::Body::from_stream(stream));
    let h = resp.headers_mut();
    h.insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("application/x-ndjson"),
    );
    h.insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static("no-store"),
    );
    Ok(resp)
}

/// The request of exchange `id` as captured, ready to resend (what replay sends).
fn replay_request(app: &App, id: ExchangeId) -> ApiResult<EditableRequest> {
    let (mut req, _, truncated) = super::exchanges::captured_request(app, id)?;
    if truncated {
        return Err(ApiError::unprocessable(
            "body-truncated",
            "the request body was only partially captured; open it in Compose and provide the request explicitly",
        ));
    }
    req.headers = super::exchanges::strip_proxy_headers(req.headers);
    Ok(req)
}

#[derive(Deserialize)]
pub struct CurlBody {
    pub command: String,
}

/// Convert a parsed cURL command into an editable request.
pub fn curl_to_editable(command: &str) -> ApiResult<EditableRequest> {
    let parsed = akev_codegen::parse_curl(command)
        .map_err(|e| ApiError::bad_request(format!("could not parse cURL command: {e}")))?;
    Ok(EditableRequest {
        method: parsed.method,
        url: parsed.url,
        headers: parsed.headers,
        body: parsed.body.unwrap_or_default(),
    })
}

/// `POST /api/import/curl`
pub async fn import_curl(JsonBody(body): JsonBody<CurlBody>) -> ApiResult<Json<EditableRequest>> {
    curl_to_editable(&body.command).map(Json)
}

/// `GET /api/codegen/targets`
pub async fn targets() -> Json<Vec<akev_codegen::TargetInfo>> {
    Json(akev_codegen::targets())
}

#[derive(Deserialize)]
pub struct CodegenBody {
    pub request: EditableRequest,
    #[serde(default)]
    pub target: Option<String>,
}

/// `POST /api/codegen`
pub async fn codegen(JsonBody(body): JsonBody<CodegenBody>) -> ApiResult<Json<SnippetOut>> {
    let target = parse_target(body.target.as_deref().unwrap_or("curl"))?;
    generate_snippet(target, body::snippet_from_editable(&body.request))
        .await
        .map(Json)
}
