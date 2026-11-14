//! Small helpers shared by route handlers.

use crate::error::{ApiError, ApiResult};
use akev_core::{ExchangeId, Side};
use axum::extract::{FromRequestParts, RawQuery};
use axum::response::{IntoResponse, Response};
use http::header::{self, HeaderValue};
use http::request::Parts;
use std::collections::HashMap;

/// Query parameters (never rejects; values are percent-decoded, last occurrence wins).
#[derive(Debug, Default)]
pub struct Params(pub HashMap<String, String>);

impl Params {
    pub fn parse(query: Option<&str>) -> Self {
        let map = query
            .map(|q| {
                url::form_urlencoded::parse(q.as_bytes())
                    .into_owned()
                    .collect()
            })
            .unwrap_or_default();
        Params(map)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0.get(key).map(String::as_str)
    }

    /// Non-empty value.
    pub fn non_empty(&self, key: &str) -> Option<&str> {
        self.get(key).map(str::trim).filter(|v| !v.is_empty())
    }

    /// `?flag`, `?flag=1`, `?flag=true` → true; `0`/`false`/absent → false.
    pub fn flag(&self, key: &str) -> bool {
        match self.get(key) {
            None => false,
            Some(v) => !matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "0" | "false" | "no" | "off"
            ),
        }
    }

    pub fn parse_num<T: std::str::FromStr>(&self, key: &str) -> ApiResult<Option<T>> {
        match self.non_empty(key) {
            None => Ok(None),
            Some(v) => v.parse().map(Some).map_err(|_| {
                ApiError::bad_request(format!("query parameter `{key}` must be a number"))
            }),
        }
    }

    /// Comma separated exchange ids (`?ids=1,2,3`).
    pub fn ids(&self, key: &str) -> ApiResult<Option<Vec<ExchangeId>>> {
        let Some(v) = self.non_empty(key) else {
            return Ok(None);
        };
        v.split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(parse_id)
            .collect::<ApiResult<Vec<_>>>()
            .map(Some)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Params {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let RawQuery(q) = RawQuery::from_request_parts(parts, state).await?;
        Ok(Params::parse(q.as_deref()))
    }
}

pub fn parse_id(s: &str) -> ApiResult<ExchangeId> {
    s.trim()
        .parse::<u64>()
        .map(ExchangeId)
        .map_err(|_| ApiError::bad_request(format!("invalid exchange id {s:?}")))
}

pub fn parse_side(s: &str) -> ApiResult<Side> {
    match s {
        "request" | "req" => Ok(Side::Request),
        "response" | "res" => Ok(Side::Response),
        other => Err(ApiError::bad_request(format!(
            "side must be `request` or `response`, not {other:?}"
        ))),
    }
}

/// Run CPU-heavy work off the async workers.
pub async fn blocking<T, F>(f: F) -> ApiResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::internal(format!("background task failed: {e}")))
}

/// Build body views (decode, format, structured decode) on the blocking pool, at most
/// [`crate::app::BODY_VIEW_PERMITS`] at once: each view of a large body can hold several
/// copies of it, so parallel views must not multiply peak memory. The permit moves into the
/// task, so it stays held until the work ends even if the caller goes away.
pub async fn body_view_blocking<T, F>(app: &crate::app::App, f: F) -> ApiResult<T>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    let permit = app.body_view_permit().await?;
    blocking(move || {
        let out = f();
        drop(permit);
        out
    })
    .await
}

/// A downloadable response.
pub fn attachment(
    bytes: impl Into<axum::body::Body>,
    content_type: &str,
    filename: &str,
) -> Response {
    let mut resp = Response::new(bytes.into());
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(content_type) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&crate::body::content_disposition(filename)) {
        h.insert(header::CONTENT_DISPOSITION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

/// `204 No Content`.
pub fn no_content() -> Response {
    http::StatusCode::NO_CONTENT.into_response()
}

/// `akev-20261108-153000` style timestamp for export file names.
pub fn timestamp_slug() -> String {
    let now = time::OffsetDateTime::now_utc();
    let fmt = time::macros::format_description!("[year][month][day]-[hour][minute][second]");
    now.format(&fmt).unwrap_or_else(|_| "export".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_parsing() {
        let p = Params::parse(Some(
            "filter=status%3A404&limit=10&decoded&download=0&ids=1,%202,,3",
        ));
        assert_eq!(p.get("filter"), Some("status:404"));
        assert_eq!(p.parse_num::<usize>("limit").unwrap(), Some(10));
        assert!(p.flag("decoded"));
        assert!(!p.flag("download"));
        assert!(!p.flag("missing"));
        assert_eq!(
            p.ids("ids").unwrap(),
            Some(vec![ExchangeId(1), ExchangeId(2), ExchangeId(3)])
        );
        assert!(
            Params::parse(Some("limit=x"))
                .parse_num::<usize>("limit")
                .is_err()
        );
        assert!(Params::parse(Some("ids=1,x")).ids("ids").is_err());
        assert_eq!(Params::parse(None).ids("ids").unwrap(), None);
    }

    #[test]
    fn ids_and_sides() {
        assert_eq!(parse_id("42").unwrap(), ExchangeId(42));
        assert!(parse_id("-1").is_err());
        assert!(parse_id("abc").is_err());
        assert_eq!(parse_side("request").unwrap(), Side::Request);
        assert_eq!(parse_side("response").unwrap(), Side::Response);
        assert!(parse_side("both").is_err());
    }

    #[test]
    fn slug_format() {
        let s = timestamp_slug();
        assert_eq!(s.len(), 15);
        assert_eq!(s.as_bytes()[8], b'-');
    }
}
