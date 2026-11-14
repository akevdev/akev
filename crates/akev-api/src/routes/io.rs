//! `/api/export/*` and `/api/import/*` — HAR and native `.akev` sessions.

use super::common::{Params, attachment, blocking, timestamp_slug};
use super::filter::IdsOut;
use crate::app::App;
use crate::error::{ApiError, ApiResult};
use akev_core::ExchangeId;
use akev_store::ExportOptions;
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use bytes::Bytes;
use std::sync::Arc;

/// Resolve the exchange selection from `?ids=` and `?filter=` (intersection when both).
pub async fn selection(app: &Arc<App>, params: &Params) -> ApiResult<Option<Vec<ExchangeId>>> {
    let ids = params.ids("ids")?;
    let Some(query) = params.non_empty("filter").map(str::to_string) else {
        return Ok(ids);
    };
    let filter = akev_filter::Filter::parse(&query).map_err(|e| ApiError::invalid_filter(&e))?;
    let a = app.clone();
    let matching = blocking(move || a.store.query_ids(&filter)).await?;
    Ok(Some(match ids {
        None => matching,
        Some(ids) => {
            let wanted: std::collections::HashSet<ExchangeId> = ids.into_iter().collect();
            matching
                .into_iter()
                .filter(|id| wanted.contains(id))
                .collect()
        }
    }))
}

/// Export options from `?redact=1` (secrets replaced with `[redacted]`).
pub fn export_options(params: &Params) -> ExportOptions {
    ExportOptions {
        redact: params.flag("redact"),
    }
}

/// Serialize a HAR export into memory (blocking). Streams from the store, so only the output
/// is held, not a JSON tree of every exchange as well.
pub fn har_bytes(
    app: &App,
    ids: Option<&[ExchangeId]>,
    options: ExportOptions,
) -> ApiResult<Vec<u8>> {
    let mut out = Vec::new();
    app.store
        .write_har_with(&mut out, ids, options)
        .map_err(|e| ApiError::internal(format!("failed to serialize HAR: {e}")))?;
    Ok(out)
}

/// Chunks of a response body produced on a blocking thread.
type BodyChunks = tokio::sync::mpsc::Sender<std::io::Result<Bytes>>;

/// How many chunks (of about 64 KiB, `Store::write_har`'s buffer) may wait for the client.
const BODY_CHUNKS_IN_FLIGHT: usize = 4;

/// `io::Write` adapter that hands each write to an async response body. Writes block (the
/// writer runs on a blocking thread) while the client is behind, and fail with `BrokenPipe`
/// once it has gone away.
struct ChannelWriter(BodyChunks);

impl std::io::Write for ChannelWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if data.is_empty() {
            return Ok(0);
        }
        self.0
            .blocking_send(Ok(Bytes::copy_from_slice(data)))
            .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))?;
        Ok(data.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A response body written by `write` on a blocking thread, streamed to the client as it is
/// produced (constant memory, however large the output). If `write` fails, the body ends with
/// an error, so the client sees a failed download rather than a truncated file.
fn streamed_body<F>(write: F) -> axum::body::Body
where
    F: FnOnce(&mut ChannelWriter) -> Result<(), String> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::channel(BODY_CHUNKS_IN_FLIGHT);
    tokio::task::spawn_blocking(move || {
        let mut writer = ChannelWriter(tx.clone());
        if let Err(e) = write(&mut writer)
            && !tx.is_closed()
        {
            tracing::warn!("streamed export failed: {e}");
            let _ = tx.blocking_send(Err(std::io::Error::other(e)));
        }
    });
    // `poll_fn` (not `unfold`): the body may be polled again after it ended.
    axum::body::Body::from_stream(futures::stream::poll_fn(move |cx| rx.poll_recv(cx)))
}

/// `GET /api/export/har?ids=&filter=&redact=` (streamed: see [`streamed_body`])
pub async fn export_har(State(app): State<Arc<App>>, params: Params) -> ApiResult<Response> {
    let ids = selection(&app, &params).await?;
    let options = export_options(&params);
    let body = streamed_body(move |out| {
        app.store
            .write_har_with(out, ids.as_deref(), options)
            .map(drop)
            .map_err(|e| format!("failed to write HAR: {e}"))
    });
    Ok(attachment(
        body,
        "application/json",
        &format!("akev-{}.har", timestamp_slug()),
    ))
}

/// `POST /api/import/har` (read into a single buffer: see [`super::upload`])
pub async fn import_har(
    State(app): State<Arc<App>>,
    headers: http::HeaderMap,
    body: axum::body::Body,
) -> ApiResult<Json<IdsOut>> {
    let size = super::upload::content_length(&headers);
    let body = super::upload::read_whole(body, size, super::MAX_REQUEST_BODY).await?;
    if body.is_empty() {
        return Err(ApiError::bad_request("empty HAR file"));
    }
    let ids = blocking(move || app.store.import_har(&body))
        .await?
        .map_err(|e| ApiError::bad_request(format!("could not import HAR: {e}")))?;
    Ok(Json(IdsOut { ids }))
}

/// `GET /api/export/session?ids=&filter=&redact=`
pub async fn export_session(State(app): State<Arc<App>>, params: Params) -> ApiResult<Response> {
    let ids = selection(&app, &params).await?;
    let options = export_options(&params);
    let bytes = blocking(move || {
        let mut out = Vec::new();
        app.store
            .save_session_with(&mut out, ids.as_deref(), None, options)
            .map(|_| out)
    })
    .await?
    .map_err(|e| ApiError::internal(format!("failed to write session: {e}")))?;
    Ok(attachment(
        bytes,
        "application/octet-stream",
        &format!("akev-{}.akev", timestamp_slug()),
    ))
}

/// `POST /api/import/session` (streamed into the parser: the upload is never held whole)
pub async fn import_session(
    State(app): State<Arc<App>>,
    body: axum::body::Body,
) -> ApiResult<Json<IdsOut>> {
    let mut reader = super::upload::BodyReader::new(body, super::MAX_REQUEST_BODY);
    let ids = blocking(move || {
        if reader.is_empty()? {
            return Ok(None);
        }
        app.store.load_session(&mut reader).map(Some)
    })
    .await?
    .map_err(|e| ApiError::bad_request(format!("could not import session: {e}")))?
    .ok_or_else(|| ApiError::bad_request("empty session file"))?;
    Ok(Json(IdsOut { ids }))
}
