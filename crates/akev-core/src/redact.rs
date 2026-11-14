//! Secret redaction for exports and for what AI agents (MCP) see.
//!
//! One shared, conservative redactor: values become [`REDACTED`] while the structure stays
//! intact (header names, cookie names, query parameter names, JSON keys, the auth scheme),
//! so a redacted export still shows what was sent.
//!
//! * **Headers** (always): `Authorization`, `Proxy-Authorization` (scheme kept), `Cookie`
//!   (cookie names kept), `Set-Cookie` (name and attributes kept), `X-Api-Key`, `Api-Key`,
//!   `X-Auth-Token`, the Google/Anthropic/OpenAI/AWS key and token headers, and any header
//!   named `*-api-key`, `*-token` or `*-secret`.
//! * **URLs and query strings** (always): the password of `user:password@`, and parameters
//!   whose name has a word like `token`, `key`, `secret`, `password`, `sig`/`signature`,
//!   `auth`, `credential` (`access_token`, `apiKey`, `X-Amz-Signature`, `client_secret`...).
//! * **Text bodies** (best effort, cheap patterns only): `Bearer <token>`, JWTs, `sk-...`
//!   API keys (OpenAI, Anthropic), Stripe secret keys, and string values of JSON fields whose
//!   name looks secret (`"access_token": "..."`). Form bodies use the query string rules.
//!   Patterns are deliberately narrow: a false positive that changes a body is worse than a
//!   miss.

use crate::http::{Headers, RequestHead, ResponseHead};
use std::borrow::Cow;

/// What a redacted value is replaced with.
pub const REDACTED: &str = "[redacted]";

/// Header names (lowercase) whose values are always secret.
const SECRET_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "apikey",
    "x-auth-token",
    "x-goog-api-key",
    "anthropic-api-key",
    "openai-api-key",
    "x-amz-security-token",
    "x-csrf-token",
    "x-xsrf-token",
];

/// Header name suffixes (lowercase) that mark a secret header.
const SECRET_HEADER_SUFFIXES: &[&str] = &["-api-key", "-token", "-secret"];

/// Whether a header's value is a secret (case-insensitive name).
pub fn is_secret_header(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    SECRET_HEADERS.contains(&name.as_str())
        || SECRET_HEADER_SUFFIXES.iter().any(|s| name.ends_with(s))
}

/// The redacted form of a header value, or `None` when the header isn't secret.
pub fn header_value(name: &str, value: &str) -> Option<String> {
    if !is_secret_header(name) {
        return None;
    }
    let lower = name.trim().to_ascii_lowercase();
    Some(match lower.as_str() {
        "authorization" | "proxy-authorization" => match value.trim().split_once(' ') {
            Some((scheme, _)) if !scheme.is_empty() => format!("{scheme} {REDACTED}"),
            _ => REDACTED.to_string(),
        },
        "cookie" => value
            .split(';')
            .map(|pair| match pair.split_once('=') {
                Some((name, _)) => format!("{}={REDACTED}", name.trim()),
                None => REDACTED.to_string(),
            })
            .collect::<Vec<_>>()
            .join("; "),
        "set-cookie" => {
            let (first, attributes) = value.split_once(';').unwrap_or((value, ""));
            let first = match first.split_once('=') {
                Some((name, _)) => format!("{}={REDACTED}", name.trim()),
                None => REDACTED.to_string(),
            };
            if attributes.is_empty() {
                first
            } else {
                format!("{first};{attributes}")
            }
        }
        _ => REDACTED.to_string(),
    })
}

/// Redact secret header values in place. Returns whether anything changed.
pub fn headers(headers: &mut Headers) -> bool {
    let mut changed = false;
    for (name, value) in headers.0.iter_mut() {
        if let Some(redacted) = header_value(name, value)
            && redacted != *value
        {
            *value = redacted;
            changed = true;
        }
    }
    changed
}

/// Words of a parameter / field name: split on non-alphanumerics and camelCase humps,
/// lowercased (`X-Amz-Signature` → `x amz signature`, `apiKey` → `api key`).
fn name_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if c.is_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        current.extend(c.to_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

/// Whether a query parameter, form field or JSON key names a secret: its last word is a
/// secret noun (`access_token`, `apiKey`, `X-Amz-Signature`, but not `token_type` or
/// `key_id`), or any word is one that is always secret (`password_confirmation`).
pub fn is_secret_name(name: &str) -> bool {
    /// Secret when the name ends with them.
    const LAST: &[&str] = &[
        "token",
        "key",
        "apikey",
        "sig",
        "signature",
        "auth",
        "jwt",
        "sessionid",
    ];
    /// Secret wherever they appear.
    const ANY: &[&str] = &[
        "secret",
        "password",
        "passwd",
        "pwd",
        "credential",
        "credentials",
    ];
    /// Run-together words: `accesstoken`, `clientsecret`.
    const SUFFIXES: &[&str] = &["token", "secret", "password", "apikey", "passwd"];
    let words = name_words(name);
    let suffixed = |w: &str| SUFFIXES.iter().any(|s| w.len() > s.len() && w.ends_with(s));
    words
        .last()
        .is_some_and(|w| LAST.contains(&w.as_str()) || suffixed(w))
        || words.iter().any(|w| ANY.contains(&w.as_str()))
}

/// Redact secret parameters of a query string (or form body): `a=1&token=x` →
/// `a=1&token=[redacted]`. Order, separators and other values are kept byte for byte.
pub fn query(query: &str) -> Cow<'_, str> {
    let mut out: Option<String> = None;
    let mut start = 0;
    for (i, piece) in split_keep(query, b'&') {
        let (name, has_value) = match piece.split_once('=') {
            Some((name, value)) => (name, !value.is_empty()),
            None => (piece, false),
        };
        let decoded = percent_decode(name);
        if has_value && is_secret_name(&decoded) {
            let buf = out.get_or_insert_with(|| String::with_capacity(query.len()));
            buf.push_str(&query[start..i]);
            buf.push_str(name);
            buf.push('=');
            buf.push_str(REDACTED);
            start = i + piece.len();
        }
    }
    match out {
        Some(mut buf) => {
            buf.push_str(&query[start..]);
            Cow::Owned(buf)
        }
        None => Cow::Borrowed(query),
    }
}

/// `(offset, piece)` of each `sep`-separated piece.
fn split_keep(s: &str, sep: u8) -> impl Iterator<Item = (usize, &str)> {
    let mut offset = 0;
    s.split(char::from(sep)).map(move |piece| {
        let at = offset;
        offset += piece.len() + 1;
        (at, piece)
    })
}

/// Lenient `%XX` / `+` decoding of a parameter name (for matching only).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
                match hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                    Some(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    None => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Redact the query of a path (`/a?token=x` → `/a?token=[redacted]`).
pub fn path(path: &str) -> Cow<'_, str> {
    let Some((before, rest)) = path.split_once('?') else {
        return Cow::Borrowed(path);
    };
    let (q, fragment) = match rest.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (rest, None),
    };
    match query(q) {
        Cow::Borrowed(_) => Cow::Borrowed(path),
        Cow::Owned(q) => {
            let mut out = format!("{before}?{q}");
            if let Some(f) = fragment {
                out.push('#');
                out.push_str(f);
            }
            Cow::Owned(out)
        }
    }
}

/// Redact a URL: the password of `user:password@` and secret query parameters.
pub fn url(url: &str) -> Cow<'_, str> {
    let mut current: Cow<'_, str> = Cow::Borrowed(url);
    if let Some(scheme_end) = url.find("://") {
        let authority_start = scheme_end + 3;
        let rest = &url[authority_start..];
        let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        if let Some(at) = authority.rfind('@')
            && let Some(colon) = authority[..at].find(':')
            && colon + 1 < at
        {
            let mut out = String::with_capacity(url.len());
            out.push_str(&url[..authority_start + colon + 1]);
            out.push_str(REDACTED);
            out.push_str(&url[authority_start + at..]);
            current = Cow::Owned(out);
        }
    }
    match path(&current) {
        Cow::Borrowed(_) => current,
        Cow::Owned(p) => Cow::Owned(p),
    }
}

/// Redact a request head: secret headers, URL and path query.
pub fn request_head(head: &mut RequestHead) {
    headers(&mut head.headers);
    if let Cow::Owned(u) = url(&head.url) {
        head.url = u;
    }
    if let Cow::Owned(p) = path(&head.path) {
        head.path = p;
    }
}

/// Redact a response head's secret headers (`Set-Cookie`...).
pub fn response_head(head: &mut ResponseHead) {
    headers(&mut head.headers);
}

/// Redact a body given its (decoded) bytes and `Content-Type`. Only text is touched; returns
/// `None` when nothing changed (or the body isn't text).
pub fn body(bytes: &[u8], content_type: Option<&str>) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let essence = content_type
        .and_then(|c| c.split(';').next())
        .map(|c| c.trim().to_ascii_lowercase());
    let redacted = if essence.as_deref() == Some("application/x-www-form-urlencoded") {
        query(text)
    } else {
        self::text(text)
    };
    match redacted {
        Cow::Owned(s) => Some(s.into_bytes()),
        Cow::Borrowed(_) => None,
    }
}

/// Redact obvious secrets in free text (bodies, WebSocket messages, search snippets):
/// `Bearer <token>`, JWTs, `sk-...` / Stripe keys and secret-named JSON string fields.
pub fn text(text: &str) -> Cow<'_, str> {
    let mut current: Cow<'_, str> = Cow::Borrowed(text);
    for pass in [json_fields, bearer_tokens, jwts, api_keys] {
        if let Some(next) = pass(&current) {
            current = Cow::Owned(next);
        }
    }
    current
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Replace `ranges` (sorted, disjoint byte ranges on ASCII boundaries) with [`REDACTED`].
fn replace_ranges(text: &str, ranges: &[(usize, usize)]) -> Option<String> {
    if ranges.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for &(start, end) in ranges {
        out.push_str(&text[last..start]);
        out.push_str(REDACTED);
        last = end;
    }
    out.push_str(&text[last..]);
    Some(out)
}

/// `"name" : "value"` where `name` looks secret: the value's characters.
fn json_fields(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'"' {
            i += 1;
            continue;
        }
        // A key: "..." (escapes skipped).
        let Some(key_end) = string_end(b, i + 1) else {
            break;
        };
        let key = &text[i + 1..key_end];
        let mut j = key_end + 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        if j < b.len() && b[j] == b':' {
            j += 1;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == b'"' {
                let Some(value_end) = string_end(b, j + 1) else {
                    break;
                };
                if value_end > j + 1 && is_secret_name(key) && &text[j + 1..value_end] != REDACTED {
                    ranges.push((j + 1, value_end));
                }
                i = value_end + 1;
                continue;
            }
        }
        i = key_end + 1;
    }
    replace_ranges(text, &ranges)
}

/// Index of the closing quote of a JSON string starting at `from` (after the opening quote).
fn string_end(b: &[u8], from: usize) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            b'"' => return Some(i),
            b'\n' => return None,
            _ => i += 1,
        }
    }
    None
}

/// `Bearer <token>` (case-insensitive scheme, token of 8+ token68 characters).
fn bearer_tokens(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 7 < b.len() {
        if b[i..i + 6].eq_ignore_ascii_case(b"bearer")
            && (i == 0 || !b[i - 1].is_ascii_alphanumeric())
            && b[i + 6] == b' '
        {
            let mut start = i + 6;
            while start < b.len() && b[start] == b' ' {
                start += 1;
            }
            let mut end = start;
            while end < b.len() && (b[end].is_ascii_alphanumeric() || b"-._~+/=".contains(&b[end]))
            {
                end += 1;
            }
            if end - start >= 8 && &text[start..end] != REDACTED {
                ranges.push((start, end));
            }
            i = end.max(i + 1);
            continue;
        }
        i += 1;
    }
    replace_ranges(text, &ranges)
}

fn base64url_run(b: &[u8], from: usize) -> usize {
    let mut i = from;
    while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-' || b[i] == b'_') {
        i += 1;
    }
    i
}

/// JWTs: `eyJ<header>.eyJ<payload>.<signature>` (base64url segments).
fn jwts(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 3 <= b.len() {
        if &b[i..i + 3] == b"eyJ" && (i == 0 || !is_word_byte(b[i - 1])) {
            let header_end = base64url_run(b, i);
            if header_end - i >= 10
                && header_end + 4 < b.len()
                && b[header_end] == b'.'
                && &b[header_end + 1..header_end + 4] == b"eyJ"
            {
                let payload_end = base64url_run(b, header_end + 1);
                if payload_end - header_end > 10 && payload_end < b.len() && b[payload_end] == b'.'
                {
                    let end = base64url_run(b, payload_end + 1);
                    ranges.push((i, end));
                    i = end;
                    continue;
                }
            }
            i = header_end.max(i + 1);
            continue;
        }
        i += 1;
    }
    replace_ranges(text, &ranges)
}

/// `sk-...` keys (OpenAI, Anthropic `sk-ant-...`: 20+ key characters) and Stripe secret /
/// restricted keys (`sk_live_`, `sk_test_`, `rk_live_`, `rk_test_`).
fn api_keys(text: &str) -> Option<String> {
    let b = text.as_bytes();
    let mut ranges = Vec::new();
    let mut i = 0;
    while i + 3 < b.len() {
        let boundary = i == 0 || !is_word_byte(b[i - 1]);
        if boundary && &b[i..i + 3] == b"sk-" {
            let end = base64url_run(b, i + 3);
            if end - (i + 3) >= 20 {
                ranges.push((i, end));
                i = end;
                continue;
            }
        }
        if boundary && i + 8 <= b.len() {
            let prefix = &b[i..i + 8];
            if [b"sk_live_", b"sk_test_", b"rk_live_", b"rk_test_"]
                .iter()
                .any(|p| prefix == *p)
            {
                let end = base64url_run(b, i + 8);
                if end - (i + 8) >= 16 {
                    ranges.push((i, end));
                    i = end;
                    continue;
                }
            }
        }
        i += 1;
    }
    replace_ranges(text, &ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secret_headers() {
        for name in [
            "Authorization",
            "proxy-authorization",
            "Cookie",
            "Set-Cookie",
            "X-Api-Key",
            "api-key",
            "X-Auth-Token",
            "x-goog-api-key",
            "X-Amz-Security-Token",
            "x-my-service-secret",
        ] {
            assert!(is_secret_header(name), "{name}");
        }
        for name in [
            "Content-Type",
            "Accept",
            "X-Request-Id",
            "Keep-Alive",
            "Host",
        ] {
            assert!(!is_secret_header(name), "{name}");
        }
    }

    #[test]
    fn header_values_keep_their_structure() {
        assert_eq!(
            header_value("Authorization", "Bearer abc.def").as_deref(),
            Some("Bearer [redacted]")
        );
        assert_eq!(
            header_value("authorization", "opaque").as_deref(),
            Some("[redacted]")
        );
        assert_eq!(
            header_value("Cookie", "sid=1; theme=dark").as_deref(),
            Some("sid=[redacted]; theme=[redacted]")
        );
        assert_eq!(
            header_value("Set-Cookie", "sid=abc; Path=/; HttpOnly").as_deref(),
            Some("sid=[redacted]; Path=/; HttpOnly")
        );
        assert_eq!(
            header_value("x-api-key", "sk-ant-123").as_deref(),
            Some("[redacted]")
        );
        assert_eq!(header_value("Accept", "*/*"), None);

        let mut h = Headers::new();
        h.append("Accept", "*/*");
        h.append("Authorization", "Basic dXNlcjpwYXNz");
        assert!(headers(&mut h));
        assert_eq!(h.get("accept"), Some("*/*"));
        assert_eq!(h.get("authorization"), Some("Basic [redacted]"));
        assert!(!headers(&mut h), "idempotent");
    }

    #[test]
    fn secret_names() {
        for name in [
            "token",
            "access_token",
            "apiKey",
            "api_key",
            "apikey",
            "key",
            "client_secret",
            "password",
            "sig",
            "X-Amz-Signature",
            "auth",
            "refreshToken",
            "id_token",
            "accesstoken",
            "password_confirmation",
        ] {
            assert!(is_secret_name(name), "{name}");
        }
        for name in [
            "q",
            "page",
            "monkey",
            "author",
            "keyboard",
            "signal",
            "design",
            "tokens_used",
            "token_type",
            "key_id",
            "max_tokens",
        ] {
            assert!(!is_secret_name(name), "{name}");
        }
    }

    #[test]
    fn queries_paths_and_urls() {
        assert_eq!(query("a=1&token=abc&b=2"), "a=1&token=[redacted]&b=2");
        assert_eq!(query("api%5Fkey=x"), "api%5Fkey=[redacted]");
        assert!(matches!(query("a=1&b=2"), Cow::Borrowed(_)));
        assert_eq!(query("token="), "token=", "empty values stay empty");
        assert_eq!(query("token"), "token");
        assert_eq!(
            path("/v1/items?key=AIza123&q=cats#top"),
            "/v1/items?key=[redacted]&q=cats#top"
        );
        assert_eq!(path("/plain"), "/plain");
        assert_eq!(
            url("https://user:hunter2@example.com/a?X-Amz-Signature=ff&x=1"),
            "https://user:[redacted]@example.com/a?X-Amz-Signature=[redacted]&x=1"
        );
        assert_eq!(
            url("https://user@example.com/"),
            "https://user@example.com/"
        );
        assert_eq!(url("http://example.com/?q=1"), "http://example.com/?q=1");

        let mut head = RequestHead {
            method: "GET".into(),
            url: "https://api.test/x?access_token=t0k".into(),
            scheme: "https".into(),
            host: "api.test".into(),
            port: 443,
            path: "/x?access_token=t0k".into(),
            http_version: crate::http::HttpVersion::Http11,
            headers: Headers::new(),
        };
        head.headers.append("X-Api-Key", "secret");
        request_head(&mut head);
        assert_eq!(head.url, "https://api.test/x?access_token=[redacted]");
        assert_eq!(head.path, "/x?access_token=[redacted]");
        assert_eq!(head.headers.get("x-api-key"), Some("[redacted]"));
    }

    #[test]
    fn text_patterns() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        assert_eq!(text(&format!("token: {jwt} end")), "token: [redacted] end");
        assert_eq!(
            text("curl -H 'Authorization: Bearer abcdef123456'"),
            "curl -H 'Authorization: Bearer [redacted]'"
        );
        assert_eq!(
            text("bearer short"),
            "bearer short",
            "too short to be a token"
        );
        assert_eq!(
            text("key sk-proj-abcdefghijklmnopqrstuvwx and sk-ant-api03-abcdefghijklmnopqrstu"),
            "key [redacted] and [redacted]"
        );
        assert_eq!(
            text("ask-me-anything-please-now-ok"),
            "ask-me-anything-please-now-ok"
        );
        assert_eq!(
            text("stripe sk_live_abcdefghijklmnop1234"),
            "stripe [redacted]"
        );
        assert_eq!(
            text(r#"{"access_token": "abc\"def", "token_type": "bearer", "expires_in": 3600}"#),
            r#"{"access_token": "[redacted]", "token_type": "bearer", "expires_in": 3600}"#
        );
        assert_eq!(
            text(r#"{"password":"p","user":"me","apiKey":"k","n":{"client_secret":"s"}}"#),
            r#"{"password":"[redacted]","user":"me","apiKey":"[redacted]","n":{"client_secret":"[redacted]"}}"#
        );
        // Ordinary text and JSON pass through untouched (borrowed).
        for plain in [
            r#"{"name":"Akev","tokens":["a","b"],"key":12}"#,
            "hello world",
            "eyJ but not a jwt",
            "",
        ] {
            assert!(matches!(text(plain), Cow::Borrowed(_)), "{plain}");
        }
        // Multi-byte text around matches is kept intact.
        assert_eq!(text("é Bearer abcdefgh123 ü"), "é Bearer [redacted] ü");
        assert_eq!(text(&text(&format!("Bearer {jwt}"))), "Bearer [redacted]");
    }

    #[test]
    fn bodies_by_content_type() {
        assert_eq!(
            body(
                b"user=me&password=pw",
                Some("application/x-www-form-urlencoded; charset=utf-8")
            ),
            Some(b"user=me&password=[redacted]".to_vec())
        );
        assert_eq!(
            body(br#"{"token":"t"}"#, Some("application/json")),
            Some(br#"{"token":"[redacted]"}"#.to_vec())
        );
        assert_eq!(body(b"nothing here", Some("text/plain")), None);
        assert_eq!(
            body(&[0xff, 0xfe, 0x00], None),
            None,
            "binary is left alone"
        );
    }
}
