//! `/api/filter/*` — the traffic filter language.

use super::common::blocking;
use crate::app::App;
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::ExchangeId;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct QueryBody {
    #[serde(default)]
    pub query: String,
}

#[derive(Deserialize)]
pub struct SuggestBody {
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub cursor: Option<usize>,
}

/// `POST /api/filter/validate`
pub async fn validate(JsonBody(body): JsonBody<QueryBody>) -> Json<Value> {
    Json(match akev_filter::Filter::parse(&body.query) {
        Ok(_) => json!({ "ok": true }),
        Err(e) => {
            json!({ "ok": false, "error": { "message": e.message, "start": e.start, "end": e.end } })
        }
    })
}

/// Clamp a cursor to the query length and a char boundary.
pub fn clamp_cursor(query: &str, cursor: Option<usize>) -> usize {
    let mut c = cursor.unwrap_or(query.len()).min(query.len());
    while c > 0 && !query.is_char_boundary(c) {
        c -= 1;
    }
    c
}

/// `POST /api/filter/suggest`
pub async fn suggest(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<SuggestBody>,
) -> ApiResult<Json<Vec<akev_filter::Suggestion>>> {
    let cursor = clamp_cursor(&body.query, body.cursor);
    let ctx = app.suggest_context();
    Ok(Json(akev_filter::suggest(&body.query, cursor, &ctx)))
}

#[derive(serde::Serialize)]
pub struct IdsOut {
    pub ids: Vec<ExchangeId>,
}

/// `POST /api/filter/query`
pub async fn query(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<QueryBody>,
) -> ApiResult<Json<IdsOut>> {
    let filter =
        akev_filter::Filter::parse(&body.query).map_err(|e| ApiError::invalid_filter(&e))?;
    let ids = blocking(move || app.store.query_ids(&filter)).await?;
    Ok(Json(IdsOut { ids }))
}

/// `GET /api/filter/docs`
pub async fn docs() -> Json<Vec<akev_filter::FieldDoc>> {
    Json(akev_filter::field_docs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_clamping() {
        assert_eq!(clamp_cursor("abc", None), 3);
        assert_eq!(clamp_cursor("abc", Some(10)), 3);
        assert_eq!(clamp_cursor("abc", Some(1)), 1);
        // 'é' is two bytes: offset 2 is inside it
        assert_eq!(clamp_cursor("aé", Some(2)), 1);
        assert_eq!(clamp_cursor("", Some(5)), 0);
    }
}
