//! Breakpoints, interceptors, settings, CA, capture, status and lifecycle endpoints.

use super::common::{Params, attachment, no_content, parse_id};
use crate::app::{App, Status};
use crate::breakpoints::ResumeError;
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::PausedExchange;
use akev_core::interceptor::{ActivationResult, InterceptorInfo};
use akev_core::settings::Settings;
use akev_intercept::InterceptError;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use http::header::{self, HeaderValue};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ---------- status & lifecycle ----------

/// `GET /api/status`
pub async fn status(State(app): State<Arc<App>>) -> Json<Status> {
    Json(app.status())
}

/// `POST /api/shutdown` — answers first, then signals the embedding app.
pub async fn shutdown(State(app): State<Arc<App>>) -> Response {
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        app.request_shutdown();
    });
    no_content()
}

// ---------- breakpoints ----------

/// `GET /api/breakpoints`
pub async fn breakpoints(State(app): State<Arc<App>>) -> Json<Vec<PausedExchange>> {
    Json(app.breakpoints.list())
}

/// `POST /api/breakpoints/:id/resume`
pub async fn resume(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(payload): JsonBody<serde_json::Value>,
) -> ApiResult<Response> {
    let id = parse_id(&id)?;
    app.breakpoints.resume(id, payload).map_err(|e| match e {
        ResumeError::NotPaused(_) => ApiError::not_found(e.to_string()),
        ResumeError::Invalid(_) => ApiError::bad_request(e.to_string()),
    })?;
    Ok(no_content())
}

/// `POST /api/breakpoints/resume-all`
pub async fn resume_all(State(app): State<Arc<App>>) -> Response {
    app.breakpoints.resume_all();
    no_content()
}

// ---------- interceptors ----------

/// `GET /api/interceptors`
pub async fn interceptors(State(app): State<Arc<App>>) -> Json<Vec<InterceptorInfo>> {
    Json(app.refresh_interceptors(false).await)
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct OptionsBody {
    pub options: Option<serde_json::Value>,
}

fn intercept_error(e: InterceptError) -> ApiError {
    match e {
        InterceptError::NotFound(id) => {
            ApiError::not_found(format!("interceptor {id:?} not found"))
        }
        other => ApiError::internal(other.to_string()),
    }
}

/// Activate an interceptor; friendly failures become `{success: false, message}`.
pub async fn activate_interceptor(
    app: &Arc<App>,
    id: &str,
    options: serde_json::Value,
) -> ApiResult<ActivationResult> {
    if let Some(feature) = crate::license::interceptor_feature(id) {
        app.license.require(feature)?;
    }
    let ctx = app.intercept_ctx();
    let result = match app.interceptors.activate(id, &ctx, options).await {
        Ok(r) => r,
        Err(InterceptError::Unavailable(msg) | InterceptError::Failed(msg)) => ActivationResult {
            success: false,
            message: Some(msg),
            metadata: serde_json::Value::Null,
        },
        Err(e) => return Err(intercept_error(e)),
    };
    refresh_interceptors_later(app);
    Ok(result)
}

pub async fn deactivate_interceptor(
    app: &Arc<App>,
    id: &str,
    options: serde_json::Value,
) -> ApiResult<()> {
    let ctx = app.intercept_ctx();
    let outcome = app.interceptors.deactivate(id, &ctx, options).await;
    refresh_interceptors_later(app);
    outcome.map_err(intercept_error)
}

fn refresh_interceptors_later(app: &Arc<App>) {
    let app = app.clone();
    tokio::spawn(async move {
        app.refresh_interceptors(true).await;
    });
}

/// `POST /api/interceptors/:id/activate`
pub async fn activate(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<OptionsBody>,
) -> ApiResult<Json<ActivationResult>> {
    activate_interceptor(&app, &id, body.options.unwrap_or(serde_json::Value::Null))
        .await
        .map(Json)
}

/// `POST /api/interceptors/:id/deactivate`
pub async fn deactivate(
    State(app): State<Arc<App>>,
    Path(id): Path<String>,
    JsonBody(body): JsonBody<OptionsBody>,
) -> ApiResult<Response> {
    deactivate_interceptor(&app, &id, body.options.unwrap_or(serde_json::Value::Null)).await?;
    Ok(no_content())
}

const SHELLS: &[&str] = &["bash", "zsh", "fish", "powershell", "cmd", "sh"];

/// `GET /api/setup/terminal?shell=bash`
pub async fn terminal(State(app): State<Arc<App>>, params: Params) -> ApiResult<Response> {
    let shell = params
        .non_empty("shell")
        .unwrap_or("bash")
        .to_ascii_lowercase();
    if !SHELLS.contains(&shell.as_str()) {
        return Err(ApiError::bad_request(format!(
            "unsupported shell {shell:?} (one of {})",
            SHELLS.join(", ")
        )));
    }
    let shell = if shell == "sh" {
        "bash".to_string()
    } else {
        shell
    };
    let script = akev_intercept::existing_terminal_script(&app.intercept_ctx(), &shell);
    let mut resp = script.into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    resp.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

// ---------- settings ----------

/// `GET /api/settings`
pub async fn get_settings(State(app): State<Arc<App>>) -> Json<Settings> {
    Json(app.settings())
}

/// `PUT /api/settings`
pub async fn put_settings(
    State(app): State<Arc<App>>,
    JsonBody(settings): JsonBody<Settings>,
) -> ApiResult<Json<Settings>> {
    app.update_settings(settings).await.map(Json)
}

/// `PATCH /api/settings/ui`
pub async fn patch_ui(
    State(app): State<Arc<App>>,
    JsonBody(ui): JsonBody<serde_json::Value>,
) -> Response {
    app.set_ui_settings(ui);
    no_content()
}

// ---------- CA ----------

/// `GET /api/ca`
pub async fn ca_info(State(app): State<Arc<App>>) -> Json<akev_ca::CaInfo> {
    Json(app.ca().info())
}

/// `GET /api/ca/cert.pem`
pub async fn ca_pem(State(app): State<Arc<App>>) -> Response {
    attachment(
        app.ca().cert_pem().to_string(),
        "application/x-pem-file",
        "akev-ca.pem",
    )
}

/// `GET /api/ca/cert.der`
pub async fn ca_der(State(app): State<Arc<App>>) -> Response {
    attachment(
        app.ca().cert_der().to_vec(),
        "application/x-x509-ca-cert",
        "akev-ca.crt",
    )
}

/// `GET /api/update`: the last update check, refreshed first when it's a day old and automatic
/// checks are on.
pub async fn update_get(State(app): State<Arc<App>>) -> Json<crate::updates::UpdateInfo> {
    Json(if app.settings().updates.check_automatically {
        app.updates.check_if_stale().await
    } else {
        app.updates.cached()
    })
}

/// `POST /api/update/check`: check for a new version now (an explicit user action).
pub async fn update_check(State(app): State<Arc<App>>) -> Json<crate::updates::UpdateInfo> {
    Json(app.updates.check().await)
}

/// `GET /api/ca/trust`: whether the operating system's certificate stores trust the CA.
pub async fn ca_trust(State(app): State<Arc<App>>) -> Json<akev_intercept::TrustReport> {
    Json(app.ca_trust.report(&app.intercept_ctx()).await)
}

#[derive(Deserialize)]
pub struct TrustChange {
    /// Store id from `GET /api/ca/trust` (`user`, `system`, `nss`).
    pub store: String,
}

/// `POST /api/ca/trust` `{store}`: install the CA there. The operating system asks the user to
/// confirm. Deliberately not exposed to MCP: only the user installs a root certificate.
pub async fn ca_trust_install(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<TrustChange>,
) -> ApiResult<Json<akev_intercept::TrustReport>> {
    let ctx = app.intercept_ctx();
    app.ca_trust
        .install(&ctx, &body.store)
        .await
        .map_err(trust_error)?;
    Ok(Json(app.ca_trust.report(&ctx).await))
}

/// `DELETE /api/ca/trust` `{store}`: remove the CA from that store (the OS asks to confirm).
pub async fn ca_trust_remove(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<TrustChange>,
) -> ApiResult<Json<akev_intercept::TrustReport>> {
    let ctx = app.intercept_ctx();
    app.ca_trust
        .remove(&ctx, &body.store)
        .await
        .map_err(trust_error)?;
    Ok(Json(app.ca_trust.report(&ctx).await))
}

fn trust_error(e: akev_intercept::TrustError) -> ApiError {
    match e {
        akev_intercept::TrustError::Unavailable(m) => {
            ApiError::new(http::StatusCode::CONFLICT, "trust-unavailable", m)
        }
        akev_intercept::TrustError::Failed(m) => {
            ApiError::new(http::StatusCode::BAD_GATEWAY, "trust-failed", m)
        }
    }
}

/// `POST /api/ca/regenerate`
pub async fn ca_regenerate(State(app): State<Arc<App>>) -> ApiResult<Json<akev_ca::CaInfo>> {
    app.regenerate_ca().await.map(Json)
}

// ---------- capture ----------

#[derive(Serialize, Deserialize)]
pub struct Capture {
    pub recording: bool,
}

/// `GET /api/capture`
pub async fn capture_get(State(app): State<Arc<App>>) -> Json<Capture> {
    Json(Capture {
        recording: app.recording.is_recording(),
    })
}

/// `PUT /api/capture`
pub async fn capture_put(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<Capture>,
) -> Json<Capture> {
    app.set_recording(body.recording);
    Json(Capture {
        recording: app.recording.is_recording(),
    })
}
