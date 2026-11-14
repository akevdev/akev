//! Import of HTTP Toolkit rule exports (`.htkrules`, and the raw Mockttp rule wire format).
//!
//! [`convert`] maps HTTP Toolkit's rule tree onto an Akev [`RuleSet`] as faithfully as
//! possible:
//!
//! * The root `{id: "root", items}` and legacy `{rules: [...]}` exports (default rules moved
//!   into a trailing "Default rules" group, like HTTP Toolkit's own migration) are accepted, as
//!   is a bare array of rules/groups. Data with a `version` field is rejected, exactly like
//!   HTTP Toolkit does. Legacy single-`handler` rules and `forwardToLocation` / `forwarding`
//!   passthrough options are migrated first.
//! * Groups keep their id and title and are imported collapsed. Rules keep id, title and the
//!   `activated` flag (→ `enabled`). `completionChecker` `once`/`twice`/`thrice`/`times` →
//!   `maxMatches`.
//! * HTTP Toolkit rules never stack: the first matching rule handles the request. Each HTK
//!   rule therefore becomes one terminal Akev rule, preceded — with identical matchers — by
//!   modifier rules for its non-final steps (`delay`), breakpoints and generated scripts. HTTP
//!   rules get a `protocol: http/https` matcher and WebSocket rules `ws/wss`, because HTTP
//!   Toolkit keeps the two rule kinds apart.
//! * Rule priorities (HTK `OVERRIDE` = 2 > default 1 > `FALLBACK` = 0) are honoured by
//!   emitting the tree once per priority level, highest first (groups present at several
//!   levels are split; later copies get an id suffix `~p<priority>`).
//! * Matchers map 1:1 where Akev has an equivalent (`simple-path` → host/port/path matchers,
//!   `exact-query-string` → an anchored URL regex, `header`/`query`/`form-data`/`cookie` →
//!   case-sensitive equality, JS regexes → Rust regexes with `i`/`s`/`m` flags...).
//! * Steps: `simple`/`file` → `mock`, `passthrough` → `passthrough`, `forward-to-host` →
//!   `forward`, breakpoints → `breakpoint` + `passthrough`, `req-res-transformer` → transforms
//!   (host/path/query match-replace and `replaceHeaders` become a generated script with the
//!   exact JavaScript semantics), `json-rpc-response` (Ethereum results) → a generated script,
//!   `close-connection`/`reset-connection`/`timeout`/`ws-echo`/`ws-reject` → their Akev
//!   counterparts.
//! * Anything that can't be reproduced makes the rule **disabled** with a `note` explaining
//!   why (callback matchers/steps, WebRTC rules, `ws-listen`, multipart filename matching,
//!   regexes using lookaround...). Unsupported side-effect steps (`webhook`,
//!   `informational-response`) become separate disabled placeholder rules, leaving the rest of
//!   the rule active. Minor semantic differences (dropped trailers, RFC 7386 vs lodash JSON
//!   merging, replace-all vs first-match...) are recorded in the rule's `note`.
//! * HTTP Toolkit's built-in default rules (`default-wildcard`, `default-ws-wildcard`,
//!   `default-amiusing`, `default-certificate`, `default-android-certificate`) are imported
//!   disabled: Akev passes unmatched traffic through by default and serves its own CA.
//!
//! ## Mockttp parity
//!
//! Every Mockttp rule capability and its Akev equivalent (✓ = imported automatically):
//!
//! | Mockttp | Akev | |
//! |---|---|---|
//! | `wildcard` matcher | empty matcher list | ✓ |
//! | `method` (enum index) | `method` (any method name, incl. custom ones) | ✓ |
//! | `protocol` | `protocol` (`ws`/`wss` detected from upgrades) | ✓ |
//! | `host` / `hostname` / `port` | `host` (+ `port`) | ✓ |
//! | `simple-path` (relative, absolute, protocol-less) | `path` equality (+ `protocol`/`host`/`port`) | ✓ |
//! | `regex-path` | `url` regex OR `path` regex | ✓ (JS-only regex syntax → disabled) |
//! | `regex-url` | `url` regex (Akev URLs include the query) | ✓ |
//! | `header` / `query` / `form-data` / `cookie` | `header` / `query` / `form` / `cookie` | ✓ |
//! | `exact-query-string` | anchored `url` regex | ✓ |
//! | `multipart-form-data` | `form` (name + text content) | ✓ except filename / binary-content conditions |
//! | `raw-body` / `raw-body-includes` / `raw-body-regexp` | `body` equals / contains / regex | ✓ |
//! | `json-body` / `json-body-matching` | `json-body` exact / partial (arrays positional) | ✓ |
//! | `callback` matcher | no equivalent: it runs code in the remote client; use a `script` | disabled |
//! | completion checkers (`once`…`times`) | `maxMatches` (atomic) | ✓ |
//! | priorities | rule order (tree emitted per priority level) | ✓ |
//! | `simple` / `file` steps | `mock` (text, base64 or file body) | ✓ (trailers dropped) |
//! | `passthrough` (+ `transformRequest` / `transformResponse`) | `passthrough` / `forward` + transforms, scripts for host/path/query match-replace and `replaceHeaders` | ✓ |
//! | `beforeRequest` / `beforeResponse` (HTK breakpoints) | `breakpoint` | ✓ |
//! | `close-connection` / `reset-connection` / `timeout` | `close` / `reset` / `timeout` | ✓ |
//! | `json-rpc-response` | generated `script` echoing the request id | ✓ |
//! | `delay` | `delay` (Akev adds jitter and `throttle`) | ✓ |
//! | `wait-for-request-body` | not needed: the proxy buffers bodies whenever a rule needs them | dropped (noted) |
//! | `callback` / `stream` steps | no equivalent for remote callbacks/streams; use a `script` | disabled |
//! | `webhook` / `informational-response` | not rule actions in Akev (traffic is observable via the API/MCP stream) | disabled placeholder |
//! | `ws-passthrough` / `ws-echo` / `ws-reject` | `passthrough` / `ws-echo` / `ws-reject` | ✓ (reject headers/message dropped) |
//! | `ws-listen` | no equivalent | disabled |
//! | passthrough connection options (TLS trust, client certs, upstream proxy, DNS) | global proxy settings, not per-rule | not imported |

use crate::RulesError;
use akev_core::Headers;
use akev_core::ids::new_uuid;
use akev_core::rules::{
    Action, BodySource, BodyTransform, KeyValueMatch, MatchOp, MatchReplace, Matcher,
    RULESET_VERSION, RequestTransform, ResponseTransform, Rule, RuleGroup, RuleItem, RuleSet,
    StringMatch,
};
use base64::Engine;
use serde_json::{Map, Value};

/// HTTP Toolkit's placeholder for `undefined` inside stringified transform objects.
const UNDEFINED_PLACEHOLDER: &str = "__http_toolkit_undefined_placeholder__";
/// Mockttp's wire placeholder for omitted values.
const MOCKTTP_OMIT: &str = "__mockttp__transform__omit__";
/// Mockttp's `Method` enum order (the wire value is the index).
const METHODS: [&str; 9] = [
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "QUERY",
];
const DEFAULT_PRIORITY: i64 = 1;

/// Convert an HTTP Toolkit rules export into an Akev rule set.
///
/// Structural problems with the export as a whole are errors; problems with individual rules
/// never are — such rules are imported disabled, with a note.
pub fn convert(value: &Value) -> Result<RuleSet, RulesError> {
    let items = root_items(value)?;
    let nodes = items.iter().map(convert_item).collect::<Vec<_>>();
    Ok(RuleSet {
        version: RULESET_VERSION,
        items: split_by_priority(nodes),
    })
}

/// Heuristic: does this JSON look like an HTTP Toolkit export rather than an Akev
/// [`RuleSet`]? (Akev items are tagged `"type": "rule" | "group"`.)
pub fn looks_like_htk(value: &Value) -> bool {
    fn is_akev_item(v: &Value) -> bool {
        matches!(
            v.get("type").and_then(Value::as_str),
            Some("rule" | "group")
        )
    }
    fn is_htk_item(v: &Value) -> bool {
        v.get("steps").is_some()
            || v.get("handler").is_some()
            || v.get("activated").is_some()
            || (v.get("items").is_some() && v.get("type").is_none())
    }
    match value {
        Value::Object(o) => {
            if o.get("isRoot") == Some(&Value::Bool(true))
                || o.get("id").and_then(Value::as_str) == Some("root")
            {
                return true;
            }
            if o.get("rules").is_some_and(Value::is_array) && !o.contains_key("items") {
                return true;
            }
            match o.get("items").and_then(Value::as_array) {
                Some(items) => items.iter().any(is_htk_item) && !items.iter().any(is_akev_item),
                None => false,
            }
        }
        Value::Array(items) => items.iter().any(is_htk_item) && !items.iter().any(is_akev_item),
        _ => false,
    }
}

fn structural(message: impl Into<String>) -> RulesError {
    RulesError::InvalidRule {
        rule: "root".to_string(),
        message: message.into(),
    }
}

/// The top-level items, after HTTP Toolkit's legacy migrations.
fn root_items(value: &Value) -> Result<Vec<Value>, RulesError> {
    match value {
        Value::Array(items) => Ok(items.clone()),
        Value::Object(o) => {
            if let Some(v) = o.get("version").filter(|v| !v.is_null()) {
                return Err(structural(format!(
                    "unknown HTTP Toolkit rules format version {v}"
                )));
            }
            if let Some(rules) = o.get("rules").and_then(Value::as_array) {
                // Legacy flat export: default rules go into a trailing "Default rules" group.
                let (defaults, others): (Vec<&Value>, Vec<&Value>) = rules.iter().partition(|r| {
                    r.get("id")
                        .and_then(Value::as_str)
                        .is_some_and(|id| id.starts_with("default-"))
                });
                let mut items: Vec<Value> = others.into_iter().cloned().collect();
                if !defaults.is_empty() {
                    items.push(serde_json::json!({
                        "id": "default-group",
                        "title": "Default rules",
                        "items": defaults,
                    }));
                }
                return Ok(items);
            }
            if let Some(items) = o.get("items").and_then(Value::as_array) {
                return Ok(items.clone());
            }
            if o.contains_key("matchers") && (o.contains_key("steps") || o.contains_key("handler"))
            {
                return Ok(vec![value.clone()]);
            }
            Err(structural(
                "not an HTTP Toolkit rules export (expected `items` or `rules`)",
            ))
        }
        _ => Err(structural(
            "not an HTTP Toolkit rules export (expected a JSON object)",
        )),
    }
}

// ------------------------------------------------------------------------------------------
// Tree & priorities
// ------------------------------------------------------------------------------------------

enum Node {
    Group {
        id: String,
        title: String,
        children: Vec<Node>,
    },
    Rules {
        priority: i64,
        rules: Vec<Rule>,
    },
}

fn convert_item(item: &Value) -> Node {
    if let Some(children) = item.get("items").and_then(Value::as_array) {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map_or_else(new_uuid, str::to_string);
        let title = item
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("Group")
            .to_string();
        return Node::Group {
            id,
            title,
            children: children.iter().map(convert_item).collect(),
        };
    }
    let (priority, rules) = convert_rule(item);
    Node::Rules { priority, rules }
}

fn collect_priorities(nodes: &[Node], out: &mut Vec<i64>) {
    for n in nodes {
        match n {
            Node::Group { children, .. } => collect_priorities(children, out),
            Node::Rules { priority, .. } => {
                if !out.contains(priority) {
                    out.push(*priority);
                }
            }
        }
    }
}

fn split_by_priority(nodes: Vec<Node>) -> Vec<RuleItem> {
    let mut priorities = Vec::new();
    collect_priorities(&nodes, &mut priorities);
    priorities.sort_unstable_by(|a, b| b.cmp(a));
    let mut out = Vec::new();
    let mut seen_groups: Vec<String> = Vec::new();
    for p in &priorities {
        let only = priorities.len() == 1;
        out.extend(filter_level(&nodes, *p, only, &mut seen_groups));
    }
    out
}

fn filter_level(
    nodes: &[Node],
    level: i64,
    only: bool,
    seen_groups: &mut Vec<String>,
) -> Vec<RuleItem> {
    let mut out = Vec::new();
    for n in nodes {
        match n {
            Node::Rules { priority, rules } if only || *priority == level => {
                out.extend(rules.iter().cloned().map(RuleItem::Rule));
            }
            Node::Rules { .. } => {}
            Node::Group {
                id,
                title,
                children,
            } => {
                let items = filter_level(children, level, only, seen_groups);
                if items.is_empty() && !(only && children.is_empty()) {
                    continue;
                }
                let gid = if seen_groups.contains(id) {
                    format!("{id}~p{level}")
                } else {
                    id.clone()
                };
                seen_groups.push(id.clone());
                out.push(RuleItem::Group(RuleGroup {
                    id: gid,
                    title: title.clone(),
                    enabled: true,
                    collapsed: true,
                    items,
                }));
            }
        }
    }
    out
}

// ------------------------------------------------------------------------------------------
// Rules
// ------------------------------------------------------------------------------------------

/// A piece of a converted HTK rule.
struct Part {
    /// Id suffix (`None` = the terminal part, which keeps the HTK rule id).
    suffix: Option<&'static str>,
    label: &'static str,
    action: Action,
    /// Placeholder parts are always disabled.
    placeholder: bool,
    note: Option<String>,
}

impl Part {
    fn terminal(action: Action) -> Self {
        Part {
            suffix: None,
            label: "",
            action,
            placeholder: false,
            note: None,
        }
    }
    fn modifier(suffix: &'static str, label: &'static str, action: Action) -> Self {
        Part {
            suffix: Some(suffix),
            label,
            action,
            placeholder: false,
            note: None,
        }
    }
    fn placeholder(suffix: &'static str, label: &'static str, note: String) -> Self {
        Part {
            suffix: Some(suffix),
            label,
            action: Action::Highlight {
                color: "gray".to_string(),
                note: Some(note.clone()),
            },
            placeholder: true,
            note: Some(note),
        }
    }
}

/// Collected conversion problems for one rule.
#[derive(Default)]
struct Issues {
    /// Reasons the rule must be disabled.
    fatal: Vec<String>,
    /// Lossy-but-acceptable differences.
    notes: Vec<String>,
}

fn convert_rule(raw: &Value) -> (i64, Vec<Rule>) {
    let Some(obj) = raw.as_object() else {
        let rule = Rule {
            id: new_uuid(),
            title: Some("Invalid HTTP Toolkit rule".to_string()),
            enabled: false,
            matchers: Vec::new(),
            action: Action::Passthrough {
                request: None,
                response: None,
            },
            max_matches: None,
            note: Some(format!(
                "Not imported: the rule is not a JSON object ({raw})"
            )),
        };
        return (DEFAULT_PRIORITY, vec![rule]);
    };
    let id = obj
        .get("id")
        .and_then(Value::as_str)
        .map_or_else(new_uuid, str::to_string);
    let title = obj.get("title").and_then(Value::as_str).map(str::to_string);
    let activated = obj
        .get("activated")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let rule_type = obj.get("type").and_then(Value::as_str).unwrap_or("http");
    let priority = obj
        .get("priority")
        .and_then(Value::as_i64)
        .unwrap_or(DEFAULT_PRIORITY);
    let max_matches = completion_to_max(obj.get("completionChecker"));
    let mut issues = Issues::default();

    // Matchers.
    let mut matchers = Vec::new();
    let raw_matchers = obj
        .get("matchers")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if raw_matchers.is_empty() {
        issues
            .fatal
            .push("the rule has no matchers (it never matched in HTTP Toolkit)".to_string());
    }
    for m in &raw_matchers {
        match convert_matcher(m, &mut issues.notes) {
            Ok(ms) => matchers.extend(ms),
            Err(reason) => issues.fatal.push(reason),
        }
    }
    match rule_type {
        "http" | "ethereum" | "ipfs" | "websocket" => {
            if !matchers
                .iter()
                .any(|m| matches!(m, Matcher::Protocol { .. }))
            {
                let protocols = if rule_type == "websocket" {
                    ["ws", "wss"]
                } else {
                    ["http", "https"]
                };
                matchers.insert(
                    0,
                    Matcher::Protocol {
                        protocols: protocols.iter().map(|p| p.to_string()).collect(),
                    },
                );
            }
        }
        "webrtc" => issues
            .fatal
            .push("WebRTC rules are not supported by Akev".to_string()),
        other => issues
            .fatal
            .push(format!("unknown HTTP Toolkit rule type `{other}`")),
    }

    // Steps (legacy single `handler` supported).
    let steps: Vec<Value> = match (
        obj.get("steps").and_then(Value::as_array),
        obj.get("handler"),
    ) {
        (Some(steps), _) if !steps.is_empty() => steps.clone(),
        (_, Some(handler)) if handler.is_object() => vec![handler.clone()],
        _ => Vec::new(),
    };
    let mut parts = if steps.is_empty() {
        issues.fatal.push("the rule has no steps".to_string());
        vec![Part::terminal(Action::Passthrough {
            request: None,
            response: None,
        })]
    } else {
        convert_steps(&steps, rule_type, &mut issues)
    };

    // HTTP Toolkit's built-in rules.
    let is_plain_passthrough = parts.len() == 1
        && matches!(
            parts[0].action,
            Action::Passthrough {
                request: None,
                response: None
            }
        )
        && issues.fatal.is_empty();
    match id.as_str() {
        "default-wildcard" | "default-ws-wildcard" if is_plain_passthrough => issues.fatal.push(
            "HTTP Toolkit's built-in catch-all passthrough rule; Akev passes unmatched traffic through by default"
                .to_string(),
        ),
        "default-amiusing" => issues
            .fatal
            .push("HTTP Toolkit's built-in rule for amiusing.httptoolkit.tech is not needed in Akev".to_string()),
        "default-certificate" | "default-android-certificate" => issues.fatal.push(
            "this built-in rule serves HTTP Toolkit's CA certificate; Akev serves its own at http://akev.cert/"
                .to_string(),
        ),
        _ => {}
    }

    for part in &mut parts {
        sanitize_action(&mut part.action, &mut issues.notes);
    }
    let enabled = activated && issues.fatal.is_empty();
    let main_note = build_note(&issues);
    let mut rules = Vec::with_capacity(parts.len());
    for part in parts.drain(..) {
        let is_main = part.suffix.is_none();
        let rule_id = match part.suffix {
            Some(s) => format!("{id}/{s}"),
            None => id.clone(),
        };
        let part_title = match (&title, part.suffix) {
            (Some(t), Some(_)) => Some(format!("{t} ({})", part.label)),
            (Some(t), None) => Some(t.clone()),
            (None, _) => None,
        };
        let note = if is_main {
            main_note.clone()
        } else {
            match (&part.note, part.placeholder || !enabled) {
                (Some(n), _) => Some(n.clone()),
                (None, true) if !issues.fatal.is_empty() => {
                    Some(format!("Part of HTTP Toolkit rule {id}, disabled with it."))
                }
                _ => None,
            }
        };
        rules.push(Rule {
            id: rule_id,
            title: part_title,
            enabled: enabled && !part.placeholder,
            matchers: matchers.clone(),
            action: part.action,
            max_matches,
            note,
        });
    }
    (priority, rules)
}

fn build_note(issues: &Issues) -> Option<String> {
    let mut parts = Vec::new();
    if !issues.fatal.is_empty() {
        parts.push(format!(
            "Imported disabled from HTTP Toolkit: {}.",
            issues.fatal.join("; ")
        ));
    }
    if !issues.notes.is_empty() {
        parts.push(format!("Import notes: {}.", issues.notes.join("; ")));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

fn completion_to_max(checker: Option<&Value>) -> Option<u32> {
    let checker = checker?;
    match checker.get("type").and_then(Value::as_str)? {
        "once" => Some(1),
        "twice" => Some(2),
        "thrice" => Some(3),
        "times" => checker
            .get("count")
            .and_then(Value::as_u64)
            .and_then(|c| u32::try_from(c).ok()),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------
// Matchers
// ------------------------------------------------------------------------------------------

fn part_type(v: &Value) -> &str {
    v.get("type")
        .and_then(Value::as_str)
        .or_else(|| v.get("uiType").and_then(Value::as_str))
        .unwrap_or("")
}

fn exact(value: impl Into<String>) -> StringMatch {
    StringMatch {
        op: MatchOp::Equals,
        value: value.into(),
        case_sensitive: true,
        negate: false,
    }
}

fn str_field<'a>(v: &'a Value, key: &str) -> Result<&'a str, String> {
    v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{}` matcher without `{key}`", part_type(v)))
}

fn value_to_strings(v: &Value) -> Vec<String> {
    match v {
        Value::Array(items) => items.iter().flat_map(value_to_strings).collect(),
        Value::String(s) => vec![s.clone()],
        Value::Null => Vec::new(),
        other => vec![other.to_string()],
    }
}

fn kv_matches(obj: Option<&Value>) -> Vec<KeyValueMatch> {
    let mut out = Vec::new();
    if let Some(Value::Object(map)) = obj {
        for (k, v) in map {
            for value in value_to_strings(v) {
                out.push(KeyValueMatch {
                    key: k.clone(),
                    value: Some(exact(value)),
                });
            }
        }
    }
    out
}

fn convert_matcher(m: &Value, notes: &mut Vec<String>) -> Result<Vec<Matcher>, String> {
    let t = part_type(m);
    Ok(match t {
        "wildcard" | "default-wildcard" | "ws-wildcard" | "default-ws-wildcard" => Vec::new(),
        "method" => {
            let name = match m.get("method") {
                Some(Value::Number(n)) => n
                    .as_u64()
                    .and_then(|i| METHODS.get(i as usize))
                    .map(|s| s.to_string())
                    .ok_or_else(|| format!("unknown HTTP method index {n}"))?,
                Some(Value::String(s)) => s.to_ascii_uppercase(),
                _ => m
                    .get("uiType")
                    .and_then(Value::as_str)
                    .filter(|u| METHODS.contains(u))
                    .map(str::to_string)
                    .ok_or("method matcher without a method")?,
            };
            vec![Matcher::Method {
                methods: vec![name],
            }]
        }
        "protocol" => vec![Matcher::Protocol {
            protocols: vec![str_field(m, "protocol")?.to_ascii_lowercase()],
        }],
        "host" => host_port_matchers(str_field(m, "host")?)?,
        "hostname" => vec![Matcher::Host {
            pattern: exact(str_field(m, "hostname")?),
        }],
        "port" => {
            let port = match m.get("port") {
                Some(Value::String(s)) => s.parse::<u16>().ok(),
                Some(Value::Number(n)) => n.as_u64().and_then(|n| u16::try_from(n).ok()),
                _ => None,
            }
            .ok_or("port matcher with an invalid port")?;
            vec![Matcher::Port { ports: vec![port] }]
        }
        "simple-path" => flexible_path(str_field(m, "path")?)?,
        "regex-path" => {
            let (re, cs) = js_regex(m, "regexSource")?;
            vec![Matcher::Any {
                matchers: vec![
                    Matcher::Url {
                        pattern: regex_match(&re, cs),
                    },
                    Matcher::Path {
                        pattern: regex_match(&re, cs),
                    },
                ],
            }]
        }
        "regex-url" => {
            let (re, cs) = js_regex(m, "regexSource")?;
            vec![Matcher::Url {
                pattern: regex_match(&re, cs),
            }]
        }
        "header" => vec![Matcher::Header {
            headers: kv_matches(m.get("headers")),
        }],
        "query" => vec![Matcher::Query {
            params: kv_matches(m.get("queryObject")),
        }],
        "exact-query-string" => {
            let q = m.get("query").and_then(Value::as_str).unwrap_or("");
            let re = if q.is_empty() {
                "^[^?]*$".to_string()
            } else {
                format!("^[^?]*{}$", regex::escape(q))
            };
            vec![Matcher::Url {
                pattern: regex_match(&re, true),
            }]
        }
        "form-data" => vec![Matcher::Form {
            fields: kv_matches(m.get("formData")),
        }],
        "multipart-form-data" => {
            let conditions = m
                .get("matchConditions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let mut fields = Vec::new();
            for c in &conditions {
                if c.get("filename").is_some_and(|f| !f.is_null()) {
                    return Err("multipart filename matching is not supported".to_string());
                }
                let name = c.get("name").and_then(Value::as_str);
                let content = match c.get("content") {
                    None | Some(Value::Null) => None,
                    Some(v) => Some(
                        String::from_utf8(buffer_bytes(v).ok_or("unsupported multipart content")?)
                            .map_err(|_| {
                                "binary multipart content matching is not supported".to_string()
                            })?,
                    ),
                };
                match name {
                    Some(name) => fields.push(KeyValueMatch {
                        key: name.to_string(),
                        value: content.map(exact),
                    }),
                    None if content.is_some() => {
                        return Err(
                            "multipart matching on content without a field name is not supported"
                                .to_string(),
                        );
                    }
                    None => {}
                }
            }
            vec![Matcher::Form { fields }]
        }
        "raw-body" => vec![Matcher::Body {
            pattern: exact(str_field(m, "content")?),
        }],
        "raw-body-includes" => vec![Matcher::Body {
            pattern: StringMatch {
                op: MatchOp::Contains,
                value: str_field(m, "content")?.to_string(),
                case_sensitive: true,
                negate: false,
            },
        }],
        "raw-body-regexp" => {
            let (re, cs) = js_regex(m, "regexString")?;
            vec![Matcher::Body {
                pattern: regex_match(&re, cs),
            }]
        }
        "json-body" => vec![Matcher::JsonBody {
            value: m.get("body").cloned().unwrap_or(Value::Null),
            partial: false,
        }],
        "json-body-matching" => {
            let body = m.get("body").cloned().unwrap_or(Value::Null);
            if contains_array(&body) {
                notes.push(
                    "JSON arrays in 'JSON body including' matchers are compared position by position in Akev".to_string(),
                );
            }
            vec![Matcher::JsonBody {
                value: body,
                partial: true,
            }]
        }
        "cookie" => {
            let cookies = kv_matches(m.get("cookie"));
            if cookies.len() > 1 {
                notes.push(
                    "multi-cookie matchers never matched in HTTP Toolkit (a known bug); Akev requires all listed cookies"
                        .to_string(),
                );
            }
            vec![Matcher::Cookie { cookies }]
        }
        "callback" => {
            return Err(
                "callback matchers run code inside HTTP Toolkit and can't be imported".to_string(),
            );
        }
        t if t.starts_with("rtc-") || t.starts_with("has-rtc-") => {
            return Err("WebRTC matchers are not supported".to_string());
        }
        "" => return Err("matcher without a type".to_string()),
        other => return Err(format!("unsupported matcher `{other}`")),
    })
}

fn contains_array(v: &Value) -> bool {
    match v {
        Value::Array(_) => true,
        Value::Object(m) => m.values().any(contains_array),
        _ => false,
    }
}

fn regex_match(re: &str, case_sensitive: bool) -> StringMatch {
    StringMatch {
        op: MatchOp::Regex,
        value: re.to_string(),
        case_sensitive,
        negate: false,
    }
}

/// `host[:port]` → host (+ port) matchers.
fn host_port_matchers(host: &str) -> Result<Vec<Matcher>, String> {
    let (h, port) =
        crate::urlutil::split_authority(host).ok_or_else(|| format!("invalid host `{host}`"))?;
    let mut out = vec![Matcher::Host { pattern: exact(h) }];
    if let Some(p) = port {
        out.push(Matcher::Port { ports: vec![p] });
    }
    Ok(out)
}

/// Mockttp's `FlexiblePathMatcher`: `/path` (any host), `http(s)://host[:port]/path` or
/// `host[:port]/path`; exact, case-sensitive, query ignored.
fn flexible_path(path: &str) -> Result<Vec<Matcher>, String> {
    if path.is_empty() {
        return Err("empty path matcher".to_string());
    }
    let path_matcher = |p: &str| {
        let p = if p.is_empty() { "/" } else { p };
        let p = if p.contains('%') {
            percent_encoding::percent_decode_str(p)
                .decode_utf8_lossy()
                .into_owned()
        } else {
            p.to_string()
        };
        Matcher::Path { pattern: exact(p) }
    };
    if path.starts_with('/') {
        return Ok(vec![path_matcher(path.split('?').next().unwrap_or(path))]);
    }
    let lower = path.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        let parts =
            crate::urlutil::split_url(path).ok_or_else(|| format!("invalid URL `{path}`"))?;
        let port = parts.effective_port().unwrap_or(80);
        let p = parts.path.split('?').next().unwrap_or("/").to_string();
        return Ok(vec![
            Matcher::Protocol {
                protocols: vec![parts.scheme.clone()],
            },
            Matcher::Host {
                pattern: exact(parts.host),
            },
            Matcher::Port { ports: vec![port] },
            path_matcher(&p),
        ]);
    }
    let (authority, rest) = match path.find('/') {
        Some(i) => (&path[..i], &path[i..]),
        None => (path, "/"),
    };
    let mut out = host_port_matchers(authority)?;
    out.push(path_matcher(rest.split('?').next().unwrap_or(rest)));
    Ok(out)
}

/// Translate a serialized JS regex (`{<source_key>, regexFlags|flags}`) into a Rust regex and
/// whether it's case-sensitive.
fn js_regex(m: &Value, source_key: &str) -> Result<(String, bool), String> {
    let source = m
        .get(source_key)
        .or_else(|| m.get("source"))
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{}` matcher without a regex", part_type(m)))?;
    let flags = m
        .get("regexFlags")
        .or_else(|| m.get("flags"))
        .and_then(Value::as_str)
        .unwrap_or("");
    translate_js_regex(source, flags)
}

/// JS regex source + flags → Rust regex source (validated) + case sensitivity.
pub(crate) fn translate_js_regex(source: &str, flags: &str) -> Result<(String, bool), String> {
    let mut prefix = String::new();
    if flags.contains('s') {
        prefix.push_str("(?s)");
    }
    if flags.contains('m') {
        prefix.push_str("(?m)");
    }
    // JS allows `\/`; normalise it to `/` (handling escaped backslashes correctly).
    let mut out = String::with_capacity(source.len() + prefix.len());
    out.push_str(&prefix);
    let mut chars = source.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('/') => out.push('/'),
                Some(n) => {
                    out.push('\\');
                    out.push(n);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    let case_sensitive = !flags.contains('i');
    crate::matchers::build_regex(&out, !case_sensitive).map_err(|e| {
        format!("regular expression /{source}/{flags} uses syntax Akev doesn't support (lookaround and backreferences aren't available): {e}")
    })?;
    Ok((out, case_sensitive))
}

/// Bytes of a serialized buffer (`{type: "Buffer", data: [..]}`, a byte array) or a string.
fn buffer_bytes(v: &Value) -> Option<Vec<u8>> {
    match v {
        Value::String(s) => Some(s.as_bytes().to_vec()),
        Value::Object(o) => buffer_bytes(o.get("data")?),
        Value::Array(items) => items
            .iter()
            .map(|b| b.as_u64().and_then(|b| u8::try_from(b).ok()))
            .collect(),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------
// Steps
// ------------------------------------------------------------------------------------------

fn is_final_step(t: &str) -> bool {
    !matches!(
        t,
        "delay" | "wait-for-request-body" | "webhook" | "informational-response"
    )
}

fn convert_steps(steps: &[Value], rule_type: &str, issues: &mut Issues) -> Vec<Part> {
    let mut parts = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let t = part_type(step);
        if !is_final_step(t) {
            match t {
                "delay" => {
                    let ms = step.get("delayMs").and_then(Value::as_u64).unwrap_or(0);
                    parts.push(Part::modifier(
                        "delay",
                        "delay",
                        Action::Delay { ms, jitter_ms: 0 },
                    ));
                }
                "wait-for-request-body" => {
                    issues
                        .notes
                        .push("the 'wait for request body' step was dropped".to_string());
                }
                "webhook" => {
                    let url = step.get("url").and_then(Value::as_str).unwrap_or("?");
                    let events =
                        value_to_strings(step.get("events").unwrap_or(&Value::Null)).join(", ");
                    parts.push(Part::placeholder(
                        "webhook",
                        "webhook",
                        format!("HTTP Toolkit webhook step ({url}, events: {events}) is not supported by Akev"),
                    ));
                }
                _ => {
                    let status = step.get("status").map(Value::to_string).unwrap_or_default();
                    parts.push(Part::placeholder(
                        "informational",
                        "informational response",
                        format!("HTTP Toolkit informational ({status}) response step is not supported by Akev"),
                    ));
                }
            }
            continue;
        }
        match convert_final_step(step, rule_type, issues) {
            Ok(mut ps) => parts.append(&mut ps),
            Err(reason) => {
                issues.fatal.push(reason);
                parts.push(Part::terminal(Action::Passthrough {
                    request: None,
                    response: None,
                }));
            }
        }
        if i + 1 < steps.len() {
            issues
                .notes
                .push("steps after the final step were ignored".to_string());
        }
        return parts;
    }
    issues
        .notes
        .push("the rule had no final step; it passes requests through".to_string());
    parts.push(Part::terminal(Action::Passthrough {
        request: None,
        response: None,
    }));
    parts
}

fn convert_final_step(
    step: &Value,
    rule_type: &str,
    issues: &mut Issues,
) -> Result<Vec<Part>, String> {
    let ui = step.get("uiType").and_then(Value::as_str);
    let t = part_type(step);
    let passthrough = || Action::Passthrough {
        request: None,
        response: None,
    };
    match (t, ui) {
        (_, Some("request-breakpoint")) => Ok(breakpoint(true, false)),
        (_, Some("response-breakpoint")) => Ok(breakpoint(false, true)),
        (_, Some("request-and-response-breakpoint")) => Ok(breakpoint(true, true)),
        ("passthrough" | "ws-passthrough", _) => {
            let req_cb = step
                .get("hasBeforeRequestCallback")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let res_cb = step
                .get("hasBeforeResponseCallback")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if req_cb || res_cb {
                // Mockttp wire format: HTTP Toolkit implements breakpoints as callbacks.
                return Ok(breakpoint(req_cb, res_cb));
            }
            let mut step = step.clone();
            migrate_forwarding(&mut step);
            let has_transforms = ["transformRequest", "transformResponse"].iter().any(|k| {
                step.get(*k)
                    .and_then(Value::as_object)
                    .is_some_and(|o| o.values().any(|v| !v.is_null()))
            });
            if has_transforms {
                convert_transforming(&step, rule_type, issues)
            } else {
                Ok(vec![Part::terminal(passthrough())])
            }
        }
        ("simple", _) => Ok(vec![Part::terminal(fixed_response(step, issues)?)]),
        ("file", _) => {
            let path = step.get("filePath").and_then(Value::as_str).unwrap_or("");
            if path.is_empty() {
                return Err("the file response step has no file".to_string());
            }
            Ok(vec![Part::terminal(Action::Mock {
                status: status_of(step, "status")?,
                status_message: step
                    .get("statusMessage")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                headers: headers_of(step.get("headers")),
                body: BodySource::File {
                    path: path.to_string(),
                },
            })])
        }
        ("close-connection", _) => Ok(vec![Part::terminal(Action::Close)]),
        ("reset-connection", _) => Ok(vec![Part::terminal(Action::Reset)]),
        ("timeout", _) => Ok(vec![Part::terminal(Action::Timeout)]),
        ("ws-echo", _) => Ok(vec![Part::terminal(Action::WsEcho)]),
        ("ws-reject", _) => {
            let status = status_of(step, "statusCode")
                .or_else(|_| status_of(step, "status"))
                .unwrap_or(400);
            let body = match step.get("body") {
                Some(v) if !v.is_null() => body_from_data(v),
                _ => BodySource::Empty,
            };
            if step
                .get("headers")
                .and_then(Value::as_object)
                .is_some_and(|h| !h.is_empty())
            {
                issues
                    .notes
                    .push("custom headers on the WebSocket rejection were dropped".to_string());
            }
            Ok(vec![Part::terminal(Action::WsReject { status, body })])
        }
        ("json-rpc-response", _) => {
            let result = step
                .get("result")
                .and_then(Value::as_object)
                .ok_or("JSON-RPC response step without a result")?;
            let (kind, value) = match (result.get("result"), result.get("error")) {
                (Some(v), _) => ("result", v),
                (None, Some(e)) => ("error", e),
                _ => return Err("JSON-RPC response step without a result or error".to_string()),
            };
            Ok(vec![
                Part::modifier(
                    "script",
                    "JSON-RPC response",
                    Action::Script {
                        source: json_rpc_script(kind, value),
                    },
                ),
                Part::terminal(passthrough()),
            ])
        }
        ("callback", _) => {
            Err("callback steps run code inside HTTP Toolkit and can't be imported".to_string())
        }
        ("stream", _) => Err("stream steps can't be imported".to_string()),
        ("ws-listen", _) => {
            Err("the 'accept and listen' WebSocket step is not supported by Akev".to_string())
        }
        (t, _) if t.contains("rtc") => Err("WebRTC steps are not supported".to_string()),
        ("", _) => Err("step without a type".to_string()),
        (other, _) => Err(format!("unsupported step `{other}`")),
    }
}

fn breakpoint(request: bool, response: bool) -> Vec<Part> {
    vec![
        Part::modifier(
            "breakpoint",
            "breakpoint",
            Action::Breakpoint { request, response },
        ),
        Part::terminal(Action::Passthrough {
            request: None,
            response: None,
        }),
    ]
}

/// Mockttp's legacy passthrough `forwardToLocation` / `forwarding` → `transformRequest`.
fn migrate_forwarding(step: &mut Value) {
    let Some(obj) = step.as_object_mut() else {
        return;
    };
    if let Some(location) = obj.remove("forwardToLocation") {
        obj.entry("forwarding")
            .or_insert(serde_json::json!({ "targetHost": location, "updateHostHeader": true }));
    }
    let Some(forwarding) = obj.remove("forwarding") else {
        return;
    };
    let Some(target) = forwarding.get("targetHost").and_then(Value::as_str) else {
        return;
    };
    let (protocol, host) = match target.split_once("://") {
        Some((p, h)) => (Some(p.to_string()), h.to_string()),
        None => (None, target.to_string()),
    };
    let update = forwarding
        .get("updateHostHeader")
        .cloned()
        .unwrap_or(Value::Bool(true));
    let tr = obj
        .entry("transformRequest")
        .or_insert_with(|| Value::Object(Map::new()));
    if let Some(tr) = tr.as_object_mut() {
        tr.insert(
            "replaceHost".into(),
            serde_json::json!({ "targetHost": host, "updateHostHeader": update }),
        );
        if let Some(p) = protocol {
            tr.insert("setProtocol".into(), Value::String(p));
        }
    }
}

fn status_of(step: &Value, key: &str) -> Result<u16, String> {
    step.get(key)
        .and_then(Value::as_u64)
        .and_then(|s| u16::try_from(s).ok())
        .filter(|s| (100..=999).contains(s))
        .ok_or_else(|| format!("`{}` step with an invalid status", part_type(step)))
}

/// Drop headers Akev can't send (pseudo-headers, invalid names, CR/LF in values).
fn sanitize_headers(headers: &mut Headers, notes: &mut Vec<String>) {
    headers.0.retain(|(name, value)| {
        let ok = crate::transform::validate_header_name(name).is_ok()
            && crate::transform::validate_header_value(name, value).is_ok();
        if !ok {
            notes.push(format!("the invalid header `{name}` was dropped"));
        }
        ok
    });
}

/// Make every header in a converted action valid for Akev.
fn sanitize_action(action: &mut Action, notes: &mut Vec<String>) {
    let fix_req = |t: &mut Option<RequestTransform>, notes: &mut Vec<String>| {
        if let Some(t) = t {
            sanitize_headers(&mut t.set_headers, notes);
            sanitize_headers(&mut t.add_headers, notes);
        }
    };
    match action {
        Action::Mock {
            headers,
            status_message,
            ..
        } => {
            sanitize_headers(headers, notes);
            if status_message
                .as_deref()
                .is_some_and(|m| crate::transform::validate_header_value("status", m).is_err())
            {
                *status_message = None;
            }
        }
        Action::Passthrough { request, response }
        | Action::Forward {
            request, response, ..
        } => {
            fix_req(request, notes);
            if let Some(t) = response {
                sanitize_headers(&mut t.set_headers, notes);
                sanitize_headers(&mut t.add_headers, notes);
            }
        }
        _ => {}
    }
}

fn headers_of(v: Option<&Value>) -> Headers {
    let mut out = Headers::new();
    match v {
        Some(Value::Object(map)) => {
            for (k, v) in map {
                for value in value_to_strings(v) {
                    out.append(k.clone(), value);
                }
            }
        }
        // Raw `[[name, value], ...]` pairs.
        Some(Value::Array(pairs)) => {
            for pair in pairs {
                if let Some([k, v]) = pair.as_array().map(Vec::as_slice)
                    && let (Some(k), Some(v)) = (k.as_str(), v.as_str())
                {
                    out.append(k, v);
                }
            }
        }
        _ => {}
    }
    out
}

fn body_from_data(v: &Value) -> BodySource {
    match v {
        Value::String(s) if s.is_empty() => BodySource::Empty,
        Value::String(s) => BodySource::Text { text: s.clone() },
        other => match buffer_bytes(other) {
            Some(bytes) if bytes.is_empty() => BodySource::Empty,
            Some(bytes) => match String::from_utf8(bytes) {
                Ok(text) => BodySource::Text { text },
                Err(e) => BodySource::Base64 {
                    data: base64::engine::general_purpose::STANDARD.encode(e.into_bytes()),
                },
            },
            None => BodySource::Empty,
        },
    }
}

fn fixed_response(step: &Value, issues: &mut Issues) -> Result<Action, String> {
    if step
        .get("trailers")
        .and_then(Value::as_object)
        .is_some_and(|t| !t.is_empty())
    {
        issues
            .notes
            .push("response trailers are not supported and were dropped".to_string());
    }
    Ok(Action::Mock {
        status: status_of(step, "status")?,
        status_message: step
            .get("statusMessage")
            .and_then(Value::as_str)
            .map(str::to_string),
        headers: headers_of(step.get("headers")),
        body: step.get("data").map_or(BodySource::Empty, body_from_data),
    })
}

/// A Script generating a JSON-RPC response that echoes the request id (Mockttp
/// `json-rpc-response` semantics, including the 500 for invalid JSON-RPC requests).
fn json_rpc_script(kind: &str, value: &Value) -> String {
    format!(
        r#"// Generated from an HTTP Toolkit JSON-RPC response step.
function onRequest(req) {{
  var id, ok = false;
  try {{
    var b = JSON.parse(req.body);
    if (b && typeof b === 'object' && b.jsonrpc === '2.0' && 'id' in b) {{ id = b.id; ok = true; }}
  }} catch (e) {{}}
  if (!ok) {{
    return {{ status: 500, headers: {{ 'content-type': 'text/plain' }}, body: "Can't send a JSON-RPC response to an invalid JSON-RPC request" }};
  }}
  var response = {{ jsonrpc: '2.0', id: id }};
  response[{kind}] = {value};
  return {{ status: 200, headers: {{ 'content-type': 'application/json' }}, body: JSON.stringify(response) }};
}}
"#,
        kind = js_string(kind),
        value = value,
    )
}

fn js_string(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

// ------------------------------------------------------------------------------------------
// Transforming steps
// ------------------------------------------------------------------------------------------

/// Parse a transform field that HTTP Toolkit stores as a JSON string with `undefined`
/// placeholders (or Mockttp sends as a string / object).
fn maybe_stringified(v: &Value) -> Option<Value> {
    match v {
        Value::Null => None,
        Value::String(s) => serde_json::from_str(s).ok(),
        other => Some(other.clone()),
    }
}

fn is_placeholder(v: &Value) -> bool {
    matches!(v.as_str(), Some(UNDEFINED_PLACEHOLDER | MOCKTTP_OMIT))
}

/// `updateHeaders` → set/remove lists.
fn update_headers(v: &Value, set: &mut Headers, remove: &mut Vec<String>) {
    let Some(Value::Object(map)) = maybe_stringified(v) else {
        return;
    };
    for (k, v) in map {
        if v.is_null() || is_placeholder(&v) {
            remove.push(k);
        } else {
            for value in value_to_strings(&v) {
                set.append(k.clone(), value);
            }
        }
    }
}

/// `updateJsonBody` (lodash merge, `undefined` deletes) → RFC 7386 merge patch.
fn update_json_body(v: &Value, notes: &mut Vec<String>) -> Option<Value> {
    fn convert(v: Value, literal_null: &mut bool, array: &mut bool) -> Value {
        match v {
            Value::Object(map) => Value::Object(
                map.into_iter()
                    .map(|(k, v)| (k, convert(v, literal_null, array)))
                    .collect(),
            ),
            Value::Array(items) => {
                *array = true;
                Value::Array(items)
            }
            Value::Null => {
                *literal_null = true;
                Value::Null
            }
            v if is_placeholder(&v) => Value::Null,
            other => other,
        }
    }
    let raw = maybe_stringified(v)?;
    let (mut literal_null, mut array) = (false, false);
    let patch = convert(raw, &mut literal_null, &mut array);
    if literal_null {
        notes.push("null values in a JSON body merge now delete the key (RFC 7386) instead of setting null".to_string());
    }
    if array {
        notes.push(
            "arrays in a JSON body merge replace the original array instead of merging by index"
                .to_string(),
        );
    }
    Some(patch)
}

/// Rust replacement syntax for a JS `String.prototype.replace` replacement string.
fn js_replacement(rep: &str, groups: usize, notes: &mut Vec<String>) -> String {
    let chars: Vec<char> = rep.chars().collect();
    let mut out = String::with_capacity(rep.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c != '$' {
            out.push(c);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('$') => {
                out.push_str("$$");
                i += 2;
            }
            Some('&') => {
                out.push_str("${0}");
                i += 2;
            }
            Some(d) if d.is_ascii_digit() => {
                let one = d.to_digit(10).unwrap_or(0) as usize;
                let two = chars
                    .get(i + 2)
                    .and_then(|c| c.to_digit(10))
                    .map(|d2| one * 10 + d2 as usize);
                match two {
                    Some(n) if n >= 1 && n <= groups => {
                        out.push_str(&format!("${{{n}}}"));
                        i += 3;
                    }
                    _ if one >= 1 && one <= groups => {
                        out.push_str(&format!("${{{one}}}"));
                        i += 2;
                    }
                    _ => {
                        out.push_str("$$");
                        i += 1;
                    }
                }
            }
            Some('<') => match chars[i + 2..].iter().position(|&c| c == '>') {
                Some(end) => {
                    let name: String = chars[i + 2..i + 2 + end].iter().collect();
                    out.push_str(&format!("${{{name}}}"));
                    i += 3 + end;
                }
                None => {
                    out.push_str("$$");
                    i += 1;
                }
            },
            Some('`' | '\'') => {
                notes.push(
                    "the $` and $' replacement patterns are not supported and were kept literally"
                        .to_string(),
                );
                out.push_str("$$");
                i += 1;
            }
            _ => {
                out.push_str("$$");
                i += 1;
            }
        }
    }
    out
}

/// Serialized match/replace pairs → Akev find/replace entries (exact for global regexes).
fn convert_pairs(v: &Value, notes: &mut Vec<String>) -> Result<Vec<MatchReplace>, String> {
    let pairs = v.as_array().ok_or("invalid match/replace configuration")?;
    let mut out = Vec::with_capacity(pairs.len());
    for pair in pairs {
        let Some([pattern, replacement]) = pair.as_array().map(Vec::as_slice) else {
            return Err("invalid match/replace pair".to_string());
        };
        let replacement = replacement.as_str().unwrap_or("");
        match pattern {
            Value::String(s) => {
                notes.push("literal match/replace patterns replaced only the first occurrence in HTTP Toolkit; Akev replaces every occurrence".to_string());
                out.push(MatchReplace {
                    find: regex::escape(s),
                    replace: js_replacement(replacement, 0, notes),
                    regex: true,
                    case_sensitive: true,
                });
            }
            Value::Object(o) => {
                let source = o
                    .get("source")
                    .or_else(|| o.get("regexSource"))
                    .and_then(Value::as_str)
                    .ok_or("invalid regex in match/replace pair")?;
                let flags = o
                    .get("flags")
                    .or_else(|| o.get("regexFlags"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let (re, cs) = translate_js_regex(source, flags)?;
                if !flags.contains('g') {
                    notes.push(format!("/{source}/ replaced only its first match in HTTP Toolkit; Akev replaces every match"));
                }
                let groups = regex::Regex::new(&re)
                    .map(|r| r.captures_len().saturating_sub(1))
                    .unwrap_or(0);
                out.push(MatchReplace {
                    find: re,
                    replace: js_replacement(replacement, groups, notes),
                    regex: true,
                    case_sensitive: cs,
                });
            }
            _ => return Err("invalid match/replace pattern".to_string()),
        }
    }
    Ok(out)
}

/// JS statements applying match/replace pairs to the variable `var_name`.
fn js_pairs(var_name: &str, pairs: &Value) -> Result<String, String> {
    let pairs = pairs
        .as_array()
        .ok_or("invalid match/replace configuration")?;
    let mut out = String::new();
    for pair in pairs {
        let Some([pattern, replacement]) = pair.as_array().map(Vec::as_slice) else {
            return Err("invalid match/replace pair".to_string());
        };
        let rep = js_string(replacement.as_str().unwrap_or(""));
        let pat = match pattern {
            Value::String(s) => js_string(s),
            Value::Object(o) => {
                let source = o
                    .get("source")
                    .or_else(|| o.get("regexSource"))
                    .and_then(Value::as_str)
                    .ok_or("invalid regex in match/replace pair")?;
                let flags = o
                    .get("flags")
                    .or_else(|| o.get("regexFlags"))
                    .and_then(Value::as_str)
                    .unwrap_or("");
                format!("new RegExp({}, {})", js_string(source), js_string(flags))
            }
            _ => return Err("invalid match/replace pattern".to_string()),
        };
        out.push_str(&format!(
            "  {var_name} = {var_name}.replace({pat}, {rep});\n"
        ));
    }
    Ok(out)
}

fn body_transform(
    t: &Map<String, Value>,
    notes: &mut Vec<String>,
) -> Result<Option<BodyTransform>, String> {
    if let Some(Value::String(data)) = t.get("replaceBody") {
        let body = if crate::respond::decode_base64(data).is_ok() {
            BodySource::Base64 { data: data.clone() }
        } else {
            BodySource::Text { text: data.clone() }
        };
        return Ok(Some(BodyTransform::Replace { body }));
    }
    if let Some(v) = t
        .get("replaceBody")
        .filter(|v| v.is_object() || v.is_array())
        && let Some(bytes) = buffer_bytes(v)
    {
        return Ok(Some(BodyTransform::Replace {
            body: BodySource::Base64 {
                data: base64::engine::general_purpose::STANDARD.encode(bytes),
            },
        }));
    }
    if let Some(path) = t.get("replaceBodyFromFile").and_then(Value::as_str) {
        return Ok(Some(BodyTransform::Replace {
            body: BodySource::File {
                path: path.to_string(),
            },
        }));
    }
    if let Some(v) = t.get("updateJsonBody").filter(|v| !v.is_null())
        && let Some(patch) = update_json_body(v, notes)
    {
        return Ok(Some(BodyTransform::JsonMerge { patch }));
    }
    if let Some(ops) = t.get("patchJsonBody").and_then(Value::as_array) {
        crate::json::validate_patch(ops)?;
        return Ok(Some(BodyTransform::JsonPatch {
            operations: ops.clone(),
        }));
    }
    if let Some(pairs) = t.get("matchReplaceBody").filter(|v| !v.is_null()) {
        return Ok(Some(BodyTransform::MatchReplace {
            replacements: convert_pairs(pairs, notes)?,
        }));
    }
    Ok(None)
}

const URL_SCRIPT_PREAMBLE: &str = r#"  var m = /^([A-Za-z][A-Za-z0-9+.\-]*):\/\/([^\/?#]*)([^?#]*)(\?[^#]*)?/.exec(req.url);
  if (!m) return req;
  var defaults = { http: '80', https: '443', ws: '80', wss: '443' };
  var protocol = m[1].toLowerCase();
  var hp = /^(\[[^\]]*\]|[^:]*)(?::(\d*))?$/.exec(m[2].replace(/^.*@/, ''));
  var hostname = hp ? hp[1] : m[2];
  var port = hp && hp[2] ? hp[2] : null;
  var pathname = m[3] || '/';
  var search = m[4] || '';
"#;

const URL_SCRIPT_EPILOGUE: &str = r#"  var host = hostname + (port !== null && port !== '' && port !== defaults[protocol] ? ':' + port : '');
  req.url = protocol + '://' + host + pathname + search;
"#;

fn convert_transforming(
    step: &Value,
    rule_type: &str,
    issues: &mut Issues,
) -> Result<Vec<Part>, String> {
    let empty = Map::new();
    let tr = step
        .get("transformRequest")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let ts = step
        .get("transformResponse")
        .and_then(Value::as_object)
        .unwrap_or(&empty);
    let present = |m: &Map<String, Value>, k: &str| m.get(k).is_some_and(|v| !v.is_null());
    let mut req = RequestTransform::default();
    let mut res = ResponseTransform::default();
    let mut on_request = String::new();
    let mut on_response = String::new();
    let mut forward: Option<(String, bool)> = None;

    let set_protocol = tr
        .get("setProtocol")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase);
    let replace_host = tr.get("replaceHost").and_then(Value::as_object);
    let match_host = tr.get("matchReplaceHost").and_then(Value::as_object);
    let needs_url_script =
        match_host.is_some() || present(tr, "matchReplacePath") || present(tr, "matchReplaceQuery");
    let update_host_header = replace_host
        .and_then(|r| r.get("updateHostHeader"))
        .or_else(|| match_host.and_then(|m| m.get("updateHostHeader")))
        .cloned()
        .unwrap_or(Value::Bool(true));

    // `replaceHeaders` runs first so a Host header derived from a rewritten URL survives it.
    if let Some(h) = tr.get("replaceHeaders").and_then(maybe_stringified) {
        on_request.push_str(&format!("  req.headers = {h};\n"));
    }
    if needs_url_script {
        let mut js = String::from(URL_SCRIPT_PREAMBLE);
        if let Some(p) = &set_protocol {
            js.push_str(&format!(
                "  var wasDefault = port === null || defaults[protocol] === port;\n  protocol = {};\n  if (wasDefault) port = null;\n",
                js_string(p)
            ));
        }
        if let Some(target) = replace_host
            .and_then(|r| r.get("targetHost"))
            .and_then(Value::as_str)
        {
            js.push_str(&format!(
                "  var th = /^(\\[[^\\]]*\\]|[^:]*)(?::(\\d*))?$/.exec({t}) || [null, {t}, null];\n  hostname = th[1]; port = th[2] || null;\n",
                t = js_string(target)
            ));
        }
        if let Some(pairs) = match_host.and_then(|m| m.get("replacements")) {
            js.push_str("  var hostPort = port === null ? hostname : hostname + ':' + port;\n");
            js.push_str(&js_pairs("hostPort", pairs)?);
            js.push_str("  var hostParts = hostPort.split(':');\n  hostname = hostParts[0]; port = hostParts.length > 1 ? hostParts[1] : null;\n");
        }
        if let Some(pairs) = tr.get("matchReplacePath").filter(|v| !v.is_null()) {
            js.push_str(&js_pairs("pathname", pairs)?);
        }
        if let Some(pairs) = tr.get("matchReplaceQuery").filter(|v| !v.is_null()) {
            js.push_str(&js_pairs("search", pairs)?);
        }
        js.push_str(URL_SCRIPT_EPILOGUE);
        match &update_host_header {
            Value::Bool(false) => js.push_str("  req.preserveHost = true;\n"),
            Value::String(h) => js.push_str(&set_header_js("req", "host", &js_string(h))),
            _ => {}
        }
        on_request.push_str(&js);
    } else if let Some(target) = replace_host
        .and_then(|r| r.get("targetHost"))
        .and_then(Value::as_str)
    {
        let scheme = set_protocol.clone().map(|p| match (rule_type, p.as_str()) {
            ("websocket", "http") => "ws".to_string(),
            ("websocket", "https") => "wss".to_string(),
            _ => p,
        });
        let target = match scheme {
            Some(s) => format!("{s}://{target}"),
            None => target.to_string(),
        };
        let preserve = !matches!(update_host_header, Value::Bool(true) | Value::Null);
        if let Value::String(h) = &update_host_header {
            req.set_headers.append("host", h.clone());
        }
        forward = Some((target, preserve));
    } else if let Some(p) = &set_protocol {
        req.url_replace.push(MatchReplace {
            find: "^[A-Za-z][A-Za-z0-9+.-]*:".to_string(),
            replace: format!("{p}:"),
            regex: true,
            case_sensitive: false,
        });
    }

    if let Some(m) = tr.get("replaceMethod").and_then(Value::as_str) {
        req.method = Some(m.to_ascii_uppercase());
    }
    if let Some(v) = tr.get("updateHeaders").filter(|v| !v.is_null()) {
        update_headers(v, &mut req.set_headers, &mut req.remove_headers);
    }
    req.body = body_transform(tr, &mut issues.notes)?;

    if let Some(s) = ts.get("replaceStatus").and_then(Value::as_u64) {
        res.status = Some(
            u16::try_from(s)
                .ok()
                .filter(|s| (100..=999).contains(s))
                .ok_or("invalid replacement status")?,
        );
    }
    if let Some(v) = ts.get("updateHeaders").filter(|v| !v.is_null()) {
        update_headers(v, &mut res.set_headers, &mut res.remove_headers);
    }
    if let Some(h) = ts.get("replaceHeaders").and_then(maybe_stringified) {
        on_response.push_str(&format!("  res.headers = {h};\n"));
    }
    res.body = body_transform(ts, &mut issues.notes)?;

    let mut parts = Vec::new();
    if !on_request.is_empty() || !on_response.is_empty() {
        let mut source = String::from("// Generated from an HTTP Toolkit transform step.\n");
        if !on_request.is_empty() {
            source.push_str(&format!(
                "function onRequest(req) {{\n{on_request}  return req;\n}}\n"
            ));
        }
        if !on_response.is_empty() {
            source.push_str(&format!(
                "function onResponse(req, res) {{\n{on_response}  return res;\n}}\n"
            ));
        }
        parts.push(Part::modifier(
            "script",
            "transform script",
            Action::Script { source },
        ));
    }
    let request = (!req.is_empty()).then_some(req);
    let response = (!res.is_empty()).then_some(res);
    let terminal = match forward {
        Some((target, preserve_host)) => Action::Forward {
            target,
            strip_prefix: None,
            preserve_host,
            request,
            response,
        },
        None => Action::Passthrough { request, response },
    };
    parts.push(Part::terminal(terminal));
    Ok(parts)
}

fn set_header_js(obj: &str, name: &str, value_js: &str) -> String {
    format!(
        "  var hk = Object.keys({obj}.headers).filter(function (n) {{ return n.toLowerCase() === {n}; }})[0] || {n};\n  {obj}.headers[hk] = {value_js};\n",
        n = js_string(name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn js_replacement_translation() {
        let mut notes = Vec::new();
        assert_eq!(js_replacement("$1-$2", 2, &mut notes), "${1}-${2}");
        assert_eq!(js_replacement("$10", 1, &mut notes), "${1}0");
        assert_eq!(js_replacement("$10", 10, &mut notes), "${10}");
        assert_eq!(js_replacement("$&!", 0, &mut notes), "${0}!");
        assert_eq!(js_replacement("$$", 0, &mut notes), "$$");
        assert_eq!(js_replacement("$<year>", 1, &mut notes), "${year}");
        assert_eq!(js_replacement("cost: $5", 0, &mut notes), "cost: $$5");
        assert_eq!(js_replacement("end$", 0, &mut notes), "end$$");
        assert!(notes.is_empty());
        assert_eq!(js_replacement("$'", 0, &mut notes), "$$'");
        assert_eq!(notes.len(), 1);
    }

    #[test]
    fn js_regex_translation() {
        assert_eq!(
            translate_js_regex(r"^https?:\/\/a\.b\/?$", "").unwrap(),
            (r"^https?://a\.b/?$".to_string(), true)
        );
        assert_eq!(
            translate_js_regex(r"a\\/b", "i").unwrap(),
            (r"a\\/b".to_string(), false)
        );
        assert_eq!(translate_js_regex("a.b", "gs").unwrap().0, "(?s)a.b");
        assert!(translate_js_regex("(?<=x)y", "").is_err());
        assert!(translate_js_regex(r"(a)\1", "").is_err());
    }
}
