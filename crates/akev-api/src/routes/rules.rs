//! `/api/rules*` — rule set management, stats, dry runs, import/export.

use super::common::{attachment, blocking};
use crate::app::{App, parse_rule_import};
use crate::body;
use crate::error::{ApiError, ApiResult, JsonBody};
use akev_core::rules::{Action, RuleSet, RuleStats};
use akev_core::{EditableRequest, ExchangeId, RuleId, Side};
use akev_rules::{Decision, MatchContext, RulesEngine};
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// `GET /api/rules`
pub async fn get(State(app): State<Arc<App>>) -> Json<RuleSet> {
    Json(app.rules.rules())
}

#[derive(Serialize)]
pub struct PutOut {
    pub errors: Vec<(RuleId, String)>,
}

/// `PUT /api/rules` (compiled on a blocking thread: script rules run their top-level code)
pub async fn put(
    State(app): State<Arc<App>>,
    JsonBody(rules): JsonBody<RuleSet>,
) -> ApiResult<Json<PutOut>> {
    Ok(Json(PutOut {
        errors: app.set_rules_blocking(rules).await?,
    }))
}

/// `GET /api/rules/stats`
pub async fn stats(State(app): State<Arc<App>>) -> Json<Vec<RuleStats>> {
    Json(app.rules.stats())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct TestBody {
    pub exchange_id: Option<ExchangeId>,
    pub request: Option<EditableRequest>,
    pub rules: Option<RuleSet>,
    /// Execute matching `script` rules (sandboxed) and report their console output.
    pub run_scripts: bool,
}

impl Default for TestBody {
    fn default() -> Self {
        TestBody {
            exchange_id: None,
            request: None,
            rules: None,
            run_scripts: true,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestOut {
    pub matched: Vec<RuleId>,
    pub terminal_rule: Option<RuleId>,
    pub action: Action,
    /// Console output of the scripts that ran (`onRequest` then `onResponse`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub script_logs: Vec<String>,
    /// First script error, if any (message includes the line number).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// What the scripts did: `continue` (request possibly modified) or `respond`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub script_outcome: Option<ScriptOutcome>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ScriptOutcome {
    /// The request continues upstream (as modified by the scripts).
    Continue {
        method: String,
        url: String,
        headers: akev_core::Headers,
        body: String,
        /// Status/headers after `onResponse`, when the exchange has a captured response.
        #[serde(skip_serializing_if = "Option::is_none")]
        response: Option<ScriptResponse>,
    },
    /// A script answered the request itself.
    Respond { response: ScriptResponse },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: akev_core::Headers,
    pub body: String,
}

impl From<Decision> for TestOut {
    fn from(d: Decision) -> Self {
        TestOut {
            matched: d.matched,
            terminal_rule: d.terminal_rule,
            action: d.action,
            script_logs: Vec::new(),
            error: None,
            script_outcome: None,
        }
    }
}

/// Run the decision's scripts against the request (and the captured response, if any).
fn run_scripts(
    scripts: &[String],
    mut head: akev_core::RequestHead,
    mut body: bytes::Bytes,
    response: Option<(akev_core::ResponseHead, bytes::Bytes)>,
    out: &mut TestOut,
) {
    use akev_rules::script::{ScriptRequestOutcome, run_on_request, run_on_response};
    let text = |b: &bytes::Bytes| String::from_utf8_lossy(b).into_owned();
    for source in scripts {
        match run_on_request(source, head.clone(), body.clone()) {
            Ok((ScriptRequestOutcome::Continue { head: h, body: b }, log)) => {
                out.script_logs.extend(log.lines);
                head = h;
                body = b;
            }
            Ok((ScriptRequestOutcome::Respond { head: h, body: b }, log)) => {
                out.script_logs.extend(log.lines);
                out.script_outcome = Some(ScriptOutcome::Respond {
                    response: ScriptResponse {
                        status: h.status,
                        headers: h.headers,
                        body: text(&b),
                    },
                });
                return;
            }
            Err(e) => {
                out.error = Some(e.to_string());
                return;
            }
        }
    }
    let mut response_out = None;
    if let Some((mut rh, mut rb)) = response {
        for source in scripts {
            match run_on_response(source, &head, rh.clone(), rb.clone()) {
                Ok((h, b, log)) => {
                    out.script_logs.extend(log.lines);
                    rh = h;
                    rb = b;
                }
                Err(e) => {
                    out.error = Some(e.to_string());
                    break;
                }
            }
        }
        response_out = Some(ScriptResponse {
            status: rh.status,
            headers: rh.headers,
            body: text(&rb),
        });
    }
    out.script_outcome = Some(ScriptOutcome::Continue {
        method: head.method.clone(),
        url: head.url.clone(),
        headers: head.headers.clone(),
        body: text(&body),
        response: response_out,
    });
}

/// Dry-run rules against a captured exchange or an ad-hoc request.
pub async fn dry_run(app: &Arc<App>, body: TestBody) -> ApiResult<TestOut> {
    let (head, req_body, source) = if let Some(id) = body.exchange_id {
        let detail = app
            .store
            .detail(id)
            .ok_or_else(|| ApiError::not_found(format!("exchange {id} not found")))?;
        let head = detail
            .original_request
            .clone()
            .unwrap_or(detail.request.clone());
        let raw = app
            .store
            .original_body(id, Side::Request)
            .or_else(|| app.store.body(id, Side::Request));
        let decoded = raw
            .map(|b| body::decode_or_raw(&b.bytes, &head.headers).0)
            .unwrap_or_default();
        (head, decoded, detail.summary.source.clone())
    } else if let Some(req) = &body.request {
        let head = body::request_head_from_editable(req).map_err(ApiError::bad_request)?;
        (head, req.body.clone(), None)
    } else {
        return Err(ApiError::bad_request("provide `exchangeId` or `request`"));
    };
    // The captured response (decoded), so `onResponse` hooks can be tested too.
    let response = match body.exchange_id {
        Some(id) if body.run_scripts => app.store.detail(id).and_then(|d| {
            let head = d.original_response.clone().or(d.response.clone())?;
            let raw = app
                .store
                .original_body(id, Side::Response)
                .or_else(|| app.store.body(id, Side::Response));
            let decoded = raw
                .map(|b| body::decode_or_raw(&b.bytes, &head.headers).0)
                .unwrap_or_default();
            Some((head, decoded))
        }),
        _ => None,
    };
    let rules = body.rules;
    let run = body.run_scripts;
    let engine = app.rules.clone();
    let out = blocking(move || {
        let decision = {
            let ctx = MatchContext {
                head: &head,
                body: Some(&req_body),
                source: source.as_deref(),
            };
            // Inline rules are compiled here, on the blocking thread (compiling runs script
            // rules' top-level code).
            match rules {
                Some(rules) => RulesEngine::new(rules).dry_run(&ctx),
                None => engine.dry_run(&ctx),
            }
        };
        let scripts = decision.scripts.clone();
        let mut out = TestOut::from(decision);
        if run && !scripts.is_empty() {
            run_scripts(&scripts, head, req_body, response, &mut out);
        }
        out
    })
    .await?;
    Ok(out)
}

/// `POST /api/rules/stats/reset` — zero every rule's hit counter (and re-arm match limits).
pub async fn reset_stats(State(app): State<Arc<App>>) -> Json<Vec<RuleStats>> {
    app.rules.reset_stats();
    Json(app.rules.stats())
}

/// `POST /api/rules/test`
pub async fn test(
    State(app): State<Arc<App>>,
    JsonBody(body): JsonBody<TestBody>,
) -> ApiResult<Json<TestOut>> {
    dry_run(&app, body).await.map(Json)
}

/// `GET /api/rules/export`
pub async fn export(State(app): State<Arc<App>>) -> ApiResult<Response> {
    let rules = app.rules.rules();
    let mut bytes =
        serde_json::to_vec_pretty(&rules).map_err(|e| ApiError::internal(e.to_string()))?;
    bytes.push(b'\n');
    Ok(attachment(bytes, "application/json", "akev-rules.json"))
}

/// `POST /api/rules/import` — Akev `RuleSet` or HTTP Toolkit export → `RuleSet` (not applied).
pub async fn import(JsonBody(value): JsonBody<serde_json::Value>) -> ApiResult<Json<RuleSet>> {
    let rules = blocking(move || parse_rule_import(&value))
        .await?
        .map_err(ApiError::bad_request)?;
    Ok(Json(rules))
}
