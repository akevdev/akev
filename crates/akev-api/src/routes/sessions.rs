//! `/api/sessions/*` — the session library (named `.akev` sessions in the data directory),
//! `/api/import/{charles,mitmproxy}` — other tools' captures, and `/api/proxy/rebind`.

use super::common::{Params, attachment, blocking, no_content};
use super::filter::IdsOut;
use super::io::selection;
use crate::app::{App, ProxyStatus};
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::ExchangeId;
use akev_store::{LibraryError, OpenMode, SessionInfo, StoreError};
use axum::Json;
use axum::extract::{Path, State};
use axum::response::Response;
use bytes::Bytes;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;

fn library_error(e: LibraryError) -> ApiError {
    match e {
        LibraryError::NotFound => ApiError::not_found("session not found"),
        LibraryError::Invalid(msg) => ApiError::bad_request(msg),
        LibraryError::Store(StoreError::InvalidFile(msg)) => ApiError::unprocessable(
            "invalid-session",
            format!("the session file is unusable: {msg}"),
        ),
        LibraryError::Store(StoreError::NotFound) => ApiError::not_found("session not found"),
        LibraryError::Store(StoreError::Io(e)) => {
            ApiError::internal(format!("session storage failed: {e}"))
        }
    }
}

/// `GET /api/sessions`
pub async fn list(State(app): State<Arc<App>>) -> ApiResult<Json<Vec<SessionInfo>>> {
    let library = app.sessions.library.clone();
    let list = blocking(move || library.list())
        .await?
        .map_err(library_error)?;
    Ok(Json(list))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveBody {
    name: String,
    #[serde(default)]
    ids: Option<Vec<ExchangeId>>,
    #[serde(default)]
    filter: Option<String>,
}

/// `POST /api/sessions` `{ name, ids?, filter? }` — save the current traffic (all, the ids,
/// the filter's matches, or both intersected) as a new session.
pub async fn save(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<SaveBody>,
) -> ApiResult<Json<SessionInfo>> {
    let mut query = HashMap::new();
    if let Some(ids) = &body.ids {
        let joined: Vec<String> = ids.iter().map(ToString::to_string).collect();
        // An explicit empty list selects nothing (not everything).
        if joined.is_empty() {
            return Err(ApiError::bad_request("no exchanges selected"));
        }
        query.insert("ids".to_string(), joined.join(","));
    }
    if let Some(filter) = body.filter.filter(|f| !f.trim().is_empty()) {
        query.insert("filter".to_string(), filter);
    }
    let ids = selection(&app, &Params(query)).await?;
    let (library, store, name) = (app.sessions.library.clone(), app.store.clone(), body.name);
    let info = blocking(move || library.save(&store, &name, ids.as_deref()))
        .await?
        .map_err(library_error)?;
    Ok(Json(info))
}

/// `GET /api/sessions/:id` — the `.akev` file (streamed from disk).
pub async fn download(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Response> {
    let library = app.sessions.library.clone();
    let lookup = id.clone();
    let (info, file) = blocking(move || {
        let info = library.info(&lookup)?;
        let file = library.open_file(&lookup)?;
        Ok::<_, LibraryError>((info, file))
    })
    .await?
    .map_err(library_error)?;
    let body = axum::body::Body::from_stream(file_chunks(tokio::fs::File::from_std(file)));
    let mut resp = attachment(body, "application/octet-stream", &download_name(&info.name));
    if let Ok(len) = http::HeaderValue::from_str(&info.size.to_string()) {
        resp.headers_mut().insert(http::header::CONTENT_LENGTH, len);
    }
    Ok(resp)
}

/// `Checkout bug` → `Checkout bug.akev` (path separators and control characters replaced).
fn download_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .map(|c| {
            if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let clean = clean.trim().trim_start_matches('.');
    if clean.is_empty() {
        "session.akev".into()
    } else {
        format!("{clean}.akev")
    }
}

/// A file as a stream of 64 KiB chunks; an I/O error ends the stream with that error.
fn file_chunks(
    file: tokio::fs::File,
) -> impl futures::Stream<Item = std::io::Result<Bytes>> + Send + 'static {
    use tokio::io::AsyncReadExt;
    futures::stream::unfold(Some(file), |state| async move {
        let mut file = state?;
        let mut buf = vec![0u8; 64 * 1024];
        match file.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok(Bytes::from(buf)), Some(file)))
            }
            Err(e) => Some((Err(e), None)),
        }
    })
}

#[derive(Deserialize)]
pub struct RenameBody {
    name: String,
}

/// `PUT /api/sessions/:id` `{ name }` — rename (an autosave becomes a saved session with a
/// new id: see the returned info).
pub async fn rename(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<RenameBody>,
) -> ApiResult<Json<SessionInfo>> {
    let library = app.sessions.library.clone();
    let info = blocking(move || library.rename(&id, &body.name))
        .await?
        .map_err(library_error)?;
    Ok(Json(info))
}

/// `DELETE /api/sessions/:id`
pub async fn delete(State(app): State<Arc<App>>, Path(id): Path<String>) -> ApiResult<Response> {
    let library = app.sessions.library.clone();
    blocking(move || library.delete(&id))
        .await?
        .map_err(library_error)?;
    Ok(no_content())
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct OpenBody {
    mode: OpenMode,
}

/// `POST /api/sessions/:id/open` `{ mode: "replace" | "append" }`
pub async fn open(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<OpenBody>,
) -> ApiResult<Json<IdsOut>> {
    let (library, store) = (app.sessions.library.clone(), app.store.clone());
    let ids = blocking(move || library.open(&id, &store, body.mode))
        .await?
        .map_err(library_error)?;
    Ok(Json(IdsOut { ids }))
}

/// `POST /api/import/charles` — a Charles JSON session (`.chlsj`).
pub async fn import_charles(
    State(app): State<Arc<App>>,
    headers: http::HeaderMap,
    body: axum::body::Body,
) -> ApiResult<Json<IdsOut>> {
    let size = super::upload::content_length(&headers);
    let body = super::upload::read_whole(body, size, super::MAX_REQUEST_BODY).await?;
    if body.is_empty() {
        return Err(ApiError::bad_request("empty Charles session file"));
    }
    let ids = blocking(move || app.store.import_charles(&body))
        .await?
        .map_err(|e| ApiError::bad_request(format!("could not import the Charles session: {e}")))?;
    Ok(Json(IdsOut { ids }))
}

/// `POST /api/import/fiddler` — a Fiddler session archive (`.saz`).
pub async fn import_fiddler(
    State(app): State<Arc<App>>,
    headers: http::HeaderMap,
    body: axum::body::Body,
) -> ApiResult<Json<IdsOut>> {
    let size = super::upload::content_length(&headers);
    let body = super::upload::read_whole(body, size, super::MAX_REQUEST_BODY).await?;
    if body.is_empty() {
        return Err(ApiError::bad_request("empty Fiddler session archive"));
    }
    let ids = blocking(move || app.store.import_fiddler(&body))
        .await?
        .map_err(|e| {
            ApiError::bad_request(format!("could not import the Fiddler sessions: {e}"))
        })?;
    Ok(Json(IdsOut { ids }))
}

/// `POST /api/import/mitmproxy` — a mitmproxy flow file.
pub async fn import_mitmproxy(
    State(app): State<Arc<App>>,
    headers: http::HeaderMap,
    body: axum::body::Body,
) -> ApiResult<Json<IdsOut>> {
    let size = super::upload::content_length(&headers);
    let body = super::upload::read_whole(body, size, super::MAX_REQUEST_BODY).await?;
    if body.is_empty() {
        return Err(ApiError::bad_request("empty mitmproxy flow file"));
    }
    let ids = blocking(move || app.store.import_mitmproxy(&body))
        .await?
        .map_err(|e| ApiError::bad_request(format!("could not import the mitmproxy flows: {e}")))?;
    Ok(Json(IdsOut { ids }))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RebindBody {
    port: u16,
    #[serde(default)]
    bind_address: Option<String>,
}

/// `POST /api/proxy/rebind` `{ port, bindAddress? }` — move the proxy there now.
pub async fn rebind(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<RebindBody>,
) -> ApiResult<Json<ProxyStatus>> {
    let bind = body
        .bind_address
        .as_deref()
        .filter(|b| !b.trim().is_empty());
    Ok(Json(app.rebind_proxy(body.port, bind).await?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_names() {
        assert_eq!(download_name("Checkout bug"), "Checkout bug.akev");
        assert_eq!(download_name("a/b\\c:d"), "a_b_c_d.akev");
        assert_eq!(download_name("..hidden"), "hidden.akev");
        assert_eq!(download_name("   "), "session.akev");
        assert_eq!(download_name("日本"), "日本.akev");
    }

    #[test]
    fn library_errors_map_to_statuses() {
        assert_eq!(library_error(LibraryError::NotFound).status, 404);
        assert_eq!(library_error(LibraryError::Invalid("x".into())).status, 400);
        assert_eq!(
            library_error(LibraryError::Store(StoreError::InvalidFile("bad".into()))).status,
            422
        );
        assert_eq!(
            library_error(LibraryError::Store(StoreError::Io(std::io::Error::other(
                "disk"
            ))))
            .status,
            500
        );
    }
}
