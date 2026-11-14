//! # akev-rules
//!
//! Evaluates the user's [`akev_core::rules::RuleSet`] against requests and provides the pure
//! helper functions the proxy uses to apply rule outcomes (transforms, mock responses,
//! map-local, forwarding targets, scripts). See `akev_core::rules` for semantics.
//!
//! The engine is lock-free on the hot path: rules are compiled (regexes, globs) into an
//! immutable snapshot swapped atomically on update; match counters are atomics.
//!
//! ## Evaluation
//!
//! Rules are evaluated top-to-bottom, depth-first through groups. Disabled rules, rules inside
//! disabled groups, invalid rules (see [`RulesEngine::errors`]) and rules whose `maxMatches`
//! budget is exhausted are skipped. For each rule whose matchers all match:
//!
//! * modifier actions accumulate into the [`Decision`] and evaluation continues:
//!   `transform` (appended to the transform lists in rule order), `breakpoint` (flags OR-ed),
//!   `delay` (summed, jitter resolved randomly per evaluation), `throttle` (the most
//!   restrictive values: smallest non-zero bandwidths, largest latency), `highlight` (last
//!   wins), `script` (sources collected in order) and `ws-breakpoint` (rule ids collected in
//!   order; such rules only match WebSocket upgrades — see [`ws`]);
//! * the first terminal action stops evaluation; a `passthrough`/`forward` action's own
//!   request/response transforms are appended after the modifiers' transforms;
//! * without a terminal match the decision is an implicit plain passthrough.
//!
//! A rule's `maxMatches` budget is reserved atomically when it matches (concurrent requests
//! can never exceed it). Counters and budgets survive [`RulesEngine::set_rules`] for rule ids
//! that persist, and [`RulesEngine::reset_stats`] clears both (re-arming "once" rules).
//!
//! Body-dependent matchers (`body`, `json-body`, `form`, `graphql-operation`) need the decoded
//! body; see [`RulesEngine::needs_body`]. When `evaluate` gets no body, those matchers don't
//! match — not even when negated.
//!
//! ## Matching semantics
//!
//! * `equals` / `starts-with` / `ends-with` / `contains` compare literally; case-insensitive
//!   unless `caseSensitive` (full Unicode case folding for non-ASCII patterns).
//! * `glob` — anchored shell-style patterns: `*`, `**`, `?`, `[a-z]` / `[!a-z]` classes,
//!   `{a,b}` alternatives and `\x` escapes. In **path** matchers `*` and `?` stay within one
//!   segment and `**` crosses segments (`/api/**` matches `/api`, `/api/` and `/api/a/b`;
//!   `/a/**/b` matches `/a/b`); everywhere else (host, URL, headers, ...) `*` matches any
//!   characters, dots and slashes included (`*.example.com` matches `a.b.example.com` but not
//!   `example.com` — use `{,*.}example.com` for both).
//! * `regex` — Rust `regex` syntax, unanchored (use `^…$`), case-insensitive unless
//!   `caseSensitive`.
//! * `exists` — the value is present (for single-valued fields: present and non-empty).
//! * `negate` inverts the final result. For key/value matchers (headers, query, cookies, form
//!   fields) the negation applies to "the key has a matching value", so a negated matcher also
//!   matches when the key is absent (e.g. `{key: "authorization", value: {op: "exists",
//!   negate: true}}` = "no Authorization header").
//!
//! ### Fields
//!
//! * **Host** is always case-insensitive, ignores a trailing `.`, and accepts IPv6 literals with
//!   or without brackets. An `equals` pattern of the form `name:port` also requires that port.
//! * **Path** excludes the query string. It is tested against the raw path and, when the path
//!   contains `%` escapes, also against its percent-decoded form — either matching counts.
//! * **Url** is the full absolute URL including the query string.
//! * **Query** parameters are form-decoded (`+` → space); a key may repeat, and a value matcher
//!   matches when *any* value of the key matches. Keys are case-sensitive.
//! * **Header** names are case-insensitive; each header line is one value.
//! * **Cookie** reads every `Cookie` header (`name=value; ...`, split at the first `=`). Values
//!   are matched raw and, if they contain `%`, percent-decoded.
//! * **Body** is the decoded body as UTF-8 (lossy).
//! * **JsonBody** parses the body as JSON (invalid JSON never matches); see [`json`] for the
//!   exact vs partial semantics.
//! * **Form** reads `application/x-www-form-urlencoded` and `multipart/form-data` bodies (other
//!   content types never match); multipart part contents are matched as UTF-8 (lossy).
//! * **GraphqlOperation** collects operation names from the JSON body's `operationName` (or the
//!   first named operation in its `query` document; batched arrays are supported), from
//!   `application/graphql` bodies, and from the `operationName` / `query` URL parameters.
//! * **Protocol** is `ws`/`wss` for WebSocket upgrades (`Upgrade: websocket`, or HTTP/2
//!   extended CONNECT with `:protocol: websocket`), otherwise the scheme (`http`/`https`).
//! * **Method**, **Protocol** and **HttpVersion** compare case-insensitively. Empty lists
//!   (methods, ports, protocols, versions) match nothing.
//! * **Source**: an unknown source counts as absent.
//!
//! Transform semantics are documented on [`apply_request_transform`], scripting in [`script`],
//! HTTP Toolkit import (and the Mockttp parity table) in [`htk_import`], rule summaries in
//! [`describe`].

use akev_core::rules::{
    Action, BodySource, RequestTransform, ResponseTransform, RuleSet, RuleStats,
};
use akev_core::{Headers, RequestHead, ResponseHead, RuleId};
use arc_swap::ArcSwap;
use bytes::Bytes;
use parking_lot::Mutex;

pub use engine::LockedActions;
use std::sync::Arc;

pub mod describe;
mod engine;
mod glob;
pub mod htk_import;
pub mod json;
mod matchers;
mod respond;
pub mod script;
#[cfg(feature = "scripting")]
mod supervisor;
mod transform;
mod urlutil;
pub mod ws;

#[cfg(test)]
mod tests;

/// Errors produced by the rules engine and its helpers.
#[derive(Debug, thiserror::Error)]
pub enum RulesError {
    #[error("invalid rule {rule}: {message}")]
    InvalidRule { rule: RuleId, message: String },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transform failed: {0}")]
    Transform(String),
    #[error("script error: {0}")]
    Script(String),
}

pub type Result<T> = std::result::Result<T, RulesError>;

/// Inputs available to matchers.
pub struct MatchContext<'a> {
    pub head: &'a RequestHead,
    /// Decoded request body, when available (see [`RulesEngine::needs_body`]).
    pub body: Option<&'a [u8]>,
    /// Process name / interceptor id of the client, if known.
    pub source: Option<&'a str>,
}

/// Throttling parameters accumulated from rules.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Throttle {
    pub download_bps: u64,
    pub upload_bps: u64,
    pub latency_ms: u64,
}

/// The outcome of evaluating the rule set for one request.
#[derive(Clone, Debug)]
pub struct Decision {
    /// The terminal action to perform (`Passthrough{None,None}` when nothing terminal matched).
    pub action: Action,
    /// Rule providing the terminal action (None = implicit passthrough).
    pub terminal_rule: Option<RuleId>,
    /// All rules that matched, in order (modifiers + terminal).
    pub matched: Vec<RuleId>,
    /// Request transforms to apply in order (modifier rules first, then the terminal action's).
    pub request_transforms: Vec<RequestTransform>,
    /// Response transforms to apply in order.
    pub response_transforms: Vec<ResponseTransform>,
    pub break_on_request: bool,
    pub break_on_response: bool,
    /// Total delay to apply before handling (ms, jitter already resolved).
    pub delay_ms: u64,
    pub throttle: Option<Throttle>,
    /// Highlight colour + note from `highlight` rules (last wins).
    pub highlight: Option<(String, Option<String>)>,
    /// Script sources to run, in order.
    pub scripts: Vec<String>,
    /// Matched `ws-breakpoint` rules (WebSocket upgrades only), in order. The proxy checks every
    /// message of the connection against them with [`RulesEngine::ws_message_breakpoint`].
    pub ws_breakpoints: Vec<RuleId>,
    /// Upstream TLS fingerprint from `tls-profile` rules (last wins); `None` = settings.
    pub tls_profile: Option<akev_core::settings::TlsProfile>,
}

impl Decision {
    /// Default decision: plain passthrough, nothing matched.
    pub fn passthrough() -> Self {
        Decision {
            action: Action::Passthrough {
                request: None,
                response: None,
            },
            terminal_rule: None,
            matched: Vec::new(),
            request_transforms: Vec::new(),
            response_transforms: Vec::new(),
            break_on_request: false,
            break_on_response: false,
            delay_ms: 0,
            throttle: None,
            highlight: None,
            scripts: Vec::new(),
            ws_breakpoints: Vec::new(),
            tls_profile: None,
        }
    }

    /// True if the decision modifies anything at all.
    pub fn is_plain_passthrough(&self) -> bool {
        self.terminal_rule.is_none()
            && self.request_transforms.is_empty()
            && self.response_transforms.is_empty()
            && !self.break_on_request
            && !self.break_on_response
            && self.delay_ms == 0
            && self.throttle.is_none()
            && self.scripts.is_empty()
            && self.ws_breakpoints.is_empty()
            && self.tls_profile.is_none()
    }
}

/// The rules engine: an atomically swappable compiled rule set plus match statistics.
///
/// `RulesEngine` is `Send + Sync`; share it behind an `Arc`. Evaluation never blocks on
/// writers.
pub struct RulesEngine {
    snapshot: ArcSwap<engine::Snapshot>,
    /// Serializes writers so counters are carried over consistently; holds the action types
    /// that may not run (see [`RulesEngine::set_locked_actions`]).
    write_lock: Mutex<LockedActions>,
}

impl RulesEngine {
    /// Compile a rule set. Invalid rules (bad regex etc.) are reported via [`RulesEngine::errors`]
    /// and skipped rather than failing the whole set.
    pub fn new(rules: RuleSet) -> Self {
        Self::with_locked_actions(rules, LockedActions::new())
    }

    /// Like [`RulesEngine::new`], with action types that may not run (see
    /// [`RulesEngine::set_locked_actions`]).
    pub fn with_locked_actions(rules: RuleSet, locked: LockedActions) -> Self {
        RulesEngine {
            snapshot: ArcSwap::from_pointee(engine::Snapshot::compile(rules, None, &locked)),
            write_lock: Mutex::new(locked),
        }
    }

    /// Replace the rule set atomically. Match counters are kept for rules whose id persists.
    pub fn set_rules(&self, rules: RuleSet) {
        let locked = self.write_lock.lock();
        let current = self.snapshot.load_full();
        let next = engine::Snapshot::compile(rules, Some(&current), &locked);
        self.snapshot.store(Arc::new(next));
    }

    /// Set the action types (`Action::type_name`, e.g. `script`) that may not run, each with
    /// the message reported for enabled rules using it, and recompile the current rules.
    /// Such rules stay in the rule set unchanged (still listed, saved and exported); they are
    /// skipped like invalid rules and listed in [`RulesEngine::errors`].
    pub fn set_locked_actions(&self, locked: LockedActions) {
        let mut current_locked = self.write_lock.lock();
        if *current_locked == locked {
            return;
        }
        *current_locked = locked;
        let current = self.snapshot.load_full();
        let next =
            engine::Snapshot::compile(current.rules.clone(), Some(&current), &current_locked);
        self.snapshot.store(Arc::new(next));
    }

    /// The current rule set (as given, including disabled and invalid rules).
    pub fn rules(&self) -> RuleSet {
        self.snapshot.load().rules.clone()
    }

    /// Compilation errors of the current rule set: (rule id, message).
    pub fn errors(&self) -> Vec<(RuleId, String)> {
        self.snapshot.load().errors.clone()
    }

    /// Whether any enabled rule that could match this request (judging by non-body matchers)
    /// requires the request body — the proxy then buffers the body before calling `evaluate`.
    ///
    /// Rules after a terminal rule that certainly matches are not considered (evaluation would
    /// never reach them), unless that rule has a `maxMatches` budget: concurrent requests may
    /// use it up first, and evaluation then continues past it. When `evaluate` is later called
    /// without a body, body-dependent matchers don't match (even when negated).
    ///
    /// This and [`RulesEngine::evaluate`] each use the rule set current at the time of the
    /// call; use a [`RulesEngine::snapshot`] to make both decisions on the same rule set.
    pub fn needs_body(&self, head: &RequestHead, source: Option<&str>) -> bool {
        self.snapshot.load().needs_body(&MatchContext {
            head,
            body: None,
            source,
        })
    }

    /// Evaluate rules for a request. Increments match counters of matched rules and consumes
    /// `maxMatches` budgets.
    pub fn evaluate(&self, ctx: &MatchContext<'_>) -> Decision {
        self.snapshot.load().evaluate(ctx, true)
    }

    /// The current rule set, frozen. A concurrent [`RulesEngine::set_rules`] doesn't affect it,
    /// so [`RulesSnapshot::needs_body`] and [`RulesSnapshot::evaluate`] called on one snapshot
    /// always see the same rules. Match counters and budgets are shared with the engine.
    pub fn snapshot(&self) -> RulesSnapshot {
        RulesSnapshot(self.snapshot.load_full())
    }

    /// Evaluate without side effects (no counters) — used by the UI "test rule" feature.
    /// Rules whose budget is already exhausted are skipped, as `evaluate` would.
    pub fn dry_run(&self, ctx: &MatchContext<'_>) -> Decision {
        self.snapshot.load().evaluate(ctx, false)
    }

    /// Match statistics for every rule of the current set (walk order, including disabled
    /// rules).
    pub fn stats(&self) -> Vec<RuleStats> {
        self.snapshot.load().stats()
    }

    /// Reset match counters and `last_matched_at` (this also restores `maxMatches` budgets).
    pub fn reset_stats(&self) {
        self.snapshot.load().reset_stats();
    }

    /// Whether any of the `ws-breakpoint` rules `rules` (from [`Decision::ws_breakpoints`]) is
    /// still enabled in the current rule set and may pause messages travelling in `direction`.
    /// The proxy only holds back a message for inspection when this is true.
    pub fn ws_may_break(&self, rules: &[RuleId], direction: akev_core::Direction) -> bool {
        let snapshot = self.snapshot.load();
        rules.iter().any(|id| {
            snapshot
                .ws_filter(id)
                .is_some_and(|f| f.applies_to(direction))
        })
    }

    /// The first of the `ws-breakpoint` rules `rules` that is still enabled in the current rule
    /// set and pauses this complete message (see [`ws::WsMessageFilter::matches`]).
    pub fn ws_message_breakpoint(
        &self,
        rules: &[RuleId],
        direction: akev_core::Direction,
        kind: akev_core::MessageKind,
        payload: &[u8],
    ) -> Option<RuleId> {
        let snapshot = self.snapshot.load();
        rules
            .iter()
            .find(|id| {
                snapshot
                    .ws_filter(id)
                    .is_some_and(|f| f.matches(direction, kind, payload))
            })
            .cloned()
    }
}

/// A consistent view of the rule set at one point in time; see [`RulesEngine::snapshot`].
#[derive(Clone)]
pub struct RulesSnapshot(Arc<engine::Snapshot>);

impl RulesSnapshot {
    /// [`RulesEngine::needs_body`] for this rule set.
    pub fn needs_body(&self, head: &RequestHead, source: Option<&str>) -> bool {
        self.0.needs_body(&MatchContext {
            head,
            body: None,
            source,
        })
    }

    /// [`RulesEngine::evaluate`] for this rule set (counters and budgets are the engine's).
    pub fn evaluate(&self, ctx: &MatchContext<'_>) -> Decision {
        self.0.evaluate(ctx, true)
    }

    /// [`RulesEngine::dry_run`] for this rule set.
    pub fn dry_run(&self, ctx: &MatchContext<'_>) -> Decision {
        self.0.evaluate(ctx, false)
    }
}

impl Default for RulesEngine {
    fn default() -> Self {
        RulesEngine::new(RuleSet::default())
    }
}

/// Resolve a body source to bytes (reads files at call time).
///
/// `Text` → UTF-8 bytes, `Base64` → decoded bytes (standard or URL-safe alphabet, padding
/// optional), `Json` → compact serialization, `File` → the file's current contents (a leading
/// `~/` expands to the home directory; this is blocking I/O).
pub fn resolve_body(src: &BodySource) -> Result<Bytes> {
    respond::resolve(src)
}

/// Apply a request transform in place. `body` is the DECODED body; the caller re-encodes and
/// fixes `Content-Length`. Updates `head.url/host/port/scheme/path` consistently when the URL
/// or query changes.
///
/// Order: `method` → `urlReplace` → `setQuery` → `removeHeaders` → `setHeaders` →
/// `addHeaders` → `body`.
///
/// * `urlReplace` find/replace runs on the full absolute URL and replaces **every**
///   occurrence (regex: `$1` / `${name}` expansion; literal: the replacement is inserted
///   verbatim). The result must be an absolute `http(s)://` / `ws(s)://` URL; scheme, host,
///   port and path are re-derived from it **without normalizing the path** (`/a/../b` and `//`
///   survive), and the `Host` (and HTTP/2 `:authority`) header follows a changed authority.
/// * `setQuery` replaces every occurrence of a parameter with one value at the position of the
///   first occurrence (appending when absent); `None` removes it. Other parameters keep their
///   exact bytes; new keys/values are form-urlencoded.
/// * `setHeaders` replaces all values of a header, keeping the first one's position and using
///   the transform's casing; several entries with the same name produce several lines.
///   `addHeaders` appends. Names match case-insensitively.
/// * Body: `replace` (see [`resolve_body`]); `match-replace` works on bytes (binary content
///   outside the matches is preserved) and replaces every occurrence; `json-merge` (RFC 7386)
///   and `json-patch` (RFC 6902, atomic) treat an empty body as `null` and re-serialize
///   compactly, writing every number exactly as it appeared in the body (no rounding of big
///   integers or long decimals; numbers beyond the `f64` range are accepted); invalid JSON
///   (or nesting deeper than 127 levels) is an error.
///
/// Transforms never touch framing headers (`Content-Length`, `Transfer-Encoding`,
/// `Content-Encoding`). On error the body is left unchanged (URL/header edits made before the
/// failing step remain).
pub fn apply_request_transform(
    head: &mut RequestHead,
    body: &mut Bytes,
    t: &RequestTransform,
) -> Result<()> {
    transform::apply_request(head, body, t)
}

/// Apply a response transform in place (`body` decoded).
///
/// Order: `status`/`statusMessage` (a status change without a message resets the reason phrase
/// to the canonical one) → `removeHeaders` → `setHeaders` → `addHeaders` → `body`. The caller
/// fixes framing.
pub fn apply_response_transform(
    head: &mut ResponseHead,
    body: &mut Bytes,
    t: &ResponseTransform,
) -> Result<()> {
    transform::apply_response(head, body, t)
}

/// Whether applying these transforms needs the body (body transforms present).
/// Returns `(request side, response side)`.
pub fn transforms_need_body(req: &[RequestTransform], res: &[ResponseTransform]) -> (bool, bool) {
    (
        req.iter().any(|t| t.body.is_some()),
        res.iter().any(|t| t.body.is_some()),
    )
}

/// Build the response for an `Action::Mock` (sets Content-Type from JSON bodies if absent,
/// Content-Length always correct, Date header).
///
/// Also accepts `Action::WsReject`. The status message defaults to the canonical reason
/// phrase, the HTTP version follows the request, a `File` body gets a Content-Type guessed from
/// its extension when none is set, `HEAD` requests get an empty body (with the full
/// `Content-Length`), and 1xx/204/304 responses never carry a body. When the mock's headers
/// declare `Transfer-Encoding: chunked`, no `Content-Length` is added.
pub fn mock_response(action: &Action, request: &RequestHead) -> Result<(ResponseHead, Bytes)> {
    respond::mock(action, request)
}

/// Files at least this large are streamed from disk by [`map_local`]
/// ([`MapLocalOutcome::FoundFile`]) instead of being read into memory.
pub const MAP_LOCAL_STREAM_MIN: u64 = 1024 * 1024;

/// Outcome of [`map_local`].
#[derive(Debug)]
pub enum MapLocalOutcome {
    /// File found: response head (200, Content-Type by extension, Last-Modified) + bytes.
    Found(ResponseHead, Bytes),
    /// A large file found ([`MAP_LOCAL_STREAM_MIN`] or more): the head (with its
    /// `Content-Length`) and the open file to stream, `len` bytes. `file` is `None` when no
    /// body is sent (`HEAD` requests).
    FoundFile {
        head: ResponseHead,
        file: Option<std::fs::File>,
        len: u64,
    },
    /// Not found and fallback requested.
    FallbackToUpstream,
    /// Not found: 404 response.
    NotFound(ResponseHead, Bytes),
}

/// Resolve an `Action::MapLocal` for a request. Must prevent path traversal outside `path`.
///
/// When `path` is a file it is served for every request. When it's a directory, the request
/// path (query removed, `stripPrefix` removed at a segment boundary when it applies) is
/// percent-decoded segment by segment and resolved inside it; directory requests serve `index`
/// (default `index.html`). Segments that are `..`, contain an (encoded) `/`, `\` or NUL, or that
/// resolve outside the directory (symlinks included — both sides are canonicalized) are
/// rejected with a 404, never forwarded upstream. The action's headers override the generated
/// ones (`Content-Type` from the extension, `Last-Modified`, `Date`, `Content-Length`); a name
/// the action lists several times (e.g. `Set-Cookie`) sends every value, in order.
pub fn map_local(action: &Action, request: &RequestHead) -> Result<MapLocalOutcome> {
    respond::map_local(action, request)
}

/// Compute the rewritten request head for an `Action::Forward` (scheme/host/port/path/url and
/// Host header unless `preserve_host`).
///
/// The target is `scheme://host[:port][/base][?query]`, or `host[:port][/base]` to keep the
/// request's scheme. WebSocket requests forwarded to an `http(s)` target use `ws(s)`. The port
/// defaults to the scheme's. The request path (with `stripPrefix` removed at a segment boundary
/// when it applies) is appended to the target's base path, byte-for-byte; the request query is
/// kept (after the target's own query, if any). The action's transforms are not applied here —
/// they're part of [`Decision::request_transforms`].
pub fn forward_request(action: &Action, request: &RequestHead) -> Result<RequestHead> {
    respond::forward(action, request)
}

/// Helper used by matchers & transforms: does `headers` contain the cookie `name`?
///
/// Returns every cookie from all `Cookie` headers as `(name, value)` pairs in order. Pairs are
/// separated by `;`, split at the first `=` (values may contain `=`), and trimmed; a pair
/// without `=` yields the token as name with an empty value. Values are returned raw (no
/// percent-decoding or quote removal).
pub fn parse_cookies(headers: &Headers) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for value in headers.get_all("cookie") {
        for pair in value.split(';') {
            let pair = pair.trim();
            if pair.is_empty() {
                continue;
            }
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            let name = name.trim();
            if name.is_empty() && value.trim().is_empty() {
                continue;
            }
            out.push((name.to_string(), value.trim().to_string()));
        }
    }
    out
}
