//! WebSocket tools: injecting messages into live WebSockets and resolving messages paused by
//! `ws-breakpoint` rules.

use super::common::{no_content, parse_id};
use crate::app::App;
use crate::error::{ApiError, ApiResult, JsonBody};
use crate::ws_breakpoints::{MessageBody, MessageResumeError};
use akev_core::{Direction, ExchangeId, MessageKind, PausedMessage};
use akev_proxy::WsInjectError;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

/// How long an injection waits for its frame to be written before answering without a `seq`
/// (the direction is in the middle of a long message; it is sent once that one is through).
const INJECT_WAIT: Duration = Duration::from_secs(2);

/// `POST /api/exchanges/:id/messages` body.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InjectBody {
    /// `sent` = to the server, `received` = to the client.
    pub direction: Direction,
    #[serde(flatten)]
    pub message: MessageBody,
}

/// `POST /api/exchanges/:id/messages` response.
#[derive(Debug, Serialize)]
pub struct InjectOut {
    /// Sequence number of the recorded message (`null` if it is still queued).
    pub seq: Option<u32>,
}

/// `POST /api/exchanges/:id/messages` → `202 { seq }`.
pub async fn inject(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<InjectBody>,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    let (kind, payload) = body
        .message
        .resolve(MessageKind::Binary)
        .map_err(ApiError::bad_request)?
        .ok_or_else(|| ApiError::bad_request("`text` or `base64` is required"))?;
    let proxy = app.proxy();
    let written = proxy.inject_ws_message(id, body.direction, kind, payload);
    let seq = match tokio::time::timeout(INJECT_WAIT, written).await {
        Ok(Ok(seq)) => Some(seq),
        Ok(Err(e)) => return Err(inject_error(&app, id, e)),
        Err(_) => None,
    };
    Ok((StatusCode::ACCEPTED, Json(InjectOut { seq })).into_response())
}

fn inject_error(app: &App, id: ExchangeId, e: WsInjectError) -> ApiError {
    match e {
        WsInjectError::NotOpen(_) if app.store.summary(id).is_none() => {
            ApiError::not_found(format!("exchange {id} not found"))
        }
        WsInjectError::NotOpen(_) | WsInjectError::NoServer | WsInjectError::Closed => {
            ApiError::conflict(e.to_string())
        }
        WsInjectError::Busy => ApiError::new(StatusCode::TOO_MANY_REQUESTS, "busy", e.to_string()),
        WsInjectError::UnsupportedKind | WsInjectError::InvalidText => {
            ApiError::bad_request(e.to_string())
        }
    }
}

/// `GET /api/breakpoints/messages`
pub async fn paused(State(app): State<Arc<App>>) -> Json<Vec<PausedMessage>> {
    Json(app.breakpoints.messages().list())
}

/// `POST /api/breakpoints/messages/:pauseId/resume`
pub async fn resume(
    State(app): State<Arc<App>>,
    Path(pause_id): Path<String>,
    JsonBody(payload): JsonBody<serde_json::Value>,
) -> ApiResult<Response> {
    let pause_id = pause_id
        .trim()
        .parse::<u64>()
        .map_err(|_| ApiError::bad_request(format!("invalid pause id {pause_id:?}")))?;
    app.breakpoints
        .messages()
        .resume(pause_id, payload)
        .map_err(|e| match e {
            MessageResumeError::NotPaused(_) => ApiError::not_found(e.to_string()),
            MessageResumeError::Invalid(_) => ApiError::bad_request(e.to_string()),
        })?;
    Ok(no_content())
}

/// `POST /api/breakpoints/messages/resume-all` body.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ResumeAllBody {
    /// Only this exchange's messages.
    pub id: Option<u64>,
    /// `forward` (default) or `drop`.
    pub action: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ResumeAllOut {
    pub resumed: usize,
}

/// `POST /api/breakpoints/messages/resume-all` → `{ resumed }`.
pub async fn resume_all(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<ResumeAllBody>,
) -> ApiResult<Json<ResumeAllOut>> {
    let drop_them = match body.action.as_deref() {
        None | Some("forward") => false,
        Some("drop") => true,
        Some(other) => {
            return Err(ApiError::bad_request(format!(
                "unknown action {other:?} (expected `forward` or `drop`)"
            )));
        }
    };
    let resumed = app
        .breakpoints
        .messages()
        .resume_all(body.id.map(ExchangeId), drop_them);
    Ok(Json(ResumeAllOut { resumed }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_body_shapes() {
        let b: InjectBody =
            serde_json::from_str(r#"{"direction":"received","kind":"binary","base64":"AA=="}"#)
                .unwrap();
        assert_eq!(b.direction, Direction::Received);
        assert_eq!(
            b.message.resolve(MessageKind::Binary).unwrap(),
            Some((MessageKind::Binary, bytes::Bytes::from_static(&[0])))
        );
        let b: InjectBody = serde_json::from_str(r#"{"direction":"sent","text":"hi"}"#).unwrap();
        assert_eq!(
            b.message.resolve(MessageKind::Binary).unwrap(),
            Some((MessageKind::Text, bytes::Bytes::from_static(b"hi")))
        );
        assert!(serde_json::from_str::<InjectBody>(r#"{"text":"hi"}"#).is_err());
        assert!(serde_json::from_str::<InjectBody>(r#"{"direction":"up","text":"hi"}"#).is_err());
    }
}
