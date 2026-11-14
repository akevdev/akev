//! Transparent capture (Linux): listener status / redirect detection and generation of the
//! firewall commands (HTTP API reference § Transparent capture). Akev never runs those commands.

use super::common::blocking;
use crate::app::App;
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::settings::Settings;
use akev_core::transparent::{TransparentStatus, now_ms};
use akev_intercept::Sys;
use akev_intercept::transparent::{self as tc, Plan, PlanRequest, SystemFacts};
use axum::Json;
use axum::extract::State;
use serde::Serialize;
use std::sync::Arc;

/// `GET /api/transparent`
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransparentInfo {
    /// Transparent capture can work here (Linux: original destinations are readable).
    pub supported: bool,
    /// Why not, when it can't.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<&'static str>,
    /// Redirected connections arrived recently (the redirect rules are in effect).
    pub active: bool,
    /// The transparent listener.
    pub listener: TransparentStatus,
    /// User, cgroup and tools the commands depend on.
    pub system: SystemFacts,
    /// systemd slice used by per-app capture.
    pub slice: &'static str,
}

/// `GET /api/transparent`
pub async fn status(State(app): State<Arc<App>>) -> ApiResult<Json<TransparentInfo>> {
    let listener = app.proxy().transparent_status();
    let system = blocking(|| SystemFacts::detect(&Sys::system().env)).await?;
    let supported = listener.supported && system.platform == "linux";
    Ok(Json(TransparentInfo {
        supported,
        unsupported_reason: (!supported).then_some(tc::UNSUPPORTED_REASON),
        active: listener.redirect_active(now_ms()),
        listener,
        system,
        slice: tc::plan::CAPTURE_SLICE,
    }))
}

/// `POST /api/transparent/plan` — the commands for a capture mode (nothing is run).
pub async fn plan(
    State(app): State<Arc<App>>,
    JsonBody(req): JsonBody<PlanRequest>,
) -> ApiResult<Json<Plan>> {
    app.license
        .require(akev_core::license::Feature::Transparent)?;
    let ctx = app.intercept_ctx();
    // May write the CA bundle file the launch command points at.
    blocking(move || tc::generate_plan(&ctx, &Sys::system(), &req))
        .await?
        .map(Json)
        .map_err(|e| ApiError::bad_request(e.0))
}

/// Settings checks for `proxy.transparent` (part of `PUT /api/settings` validation).
pub(crate) fn validate(s: &Settings) -> Result<(), String> {
    let t = &s.proxy.transparent;
    if t.enabled && t.port != 0 && t.port == s.proxy.port {
        return Err(format!(
            "proxy.transparent.port ({}) must differ from proxy.port: the main port keeps explicit-proxy semantics",
            t.port
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transparent_port_must_differ_from_the_proxy_port() {
        let mut s = Settings::default();
        assert!(validate(&s).is_ok());
        s.proxy.transparent.port = s.proxy.port;
        assert!(validate(&s).is_ok(), "disabled: not checked");
        s.proxy.transparent.enabled = true;
        assert!(validate(&s).unwrap_err().contains("must differ"));
        s.proxy.transparent.port = 0;
        assert!(validate(&s).is_ok());
        s.proxy.transparent.port = 8001;
        assert!(validate(&s).is_ok());
    }
}
