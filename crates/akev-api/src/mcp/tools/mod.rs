//! MCP tool catalogue: definitions (JSON Schema) and implementations over the running app.
//!
//! Results are pretty-printed JSON (or plain text for code snippets), designed for agents:
//! compact rows for lists, decoded & truncated bodies with hints on how to page through the
//! rest, and errors that say how to fix the call.

use super::{CallError, ToolExecutor, ToolOutput, redact};
use crate::app::App;
use crate::body::{self, ApiBodyView};
use crate::routes::common::{blocking, body_view_blocking};
use crate::routes::exchanges::{captured_request, strip_proxy_headers};
use akev_core::rules::{
    Action, BodySource, MatchOp, Matcher, Rule, RuleItem, RuleSet, StringMatch,
};
use akev_core::{
    BreakpointStage, EditableRequest, EditableResponse, ExchangeId, ExchangeKind, ExchangeState,
    ExchangeSummary, Headers, RequestResume, ResponseResume, Side, UnixMicros,
};
use akev_proxy::SendOptions;
use akev_store::StoreEvent;
use async_trait::async_trait;
use base64::Engine;
use bytes::Bytes;
use serde_json::{Map, Value, json};
use std::sync::Arc;
use std::time::Duration;

mod args;
mod control;
mod render;
mod rules;
mod schema;
mod send;
mod traffic;

use args::*;
use control::*;
use render::*;
use rules::*;
use schema::*;
use send::*;
use traffic::*;

/// Summary of the filter language, embedded in tool descriptions.
pub const FILTER_SYNTAX: &str = "Filter language: space-separated terms are ANDed; use OR (or |) and parentheses; \
prefix a term with - or NOT to negate. Operators: field:value (smart: contains, glob with *, regex with /re/), \
field=value (exact), != ^= (starts) $= (ends) *= (contains) ~ (regex) > >= < <=. \
Fields: method, status (404, 4xx, 400-499, >=500), host (alias domain), port, path, url, query, scheme, \
version (h1, h2), type (json, html, image, ... or a mime substring), size (response size, e.g. size:>1mb), reqsize, \
duration (e.g. duration:>500ms), header (header:x-trace = present, header.x-trace:abc = value), reqheader, resheader, \
cookie, body (decoded request or response body), reqbody, resbody, source (process/interceptor), rule, id, note, \
color, error, ip, model (LLM model), provider (LLM provider: openai, anthropic, google, ollama, ...). Flags: is:pending \
is:complete is:error is:aborted is:paused is:pinned is:mocked is:modified is:websocket is:sse is:tls-failure is:tunnel \
is:http is:https is:llm; has:note has:body has:reqbody has:resbody has:cookie \
has:auth. Bare words match the URL. Quote values with spaces: resbody:\"invalid token\". Alternatives: method:get,post. \
Examples: `host:api.example.com status:>=400`, `method:post path:/v1/* -status:2xx`, `is:error OR duration:>2s`.";

const DEFAULT_MAX_BODY_CHARS: usize = 20_000;
const MAX_LIST_LIMIT: u64 = 500;

type ToolResult = Result<Out, String>;

enum Out {
    Json(Value),
    Text(String),
}

/// Tool executor over the running application.
pub struct AppTools {
    app: Arc<App>,
}

impl AppTools {
    pub(crate) fn new(app: Arc<App>) -> Self {
        AppTools { app }
    }
}

/// MCP tools that change state need Akev Pro (`mcp-write`); reading, searching and exporting
/// stay free. Deactivating an interceptor is always allowed, so an agent can undo what it
/// started. The LLM conversation view is the LLM lens.
pub(crate) const MCP_WRITE_TOOLS: &[&str] = &[
    "akev_send_request",
    "akev_replay",
    "akev_set_rules",
    "akev_add_mock",
    "akev_clear_traffic",
    "akev_resume_breakpoint",
    "akev_set_recording",
    "akev_activate_interceptor",
];

/// The Pro feature a tool needs, if any.
fn pro_feature(tool: &str) -> Option<akev_core::license::Feature> {
    use akev_core::license::Feature;
    if MCP_WRITE_TOOLS.contains(&tool) {
        Some(Feature::McpWrite)
    } else if tool == "akev_llm_conversation" {
        Some(Feature::LlmLens)
    } else {
        None
    }
}

#[async_trait]
impl ToolExecutor for AppTools {
    fn definitions(&self) -> Vec<Value> {
        definitions()
    }

    async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, CallError> {
        let app = &self.app;
        if let Some(feature) = pro_feature(name)
            && !app.license.has(feature)
        {
            return Ok(ToolOutput::error(crate::license::pro_required_message(
                feature,
            )));
        }
        // Captured traffic is redacted unless the user allowed `reveal`.
        let redact = if redact::TOOLS.contains(&name) {
            match redact::wanted(&args, app.mcp_reveal_secrets) {
                Ok(redact) => redact,
                Err(message) => return Ok(ToolOutput::error(message)),
            }
        } else {
            false
        };
        let result = match name {
            "akev_status" => status(app),
            "akev_list_exchanges" => list_exchanges(app, &args).await,
            "akev_get_exchange" => get_exchange(app, &args, redact).await,
            "akev_get_body" => get_body(app, &args, redact).await,
            "akev_search" => search(app, &args, redact).await,
            "akev_wait_for" => wait_for(app, &args).await,
            "akev_send_request" => send_request(app, &args, redact).await,
            "akev_replay" => replay(app, &args, redact).await,
            "akev_get_rules" => get_rules(app),
            "akev_set_rules" => set_rules(app, &args).await,
            "akev_add_mock" => add_mock(app, &args).await,
            "akev_test_rules" => test_rules(app, &args).await,
            "akev_clear_traffic" => clear_traffic(app, &args),
            "akev_export_har" => export_har(app, &args, redact).await,
            "akev_snippet" => snippet(app, &args, redact).await,
            "akev_llm_conversation" => llm_conversation(app, &args).await,
            "akev_list_breakpoints" => list_breakpoints(app),
            "akev_resume_breakpoint" => resume_breakpoint(app, &args),
            "akev_set_recording" => set_recording(app, &args),
            "akev_list_interceptors" => list_interceptors(app).await,
            "akev_activate_interceptor" => activate_interceptor(app, &args).await,
            "akev_deactivate_interceptor" => deactivate_interceptor(app, &args).await,
            "akev_filter_help" => Ok(filter_help()),
            other => return Err(CallError::UnknownTool(other.to_string())),
        };
        Ok(match result {
            Ok(Out::Json(mut v)) => {
                if redact {
                    redact::json(&mut v);
                }
                ToolOutput::json(&v)
            }
            Ok(Out::Text(t)) if redact => ToolOutput::ok(akev_core::redact::text(&t).into_owned()),
            Ok(Out::Text(t)) => ToolOutput::ok(t),
            Err(message) => ToolOutput::error(message),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::batch::tests::summary;

    #[test]
    fn definitions_are_well_formed() {
        let defs = definitions();
        let mut names = std::collections::HashSet::new();
        for d in &defs {
            let name = d["name"].as_str().unwrap();
            assert!(names.insert(name.to_string()), "duplicate tool {name}");
            assert!(name.starts_with("akev_"));
            assert!(
                d["description"].as_str().unwrap().len() > 20,
                "{name} needs a description"
            );
            assert_eq!(d["inputSchema"]["type"], "object");
            let props = d["inputSchema"]["properties"].as_object().unwrap();
            for req in d["inputSchema"]["required"].as_array().unwrap() {
                assert!(
                    props.contains_key(req.as_str().unwrap()),
                    "{name}: required {req} not in properties"
                );
            }
            assert!(d["annotations"]["readOnlyHint"].is_boolean());
        }
        for required in [
            "akev_status",
            "akev_list_exchanges",
            "akev_get_exchange",
            "akev_search",
            "akev_send_request",
            "akev_replay",
            "akev_get_rules",
            "akev_set_rules",
            "akev_add_mock",
            "akev_clear_traffic",
            "akev_export_har",
            "akev_snippet",
        ] {
            assert!(
                names.contains(required),
                "missing documented tool {required}"
            );
        }
    }

    #[test]
    fn launching_interceptors_are_marked_destructive() {
        let defs = definitions();
        let activate = defs
            .iter()
            .find(|d| d["name"] == "akev_activate_interceptor")
            .unwrap();
        assert_eq!(activate["annotations"]["destructiveHint"], true);
        assert_eq!(activate["annotations"]["readOnlyHint"], false);
        assert_eq!(activate["annotations"]["openWorldHint"], true);
        let description = activate["description"].as_str().unwrap();
        assert!(description.contains("arbitrary local executable"));
        assert!(description.contains("--mcp-allow-launch"));
    }

    #[test]
    fn program_launch_needs_opt_in() {
        let refusal = mcp_launch_refusal("electron", false).unwrap();
        assert!(refusal.contains("--mcp-allow-launch"));
        assert!(mcp_launch_refusal("electron", true).is_none());
        assert!(mcp_launch_refusal("chrome", false).is_none());
        assert!(mcp_launch_refusal("fresh-terminal", false).is_none());
    }

    #[test]
    fn truncation_is_char_safe() {
        assert_eq!(truncate_chars("héllo", 2), ("hé", true));
        assert_eq!(truncate_chars("hi", 5), ("hi", false));
        assert_eq!(truncate_chars("", 0), ("", false));
        assert_eq!(slice_chars("héllo wörld", 6, 3), "wör");
        assert_eq!(slice_chars("abc", 10, 3), "");
    }

    #[test]
    fn urls_from_summaries() {
        let mut s = summary(1, ExchangeState::Complete, Some(200));
        assert_eq!(summary_url(&s), "http://example.com/1");
        s.scheme = "https".into();
        s.port = 8443;
        assert_eq!(summary_url(&s), "https://example.com:8443/1");
        s.host = "::1".into();
        s.port = 443;
        assert_eq!(summary_url(&s), "https://[::1]/1");
    }

    #[test]
    fn rows_omit_defaults() {
        let s = summary(7, ExchangeState::Complete, Some(404));
        let r = row(&s);
        assert_eq!(r["id"], 7);
        assert_eq!(r["status"], 404);
        assert!(r.get("mocked").is_none());
        assert!(r.get("kind").is_none());
        assert!(r.get("startedAt").is_some());
    }

    #[test]
    fn mock_rule_building() {
        let rule = build_mock_rule(&json!({
            "urlPattern": "api.example.com/users",
            "method": "get",
            "status": 201,
            "body": {"ok": true},
            "times": 2
        }))
        .unwrap();
        assert_eq!(rule.max_matches, Some(2));
        assert!(
            matches!(&rule.matchers[0], Matcher::Method { methods } if methods == &vec!["GET".to_string()])
        );
        match &rule.matchers[1] {
            Matcher::Url { pattern } => {
                assert_eq!(pattern.op, MatchOp::Contains);
                assert_eq!(pattern.value, "api.example.com/users");
            }
            other => panic!("unexpected {other:?}"),
        }
        match &rule.action {
            Action::Mock {
                status,
                body: BodySource::Json { value },
                ..
            } => {
                assert_eq!(*status, 201);
                assert_eq!(value, &json!({"ok": true}));
            }
            other => panic!("unexpected {other:?}"),
        }
        let glob = build_mock_rule(&json!({"urlPattern": "https://*.example.com/*", "body": "hi"}))
            .unwrap();
        assert!(
            matches!(&glob.matchers[0], Matcher::Url { pattern } if pattern.op == MatchOp::Glob)
        );
        assert!(matches!(
            &glob.action,
            Action::Mock {
                status: 200,
                body: BodySource::Text { .. },
                ..
            }
        ));
        let re = build_mock_rule(&json!({"urlPattern": "/users/\\d+/"})).unwrap();
        assert!(
            matches!(&re.matchers[0], Matcher::Url { pattern } if pattern.op == MatchOp::Regex && pattern.value == "users/\\d+")
        );
        assert!(build_mock_rule(&json!({})).is_err());
        assert!(build_mock_rule(&json!({"urlPattern": "x", "status": 42})).is_err());
        assert!(build_mock_rule(&json!({"urlPattern": "x", "match": "fuzzy"})).is_err());
    }

    #[test]
    fn argument_parsing() {
        let args = json!({"id": "#12", "limit": "30", "flag": true, "headers": {"A": "1", "B": ["2", "3"]}});
        assert_eq!(arg_id(&args, "id").unwrap(), ExchangeId(12));
        assert_eq!(arg_u64(&args, "limit", 5, 20).unwrap(), 20);
        assert_eq!(arg_u64(&args, "missing", 5, 20).unwrap(), 5);
        assert!(arg_bool(&args, "flag", false));
        assert_eq!(arg_headers(&args, "headers").unwrap().unwrap().len(), 3);
        assert!(arg_id(&json!({}), "id").is_err());
        assert!(arg_u64(&json!({"n": -1}), "n", 0, 10).is_err());
        let (b, is_json) = arg_body(&json!({"json": {"a": 1}})).unwrap().unwrap();
        assert!(is_json);
        assert_eq!(&b[..], br#"{"a":1}"#);
        let (b, _) = arg_body(&json!({"bodyBase64": "aGk="})).unwrap().unwrap();
        assert_eq!(&b[..], b"hi");
        assert!(arg_body(&json!({"bodyBase64": "!!"})).is_err());
        assert_eq!(arg_body(&json!({})).unwrap(), None);
    }

    #[test]
    fn request_edits() {
        let mut req = EditableRequest {
            method: "GET".into(),
            url: "http://a/".into(),
            headers: Headers(vec![("X-A".into(), "1".into()), ("X-B".into(), "2".into())]),
            body: Bytes::new(),
        };
        apply_request_edits(
            &mut req,
            &json!({"method": "post", "setHeaders": {"x-a": "9"}, "removeHeaders": ["X-B"], "json": [1]}),
        )
        .unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.headers.get("x-a"), Some("9"));
        assert!(!req.headers.contains("x-b"));
        assert_eq!(req.headers.get("content-type"), Some("application/json"));
        assert_eq!(&req.body[..], b"[1]");
    }

    #[test]
    fn snippets_find_context() {
        let s = find_snippet("hello world, the TOKEN is invalid", "token").unwrap();
        assert!(s.contains("TOKEN is invalid"));
        assert!(find_snippet("abc", "zzz").is_none());
        let long = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        let s = find_snippet(&long, "needle").unwrap();
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert!(s.len() < 200 * 4);
    }

    #[test]
    fn send_options_http_version() {
        use akev_proxy::UpstreamHttpVersion as V;
        assert_eq!(send_options(&json!({})).unwrap().http_version, V::Auto);
        assert_eq!(
            send_options(&json!({"httpVersion": "HTTP3"}))
                .unwrap()
                .http_version,
            V::Http3
        );
        assert_eq!(
            send_options(&json!({"httpVersion": "http1"}))
                .unwrap()
                .http_version,
            V::Http1
        );
        assert!(send_options(&json!({"httpVersion": "h9"})).is_err());
    }
}
