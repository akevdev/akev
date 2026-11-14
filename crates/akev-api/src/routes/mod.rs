//! HTTP routing: REST API (akev.dev/docs/api), live stream, MCP and the embedded UI.

pub mod common;
pub mod compose;
pub mod exchanges;
pub mod filter;
pub mod io;
pub mod license;
pub mod misc;
pub mod protos;
pub mod rules;
pub mod sessions;
pub mod tls_profiles;
pub mod transparent;
pub(crate) mod upload;
pub mod websocket;

use crate::app::App;
use crate::auth::{self, Security};
use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::{get, patch, post, put};
use std::sync::Arc;
use tower_http::classify::{ServerErrorsAsFailures, SharedClassifier};
use tower_http::compression::CompressionLayer;
use tower_http::trace::TraceLayer;

/// Request bodies accepted by default: JSON settings, filters, patches, sessions, MCP.
pub const DEFAULT_REQUEST_BODY: usize = 2 * 1024 * 1024;
/// Rule sets (`PUT /api/rules`, rule import and test): inline mock bodies and scripts.
pub const RULES_REQUEST_BODY: usize = 32 * 1024 * 1024;
/// Whole captured bodies: imports (HAR, sessions, Charles, mitmproxy), Compose requests
/// (base64 JSON), breakpoint edits, injected WebSocket messages and protobuf uploads.
pub const MAX_REQUEST_BODY: usize = 1024 * 1024 * 1024;

/// A route method that accepts up to `limit` bytes of request body (instead of
/// [`DEFAULT_REQUEST_BODY`]).
fn limited<S: Clone + Send + Sync + 'static>(
    route: axum::routing::MethodRouter<S>,
    limit: usize,
) -> axum::routing::MethodRouter<S> {
    route.layer(DefaultBodyLimit::max(limit))
}

/// Build the complete application router.
pub fn router(app: Arc<App>, security: Arc<Security>) -> Router {
    routes(security).with_state(app)
}

/// The route table with all layers, before the application state is attached.
pub fn routes(security: Arc<Security>) -> Router<Arc<App>> {
    let compressed = Router::new()
        .route("/api/status", get(misc::status))
        .route("/api/diagnostics", get(crate::diagnostics::get))
        // A one-time ticket for a browser download (`{path}` → `{ticket}`), so download URLs
        // never carry the token.
        .route(
            "/api/download-ticket",
            post({
                let security = security.clone();
                move |axum::Json(body): axum::Json<serde_json::Value>| {
                    let security = security.clone();
                    async move {
                        let path = body.get("path").and_then(|p| p.as_str()).unwrap_or("");
                        match security.issue_ticket(path) {
                            Some(ticket) => Ok(axum::Json(serde_json::json!({
                                "ticket": ticket,
                                "expiresInMs": crate::auth::TICKET_TTL.as_millis() as u64,
                            }))),
                            None => Err(crate::error::ApiError::bad_request(
                                "path must be an /api/ path without a query",
                            )),
                        }
                    }
                }
            }),
        )
        .route("/api/shutdown", post(misc::shutdown))
        .route(
            "/api/exchanges",
            get(exchanges::list).patch(exchanges::patch),
        )
        .route("/api/exchanges/delete", post(exchanges::delete))
        .route("/api/exchanges/clear", post(exchanges::clear))
        .route("/api/exchanges/headers", post(exchanges::header_values))
        .route("/api/exchanges/timings", post(exchanges::timings))
        .route("/api/exchanges/{id}", get(exchanges::detail))
        .route("/api/exchanges/{id}/body/{side}", get(exchanges::body_view))
        .route("/api/exchanges/{id}/messages", get(exchanges::messages))
        .route("/api/exchanges/{id}/llm", get(exchanges::llm))
        .route("/api/exchanges/{id}/snippet", get(exchanges::snippet))
        .route("/api/exchanges/{id}/replay", post(exchanges::replay))
        .route(
            "/api/exchanges/{id}/editable/{side}",
            get(exchanges::editable),
        )
        .route("/api/filter/validate", post(filter::validate))
        .route("/api/filter/suggest", post(filter::suggest))
        .route("/api/filter/query", post(filter::query))
        .route("/api/filter/docs", get(filter::docs))
        .route(
            "/api/rules",
            get(rules::get).merge(limited(put(rules::put), RULES_REQUEST_BODY)),
        )
        .route("/api/rules/stats", get(rules::stats))
        .route("/api/rules/stats/reset", post(rules::reset_stats))
        .route(
            "/api/rules/test",
            limited(post(rules::test), RULES_REQUEST_BODY),
        )
        .route("/api/rules/export", get(rules::export))
        .route(
            "/api/rules/import",
            limited(post(rules::import), RULES_REQUEST_BODY),
        )
        .route("/api/breakpoints", get(misc::breakpoints))
        .route("/api/breakpoints/resume-all", post(misc::resume_all))
        .route(
            "/api/breakpoints/{id}/resume",
            limited(post(misc::resume), MAX_REQUEST_BODY),
        )
        .route(
            "/api/exchanges/{id}/messages",
            limited(post(websocket::inject), MAX_REQUEST_BODY),
        )
        .route("/api/breakpoints/messages", get(websocket::paused))
        .route(
            "/api/breakpoints/messages/resume-all",
            post(websocket::resume_all),
        )
        .route(
            "/api/breakpoints/messages/{pause_id}/resume",
            limited(post(websocket::resume), MAX_REQUEST_BODY),
        )
        .route("/api/send", limited(post(compose::send), MAX_REQUEST_BODY))
        .route(
            "/api/import/curl",
            limited(post(compose::import_curl), RULES_REQUEST_BODY),
        )
        .route("/api/codegen/targets", get(compose::targets))
        .route("/api/codegen", post(compose::codegen))
        .route("/api/interceptors", get(misc::interceptors))
        .route("/api/interceptors/{id}/activate", post(misc::activate))
        .route("/api/interceptors/{id}/deactivate", post(misc::deactivate))
        .route("/api/setup/terminal", get(misc::terminal))
        .route("/api/transparent", get(transparent::status))
        .route("/api/transparent/plan", post(transparent::plan))
        .route(
            "/api/settings",
            get(misc::get_settings).put(misc::put_settings),
        )
        .route("/api/settings/ui", patch(misc::patch_ui))
        .route("/api/tls-profiles", get(tls_profiles::list))
        .route(
            "/api/license",
            get(license::get).put(license::put).delete(license::delete),
        )
        .route("/api/license/trial", post(license::trial))
        .route("/api/ca", get(misc::ca_info))
        .route("/api/ca/cert.pem", get(misc::ca_pem))
        .route("/api/ca/cert.der", get(misc::ca_der))
        .route("/api/ca/regenerate", post(misc::ca_regenerate))
        .route("/api/update", get(misc::update_get))
        .route("/api/update/check", post(misc::update_check))
        .route(
            "/api/ca/trust",
            get(misc::ca_trust)
                .post(misc::ca_trust_install)
                .delete(misc::ca_trust_remove),
        )
        .route(
            "/api/capture",
            get(misc::capture_get).put(misc::capture_put),
        )
        .route("/api/export/har", get(io::export_har))
        .route(
            "/api/import/har",
            limited(post(io::import_har), MAX_REQUEST_BODY),
        )
        .route("/api/export/session", get(io::export_session))
        .route(
            "/api/import/session",
            limited(post(io::import_session), MAX_REQUEST_BODY),
        )
        .route(
            "/api/import/charles",
            limited(post(sessions::import_charles), MAX_REQUEST_BODY),
        )
        .route(
            "/api/import/mitmproxy",
            limited(post(sessions::import_mitmproxy), MAX_REQUEST_BODY),
        )
        .route(
            "/api/import/fiddler",
            limited(post(sessions::import_fiddler), MAX_REQUEST_BODY),
        )
        .route("/api/sessions", get(sessions::list).post(sessions::save))
        .route("/api/sessions/{id}/open", post(sessions::open))
        .route("/api/proxy/rebind", post(sessions::rebind))
        .route(
            "/api/wireguard",
            get(crate::wireguard::get_info).put(crate::wireguard::put),
        )
        .route("/api/wireguard/config", get(crate::wireguard::config_file))
        .route(
            "/api/wireguard/regenerate",
            post(crate::wireguard::regenerate),
        )
        .route(
            "/api/protos",
            get(protos::list)
                .merge(limited(post(protos::upload), RULES_REQUEST_BODY))
                .delete(protos::remove),
        )
        .route("/api/protos/file", get(protos::file))
        .route(
            "/api/protos/mappings",
            get(protos::get_mappings).put(protos::put_mappings),
        )
        .route(
            "/mcp",
            limited(post(crate::mcp::http_handler), RULES_REQUEST_BODY)
                .get(crate::mcp::method_not_allowed)
                .delete(crate::mcp::method_not_allowed),
        )
        .fallback(crate::ui::serve)
        .layer(CompressionLayer::new());

    // Raw bodies are served as captured (possibly already compressed) and the stream is a
    // WebSocket: neither goes through the compression layer.
    let uncompressed = Router::new()
        .route(
            "/api/exchanges/{id}/body/{side}/raw",
            get(exchanges::raw_body),
        )
        // Session files are already zstd-compressed.
        .route(
            "/api/sessions/{id}",
            get(sessions::download)
                .put(sessions::rename)
                .delete(sessions::delete),
        )
        .route("/api/stream", get(crate::stream::ws_handler))
        // Streams one line per finished request: compression would hold the progress back.
        .route(
            "/api/send/repeat",
            limited(post(compose::repeat), MAX_REQUEST_BODY),
        );

    compressed
        .merge(uncompressed)
        .layer(DefaultBodyLimit::max(DEFAULT_REQUEST_BODY))
        .layer(trace_layer())
        .layer(axum::middleware::from_fn_with_state(security, auth::guard))
}

type MakeRequestSpan = fn(&http::Request<axum::body::Body>) -> tracing::Span;

/// Request tracing (debug level). Unlike tower-http's default span it never records the
/// `token` query parameter (the WebSocket, downloads, the terminal one-liner and the UI login
/// URL carry it), so debug logs can be shared safely.
pub(crate) fn trace_layer() -> TraceLayer<SharedClassifier<ServerErrorsAsFailures>, MakeRequestSpan>
{
    TraceLayer::new_for_http().make_span_with(request_span as MakeRequestSpan)
}

fn request_span(req: &http::Request<axum::body::Body>) -> tracing::Span {
    tracing::debug_span!(
        "request",
        method = %req.method(),
        uri = %redacted_uri(req.uri()),
        version = ?req.version(),
    )
}

/// Path and query of `uri` with the values of `token` parameters replaced.
pub(crate) fn redacted_uri(uri: &http::Uri) -> String {
    let path = uri.path();
    let Some(query) = uri.query() else {
        return path.to_string();
    };
    let pairs = url::form_urlencoded::parse(query.as_bytes());
    if !pairs.clone().any(|(k, _)| k == "token") {
        return format!("{path}?{query}");
    }
    let redacted = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(pairs.map(|(k, v)| {
            let v = if k == "token" {
                std::borrow::Cow::Borrowed("REDACTED")
            } else {
                v
            };
            (k, v)
        }))
        .finish();
    format!("{path}?{redacted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn request_bodies_are_limited_per_route() {
        use tower::ServiceExt;
        let small = Router::new()
            .route(
                "/small",
                post(|body: bytes::Bytes| async move { body.len().to_string() }),
            )
            .route(
                "/large",
                limited(
                    post(|body: bytes::Bytes| async move { body.len().to_string() }),
                    MAX_REQUEST_BODY,
                ),
            )
            .layer(DefaultBodyLimit::max(DEFAULT_REQUEST_BODY));
        let send = |path: &str, len: usize| {
            let router = small.clone();
            let req = http::Request::builder()
                .method("POST")
                .uri(path)
                .body(axum::body::Body::from(vec![b'x'; len]))
                .unwrap();
            async move { router.oneshot(req).await.unwrap().status() }
        };
        let over = DEFAULT_REQUEST_BODY + 1;
        assert_eq!(send("/small", 10).await, http::StatusCode::OK);
        assert_eq!(
            send("/small", over).await,
            http::StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(send("/large", over).await, http::StatusCode::OK);
    }

    #[test]
    fn route_table_is_valid() {
        // axum panics on conflicting/invalid routes when they are registered.
        let security = Arc::new(Security::new("t".into(), "127.0.0.1", 45456, &[]));
        let _ = routes(security);
    }

    #[test]
    fn uris_are_redacted() {
        let uri = |s: &str| s.parse::<http::Uri>().unwrap();
        assert_eq!(redacted_uri(&uri("/api/status")), "/api/status");
        assert_eq!(
            redacted_uri(&uri("/api/exchanges?filter=status%3A500")),
            "/api/exchanges?filter=status%3A500"
        );
        assert_eq!(
            redacted_uri(&uri("/api/stream?token=s3cret")),
            "/api/stream?token=REDACTED"
        );
        let r = redacted_uri(&uri(
            "/api/setup/terminal?shell=bash&%74oken=s3cret&token=x",
        ));
        assert!(!r.contains("s3cret") && r.contains("shell=bash"), "{r}");
    }

    /// Collects every field value recorded on spans and events.
    #[derive(Clone, Default)]
    struct Capture(Arc<parking_lot::Mutex<String>>);

    struct Collect<'a>(&'a mut String);

    impl tracing::field::Visit for Collect<'_> {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            use std::fmt::Write;
            let _ = write!(self.0, "{}={:?} ", field.name(), value);
        }
    }

    impl tracing::Subscriber for Capture {
        fn register_callsite(
            &self,
            _: &'static tracing::Metadata<'static>,
        ) -> tracing::subscriber::Interest {
            tracing::subscriber::Interest::always()
        }
        fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
            true
        }
        fn new_span(&self, span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            span.record(&mut Collect(&mut self.0.lock()));
            tracing::span::Id::from_u64(1)
        }
        fn record(&self, _: &tracing::span::Id, values: &tracing::span::Record<'_>) {
            values.record(&mut Collect(&mut self.0.lock()));
        }
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            event.record(&mut Collect(&mut self.0.lock()));
        }
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
    }

    #[test]
    fn request_logs_never_contain_the_token() {
        use tower::ServiceExt;
        let capture = Capture::default();
        let router: Router = Router::new()
            .route("/api/stream", get(|| async { "ok" }))
            .layer(trace_layer());
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        tracing::subscriber::with_default(capture.clone(), || {
            runtime.block_on(async {
                let req = http::Request::builder()
                    .uri("/api/stream?token=s3cr3t-t0ken&x=1")
                    .body(axum::body::Body::empty())
                    .unwrap();
                let resp = router.oneshot(req).await.unwrap();
                assert_eq!(resp.status(), http::StatusCode::OK);
            })
        });
        let logged = capture.0.lock().clone();
        assert!(logged.contains("/api/stream"), "{logged}");
        assert!(logged.contains("x=1"), "{logged}");
        assert!(!logged.contains("s3cr3t-t0ken"), "{logged}");
    }
}
