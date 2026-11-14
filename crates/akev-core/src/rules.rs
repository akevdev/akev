//! Rule model — the declarative description of how Akev should handle matching traffic.
//!
//! ## Evaluation semantics (implemented by `akev-rules`)
//!
//! Rules are evaluated top-to-bottom (depth-first through groups). Disabled rules/groups and
//! rules whose `maxMatches` is exhausted are skipped. For each rule whose matchers ALL match:
//!
//! * **Modifier** actions ([`Action::is_terminal`] == false: `transform`, `breakpoint`, `delay`,
//!   `throttle`, `highlight`, `script`) are *accumulated* and evaluation continues to the next
//!   rule. This lets users stack e.g. "add auth header to *.api.com" with "mock /users".
//! * The first **terminal** action (`passthrough`, `forward`, `mock`, `map-local`, `close`,
//!   `reset`, `timeout`, `ws-echo`, `ws-reject`) stops evaluation.
//! * If no terminal action matched, the implicit default is `passthrough`.
//!
//! Accumulated modifiers apply in rule order, followed by the terminal action's own transforms.

use crate::http::Headers;
use crate::ids::RuleId;
use serde::{Deserialize, Serialize};

/// Current rule-file schema version.
pub const RULESET_VERSION: u32 = 1;

/// The full ordered set of rules, as edited in the UI and persisted to disk / exported.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleSet {
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub items: Vec<RuleItem>,
}

fn default_version() -> u32 {
    RULESET_VERSION
}

impl Default for RuleSet {
    fn default() -> Self {
        RuleSet {
            version: RULESET_VERSION,
            items: Vec::new(),
        }
    }
}

impl RuleSet {
    /// Iterate over all rules depth-first, yielding each rule together with whether all of its
    /// ancestors (groups) are enabled.
    pub fn walk(&self) -> Vec<(&Rule, bool)> {
        fn rec<'a>(items: &'a [RuleItem], parent_enabled: bool, out: &mut Vec<(&'a Rule, bool)>) {
            for item in items {
                match item {
                    RuleItem::Rule(r) => out.push((r, parent_enabled)),
                    RuleItem::Group(g) => rec(&g.items, parent_enabled && g.enabled, out),
                }
            }
        }
        let mut out = Vec::new();
        rec(&self.items, true, &mut out);
        out
    }

    pub fn find_rule(&self, id: &str) -> Option<&Rule> {
        self.walk().into_iter().map(|(r, _)| r).find(|r| r.id == id)
    }
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum RuleItem {
    Rule(Rule),
    Group(RuleGroup),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleGroup {
    pub id: RuleId,
    pub title: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub collapsed: bool,
    #[serde(default)]
    pub items: Vec<RuleItem>,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: RuleId,
    /// Optional user-facing title; the UI auto-describes the rule when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
    /// All matchers must match (logical AND). Empty = matches every request.
    #[serde(default)]
    pub matchers: Vec<Matcher>,
    pub action: Action,
    /// Stop matching after this many matches (e.g. 1 = "once"). `None` = unlimited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_matches: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// How a string should be compared.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum MatchOp {
    #[default]
    Equals,
    Contains,
    StartsWith,
    EndsWith,
    /// Shell-style glob: `*` any chars except `/` (for paths) / any chars (for hosts), `**` any,
    /// `?` one char.
    Glob,
    Regex,
    /// Value present (any value) — only meaningful for header/query/cookie matchers.
    Exists,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StringMatch {
    #[serde(default)]
    pub op: MatchOp,
    #[serde(default)]
    pub value: String,
    #[serde(default)]
    pub case_sensitive: bool,
    /// Invert the result.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub negate: bool,
}

impl StringMatch {
    pub fn equals(v: impl Into<String>) -> Self {
        StringMatch {
            op: MatchOp::Equals,
            value: v.into(),
            case_sensitive: false,
            negate: false,
        }
    }
    pub fn glob(v: impl Into<String>) -> Self {
        StringMatch {
            op: MatchOp::Glob,
            value: v.into(),
            case_sensitive: false,
            negate: false,
        }
    }
    pub fn regex(v: impl Into<String>) -> Self {
        StringMatch {
            op: MatchOp::Regex,
            value: v.into(),
            case_sensitive: true,
            negate: false,
        }
    }
    pub fn contains(v: impl Into<String>) -> Self {
        StringMatch {
            op: MatchOp::Contains,
            value: v.into(),
            case_sensitive: false,
            negate: false,
        }
    }
}

/// A named key with an optional value constraint (headers, query params, cookies, form fields).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyValueMatch {
    pub key: String,
    /// `None` = key must merely be present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<StringMatch>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Matcher {
    /// Any of the given methods (uppercase).
    Method { methods: Vec<String> },
    /// Full absolute URL.
    Url { pattern: StringMatch },
    /// Hostname (without port).
    Host { pattern: StringMatch },
    /// Any of the given ports.
    Port { ports: Vec<u16> },
    /// Path without query string.
    Path { pattern: StringMatch },
    /// Any of `http`, `https`, `ws`, `wss`.
    Protocol { protocols: Vec<String> },
    /// All listed query params must match.
    Query { params: Vec<KeyValueMatch> },
    /// All listed headers must match.
    Header { headers: Vec<KeyValueMatch> },
    /// All listed cookies must match.
    Cookie { cookies: Vec<KeyValueMatch> },
    /// Decoded request body as text.
    Body { pattern: StringMatch },
    /// Request body parsed as JSON equals (or, when `partial`, contains) this value.
    JsonBody {
        value: serde_json::Value,
        #[serde(default)]
        partial: bool,
    },
    /// Urlencoded or multipart form fields.
    Form { fields: Vec<KeyValueMatch> },
    /// GraphQL operation name (from JSON body or query string).
    GraphqlOperation { pattern: StringMatch },
    /// Source process name / interceptor id.
    Source { pattern: StringMatch },
    /// HTTP version (`HTTP/1.1`, `HTTP/2`, ...).
    HttpVersion { versions: Vec<String> },
    /// Logical NOT.
    Not { matcher: Box<Matcher> },
    /// Logical OR.
    Any { matchers: Vec<Matcher> },
    /// Logical AND (useful inside `any`).
    All { matchers: Vec<Matcher> },
}

impl Matcher {
    /// Whether evaluating this matcher needs the request body (forces buffering).
    pub fn needs_body(&self) -> bool {
        match self {
            Matcher::Body { .. } | Matcher::JsonBody { .. } | Matcher::Form { .. } => true,
            Matcher::GraphqlOperation { .. } => true,
            Matcher::Not { matcher } => matcher.needs_body(),
            Matcher::Any { matchers } | Matcher::All { matchers } => {
                matchers.iter().any(|m| m.needs_body())
            }
            _ => false,
        }
    }
}

/// Where response/request body content comes from.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum BodySource {
    #[default]
    Empty,
    Text {
        text: String,
    },
    Base64 {
        data: String,
    },
    Json {
        value: serde_json::Value,
    },
    /// Read from a local file at response time (re-read on every request, so edits apply live).
    File {
        path: String,
    },
}

/// A find/replace operation on text.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchReplace {
    pub find: String,
    pub replace: String,
    /// Treat `find` as a regex (`replace` may use `$1`, `${name}`).
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub case_sensitive: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum BodyTransform {
    /// Replace the whole body.
    Replace { body: BodySource },
    /// Find/replace on the decoded body text.
    MatchReplace { replacements: Vec<MatchReplace> },
    /// RFC 7386 JSON merge patch (null deletes keys).
    JsonMerge { patch: serde_json::Value },
    /// RFC 6902 JSON patch operations.
    JsonPatch { operations: Vec<serde_json::Value> },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestTransform {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Find/replace applied to the full URL (lets users rewrite host, path or query).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub url_replace: Vec<MatchReplace>,
    /// Set query params (`None` value removes the param).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub set_query: Vec<(String, Option<String>)>,
    /// Replace (or add) these headers.
    #[serde(default, skip_serializing_if = "Headers::is_empty")]
    pub set_headers: Headers,
    /// Append these headers (keeping existing values).
    #[serde(default, skip_serializing_if = "Headers::is_empty")]
    pub add_headers: Headers,
    /// Remove headers by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyTransform>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponseTransform {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status_message: Option<String>,
    #[serde(default, skip_serializing_if = "Headers::is_empty")]
    pub set_headers: Headers,
    #[serde(default, skip_serializing_if = "Headers::is_empty")]
    pub add_headers: Headers,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_headers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodyTransform>,
}

impl RequestTransform {
    pub fn is_empty(&self) -> bool {
        self == &RequestTransform::default()
    }
}
impl ResponseTransform {
    pub fn is_empty(&self) -> bool {
        self == &ResponseTransform::default()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum Action {
    // ---------- terminal ----------
    /// Forward to the original destination (optionally transforming request/response).
    Passthrough {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<RequestTransform>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<ResponseTransform>,
    },
    /// Forward to a different server ("map remote"). `target` is a base URL such as
    /// `https://staging.example.com:8443` or `http://localhost:3000/api`. When the target has a
    /// path, it replaces the matched request path prefix `stripPrefix` (or is prepended).
    Forward {
        target: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strip_prefix: Option<String>,
        /// Keep the original Host header instead of rewriting it to the target's.
        #[serde(default)]
        preserve_host: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<RequestTransform>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<ResponseTransform>,
    },
    /// Respond with a fixed response without contacting upstream.
    Mock {
        status: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        status_message: Option<String>,
        #[serde(default)]
        headers: Headers,
        #[serde(default)]
        body: BodySource,
    },
    /// Serve from the local filesystem. If `path` is a directory, the request path (after
    /// stripping `stripPrefix`) is resolved inside it; `index` is used for directory requests.
    MapLocal {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        strip_prefix: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        index: Option<String>,
        /// When the file doesn't exist: passthrough upstream (true) or 404 (false).
        #[serde(default)]
        fallback_to_upstream: bool,
        #[serde(default)]
        headers: Headers,
    },
    /// Close the connection without responding.
    Close,
    /// Reset the connection (TCP RST / HTTP/2 RST_STREAM).
    Reset,
    /// Accept the request and never respond.
    Timeout,
    /// Accept a WebSocket upgrade and echo every message back.
    WsEcho,
    /// Reject a WebSocket upgrade with an HTTP response.
    WsReject {
        #[serde(default = "default_reject_status")]
        status: u16,
        #[serde(default)]
        body: BodySource,
    },

    // ---------- modifiers ----------
    /// Transform request and/or response, then continue evaluating rules.
    Transform {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<RequestTransform>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        response: Option<ResponseTransform>,
    },
    /// Pause at a breakpoint before forwarding the request and/or before returning the response.
    Breakpoint {
        #[serde(default = "yes")]
        request: bool,
        #[serde(default)]
        response: bool,
    },
    /// Add latency before handling (ms), with optional random jitter.
    Delay {
        ms: u64,
        #[serde(default)]
        jitter_ms: u64,
    },
    /// Limit bandwidth (bytes per second; 0 = unlimited) and add per-request latency.
    Throttle {
        #[serde(default)]
        download_bps: u64,
        #[serde(default)]
        upload_bps: u64,
        #[serde(default)]
        latency_ms: u64,
    },
    /// Tag matching exchanges with a colour (and optional note) in the traffic list.
    Highlight {
        color: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// Present this TLS fingerprint (ClientHello + HTTP/2 preface) to the upstream server,
    /// overriding `proxy.tlsProfile` (the last matching `tls-profile` rule wins).
    TlsProfile {
        #[serde(default)]
        profile: crate::settings::TlsProfile,
    },
    /// Run a JavaScript snippet with `onRequest(request)` and/or `onResponse(request, response)`
    /// hooks that may modify the exchange (or return a response to short-circuit upstream).
    Script { source: String },
    /// Pause the text/binary messages of a matching WebSocket so the user can edit, drop or
    /// forward each one. Only WebSocket upgrade requests match rules with this action.
    WsBreakpoint {
        /// Which direction's messages to pause.
        #[serde(default)]
        direction: WsBreakpointDirection,
        /// Only pause messages whose payload (as UTF-8 text; binary payloads lossily) matches.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pattern: Option<StringMatch>,
    },
}

/// Which WebSocket messages a `ws-breakpoint` rule pauses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WsBreakpointDirection {
    /// Client → server messages.
    Sent,
    /// Server → client messages.
    Received,
    /// Both directions.
    #[default]
    Both,
}

fn default_reject_status() -> u16 {
    403
}

impl Action {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Action::Passthrough { .. }
                | Action::Forward { .. }
                | Action::Mock { .. }
                | Action::MapLocal { .. }
                | Action::Close
                | Action::Reset
                | Action::Timeout
                | Action::WsEcho
                | Action::WsReject { .. }
        )
    }

    /// Short machine name, e.g. `mock`, `map-local`.
    pub fn type_name(&self) -> &'static str {
        match self {
            Action::Passthrough { .. } => "passthrough",
            Action::Forward { .. } => "forward",
            Action::Mock { .. } => "mock",
            Action::MapLocal { .. } => "map-local",
            Action::Close => "close",
            Action::Reset => "reset",
            Action::Timeout => "timeout",
            Action::WsEcho => "ws-echo",
            Action::WsReject { .. } => "ws-reject",
            Action::Transform { .. } => "transform",
            Action::Breakpoint { .. } => "breakpoint",
            Action::Delay { .. } => "delay",
            Action::Throttle { .. } => "throttle",
            Action::Highlight { .. } => "highlight",
            Action::TlsProfile { .. } => "tls-profile",
            Action::Script { .. } => "script",
            Action::WsBreakpoint { .. } => "ws-breakpoint",
        }
    }
}

/// Per-rule match statistics (reported by the rules engine to the UI).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RuleStats {
    pub rule_id: RuleId,
    pub matches: u64,
    /// Unix ms of last match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_matched_at: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ruleset_json_roundtrip() {
        let json = r#"{
          "version": 1,
          "items": [
            {"type":"group","id":"g1","title":"API mocks","items":[
              {"type":"rule","id":"r1","matchers":[
                 {"type":"method","methods":["GET"]},
                 {"type":"host","pattern":{"op":"glob","value":"*.example.com"}},
                 {"type":"path","pattern":{"op":"starts-with","value":"/api/users"}}
               ],
               "action":{"type":"mock","status":200,"headers":[["content-type","application/json"]],
                         "body":{"type":"json","value":{"users":[]}}},
               "maxMatches": 3}
            ]},
            {"type":"rule","id":"r2","action":{"type":"breakpoint","response":true}}
          ]
        }"#;
        let rs: RuleSet = serde_json::from_str(json).unwrap();
        let walked = rs.walk();
        assert_eq!(walked.len(), 2);
        assert!(walked[0].0.action.is_terminal());
        assert!(!walked[1].0.action.is_terminal());
        match &walked[1].0.action {
            Action::Breakpoint { request, response } => assert!(*request && *response),
            _ => panic!(),
        }
        let back = serde_json::to_string(&rs).unwrap();
        let rs2: RuleSet = serde_json::from_str(&back).unwrap();
        assert_eq!(rs, rs2);
    }

    #[test]
    fn ws_breakpoint_action_json() {
        let a: Action = serde_json::from_str(
            r#"{"type":"ws-breakpoint","direction":"sent","pattern":{"op":"contains","value":"auth"}}"#,
        )
        .unwrap();
        assert!(!a.is_terminal());
        assert_eq!(a.type_name(), "ws-breakpoint");
        assert_eq!(
            a,
            Action::WsBreakpoint {
                direction: WsBreakpointDirection::Sent,
                pattern: Some(StringMatch::contains("auth")),
            }
        );
        // Defaults: both directions, every message.
        let a: Action = serde_json::from_str(r#"{"type":"ws-breakpoint"}"#).unwrap();
        assert_eq!(
            a,
            Action::WsBreakpoint {
                direction: WsBreakpointDirection::Both,
                pattern: None,
            }
        );
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::json!({"type": "ws-breakpoint", "direction": "both"})
        );
    }
}
