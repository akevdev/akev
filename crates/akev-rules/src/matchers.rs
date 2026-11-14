//! Compiled matchers.
//!
//! Every [`akev_core::rules::Matcher`] is compiled once (regexes, globs, method sets...) into a
//! [`CMatcher`]. Evaluation is three-valued ([`Tri`]): body-dependent matchers report
//! [`Tri::Unknown`] when the body isn't available, which lets
//! [`crate::RulesEngine::needs_body`] decide whether buffering is required, and makes rules
//! *not* match during evaluation when the body they need is missing (even when negated).
//!
//! The user-facing matching semantics are documented in the crate root docs.

use crate::glob::{GlobMode, glob_to_regex};
use crate::json::{json_eq, json_subset};
use akev_core::RequestHead;
use akev_core::http::HttpVersion;
use akev_core::rules::{KeyValueMatch, MatchOp, Matcher, StringMatch};
use percent_encoding::percent_decode_str;
use regex::{Regex, RegexBuilder};
use serde_json::Value;
use std::borrow::Cow;
use std::cell::OnceCell;

/// Three-valued match result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Tri {
    True,
    False,
    /// Depends on a body that isn't available.
    Unknown,
}

impl Tri {
    fn from_bool(b: bool) -> Self {
        if b { Tri::True } else { Tri::False }
    }
    fn not(self) -> Self {
        match self {
            Tri::True => Tri::False,
            Tri::False => Tri::True,
            Tri::Unknown => Tri::Unknown,
        }
    }
}

/// Size limit for compiled regexes (protects against pathological patterns).
const REGEX_SIZE_LIMIT: usize = 4 << 20;

/// Build a regex with Akev's standard limits.
pub(crate) fn build_regex(src: &str, case_insensitive: bool) -> Result<Regex, String> {
    RegexBuilder::new(src)
        .case_insensitive(case_insensitive)
        .size_limit(REGEX_SIZE_LIMIT)
        .build()
        .map_err(|e| format!("invalid regex `{src}`: {e}"))
}

// ------------------------------------------------------------------------------------------
// Patterns (compiled StringMatch)
// ------------------------------------------------------------------------------------------

#[derive(Debug)]
enum PatKind {
    Exact(String),
    ExactAscii(String),
    Prefix(String),
    PrefixAscii(String),
    Suffix(String),
    SuffixAscii(String),
    Contains(String),
    /// Regex-free glob made of literal parts separated by `*` (any characters). With `ci`,
    /// every part is ASCII and compared case-insensitively.
    Wildcard {
        parts: Vec<String>,
        ci: bool,
    },
    /// Path glob `prefix/**`: the prefix itself or anything below it.
    PathTree {
        prefix: String,
        ci: bool,
    },
    Regex(Regex),
    Exists,
}

/// Fast-path compilation for the common simple globs (`*.example.com`, `/api/**`,
/// `https://*/v1/*`); `None` = needs the regex translation.
fn simple_glob(glob: &str, mode: GlobMode, ci: bool) -> Option<PatKind> {
    if (ci && !glob.is_ascii()) || glob.contains(['?', '[', ']', '{', '}', '\\']) {
        return None;
    }
    match mode {
        GlobMode::Any => Some(PatKind::Wildcard {
            parts: glob.split('*').map(str::to_string).collect(),
            ci,
        }),
        GlobMode::Path if !glob.contains('*') => Some(PatKind::Wildcard {
            parts: vec![glob.to_string()],
            ci,
        }),
        GlobMode::Path => glob
            .strip_suffix("/**")
            .filter(|prefix| !prefix.contains('*'))
            .map(|prefix| PatKind::PathTree {
                prefix: prefix.to_string(),
                ci,
            }),
    }
}

fn bytes_eq(a: &[u8], b: &[u8], ci: bool) -> bool {
    if ci {
        a.eq_ignore_ascii_case(b)
    } else {
        a == b
    }
}

fn find_bytes(hay: &[u8], needle: &[u8], ci: bool) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    hay.windows(needle.len())
        .position(|w| bytes_eq(w, needle, ci))
}

/// Match `*`-separated literal parts against the whole of `v`.
fn wildcard_match(v: &str, parts: &[String], ci: bool) -> bool {
    let v = v.as_bytes();
    match parts {
        [] => v.is_empty(),
        [only] => bytes_eq(v, only.as_bytes(), ci),
        [first, middle @ .., last] => {
            let (first, last) = (first.as_bytes(), last.as_bytes());
            if v.len() < first.len() + last.len()
                || !bytes_eq(&v[..first.len()], first, ci)
                || !bytes_eq(&v[v.len() - last.len()..], last, ci)
            {
                return false;
            }
            let mut pos = first.len();
            let end = v.len() - last.len();
            for part in middle
                .iter()
                .map(String::as_bytes)
                .filter(|p| !p.is_empty())
            {
                match find_bytes(&v[pos..end], part, ci) {
                    Some(i) => pos += i + part.len(),
                    None => return false,
                }
            }
            true
        }
    }
}

/// A compiled [`StringMatch`].
#[derive(Debug)]
pub(crate) struct Pattern {
    kind: PatKind,
    negate: bool,
}

/// Field-specific compilation options.
#[derive(Clone, Copy)]
pub(crate) struct PatOpts {
    pub glob: GlobMode,
    /// Force case-insensitive comparison (hosts).
    pub force_ci: bool,
}

impl PatOpts {
    pub const TEXT: PatOpts = PatOpts {
        glob: GlobMode::Any,
        force_ci: false,
    };
    pub const PATH: PatOpts = PatOpts {
        glob: GlobMode::Path,
        force_ci: false,
    };
    pub const HOST: PatOpts = PatOpts {
        glob: GlobMode::Any,
        force_ci: true,
    };
}

impl Pattern {
    pub(crate) fn compile(sm: &StringMatch, opts: PatOpts) -> Result<Self, String> {
        Self::compile_value(
            sm.op,
            &sm.value,
            sm.case_sensitive && !opts.force_ci,
            sm.negate,
            opts,
        )
    }

    fn compile_value(
        op: MatchOp,
        value: &str,
        case_sensitive: bool,
        negate: bool,
        opts: PatOpts,
    ) -> Result<Self, String> {
        let ci = !case_sensitive;
        let ascii = value.is_ascii();
        let literal_regex = |prefix: &str, suffix: &str| -> Result<PatKind, String> {
            build_regex(&format!("{prefix}{}{suffix}", regex::escape(value)), true)
                .map(PatKind::Regex)
        };
        let kind = match op {
            MatchOp::Equals if !ci => PatKind::Exact(value.to_string()),
            MatchOp::Equals if ascii => PatKind::ExactAscii(value.to_string()),
            MatchOp::Equals => literal_regex("^", "$")?,
            MatchOp::StartsWith if !ci => PatKind::Prefix(value.to_string()),
            MatchOp::StartsWith if ascii => PatKind::PrefixAscii(value.to_string()),
            MatchOp::StartsWith => literal_regex("^", "")?,
            MatchOp::EndsWith if !ci => PatKind::Suffix(value.to_string()),
            MatchOp::EndsWith if ascii => PatKind::SuffixAscii(value.to_string()),
            MatchOp::EndsWith => literal_regex("", "$")?,
            MatchOp::Contains if !ci || value.is_empty() => PatKind::Contains(value.to_string()),
            MatchOp::Contains => literal_regex("", "")?,
            MatchOp::Glob => match simple_glob(value, opts.glob, ci) {
                Some(kind) => kind,
                None => {
                    let src = glob_to_regex(value, opts.glob)
                        .map_err(|e| format!("invalid glob `{value}`: {e}"))?;
                    PatKind::Regex(build_regex(&src, ci)?)
                }
            },
            MatchOp::Regex => PatKind::Regex(build_regex(value, ci)?),
            MatchOp::Exists => PatKind::Exists,
        };
        Ok(Pattern { kind, negate })
    }

    /// Raw test (without negation) against a present value.
    fn test(&self, v: &str) -> bool {
        match &self.kind {
            PatKind::Exact(p) => v == p,
            PatKind::ExactAscii(p) => v.eq_ignore_ascii_case(p),
            PatKind::Prefix(p) => v.starts_with(p.as_str()),
            PatKind::PrefixAscii(p) => {
                v.len() >= p.len() && v.as_bytes()[..p.len()].eq_ignore_ascii_case(p.as_bytes())
            }
            PatKind::Suffix(p) => v.ends_with(p.as_str()),
            PatKind::SuffixAscii(p) => {
                v.len() >= p.len()
                    && v.as_bytes()[v.len() - p.len()..].eq_ignore_ascii_case(p.as_bytes())
            }
            PatKind::Contains(p) => v.contains(p.as_str()),
            PatKind::Wildcard { parts, ci } => wildcard_match(v, parts, *ci),
            PatKind::PathTree { prefix, ci } => {
                let (v, p) = (v.as_bytes(), prefix.as_bytes());
                v.len() >= p.len()
                    && bytes_eq(&v[..p.len()], p, *ci)
                    && (v.len() == p.len() || v[p.len()] == b'/')
            }
            PatKind::Regex(re) => re.is_match(v),
            PatKind::Exists => true,
        }
    }

    pub(crate) fn is_exists(&self) -> bool {
        matches!(self.kind, PatKind::Exists)
    }

    /// Evaluate against a single-valued field (`None` = absent; for `exists`, empty also
    /// counts as absent).
    pub(crate) fn eval(&self, v: Option<&str>) -> bool {
        let base = match v {
            Some(v) if self.is_exists() => !v.is_empty(),
            Some(v) => self.test(v),
            None => false,
        };
        base ^ self.negate
    }

    /// Evaluate against several candidate renderings of one value (e.g. raw + decoded path).
    fn eval_candidates<'s>(&self, mut candidates: impl Iterator<Item = &'s str>) -> bool {
        let base = if self.is_exists() {
            candidates.any(|c| !c.is_empty())
        } else {
            candidates.any(|c| self.test(c))
        };
        base ^ self.negate
    }

    /// Evaluate against the values of a multi-valued key: base = "any value matches" (for
    /// `exists`: "the key is present"). With `decode`, values are also tried percent-decoded.
    fn eval_values<'s>(&self, mut values: impl Iterator<Item = &'s str>, decode: bool) -> bool {
        let base = if self.is_exists() {
            values.next().is_some()
        } else if decode {
            values.any(|v| self.test_decoded(v))
        } else {
            values.any(|v| self.test(v))
        };
        base ^ self.negate
    }

    /// Test a value raw and — when it has `%` escapes — percent-decoded.
    fn test_decoded(&self, v: &str) -> bool {
        self.test(v) || (v.contains('%') && self.test(&percent_decode_str(v).decode_utf8_lossy()))
    }
}

// ------------------------------------------------------------------------------------------
// Compiled matchers
// ------------------------------------------------------------------------------------------

/// A compiled key/value constraint.
#[derive(Debug)]
pub(crate) struct CKv {
    key: String,
    value: Option<Pattern>,
}

impl CKv {
    fn compile(kv: &KeyValueMatch, opts: PatOpts) -> Result<Self, String> {
        let value = kv
            .value
            .as_ref()
            .map(|v| Pattern::compile(v, opts))
            .transpose()?;
        Ok(CKv {
            key: kv.key.clone(),
            value,
        })
    }

    /// Evaluate against the values of this key (empty iterator = key absent).
    fn eval<'s>(&self, mut values: impl Iterator<Item = &'s str>, decode: bool) -> bool {
        match &self.value {
            None => values.next().is_some(),
            Some(p) => p.eval_values(values, decode),
        }
    }
}

/// A compiled matcher.
#[derive(Debug)]
pub(crate) enum CMatcher {
    Method(Vec<String>),
    Url(Pattern),
    /// `pattern` is compiled without negation: `negate` applies to "host matches and (when
    /// the pattern named a port) the port matches" as a whole.
    Host {
        pattern: Pattern,
        port: Option<u16>,
        negate: bool,
    },
    Port(Vec<u16>),
    Path(Pattern),
    Protocol(Vec<String>),
    Query(Vec<CKv>),
    Header(Vec<CKv>),
    Cookie(Vec<CKv>),
    Body(Pattern),
    JsonBody {
        value: Value,
        partial: bool,
    },
    Form(Vec<CKv>),
    Graphql(Pattern),
    Source(Pattern),
    Version(Vec<HttpVersion>),
    Not(Box<CMatcher>),
    Any(Vec<CMatcher>),
    All(Vec<CMatcher>),
}

/// Parse a user-supplied HTTP version label.
pub(crate) fn parse_http_version(s: &str) -> Option<HttpVersion> {
    let s = s.trim().to_ascii_lowercase();
    let s = s.strip_prefix("http/").unwrap_or(&s);
    Some(match s {
        "0.9" => HttpVersion::Http09,
        "1.0" | "1" => HttpVersion::Http10,
        "1.1" | "http1.1" => HttpVersion::Http11,
        "2" | "2.0" | "h2" | "h2c" => HttpVersion::H2,
        "3" | "3.0" | "h3" => HttpVersion::H3,
        _ => return None,
    })
}

impl CMatcher {
    pub(crate) fn compile(m: &Matcher) -> Result<Self, String> {
        Ok(match m {
            Matcher::Method { methods } => CMatcher::Method(
                methods
                    .iter()
                    .map(|s| s.trim().to_ascii_uppercase())
                    .collect(),
            ),
            Matcher::Url { pattern } => CMatcher::Url(Pattern::compile(pattern, PatOpts::TEXT)?),
            Matcher::Host { pattern } => compile_host(pattern)?,
            Matcher::Port { ports } => CMatcher::Port(ports.clone()),
            Matcher::Path { pattern } => CMatcher::Path(Pattern::compile(pattern, PatOpts::PATH)?),
            Matcher::Protocol { protocols } => {
                let mut out = Vec::with_capacity(protocols.len());
                for p in protocols {
                    let p = p.trim().to_ascii_lowercase();
                    if !matches!(p.as_str(), "http" | "https" | "ws" | "wss") {
                        return Err(format!(
                            "invalid protocol `{p}` (expected http, https, ws or wss)"
                        ));
                    }
                    out.push(p);
                }
                CMatcher::Protocol(out)
            }
            Matcher::Query { params } => CMatcher::Query(compile_kvs(params, PatOpts::TEXT)?),
            Matcher::Header { headers } => CMatcher::Header(compile_kvs(headers, PatOpts::TEXT)?),
            Matcher::Cookie { cookies } => CMatcher::Cookie(compile_kvs(cookies, PatOpts::TEXT)?),
            Matcher::Body { pattern } => CMatcher::Body(Pattern::compile(pattern, PatOpts::TEXT)?),
            Matcher::JsonBody { value, partial } => CMatcher::JsonBody {
                value: value.clone(),
                partial: *partial,
            },
            Matcher::Form { fields } => CMatcher::Form(compile_kvs(fields, PatOpts::TEXT)?),
            Matcher::GraphqlOperation { pattern } => {
                CMatcher::Graphql(Pattern::compile(pattern, PatOpts::TEXT)?)
            }
            Matcher::Source { pattern } => {
                CMatcher::Source(Pattern::compile(pattern, PatOpts::TEXT)?)
            }
            Matcher::HttpVersion { versions } => {
                let mut out = Vec::with_capacity(versions.len());
                for v in versions {
                    out.push(
                        parse_http_version(v)
                            .ok_or_else(|| format!("unknown HTTP version `{v}`"))?,
                    );
                }
                CMatcher::Version(out)
            }
            Matcher::Not { matcher } => CMatcher::Not(Box::new(CMatcher::compile(matcher)?)),
            Matcher::Any { matchers } => CMatcher::Any(compile_sorted(matchers)?),
            Matcher::All { matchers } => CMatcher::All(compile_sorted(matchers)?),
        })
    }

    /// Relative evaluation cost, used to evaluate cheap matchers first.
    pub(crate) fn cost(&self) -> u8 {
        match self {
            CMatcher::Method(_)
            | CMatcher::Port(_)
            | CMatcher::Protocol(_)
            | CMatcher::Version(_) => 0,
            CMatcher::Source(_) | CMatcher::Host { .. } => 1,
            CMatcher::Url(_) | CMatcher::Path(_) => 2,
            CMatcher::Header(_) | CMatcher::Query(_) | CMatcher::Cookie(_) => 3,
            CMatcher::Graphql(_)
            | CMatcher::Body(_)
            | CMatcher::Form(_)
            | CMatcher::JsonBody { .. } => 5,
            CMatcher::Not(m) => m.cost(),
            CMatcher::Any(ms) | CMatcher::All(ms) => {
                ms.iter().map(CMatcher::cost).max().unwrap_or(0)
            }
        }
    }

    /// Whether this matcher reads the request body.
    pub(crate) fn needs_body(&self) -> bool {
        match self {
            CMatcher::Body(_)
            | CMatcher::JsonBody { .. }
            | CMatcher::Form(_)
            | CMatcher::Graphql(_) => true,
            CMatcher::Not(m) => m.needs_body(),
            CMatcher::Any(ms) | CMatcher::All(ms) => ms.iter().any(CMatcher::needs_body),
            _ => false,
        }
    }

    pub(crate) fn eval(&self, r: &ReqView<'_>) -> Tri {
        match self {
            CMatcher::Method(methods) => Tri::from_bool(
                methods
                    .iter()
                    .any(|m| m.eq_ignore_ascii_case(&r.head.method)),
            ),
            CMatcher::Url(p) => Tri::from_bool(p.eval(Some(&r.head.url))),
            CMatcher::Host {
                pattern,
                port,
                negate,
            } => {
                let host = r.head.host.strip_suffix('.').unwrap_or(&r.head.host);
                let matched = pattern.eval(Some(host)) && port.is_none_or(|p| p == r.head.port);
                Tri::from_bool(matched ^ negate)
            }
            CMatcher::Port(ports) => Tri::from_bool(ports.contains(&r.head.port)),
            CMatcher::Path(p) => {
                let raw = r.head.path_only();
                let decoded = r.decoded_path();
                Tri::from_bool(p.eval_candidates(std::iter::once(raw).chain(decoded)))
            }
            CMatcher::Protocol(protos) => {
                let proto = r.protocol();
                Tri::from_bool(protos.iter().any(|p| p == proto))
            }
            CMatcher::Query(kvs) => {
                let params = r.query();
                Tri::from_bool(kvs.iter().all(|kv| {
                    kv.eval(
                        params
                            .iter()
                            .filter(|(k, _)| k.as_ref() == kv.key)
                            .map(|(_, v)| v.as_ref()),
                        false,
                    )
                }))
            }
            CMatcher::Header(kvs) => Tri::from_bool(
                kvs.iter()
                    .all(|kv| kv.eval(r.head.headers.get_all(&kv.key), false)),
            ),
            CMatcher::Cookie(kvs) => {
                let cookies = r.cookies();
                Tri::from_bool(kvs.iter().all(|kv| {
                    kv.eval(
                        cookies
                            .iter()
                            .filter(|(k, _)| *k == kv.key)
                            .map(|(_, v)| v.as_str()),
                        true,
                    )
                }))
            }
            CMatcher::Body(p) => match r.body_text() {
                Some(text) => Tri::from_bool(p.eval(Some(text))),
                None => Tri::Unknown,
            },
            CMatcher::JsonBody { value, partial } => match r.body {
                None => Tri::Unknown,
                Some(_) => Tri::from_bool(r.json().is_some_and(|actual| {
                    if *partial {
                        json_subset(value, actual)
                    } else {
                        json_eq(value, actual)
                    }
                })),
            },
            CMatcher::Form(kvs) => match r.body {
                None => Tri::Unknown,
                Some(_) => Tri::from_bool(r.form().is_some_and(|fields| {
                    kvs.iter().all(|kv| {
                        kv.eval(
                            fields
                                .iter()
                                .filter(|(k, _)| *k == kv.key)
                                .map(|(_, v)| v.as_str()),
                            false,
                        )
                    })
                })),
            },
            CMatcher::Graphql(p) => {
                let from_query = r.graphql_from_query();
                match r.graphql_from_body() {
                    Some(from_body) => Tri::from_bool(p.eval_values(
                        from_query.iter().chain(from_body).map(String::as_str),
                        false,
                    )),
                    // The URL alone identifies the operation (typical GraphQL GET requests).
                    None if !from_query.is_empty() => {
                        Tri::from_bool(p.eval_values(from_query.iter().map(String::as_str), false))
                    }
                    None => Tri::Unknown,
                }
            }
            CMatcher::Source(p) => Tri::from_bool(p.eval(r.source)),
            CMatcher::Version(vs) => Tri::from_bool(vs.contains(&r.head.http_version)),
            CMatcher::Not(m) => m.eval(r).not(),
            CMatcher::Any(ms) => {
                let mut unknown = false;
                for m in ms {
                    match m.eval(r) {
                        Tri::True => return Tri::True,
                        Tri::Unknown => unknown = true,
                        Tri::False => {}
                    }
                }
                if unknown { Tri::Unknown } else { Tri::False }
            }
            CMatcher::All(ms) => eval_all(ms, r),
        }
    }
}

/// AND over matchers with short-circuiting on `False`.
pub(crate) fn eval_all(ms: &[CMatcher], r: &ReqView<'_>) -> Tri {
    let mut unknown = false;
    for m in ms {
        match m.eval(r) {
            Tri::False => return Tri::False,
            Tri::Unknown => unknown = true,
            Tri::True => {}
        }
    }
    if unknown { Tri::Unknown } else { Tri::True }
}

fn compile_kvs(kvs: &[KeyValueMatch], opts: PatOpts) -> Result<Vec<CKv>, String> {
    kvs.iter().map(|kv| CKv::compile(kv, opts)).collect()
}

/// Compile and order matchers cheapest-first (AND/OR are commutative for pure matchers).
pub(crate) fn compile_sorted(ms: &[Matcher]) -> Result<Vec<CMatcher>, String> {
    let mut out = ms
        .iter()
        .map(CMatcher::compile)
        .collect::<Result<Vec<_>, _>>()?;
    out.sort_by_key(CMatcher::cost);
    Ok(out)
}

fn compile_host(sm: &StringMatch) -> Result<CMatcher, String> {
    let mut value = sm.value.trim().to_string();
    let mut port = None;
    if sm.op != MatchOp::Regex {
        // `[::1]` → `::1` (hosts are stored without brackets).
        if let Some(inner) = value.strip_prefix('[') {
            if let Some((ip, rest)) = inner.split_once(']')
                && ip.contains(':')
            {
                if sm.op == MatchOp::Equals
                    && let Some(p) = rest.strip_prefix(':').and_then(|p| p.parse::<u16>().ok())
                {
                    port = Some(p);
                }
                value = ip.to_string();
            }
        } else if sm.op == MatchOp::Equals {
            // Lenient `host:port`.
            if let Some((h, p)) = value.rsplit_once(':')
                && !h.contains(':')
                && let Ok(p) = p.parse::<u16>()
            {
                port = Some(p);
                value = h.to_string();
            }
        }
        if let Some(v) = value.strip_suffix('.') {
            value = v.to_string();
        }
    }
    let pattern = StringMatch {
        op: sm.op,
        value,
        case_sensitive: false,
        negate: false,
    };
    Ok(CMatcher::Host {
        pattern: Pattern::compile(&pattern, PatOpts::HOST)?,
        port,
        negate: sm.negate,
    })
}

// ------------------------------------------------------------------------------------------
// Per-request lazily computed view
// ------------------------------------------------------------------------------------------

/// Request data shared by all matchers during one evaluation, with lazily parsed parts.
pub(crate) struct ReqView<'a> {
    pub head: &'a RequestHead,
    pub body: Option<&'a [u8]>,
    pub source: Option<&'a str>,
    decoded_path: OnceCell<Option<String>>,
    protocol: OnceCell<&'static str>,
    query: OnceCell<Vec<(Cow<'a, str>, Cow<'a, str>)>>,
    cookies: OnceCell<Vec<(String, String)>>,
    body_text: OnceCell<Option<Cow<'a, str>>>,
    json: OnceCell<Option<Value>>,
    form: OnceCell<Option<Vec<(String, String)>>>,
    graphql_query: OnceCell<Vec<String>>,
    graphql_body: OnceCell<Option<Vec<String>>>,
}

impl<'a> ReqView<'a> {
    pub(crate) fn new(
        head: &'a RequestHead,
        body: Option<&'a [u8]>,
        source: Option<&'a str>,
    ) -> Self {
        ReqView {
            head,
            body,
            source,
            decoded_path: OnceCell::new(),
            protocol: OnceCell::new(),
            query: OnceCell::new(),
            cookies: OnceCell::new(),
            body_text: OnceCell::new(),
            json: OnceCell::new(),
            form: OnceCell::new(),
            graphql_query: OnceCell::new(),
            graphql_body: OnceCell::new(),
        }
    }

    fn decoded_path(&self) -> Option<&str> {
        self.decoded_path
            .get_or_init(|| {
                let raw = self.head.path_only();
                raw.contains('%')
                    .then(|| percent_decode_str(raw).decode_utf8_lossy().into_owned())
            })
            .as_deref()
    }

    pub(crate) fn protocol(&self) -> &'static str {
        self.protocol.get_or_init(|| effective_protocol(self.head))
    }

    fn query(&self) -> &[(Cow<'a, str>, Cow<'a, str>)] {
        self.query.get_or_init(|| {
            let head: &'a RequestHead = self.head;
            match head.query() {
                Some(q) => url::form_urlencoded::parse(q.as_bytes()).collect(),
                None => Vec::new(),
            }
        })
    }

    fn cookies(&self) -> &[(String, String)] {
        self.cookies
            .get_or_init(|| crate::parse_cookies(&self.head.headers))
    }

    fn body_text(&self) -> Option<&str> {
        self.body_text
            .get_or_init(|| self.body.map(String::from_utf8_lossy))
            .as_deref()
    }

    fn json(&self) -> Option<&Value> {
        self.json
            .get_or_init(|| self.body.and_then(|b| serde_json::from_slice(b).ok()))
            .as_ref()
    }

    fn form(&self) -> Option<&[(String, String)]> {
        self.form
            .get_or_init(|| {
                let body = self.body?;
                let ct = self.head.headers.get("content-type")?;
                let essence = self.head.headers.content_type_essence()?;
                match essence.as_str() {
                    "application/x-www-form-urlencoded" => Some(
                        akev_content::form::parse_urlencoded(&String::from_utf8_lossy(body)),
                    ),
                    "multipart/form-data" => {
                        akev_content::multipart::parse(body, ct).ok().map(|parts| {
                            parts
                                .into_iter()
                                .filter_map(|p| {
                                    let name = p.name?;
                                    let value = match p.text {
                                        Some(t) => t,
                                        None => String::from_utf8_lossy(&p.data).into_owned(),
                                    };
                                    Some((name, value))
                                })
                                .collect()
                        })
                    }
                    _ => None,
                }
            })
            .as_deref()
    }

    fn graphql_from_query(&self) -> &[String] {
        self.graphql_query.get_or_init(|| {
            let params = self.query();
            let mut out = Vec::new();
            if let Some((_, name)) = params
                .iter()
                .find(|(k, v)| k == "operationName" && !v.is_empty())
            {
                out.push(name.to_string());
            } else if let Some((_, q)) = params.iter().find(|(k, _)| k == "query")
                && let Some(name) = first_operation_name(q)
            {
                out.push(name);
            }
            out
        })
    }

    /// Operation names found in the body; `None` when the body is unavailable.
    fn graphql_from_body(&self) -> Option<&[String]> {
        self.graphql_body
            .get_or_init(|| {
                let body = self.body?;
                let mut out = Vec::new();
                if self.head.headers.content_type_essence().as_deref()
                    == Some("application/graphql")
                {
                    if let Some(name) = first_operation_name(&String::from_utf8_lossy(body)) {
                        out.push(name);
                    }
                    return Some(out);
                }
                match self.json() {
                    Some(Value::Array(ops)) => {
                        ops.iter().for_each(|op| collect_gql_name(op, &mut out))
                    }
                    Some(op) => collect_gql_name(op, &mut out),
                    None => {}
                }
                Some(out)
            })
            .as_deref()
    }
}

fn collect_gql_name(op: &Value, out: &mut Vec<String>) {
    let Some(obj) = op.as_object() else { return };
    if let Some(name) = obj
        .get("operationName")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
    {
        out.push(name.to_string());
    } else if let Some(name) = obj
        .get("query")
        .and_then(Value::as_str)
        .and_then(first_operation_name)
    {
        out.push(name);
    }
}

/// `ws`/`wss` for WebSocket upgrades, otherwise `http`/`https`.
pub(crate) fn effective_protocol(head: &RequestHead) -> &'static str {
    let upgrade = head.headers.get_all("upgrade").any(|v| {
        v.split(',')
            .any(|t| t.trim().eq_ignore_ascii_case("websocket"))
    }) || (head.method.eq_ignore_ascii_case("CONNECT")
        && head
            .headers
            .get(":protocol")
            .is_some_and(|p| p.eq_ignore_ascii_case("websocket")));
    let tls = head.scheme.eq_ignore_ascii_case("https") || head.scheme.eq_ignore_ascii_case("wss");
    let ws_scheme =
        head.scheme.eq_ignore_ascii_case("ws") || head.scheme.eq_ignore_ascii_case("wss");
    match (tls, upgrade || ws_scheme) {
        (true, true) => "wss",
        (false, true) => "ws",
        (true, false) => "https",
        (false, false) => "http",
    }
}

/// Name of the first *named* operation (`query|mutation|subscription Name`) at the top level of
/// a GraphQL document. Skips comments, strings and nested selections.
pub(crate) fn first_operation_name(doc: &str) -> Option<String> {
    let b = doc.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut expect_name = false;
    while i < b.len() {
        let c = b[i];
        match c {
            b'#' => {
                while i < b.len() && b[i] != b'\n' {
                    i += 1;
                }
            }
            b'"' => {
                if b[i..].starts_with(b"\"\"\"") {
                    i += 3;
                    while i < b.len() && !b[i..].starts_with(b"\"\"\"") {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 3;
                } else {
                    i += 1;
                    while i < b.len() && b[i] != b'"' && b[i] != b'\n' {
                        i += if b[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                expect_name = false;
                continue;
            }
            b'{' | b'(' | b'[' => {
                depth += 1;
                expect_name = false;
            }
            b'}' | b')' | b']' => {
                depth = depth.saturating_sub(1);
                expect_name = false;
            }
            c if c == b'_' || c.is_ascii_alphabetic() => {
                let start = i;
                while i < b.len() && (b[i] == b'_' || b[i].is_ascii_alphanumeric()) {
                    i += 1;
                }
                let word = &doc[start..i];
                if depth == 0 {
                    if expect_name {
                        return Some(word.to_string());
                    }
                    expect_name = matches!(word, "query" | "mutation" | "subscription");
                }
                continue;
            }
            c if c.is_ascii_whitespace() || c == b',' => {}
            _ => expect_name = false,
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_operation_name_parsing() {
        assert_eq!(
            first_operation_name("query GetUser { user { id } }").as_deref(),
            Some("GetUser")
        );
        assert_eq!(
            first_operation_name("mutation   AddUser($x: Int) { add }").as_deref(),
            Some("AddUser")
        );
        assert_eq!(
            first_operation_name("subscription OnEvent{e}").as_deref(),
            Some("OnEvent")
        );
        assert_eq!(
            first_operation_name("# query Commented\nfragment F on T { query X } query Real { a }")
                .as_deref(),
            Some("Real")
        );
        assert_eq!(first_operation_name("{ user { id } }"), None);
        assert_eq!(first_operation_name("query { user }"), None);
        assert_eq!(first_operation_name("query ($id: ID) { a }"), None);
        assert_eq!(
            first_operation_name(r#"query A { f(arg: "query B {") }"#).as_deref(),
            Some("A")
        );
        assert_eq!(
            first_operation_name(r#"{ f(a: """ query X """) } query Y { z }"#).as_deref(),
            Some("Y")
        );
        assert_eq!(first_operation_name(""), None);
    }

    #[test]
    fn simple_globs_agree_with_regex_translation() {
        let cases: &[(&str, GlobMode, &[&str])] = &[
            (
                "*.example.com",
                GlobMode::Any,
                &[
                    "a.example.com",
                    "example.com",
                    "A.B.EXAMPLE.COM",
                    ".example.com",
                    "example.com.evil",
                ],
            ),
            ("*", GlobMode::Any, &["", "anything/at/all"]),
            (
                "a*b*c",
                GlobMode::Any,
                &["abc", "aXbYc", "acb", "abcbc", "ab", "a*b*c"],
            ),
            (
                "https://*/v1/*",
                GlobMode::Any,
                &["https://h/v1/x", "https://h/v2/x", "https://h/v1/"],
            ),
            ("**", GlobMode::Any, &["", "x"]),
            ("ab**cd", GlobMode::Any, &["abcd", "abXcd", "abc"]),
            (
                "/api/**",
                GlobMode::Path,
                &["/api", "/api/", "/api/a/b", "/apix", "/API/x", "/ap"],
            ),
            ("/**", GlobMode::Path, &["", "/", "/a/b", "a"]),
            (
                "/exact/path",
                GlobMode::Path,
                &["/exact/path", "/EXACT/PATH", "/exact/path/"],
            ),
        ];
        for (glob, mode, inputs) in cases {
            for ci in [false, true] {
                let fast = simple_glob(glob, *mode, ci).expect("fast path");
                let fast = Pattern {
                    kind: fast,
                    negate: false,
                };
                let re = build_regex(&glob_to_regex(glob, *mode).unwrap(), ci).unwrap();
                for input in *inputs {
                    assert_eq!(
                        fast.test(input),
                        re.is_match(input),
                        "glob {glob:?} ci={ci} input {input:?}"
                    );
                }
            }
        }
        assert!(simple_glob("/api/*", GlobMode::Path, false).is_none());
        assert!(simple_glob("file?.txt", GlobMode::Any, false).is_none());
        assert!(simple_glob("\u{e9}*", GlobMode::Any, true).is_none());
        assert!(simple_glob("\u{e9}*", GlobMode::Any, false).is_some());
    }

    #[test]
    fn http_version_labels() {
        assert_eq!(parse_http_version("HTTP/1.1"), Some(HttpVersion::Http11));
        assert_eq!(parse_http_version("http/2"), Some(HttpVersion::H2));
        assert_eq!(parse_http_version("h2"), Some(HttpVersion::H2));
        assert_eq!(parse_http_version("HTTP/2.0"), Some(HttpVersion::H2));
        assert_eq!(parse_http_version("3"), Some(HttpVersion::H3));
        assert_eq!(parse_http_version("1.0"), Some(HttpVersion::Http10));
        assert_eq!(parse_http_version("SPDY"), None);
    }

    #[test]
    fn pattern_ops() {
        let p = |op, v: &str, cs| {
            Pattern::compile(
                &StringMatch {
                    op,
                    value: v.into(),
                    case_sensitive: cs,
                    negate: false,
                },
                PatOpts::TEXT,
            )
            .unwrap()
        };
        assert!(p(MatchOp::Equals, "Foo", false).eval(Some("fOO")));
        assert!(!p(MatchOp::Equals, "Foo", true).eval(Some("fOO")));
        assert!(p(MatchOp::Equals, "\u{c4}rger", false).eval(Some("\u{e4}RGER")));
        assert!(p(MatchOp::StartsWith, "ab", false).eval(Some("ABc")));
        assert!(!p(MatchOp::StartsWith, "abcd", false).eval(Some("abc")));
        assert!(p(MatchOp::EndsWith, ".JSON", false).eval(Some("a.json")));
        assert!(!p(MatchOp::EndsWith, ".JSON", true).eval(Some("a.json")));
        assert!(p(MatchOp::StartsWith, "\u{e9}t\u{e9}", false).eval(Some("\u{c9}T\u{c9} 2024")));
        assert!(p(MatchOp::EndsWith, "\u{e9}", false).eval(Some("caf\u{c9}")));
        assert!(p(MatchOp::Contains, "needle", false).eval(Some("hay NEEDLE stack")));
        assert!(!p(MatchOp::Contains, "needle", true).eval(Some("hay NEEDLE stack")));
        assert!(p(MatchOp::Contains, "", false).eval(Some("x")));
        assert!(p(MatchOp::Regex, "^a+b$", true).eval(Some("aaab")));
        assert!(!p(MatchOp::Regex, "^a+b$", true).eval(Some("AAAB")));
        assert!(p(MatchOp::Regex, "^a+b$", false).eval(Some("AAAB")));
        assert!(p(MatchOp::Exists, "", false).eval(Some("x")));
        assert!(!p(MatchOp::Exists, "", false).eval(Some("")));
        assert!(!p(MatchOp::Exists, "", false).eval(None));
        assert!(!p(MatchOp::Equals, "", false).eval(None));
        // Multi-byte haystacks with ASCII patterns never panic on char boundaries.
        assert!(!p(MatchOp::EndsWith, "ab", false).eval(Some("\u{e9}")));
        assert!(!p(MatchOp::StartsWith, "ab", false).eval(Some("\u{e9}")));
        assert!(Pattern::compile(&StringMatch::regex("(unclosed"), PatOpts::TEXT).is_err());
    }
}
