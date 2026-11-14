//! Request sanitisation and body classification shared by every generator.

use crate::SnippetRequest;
use crate::json;
use percent_encoding::percent_decode_str;
use serde_json::Value;
use std::borrow::Cow;

/// A sanitised view of a [`SnippetRequest`], borrowed from it.
pub(crate) struct Prepared<'a> {
    /// Normalised method (standard methods upper-cased, empty → `GET`).
    pub method: &'a str,
    /// The URL, trimmed.
    pub url: &'a str,
    /// The parsed URL, when it parses.
    pub parsed: Option<url::Url>,
    /// Headers to send, in order (see the crate docs for what is dropped). `Accept-Encoding`
    /// is not included (see [`Prepared::accept_encoding`]); a `Host` header is only present
    /// when it differs from the URL's authority.
    pub headers: Vec<(&'a str, &'a str)>,
    /// The original `Accept-Encoding` value, if the request had one.
    pub accept_encoding: Option<&'a str>,
    /// The original headers minus pseudo-headers, for the raw HTTP target.
    pub raw_headers: Vec<(&'a str, &'a str)>,
    pub body: Body<'a>,
}

/// A classified request body.
pub(crate) enum Body<'a> {
    None,
    /// JSON that round-trips through `serde_json` without changing meaning.
    Json {
        text: &'a str,
        value: Value,
    },
    /// A cleanly decodable `application/x-www-form-urlencoded` body.
    Form {
        text: &'a str,
        fields: Vec<FormField<'a>>,
    },
    /// Any other UTF-8 text without NUL bytes.
    Text(&'a str),
    /// Everything else.
    Binary(&'a [u8]),
}

/// One `name=value` pair of a form body: raw (as on the wire) and decoded.
pub(crate) struct FormField<'a> {
    pub raw: &'a str,
    pub name: String,
    pub value: String,
}

impl Body<'_> {
    /// The body as text, unless it's binary or absent.
    pub fn text(&self) -> Option<&str> {
        match self {
            Body::Json { text, .. } | Body::Form { text, .. } | Body::Text(text) => Some(text),
            Body::None | Body::Binary(_) => None,
        }
    }

    pub fn is_none(&self) -> bool {
        matches!(self, Body::None)
    }

    pub fn bytes(&self) -> &[u8] {
        match self {
            Body::None => &[],
            Body::Binary(b) => b,
            other => other.text().unwrap_or_default().as_bytes(),
        }
    }
}

impl<'a> Prepared<'a> {
    /// First value of a (sanitised) header, case-insensitive.
    pub fn header(&self, name: &str) -> Option<&'a str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|&(_, v)| v)
    }

    /// The `Content-Type` value, if any.
    pub fn content_type(&self) -> Option<&'a str> {
        self.header("content-type")
    }

    /// Headers merged by case-insensitive name (first spelling wins), for map-based APIs.
    /// Repeated values are joined with `, ` (`; ` for `Cookie`).
    pub fn merged_headers(&self) -> Vec<(&'a str, Cow<'a, str>)> {
        merge_headers(&self.headers)
    }

    /// Whether the URL's path or query contains characters curl would treat as a glob.
    pub fn url_needs_globoff(&self) -> bool {
        let rest = match self.url.find("://") {
            Some(i) => &self.url[i + 3..],
            None => self.url,
        };
        // Skip the authority so IPv6 literals (`[::1]`) don't count.
        let path = rest.find(['/', '?']).map_or("", |i| &rest[i..]);
        path.contains(['[', ']', '{', '}'])
    }

    /// Whether the path contains `.`/`..` segments that clients would normalise away.
    pub fn url_has_dot_segments(&self) -> bool {
        let rest = match self.url.find("://") {
            Some(i) => &self.url[i + 3..],
            None => return false,
        };
        let Some(start) = rest.find('/') else {
            return false;
        };
        let path = &rest[start..];
        let path = path.split(['?', '#']).next().unwrap_or_default();
        path.split('/')
            .any(|seg| seg == "." || seg == ".." || seg.eq_ignore_ascii_case("%2e"))
    }

    /// The method is one that clients treat as never carrying a body.
    pub fn is_get_or_head(&self) -> bool {
        matches!(self.method, "GET" | "HEAD")
    }

    /// The URL with every character RFC 3986 doesn't allow after the authority
    /// percent-encoded as UTF-8, for clients whose URL parsers reject such URLs instead of
    /// escaping them (`java.net.URI`, Foundation's `URL`). Stray `%` signs become `%25` and any
    /// `#` after the first becomes `%23`; the scheme and authority are left untouched.
    pub fn strict_url(&self) -> Cow<'a, str> {
        strict_url(self.url)
    }

    /// Names of headers whose values contain non-ASCII characters (which several clients
    /// can't send, or send as Latin-1 unless told otherwise).
    pub fn non_ascii_header_names(&self) -> Vec<&'a str> {
        let mut names: Vec<&'a str> = Vec::new();
        for &(name, value) in &self.headers {
            if !value.is_ascii() && !names.iter().any(|n| n.eq_ignore_ascii_case(name)) {
                names.push(name);
            }
        }
        names
    }
}

fn strict_url(url: &str) -> Cow<'_, str> {
    let Some(scheme_end) = url.find("://").map(|i| i + 3) else {
        return Cow::Borrowed(url);
    };
    let rest = &url[scheme_end..];
    let start = scheme_end + rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let tail = &url[start..];
    let bytes = tail.as_bytes();
    let is_allowed = |i: usize, seen_hash: bool| match bytes[i] {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' => true,
        b'-' | b'.' | b'_' | b'~' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' => true,
        b',' | b';' | b'=' | b':' | b'@' | b'/' | b'?' => true,
        b'%' => {
            bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit)
        }
        b'#' => !seen_hash,
        _ => false,
    };
    let mut seen_hash = false;
    let valid = (0..bytes.len()).all(|i| {
        let ok = is_allowed(i, seen_hash);
        seen_hash |= bytes[i] == b'#';
        ok
    });
    if valid {
        return Cow::Borrowed(url);
    }
    let mut out = String::with_capacity(url.len() + 16);
    out.push_str(&url[..start]);
    seen_hash = false;
    for (i, &b) in bytes.iter().enumerate() {
        if is_allowed(i, seen_hash) {
            out.push(char::from(b));
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
        seen_hash |= b == b'#';
    }
    Cow::Owned(out)
}

pub(crate) fn merge_headers<'a>(headers: &[(&'a str, &'a str)]) -> Vec<(&'a str, Cow<'a, str>)> {
    let mut out: Vec<(&'a str, Cow<'a, str>)> = Vec::with_capacity(headers.len());
    for &(name, value) in headers {
        if let Some((_, existing)) = out.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            let sep = if name.eq_ignore_ascii_case("cookie") {
                "; "
            } else {
                ", "
            };
            let joined = existing.to_mut();
            joined.push_str(sep);
            joined.push_str(value);
        } else {
            out.push((name, Cow::Borrowed(value)));
        }
    }
    out
}

const STANDARD_METHODS: [&str; 9] = [
    "GET", "POST", "PUT", "DELETE", "HEAD", "OPTIONS", "PATCH", "TRACE", "CONNECT",
];

/// Is `method` one of the standard (RFC 9110 + PATCH) methods, in canonical upper case?
pub(crate) fn is_standard_method(method: &str) -> bool {
    STANDARD_METHODS.contains(&method)
}

fn normalize_method(method: &str) -> &str {
    let method = method.trim();
    if method.is_empty() {
        return "GET";
    }
    STANDARD_METHODS
        .into_iter()
        .find(|m| m.eq_ignore_ascii_case(method))
        .unwrap_or(method)
}

/// Headers dropped for every target (lower-case).
const ALWAYS_DROPPED: [&str; 11] = [
    "content-length",
    "content-encoding",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "transfer-encoding",
    "te",
    "trailer",
    "upgrade",
    "expect",
];

/// Sanitise `req` and classify its body. JSON bodies are only parsed (and classified as
/// [`Body::Json`]) when `native_json` is set, i.e. the target emits native literals; other
/// targets inline the text, so parsing a large body would be wasted work.
pub(crate) fn prepare(req: &SnippetRequest, native_json: bool) -> Prepared<'_> {
    let url = req.url.trim();
    let parsed = url::Url::parse(url).ok();

    // Headers named by `Connection` are connection-specific too.
    let connection_tokens: Vec<&str> = req
        .headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect();

    let mut headers = Vec::with_capacity(req.headers.len());
    let mut raw_headers = Vec::with_capacity(req.headers.len());
    let mut accept_encoding = None;
    for (name, value) in req.headers.iter() {
        let name = name.trim();
        if name.is_empty() || name.starts_with(':') {
            continue;
        }
        raw_headers.push((name, value));
        let lower = name.to_ascii_lowercase();
        if ALWAYS_DROPPED.contains(&lower.as_str())
            || connection_tokens
                .iter()
                .any(|t| t.eq_ignore_ascii_case(name))
        {
            continue;
        }
        match lower.as_str() {
            "accept-encoding" => {
                accept_encoding.get_or_insert(value);
            }
            "host" if parsed.as_ref().is_some_and(|u| host_matches(value, u)) => {}
            _ => headers.push((name, value)),
        }
    }

    let content_type = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|&(_, v)| v);
    let body = classify(
        req.body.as_deref().unwrap_or_default(),
        content_type,
        native_json,
    );

    Prepared {
        method: normalize_method(&req.method),
        url,
        parsed,
        headers,
        accept_encoding,
        raw_headers,
        body,
    }
}

/// Does a `Host` header value name the same authority as the URL?
fn host_matches(host_header: &str, url: &url::Url) -> bool {
    let Some(url_host) = url.host_str() else {
        return false;
    };
    let value = host_header.trim();
    let (host, port) = split_host_port(value);
    let port = match port {
        Some(p) => match p.parse::<u16>() {
            Ok(p) => Some(p),
            Err(_) => return false,
        },
        None => None,
    };
    let scheme_default = match url.scheme() {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    };
    host.trim_end_matches('.')
        .eq_ignore_ascii_case(url_host.trim_end_matches('.'))
        && port.or(scheme_default) == url.port_or_known_default()
}

/// Split `host[:port]` (IPv6 hosts keep their brackets).
fn split_host_port(value: &str) -> (&str, Option<&str>) {
    if value.starts_with('[') {
        return match value.find(']') {
            Some(end) => {
                let host = &value[..=end];
                let rest = &value[end + 1..];
                (host, rest.strip_prefix(':'))
            }
            None => (value, None),
        };
    }
    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (host, Some(port)),
        _ => (value, None),
    }
}

fn classify<'a>(bytes: &'a [u8], content_type: Option<&str>, native_json: bool) -> Body<'a> {
    if bytes.is_empty() {
        return Body::None;
    }
    let Ok(text) = std::str::from_utf8(bytes) else {
        return Body::Binary(bytes);
    };
    if text.contains('\0') {
        return Body::Binary(bytes);
    }
    let essence = content_type
        .map(|ct| {
            ct.split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase()
        })
        .unwrap_or_default();
    if is_json_type(&essence) {
        if !native_json {
            return Body::Text(text);
        }
        if let Some(value) = json::parse_faithful(text) {
            return Body::Json { text, value };
        }
    } else if essence == "application/x-www-form-urlencoded"
        && let Some(fields) = parse_form(text)
    {
        return Body::Form { text, fields };
    }
    Body::Text(text)
}

fn is_json_type(essence: &str) -> bool {
    essence == "application/json" || essence == "text/json" || essence.ends_with("+json")
}

/// Parse a form body, accepting it only when decoding is lossless: every field non-empty,
/// every `%` a valid escape, and every name/value valid UTF-8 once decoded.
fn parse_form(text: &str) -> Option<Vec<FormField<'_>>> {
    let mut fields = Vec::new();
    for raw in text.split('&') {
        if raw.is_empty() || !valid_percent_escapes(raw) {
            return None;
        }
        let (name, value) = raw.split_once('=').unwrap_or((raw, ""));
        let name = decode_form_component(name)?;
        if name.is_empty() {
            return None;
        }
        let value = decode_form_component(value)?;
        fields.push(FormField { raw, name, value });
    }
    Some(fields)
}

fn valid_percent_escapes(s: &str) -> bool {
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let ok = bytes.get(i + 1).is_some_and(u8::is_ascii_hexdigit)
                && bytes.get(i + 2).is_some_and(u8::is_ascii_hexdigit);
            if !ok {
                return false;
            }
            i += 3;
        } else {
            i += 1;
        }
    }
    true
}

fn decode_form_component(s: &str) -> Option<String> {
    let plus_decoded = s.replace('+', " ");
    let decoded = percent_decode_str(&plus_decoded).decode_utf8().ok()?;
    // Control characters would make the native form literal misleading; keep the raw text.
    if decoded
        .chars()
        .any(|c| c.is_control() && c != '\n' && c != '\t')
    {
        return None;
    }
    Some(decoded.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::Headers;
    use bytes::Bytes;

    fn req(
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> SnippetRequest {
        SnippetRequest {
            method: method.into(),
            url: url.into(),
            headers: Headers(
                headers
                    .iter()
                    .map(|(a, b)| (a.to_string(), b.to_string()))
                    .collect(),
            ),
            body: body.map(Bytes::copy_from_slice),
        }
    }

    #[test]
    fn drops_hop_by_hop_and_auto_headers() {
        let r = req(
            "get",
            "https://example.com/a",
            &[
                (":authority", "example.com"),
                ("Host", "example.com"),
                ("Content-Length", "3"),
                ("Content-Encoding", "gzip"),
                ("Connection", "keep-alive, X-Conn"),
                ("X-Conn", "1"),
                ("Transfer-Encoding", "chunked"),
                ("Accept-Encoding", "gzip, br"),
                ("Expect", "100-continue"),
                ("Cookie", "a=1"),
                ("X-Keep", "yes"),
            ],
            None,
        );
        let p = prepare(&r, true);
        assert_eq!(p.method, "GET");
        assert_eq!(p.headers, vec![("Cookie", "a=1"), ("X-Keep", "yes")]);
        assert_eq!(p.accept_encoding, Some("gzip, br"));
        assert_eq!(p.raw_headers.len(), 10);
    }

    #[test]
    fn host_kept_only_when_different() {
        for (url, host, kept) in [
            ("https://example.com/", "example.com", false),
            ("https://example.com/", "EXAMPLE.com:443", false),
            ("https://example.com/", "example.com:8443", true),
            ("http://example.com:8080/", "example.com:8080", false),
            ("http://example.com:8080/", "example.com", true),
            ("http://[::1]:8080/", "[::1]:8080", false),
            ("http://[::1]/", "[::1]", false),
            ("https://example.com/", "other.test", true),
            ("not a url", "example.com", true),
        ] {
            let p_req = req("GET", url, &[("Host", host)], None);
            let p = prepare(&p_req, true);
            assert_eq!(p.header("host").is_some(), kept, "{url} {host}");
        }
    }

    #[test]
    fn classifies_bodies() {
        let json_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/json")],
            Some(b"{\"a\":1}"),
        );
        let json = prepare(&json_req, true);
        assert!(matches!(json.body, Body::Json { .. }));
        let vnd_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/vnd.api+json; charset=utf-8")],
            Some(b"[1, 2]"),
        );
        let vnd = prepare(&vnd_req, true);
        assert!(matches!(vnd.body, Body::Json { .. }));
        // Duplicate keys can't be represented natively.
        let dup_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/json")],
            Some(b"{\"a\":1,\"a\":2}"),
        );
        let dup = prepare(&dup_req, true);
        assert!(matches!(dup.body, Body::Text(_)));
        let form_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(b"a=1&b=hello+world%21"),
        );
        let form = prepare(&form_req, true);
        match &form.body {
            Body::Form { fields, .. } => {
                assert_eq!(fields[1].name, "b");
                assert_eq!(fields[1].value, "hello world!");
                assert_eq!(fields[1].raw, "b=hello+world%21");
            }
            _ => panic!("expected form"),
        }
        let bad_form_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(b"a=%zz"),
        );
        let bad_form = prepare(&bad_form_req, true);
        assert!(matches!(bad_form.body, Body::Text(_)));
        let empty_field_req = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/x-www-form-urlencoded")],
            Some(b"a=1&&b=2"),
        );
        let empty_field = prepare(&empty_field_req, true);
        assert!(matches!(empty_field.body, Body::Text(_)));
        let bin_req = req("POST", "http://x/", &[], Some(&[0xff, 0x00]));
        let bin = prepare(&bin_req, true);
        assert!(matches!(bin.body, Body::Binary(_)));
        let nul_req = req("POST", "http://x/", &[], Some(b"a\0b"));
        let nul = prepare(&nul_req, true);
        assert!(matches!(nul.body, Body::Binary(_)));
        let empty_req = req("POST", "http://x/", &[], Some(b""));
        let empty = prepare(&empty_req, true);
        assert!(empty.body.is_none());
    }

    #[test]
    fn json_only_parsed_for_native_targets() {
        let r = req(
            "POST",
            "http://x/",
            &[("Content-Type", "application/json")],
            Some(b"{\"a\":1}"),
        );
        assert!(matches!(prepare(&r, true).body, Body::Json { .. }));
        assert!(matches!(prepare(&r, false).body, Body::Text("{\"a\":1}")));
    }

    #[test]
    fn merges_duplicate_headers() {
        let p_req = req(
            "GET",
            "http://x/",
            &[
                ("Cookie", "a=1"),
                ("X-A", "1"),
                ("cookie", "b=2"),
                ("x-a", "2"),
            ],
            None,
        );
        let p = prepare(&p_req, true);
        let merged = p.merged_headers();
        assert_eq!(merged.len(), 2);
        assert_eq!(merged[0], ("Cookie", Cow::Borrowed("a=1; b=2")));
        assert_eq!(merged[1].1, "1, 2");
    }

    #[test]
    fn url_helpers() {
        let p_req = req("GET", "http://[::1]:80/a?b[]=1", &[], None);
        let p = prepare(&p_req, true);
        assert!(p.url_needs_globoff());
        let p_req = req("GET", "http://[::1]:80/a", &[], None);
        let p = prepare(&p_req, true);
        assert!(!p.url_needs_globoff());
        let p_req = req("GET", "http://x/a/../b", &[], None);
        let p = prepare(&p_req, true);
        assert!(p.url_has_dot_segments());
        let p_req = req("GET", "http://x/a/..b?c=/../", &[], None);
        let p = prepare(&p_req, true);
        assert!(!p.url_has_dot_segments());
    }

    #[test]
    fn strict_urls() {
        for (url, expected) in [
            (
                "https://example.com/a/b?c=d&e=f#g",
                "https://example.com/a/b?c=d&e=f#g",
            ),
            ("http://[::1]:8080/x", "http://[::1]:8080/x"),
            (
                "http://x/a/../b/[1]?x={y}",
                "http://x/a/../b/%5B1%5D?x=%7By%7D",
            ),
            (
                "http://x/a b?q=\"'`^|\\<>",
                "http://x/a%20b?q=%22'%60%5E%7C%5C%3C%3E",
            ),
            ("http://x/%zz%4?a=%41", "http://x/%25zz%254?a=%41"),
            ("http://x/é?q=✓", "http://x/%C3%A9?q=%E2%9C%93"),
            ("http://x/?a#b#c", "http://x/?a#b%23c"),
            ("http://user:p@ss@host", "http://user:p@ss@host"),
            ("not a url {x}", "not a url {x}"),
        ] {
            assert_eq!(strict_url(url), expected, "{url}");
        }
        assert!(matches!(
            strict_url("https://example.com/ok?x=1"),
            Cow::Borrowed(_)
        ));
    }

    #[test]
    fn non_ascii_header_names_are_unique() {
        let r = req(
            "GET",
            "http://x/",
            &[
                ("X-A", "é"),
                ("x-a", "ü"),
                ("X-B", "ascii"),
                ("X-C", "日本"),
            ],
            None,
        );
        let p = prepare(&r, true);
        assert_eq!(p.non_ascii_header_names(), vec!["X-A", "X-C"]);
    }

    #[test]
    fn method_normalisation() {
        assert_eq!(normalize_method(" post "), "POST");
        assert_eq!(normalize_method(""), "GET");
        assert_eq!(normalize_method("purge"), "purge");
        assert_eq!(normalize_method("PROPFIND"), "PROPFIND");
    }
}
