//! API error type and JSON request-body extraction.
//!
//! Every non-2xx response carries `{"error": {"code": "...", "message": "..."}}` (codes are
//! kebab-case). Internal details (stack traces, file paths of the daemon) are never leaked
//! beyond the human readable message.

use axum::extract::{FromRequest, Request};
use axum::response::{IntoResponse, Response};
use bytes::Bytes;
use http::StatusCode;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

/// An error returned by an API handler.
#[derive(Debug, Clone)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
    /// Extra fields merged into the `error` object (e.g. filter error spans).
    pub extra: Option<Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        ApiError {
            status,
            code,
            message: message.into(),
            extra: None,
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad-request", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not-found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn unprocessable(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, code, message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }

    pub fn payload_too_large(message: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYLOAD_TOO_LARGE, "payload-too-large", message)
    }

    pub fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "missing or invalid API token (use `Authorization: Bearer <token>`, `?token=` or the akev_token cookie)",
        )
    }

    pub fn forbidden(code: &'static str, message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, code, message)
    }

    /// Error for an unparseable filter query (includes the offending byte span).
    pub fn invalid_filter(err: &akev_filter::ParseError) -> Self {
        let mut e = Self::new(
            StatusCode::BAD_REQUEST,
            "invalid-filter",
            err.message.clone(),
        );
        e.extra = Some(json!({ "start": err.start, "end": err.end }));
        e
    }

    /// The JSON body of this error.
    pub fn body(&self) -> Value {
        let mut inner = serde_json::Map::new();
        inner.insert("code".into(), Value::String(self.code.to_string()));
        inner.insert("message".into(), Value::String(self.message.clone()));
        if let Some(Value::Object(extra)) = &self.extra {
            for (k, v) in extra {
                inner.insert(k.clone(), v.clone());
            }
        }
        json!({ "error": Value::Object(inner) })
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} ({}): {}", self.status, self.code, self.message)
    }
}

impl std::error::Error for ApiError {}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.body()).unwrap_or_default();
        (
            self.status,
            [(http::header::CONTENT_TYPE, "application/json")],
            body,
        )
            .into_response()
    }
}

/// Result alias for handlers.
pub type ApiResult<T> = Result<T, ApiError>;

/// Parse a JSON body leniently: an empty body is treated as `{}` (then `null`), so endpoints
/// whose payload is entirely optional (`POST /api/exchanges/clear`, interceptor activation...)
/// accept requests without a body. The Content-Type header is not required.
pub fn parse_json<T: DeserializeOwned>(bytes: &[u8]) -> ApiResult<T> {
    let trimmed = trim_ascii(bytes);
    if trimmed.is_empty() {
        return serde_json::from_slice(b"{}")
            .or_else(|_| serde_json::from_slice(b"null"))
            .map_err(|e| ApiError::bad_request(format!("request body required: {e}")));
    }
    serde_json::from_slice(trimmed)
        .map_err(|e| ApiError::bad_request(format!("invalid JSON body: {e}")))
}

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(start, |p| p + 1);
    &bytes[start..end]
}

/// JSON body extractor that reports failures in the API error format.
pub struct JsonBody<T>(pub T);

impl<S, T> FromRequest<S> for JsonBody<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Self, Self::Rejection> {
        let bytes = Bytes::from_request(req, state).await.map_err(|rejection| {
            ApiError::new(rejection.status(), "bad-request", rejection.body_text())
        })?;
        parse_json(&bytes).map(JsonBody)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize, Debug, PartialEq)]
    #[serde(rename_all = "camelCase")]
    struct Clear {
        #[serde(default)]
        keep_pinned: bool,
    }

    #[test]
    fn empty_body_defaults() {
        let c: Clear = parse_json(b"").unwrap();
        assert_eq!(c, Clear { keep_pinned: false });
        let c: Clear = parse_json(b"  \n").unwrap();
        assert!(!c.keep_pinned);
        let o: Option<Clear> = parse_json(b"").unwrap();
        assert_eq!(o, Some(Clear { keep_pinned: false }));
        let v: Value = parse_json(b"").unwrap();
        assert_eq!(v, json!({}));
    }

    #[test]
    fn invalid_body_is_bad_request() {
        let err = parse_json::<Clear>(b"{nope").unwrap_err();
        assert_eq!(err.status, StatusCode::BAD_REQUEST);
        assert_eq!(err.code, "bad-request");
    }

    #[test]
    fn error_body_shape() {
        let err = ApiError::not_found("no such exchange");
        assert_eq!(
            err.body(),
            json!({"error": {"code": "not-found", "message": "no such exchange"}})
        );
        let f = ApiError::invalid_filter(&akev_filter::ParseError {
            message: "unknown field".into(),
            start: 2,
            end: 5,
        });
        assert_eq!(
            f.body(),
            json!({"error": {"code": "invalid-filter", "message": "unknown field", "start": 2, "end": 5}})
        );
    }
}
