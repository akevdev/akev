//! The compiled filter tree and its evaluation.
//!
//! Evaluation is allocation-free for summary-only terms. Per-exchange derived values (the full
//! URL, decoded bodies) are computed lazily at most once per [`Cx`], and the optimizer orders
//! `AND`/`OR` operands by cost so cheap summary terms short-circuit before header or body terms.

use crate::FilterTarget;
use crate::fields::{FlagId, error_name, kind_bit, kind_name, version_bit};
use crate::matcher::TextMatcher;
use akev_core::{
    ExchangeKind, ExchangeState, ExchangeSummary, Headers, HttpVersion, canonical_reason,
};
use smallvec::SmallVec;
use std::borrow::Cow;
use std::fmt::Write as _;

/// Which side(s) of the exchange a header/body term looks at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sides {
    Request,
    Response,
    Both,
}

impl Sides {
    fn request(self) -> bool {
        matches!(self, Sides::Request | Sides::Both)
    }
    fn response(self) -> bool {
        matches!(self, Sides::Response | Sides::Both)
    }
    fn label(self) -> &'static str {
        match self {
            Sides::Request => "req",
            Sides::Response => "res",
            Sides::Both => "any",
        }
    }
}

/// Text fields read from the summary (or cheap target accessors).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StrField {
    Method,
    Host,
    Path,
    Url,
    Query,
    Scheme,
    Source,
    Rule,
    Note,
    Color,
    Error,
    Ip,
    Model,
    Provider,
}

impl StrField {
    fn label(self) -> &'static str {
        match self {
            StrField::Method => "method",
            StrField::Host => "host",
            StrField::Path => "path",
            StrField::Url => "url",
            StrField::Query => "query",
            StrField::Scheme => "scheme",
            StrField::Source => "source",
            StrField::Rule => "rule",
            StrField::Note => "note",
            StrField::Color => "color",
            StrField::Error => "error",
            StrField::Ip => "ip",
            StrField::Model => "model",
            StrField::Provider => "provider",
        }
    }
}

/// Numeric fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NumField {
    Status,
    Port,
    Size,
    ReqSize,
    TotalSize,
    Duration,
    Id,
    Messages,
}

impl NumField {
    fn label(self) -> &'static str {
        match self {
            NumField::Status => "status",
            NumField::Port => "port",
            NumField::Size => "size",
            NumField::ReqSize => "reqsize",
            NumField::TotalSize => "totalsize",
            NumField::Duration => "duration",
            NumField::Id => "id",
            NumField::Messages => "messages",
        }
    }

    #[inline]
    fn get(self, s: &ExchangeSummary) -> Option<f64> {
        match self {
            NumField::Status => s.status.map(f64::from),
            NumField::Port => Some(f64::from(s.port)),
            NumField::Size => Some(s.response_size as f64),
            NumField::ReqSize => Some(s.request_size as f64),
            NumField::TotalSize => Some(s.request_size.saturating_add(s.response_size) as f64),
            NumField::Duration => s.duration,
            NumField::Id => Some(s.id.0 as f64),
            NumField::Messages => Some(f64::from(s.message_count)),
        }
    }
}

/// A numeric interval.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Range {
    pub lo: f64,
    pub hi: f64,
    pub lo_incl: bool,
    pub hi_incl: bool,
}

impl Range {
    pub(crate) fn between(lo: f64, hi: f64) -> Range {
        Range {
            lo,
            hi,
            lo_incl: true,
            hi_incl: true,
        }
    }

    #[inline]
    pub(crate) fn contains(&self, v: f64) -> bool {
        let above = if self.lo_incl {
            v >= self.lo
        } else {
            v > self.lo
        };
        let below = if self.hi_incl {
            v <= self.hi
        } else {
            v < self.hi
        };
        above && below
    }

    fn explain(&self) -> String {
        let n = |v: f64| {
            if v.fract() == 0.0 && v.abs() < 1e15 {
                format!("{}", v as i64)
            } else {
                format!("{v}")
            }
        };
        if self.lo == self.hi && self.lo_incl && self.hi_incl {
            format!("={}", n(self.lo))
        } else if self.lo == f64::NEG_INFINITY {
            format!("{}{}", if self.hi_incl { "<=" } else { "<" }, n(self.hi))
        } else if self.hi == f64::INFINITY {
            format!("{}{}", if self.lo_incl { ">=" } else { ">" }, n(self.lo))
        } else {
            format!(
                "{}{},{}{}",
                if self.lo_incl { "[" } else { "(" },
                n(self.lo),
                n(self.hi),
                if self.hi_incl { "]" } else { ")" }
            )
        }
    }
}

/// A leaf predicate.
#[derive(Clone, Debug)]
pub(crate) enum Pred {
    Flag(FlagId),
    Num(NumField, Box<[Range]>),
    Str(StrField, Box<[TextMatcher]>),
    /// Free text: URL, method and status (code and reason phrase).
    Text(TextMatcher),
    /// `type:` — content kind in the set, or a matcher on the MIME type / kind name.
    Type {
        kinds: u32,
        matchers: Box<[TextMatcher]>,
    },
    /// `version:` — version in the set, or a matcher on the version string.
    Version {
        versions: u8,
        matchers: Box<[TextMatcher]>,
    },
    HeaderName(Sides, Box<[TextMatcher]>),
    HeaderValue(Sides, Box<str>, Box<[TextMatcher]>),
    HeaderLine(Sides, Box<[TextMatcher]>),
    CookieName(Box<[TextMatcher]>),
    CookieValue(Box<str>, Box<[TextMatcher]>),
    QueryParam(Box<str>, Box<[TextMatcher]>),
    Body(Sides, Box<[TextMatcher]>),
    /// `contains:` — everything, bodies included. `long` is used for bodies.
    Deep {
        short: Box<[TextMatcher]>,
        long: Box<[TextMatcher]>,
    },
}

/// The compiled expression tree.
#[derive(Clone)]
pub(crate) enum Node {
    True,
    False,
    And(Box<[Node]>),
    Or(Box<[Node]>),
    Not(Box<Node>),
    Pred(Pred),
}

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.explain())
    }
}

/// Per-evaluation state: the target plus lazily computed, cached derived values.
pub(crate) struct Cx<'a> {
    t: &'a dyn FilterTarget,
    s: &'a ExchangeSummary,
    url: Option<SmallVec<[u8; 256]>>,
    req_body: Option<Option<Cow<'a, str>>>,
    res_body: Option<Option<Cow<'a, str>>>,
    scratch: SmallVec<[u8; 256]>,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(t: &'a dyn FilterTarget) -> Cx<'a> {
        Cx {
            t,
            s: t.summary(),
            url: None,
            req_body: None,
            res_body: None,
            scratch: SmallVec::new(),
        }
    }

    fn url(&mut self) -> &[u8] {
        let s = self.s;
        self.url.get_or_insert_with(|| {
            let mut buf = SmallVec::new();
            write_url(s, &mut buf);
            buf
        })
    }

    fn req_body(&mut self) -> Option<&str> {
        let t = self.t;
        self.req_body
            .get_or_insert_with(|| t.request_body_text())
            .as_deref()
    }

    fn res_body(&mut self) -> Option<&str> {
        let t = self.t;
        self.res_body
            .get_or_insert_with(|| t.response_body_text())
            .as_deref()
    }
}

/// `scheme://host[:port]path`, omitting the scheme's default port.
pub(crate) fn write_url(s: &ExchangeSummary, buf: &mut SmallVec<[u8; 256]>) {
    let scheme = s.scheme.as_str();
    if !scheme.is_empty() {
        buf.extend_from_slice(scheme.as_bytes());
        buf.extend_from_slice(b"://");
    }
    let ipv6 = s.host.contains(':');
    if ipv6 {
        buf.push(b'[');
    }
    buf.extend_from_slice(s.host.as_bytes());
    if ipv6 {
        buf.push(b']');
    }
    let default_port = match scheme.as_bytes() {
        b"http" | b"ws" => Some(80),
        b"https" | b"wss" => Some(443),
        _ => None,
    };
    if s.port != 0 && default_port != Some(s.port) {
        buf.push(b':');
        let mut digits = [0u8; 5];
        buf.extend_from_slice(u16_digits(s.port, &mut digits));
    }
    buf.extend_from_slice(s.path.as_bytes());
}

/// Format a `u16` into `buf`, returning the digits.
fn u16_digits(mut v: u16, buf: &mut [u8; 5]) -> &[u8] {
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    &buf[i..]
}

fn path_only(path: &str) -> &str {
    match memchr::memchr(b'?', path.as_bytes()) {
        Some(i) => &path[..i],
        None => path,
    }
}

fn query_of(path: &str) -> &str {
    match memchr::memchr(b'?', path.as_bytes()) {
        Some(i) => &path[i + 1..],
        None => "",
    }
}

fn any_match(ms: &[TextMatcher], h: &[u8]) -> bool {
    ms.iter().any(|m| m.is_match(h))
}

fn headers_for(t: &dyn FilterTarget, sides: Sides) -> impl Iterator<Item = &Headers> {
    let req = sides.request().then(|| t.request_headers());
    let res = if sides.response() {
        t.response_headers()
    } else {
        None
    };
    req.into_iter().chain(res)
}

/// Iterate `(name, value)` cookie pairs: request `Cookie` headers and response `Set-Cookie`
/// headers (the part before the first `;`).
fn for_each_cookie(t: &dyn FilterTarget, mut f: impl FnMut(&str, &str) -> bool) -> bool {
    for value in t.request_headers().get_all("cookie") {
        for pair in value.split(';') {
            let (name, val) = pair.split_once('=').unwrap_or((pair, ""));
            if f(name.trim(), val.trim()) {
                return true;
            }
        }
    }
    if let Some(res) = t.response_headers() {
        for value in res.get_all("set-cookie") {
            let pair = value.split(';').next().unwrap_or("");
            let (name, val) = pair.split_once('=').unwrap_or((pair, ""));
            if f(name.trim(), val.trim()) {
                return true;
            }
        }
    }
    false
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Decode `application/x-www-form-urlencoded` text into `out` (invalid escapes kept as-is).
fn percent_decode_into(s: &str, out: &mut SmallVec<[u8; 256]>) {
    out.clear();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex_val(b[i + 1]), hex_val(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h * 16 + l);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
}

/// Match each header's value, or its whole `Name: value` line (so both `headers=hit` and
/// `headers:"x-cache: hit"` work).
fn header_lines_match(
    t: &dyn FilterTarget,
    sides: Sides,
    ms: &[TextMatcher],
    scratch: &mut SmallVec<[u8; 256]>,
) -> bool {
    headers_for(t, sides).any(|h| {
        h.iter().any(|(k, v)| {
            if any_match(ms, v.as_bytes()) {
                return true;
            }
            scratch.clear();
            scratch.extend_from_slice(k.as_bytes());
            scratch.extend_from_slice(b": ");
            scratch.extend_from_slice(v.as_bytes());
            any_match(ms, scratch)
        })
    })
}

fn eval_flag(flag: FlagId, cx: &Cx<'_>) -> bool {
    let s = cx.s;
    match flag {
        FlagId::Pending => matches!(
            s.state,
            ExchangeState::Requesting
                | ExchangeState::Waiting
                | ExchangeState::Responding
                | ExchangeState::Paused
                | ExchangeState::Streaming
        ),
        FlagId::Complete => s.state == ExchangeState::Complete,
        FlagId::Error => {
            s.error.is_some()
                || matches!(s.kind, ExchangeKind::TlsFailure | ExchangeKind::ClientError)
        }
        FlagId::Aborted => s.state == ExchangeState::Aborted || s.kind == ExchangeKind::TlsFailure,
        FlagId::Paused => s.paused.is_some() || s.state == ExchangeState::Paused,
        FlagId::Pinned => s.pinned,
        FlagId::Mocked => s.mocked,
        FlagId::Modified => s.modified,
        FlagId::WebSocket => s.kind == ExchangeKind::WebSocket,
        FlagId::Sse => s.kind == ExchangeKind::EventStream,
        FlagId::Streaming => s.state == ExchangeState::Streaming,
        FlagId::TlsFailure => s.kind == ExchangeKind::TlsFailure,
        FlagId::Tunnel => s.kind == ExchangeKind::Tunnel,
        FlagId::Malformed => s.kind == ExchangeKind::ClientError,
        FlagId::Http => {
            s.scheme.eq_ignore_ascii_case("http") || s.scheme.eq_ignore_ascii_case("ws")
        }
        FlagId::Https => {
            s.scheme.eq_ignore_ascii_case("https") || s.scheme.eq_ignore_ascii_case("wss")
        }
        FlagId::Highlighted => s.color.as_deref().is_some_and(|c| !c.is_empty()),
        FlagId::Mutative => ["POST", "PUT", "PATCH", "DELETE"]
            .iter()
            .any(|m| s.method.eq_ignore_ascii_case(m)),
        FlagId::Note => s.has_note,
        FlagId::Body => s.request_size > 0 || s.response_size > 0,
        FlagId::ReqBody => s.request_size > 0,
        FlagId::ResBody => s.response_size > 0,
        FlagId::Query => !query_of(&s.path).is_empty(),
        FlagId::Cookie => {
            cx.t.request_headers().contains("cookie")
                || cx
                    .t
                    .response_headers()
                    .is_some_and(|h| h.contains("set-cookie"))
        }
        FlagId::Auth => {
            let h = cx.t.request_headers();
            h.contains("authorization") || h.contains("proxy-authorization")
        }
        FlagId::Rule => s.rule.is_some(),
        FlagId::Response => s.status.is_some(),
        FlagId::Messages => s.message_count > 0,
        FlagId::Llm => s.llm.is_some(),
    }
}

impl Pred {
    fn eval(&self, cx: &mut Cx<'_>) -> bool {
        let s = cx.s;
        let t = cx.t;
        match self {
            Pred::Flag(f) => eval_flag(*f, cx),
            Pred::Num(field, ranges) => match field.get(s) {
                Some(v) => ranges.iter().any(|r| r.contains(v)),
                None => false,
            },
            Pred::Str(field, ms) => {
                let ms = &ms[..];
                match field {
                    StrField::Method => any_match(ms, s.method.as_bytes()),
                    StrField::Host => any_match(ms, s.host.as_bytes()),
                    StrField::Path => any_match(ms, path_only(&s.path).as_bytes()),
                    StrField::Query => any_match(ms, query_of(&s.path).as_bytes()),
                    StrField::Scheme => any_match(ms, s.scheme.as_bytes()),
                    StrField::Url => {
                        let url = cx.url();
                        any_match(ms, url)
                    }
                    StrField::Source => s
                        .source
                        .as_deref()
                        .is_some_and(|v| any_match(ms, v.as_bytes())),
                    StrField::Color => s
                        .color
                        .as_deref()
                        .is_some_and(|v| any_match(ms, v.as_bytes())),
                    StrField::Error => s
                        .error
                        .is_some_and(|e| any_match(ms, error_name(e).as_bytes())),
                    StrField::Rule => {
                        s.rule
                            .as_deref()
                            .is_some_and(|v| any_match(ms, v.as_bytes()))
                            || t.rule_title().is_some_and(|v| any_match(ms, v.as_bytes()))
                    }
                    StrField::Note => t.note().is_some_and(|v| any_match(ms, v.as_bytes())),
                    StrField::Ip => t.client_ip().is_some_and(|v| any_match(ms, v.as_bytes())),
                    StrField::Model => s
                        .llm
                        .as_ref()
                        .and_then(|l| l.model.as_deref())
                        .is_some_and(|v| any_match(ms, v.as_bytes())),
                    StrField::Provider => s
                        .llm
                        .as_ref()
                        .is_some_and(|l| any_match(ms, l.provider.as_bytes())),
                }
            }
            Pred::Text(m) => {
                if m.is_match(s.method.as_bytes()) || status_matches(m, s.status) {
                    return true;
                }
                let url = cx.url();
                m.is_match(url)
            }
            Pred::Type { kinds, matchers } => {
                (kinds & kind_bit(s.content_kind)) != 0
                    || s.content_type
                        .as_deref()
                        .is_some_and(|ct| any_match(matchers, ct.as_bytes()))
                    || (!matchers.is_empty()
                        && any_match(matchers, kind_name(s.content_kind).as_bytes()))
            }
            Pred::Version { versions, matchers } => {
                (versions & version_bit(s.http_version)) != 0
                    || any_match(matchers, version_str(s.http_version).as_bytes())
            }
            Pred::HeaderName(sides, ms) => {
                headers_for(t, *sides).any(|h| h.iter().any(|(k, _)| any_match(ms, k.as_bytes())))
            }
            Pred::HeaderValue(sides, name, ms) => {
                headers_for(t, *sides).any(|h| h.get_all(name).any(|v| any_match(ms, v.as_bytes())))
            }
            Pred::HeaderLine(sides, ms) => header_lines_match(t, *sides, ms, &mut cx.scratch),
            Pred::CookieName(ms) => for_each_cookie(t, |name, _| any_match(ms, name.as_bytes())),
            Pred::CookieValue(want, ms) => for_each_cookie(t, |name, value| {
                name.eq_ignore_ascii_case(want) && any_match(ms, value.as_bytes())
            }),
            Pred::QueryParam(want, ms) => {
                let query = query_of(&s.path);
                let scratch = &mut cx.scratch;
                let mut name_buf: SmallVec<[u8; 256]> = SmallVec::new();
                query.split('&').filter(|p| !p.is_empty()).any(|pair| {
                    let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
                    percent_decode_into(name, &mut name_buf);
                    if !name_buf.eq_ignore_ascii_case(want.as_bytes()) {
                        return false;
                    }
                    percent_decode_into(value, scratch);
                    any_match(ms, scratch)
                })
            }
            Pred::Body(sides, ms) => {
                (sides.request() && cx.req_body().is_some_and(|b| any_match(ms, b.as_bytes())))
                    || (sides.response()
                        && cx.res_body().is_some_and(|b| any_match(ms, b.as_bytes())))
            }
            Pred::Deep { short, long } => {
                if short
                    .iter()
                    .any(|m| m.is_match(s.method.as_bytes()) || status_matches(m, s.status))
                {
                    return true;
                }
                if any_match(short, cx.url()) {
                    return true;
                }
                if header_lines_match(t, Sides::Both, short, &mut cx.scratch) {
                    return true;
                }
                cx.req_body().is_some_and(|b| any_match(long, b.as_bytes()))
                    || cx.res_body().is_some_and(|b| any_match(long, b.as_bytes()))
            }
        }
    }

    fn cost(&self) -> u32 {
        match self {
            Pred::Flag(FlagId::Cookie | FlagId::Auth) => 20,
            Pred::Flag(_) | Pred::Num(..) => 1,
            Pred::Type { .. } | Pred::Version { .. } => 2,
            Pred::Str(f, ms) => {
                let base = match f {
                    StrField::Url => 6,
                    StrField::Rule | StrField::Note | StrField::Ip => 5,
                    _ => 2,
                };
                base + ms.len() as u32
            }
            Pred::Text(_) => 8,
            Pred::QueryParam(..) => 6,
            Pred::HeaderName(..) | Pred::HeaderValue(..) => 20,
            Pred::CookieName(..) | Pred::CookieValue(..) => 25,
            Pred::HeaderLine(..) => 30,
            Pred::Body(..) => 1000,
            Pred::Deep { .. } => 1200,
        }
    }

    pub(crate) fn needs_body(&self) -> bool {
        matches!(self, Pred::Body(..) | Pred::Deep { .. })
    }

    fn explain(&self, out: &mut String) {
        let ms = |ms: &[TextMatcher]| {
            ms.iter()
                .map(TextMatcher::explain)
                .collect::<Vec<_>>()
                .join(" | ")
        };
        let _ = match self {
            Pred::Flag(f) => write!(out, "(is {f:?})"),
            Pred::Num(f, ranges) => write!(
                out,
                "({} {})",
                f.label(),
                ranges
                    .iter()
                    .map(Range::explain)
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            Pred::Str(f, m) => write!(out, "({} {})", f.label(), ms(m)),
            Pred::Text(m) => write!(out, "(text {})", m.explain()),
            Pred::Type { kinds, matchers } => {
                let mut parts: Vec<String> = Vec::new();
                if *kinds != 0 {
                    let names: Vec<&str> = ALL_KINDS
                        .iter()
                        .filter(|k| kinds & kind_bit(**k) != 0)
                        .map(|k| kind_name(*k))
                        .collect();
                    parts.push(format!("kinds[{}]", names.join(",")));
                }
                if !matchers.is_empty() {
                    parts.push(ms(matchers));
                }
                write!(out, "(type {})", parts.join(" | "))
            }
            Pred::Version { versions, matchers } => {
                let mut parts: Vec<String> = Vec::new();
                if *versions != 0 {
                    let names: Vec<&str> = ALL_VERSIONS
                        .iter()
                        .filter(|v| versions & version_bit(**v) != 0)
                        .map(|v| version_str(*v))
                        .collect();
                    parts.push(format!("[{}]", names.join(",")));
                }
                if !matchers.is_empty() {
                    parts.push(ms(matchers));
                }
                write!(out, "(version {})", parts.join(" | "))
            }
            Pred::HeaderName(sides, m) => write!(out, "({}-header-name {})", sides.label(), ms(m)),
            Pred::HeaderValue(sides, name, m) => {
                write!(out, "({}-header[{name}] {})", sides.label(), ms(m))
            }
            Pred::HeaderLine(sides, m) => write!(out, "({}-header-line {})", sides.label(), ms(m)),
            Pred::CookieName(m) => write!(out, "(cookie-name {})", ms(m)),
            Pred::CookieValue(name, m) => write!(out, "(cookie[{name}] {})", ms(m)),
            Pred::QueryParam(name, m) => write!(out, "(query[{name}] {})", ms(m)),
            Pred::Body(sides, m) => write!(out, "({}-body {})", sides.label(), ms(m)),
            Pred::Deep { short, .. } => write!(out, "(contains {})", ms(short)),
        };
    }
}

fn status_matches(m: &TextMatcher, status: Option<u16>) -> bool {
    let Some(code) = status else { return false };
    let mut digits = [0u8; 5];
    if m.is_match(u16_digits(code, &mut digits)) {
        return true;
    }
    let reason = canonical_reason(code);
    !reason.is_empty() && m.is_match(reason.as_bytes())
}

/// Version strings matched by non-alias `version:` values.
pub(crate) fn version_str(v: HttpVersion) -> &'static str {
    v.as_str()
}

const ALL_VERSIONS: [HttpVersion; 5] = [
    HttpVersion::Http09,
    HttpVersion::Http10,
    HttpVersion::Http11,
    HttpVersion::H2,
    HttpVersion::H3,
];

pub(crate) const ALL_KINDS: [akev_core::ContentKind; 27] = {
    use akev_core::ContentKind::*;
    [
        Json,
        JsonLines,
        Xml,
        Html,
        Javascript,
        Css,
        Markdown,
        Yaml,
        Csv,
        Text,
        FormUrlEncoded,
        Multipart,
        Image,
        Audio,
        Video,
        Font,
        Pdf,
        Wasm,
        Protobuf,
        Grpc,
        GrpcWeb,
        MsgPack,
        Cbor,
        EventStream,
        GraphQl,
        Binary,
        Unknown,
    ]
};

impl Node {
    #[inline]
    pub(crate) fn eval(&self, cx: &mut Cx<'_>) -> bool {
        match self {
            Node::True => true,
            Node::False => false,
            Node::And(children) => children.iter().all(|c| c.eval(cx)),
            Node::Or(children) => children.iter().any(|c| c.eval(cx)),
            Node::Not(child) => !child.eval(cx),
            Node::Pred(p) => p.eval(cx),
        }
    }

    fn cost(&self) -> u32 {
        match self {
            Node::True | Node::False => 0,
            Node::And(c) | Node::Or(c) => {
                c.iter().fold(0u32, |acc, n| acc.saturating_add(n.cost()))
            }
            Node::Not(c) => c.cost(),
            Node::Pred(p) => p.cost(),
        }
    }

    pub(crate) fn needs_body(&self) -> bool {
        match self {
            Node::True | Node::False => false,
            Node::And(c) | Node::Or(c) => c.iter().any(Node::needs_body),
            Node::Not(c) => c.needs_body(),
            Node::Pred(p) => p.needs_body(),
        }
    }

    /// Flatten nested AND/OR, fold constants, remove double negation and order operands by cost
    /// (cheap summary terms first, bodies last). Boolean semantics are unchanged because
    /// predicates are pure.
    pub(crate) fn optimize(self) -> Node {
        match self {
            Node::And(children) => {
                let mut out = Vec::with_capacity(children.len());
                for child in children.into_vec() {
                    match child.optimize() {
                        Node::True => {}
                        Node::False => return Node::False,
                        Node::And(inner) => out.extend(inner.into_vec()),
                        other => out.push(other),
                    }
                }
                finish(out, Node::True, Node::And)
            }
            Node::Or(children) => {
                let mut out = Vec::with_capacity(children.len());
                for child in children.into_vec() {
                    match child.optimize() {
                        Node::False => {}
                        Node::True => return Node::True,
                        Node::Or(inner) => out.extend(inner.into_vec()),
                        other => out.push(other),
                    }
                }
                finish(out, Node::False, Node::Or)
            }
            Node::Not(child) => match child.optimize() {
                Node::True => Node::False,
                Node::False => Node::True,
                Node::Not(inner) => *inner,
                other => Node::Not(Box::new(other)),
            },
            leaf => leaf,
        }
    }

    /// An S-expression rendering of the tree (for tests and debugging).
    pub(crate) fn explain(&self) -> String {
        let mut out = String::new();
        self.explain_into(&mut out);
        out
    }

    fn explain_into(&self, out: &mut String) {
        match self {
            Node::True => out.push_str("true"),
            Node::False => out.push_str("false"),
            Node::And(c) | Node::Or(c) => {
                out.push_str(if matches!(self, Node::And(_)) {
                    "(and"
                } else {
                    "(or"
                });
                for n in c.iter() {
                    out.push(' ');
                    n.explain_into(out);
                }
                out.push(')');
            }
            Node::Not(c) => {
                out.push_str("(not ");
                c.explain_into(out);
                out.push(')');
            }
            Node::Pred(p) => p.explain(out),
        }
    }
}

fn finish(mut out: Vec<Node>, empty: Node, wrap: fn(Box<[Node]>) -> Node) -> Node {
    match out.len() {
        0 => empty,
        1 => out.pop().unwrap_or(empty),
        _ => {
            // Stable: equal-cost operands keep the order the user wrote them in.
            out.sort_by_key(Node::cost);
            wrap(out.into_boxed_slice())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_contains() {
        let r = Range::between(400.0, 499.0);
        assert!(r.contains(400.0) && r.contains(499.0) && !r.contains(500.0));
        let r = Range {
            lo: 5.0,
            hi: f64::INFINITY,
            lo_incl: false,
            hi_incl: true,
        };
        assert!(!r.contains(5.0) && r.contains(5.1));
        assert_eq!(Range::between(404.0, 404.0).explain(), "=404");
        assert_eq!(r.explain(), ">5");
        assert_eq!(Range::between(1.5, 2.0).explain(), "[1.5,2]");
    }

    #[test]
    fn digits() {
        let mut b = [0u8; 5];
        assert_eq!(u16_digits(0, &mut b), b"0");
        assert_eq!(u16_digits(404, &mut b), b"404");
        assert_eq!(u16_digits(65535, &mut b), b"65535");
    }

    #[test]
    fn percent_decoding() {
        let mut out = SmallVec::new();
        percent_decode_into("a%20b+c%2", &mut out);
        assert_eq!(&out[..], b"a b c%2");
        percent_decode_into("%zz%41", &mut out);
        assert_eq!(&out[..], b"%zzA");
        percent_decode_into("%", &mut out);
        assert_eq!(&out[..], b"%");
        percent_decode_into("%4", &mut out);
        assert_eq!(&out[..], b"%4");
        percent_decode_into("x%41", &mut out);
        assert_eq!(&out[..], b"xA");
    }
}
