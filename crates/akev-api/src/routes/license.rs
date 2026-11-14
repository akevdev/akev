//! `/api/license` — the Akev Pro licence key and trial (see `license.rs`).

use super::common::blocking;
use crate::app::App;
use crate::error::{ApiResult, JsonBody};
use akev_core::license::LicenseStatus;
use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use std::sync::Arc;

/// `PUT /api/license` body.
#[derive(Debug, Deserialize)]
pub struct KeyBody {
    pub key: String,
}

/// `GET /api/license`
pub async fn get(State(app): State<Arc<App>>) -> Json<LicenseStatus> {
    Json(app.license.status())
}

/// `PUT /api/license` — verify and store a licence key (400 `invalid-license` otherwise).
pub async fn put(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<KeyBody>,
) -> ApiResult<Json<LicenseStatus>> {
    let worker = app.clone();
    blocking(move || worker.license.set_key(&body.key)).await??;
    changed(&app).await
}

/// `DELETE /api/license` — remove the licence key.
pub async fn delete(State(app): State<Arc<App>>) -> ApiResult<Json<LicenseStatus>> {
    let worker = app.clone();
    blocking(move || worker.license.remove_key()).await??;
    changed(&app).await
}

/// `POST /api/license/trial` — start the 14-day trial (409 `trial-unavailable` when used).
pub async fn trial(State(app): State<Arc<App>>) -> ApiResult<Json<LicenseStatus>> {
    let worker = app.clone();
    blocking(move || worker.license.start_trial()).await??;
    changed(&app).await
}

async fn changed(app: &Arc<App>) -> ApiResult<Json<LicenseStatus>> {
    app.apply_license().await;
    Ok(Json(app.license.status()))
}
