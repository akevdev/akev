//! Pure request/response transforms.
//!
//! The user-facing semantics are documented on [`crate::apply_request_transform`] and
//! [`crate::apply_response_transform`].

use crate::json::{Doc, apply_patch_doc, merge_patch_doc, validate_patch};
use crate::urlutil::{rebuild_url, set_request_url};
use crate::{Result, RulesError, resolve_body};
use akev_core::http::canonical_reason;
use akev_core::rules::{BodyTransform, MatchReplace, RequestTransform, ResponseTransform};
use akev_core::{Headers, RequestHead, ResponseHead};
use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

// ------------------------------------------------------------------------------------------
// Regex cache (transforms receive raw rule data, so compiled regexes are memoized)
// ------------------------------------------------------------------------------------------

const REGEX_CACHE_CAPACITY: usize = 512;

type RegexKey = (String, bool, bool);
static REGEX_CACHE: LazyLock<Mutex<HashMap<RegexKey, Arc<regex::bytes::Regex>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// The compiled (bytes) regex for a find/replace entry, cached.
fn replace_regex(mr: &MatchReplace) -> std::result::Result<Arc<regex::bytes::Regex>, String> {
    let key = (mr.find.clone(), mr.regex, mr.case_sensitive);
    if let Some(re) = REGEX_CACHE.lock().get(&key) {
        return Ok(Arc::clone(re));
    }
    let src = if mr.regex {
        mr.find.clone()
    } else {
        regex::escape(&mr.find)
    };
    let re = regex::bytes::RegexBuilder::new(&src)
        .case_insensitive(!mr.case_sensitive)
        .size_limit(4 << 20)
        .build()
        .map_err(|e| format!("invalid regex `{}`: {e}", mr.find))?;
    let re = Arc::new(re);
    let mut cache = REGEX_CACHE.lock();
    if cache.len() >= REGEX_CACHE_CAPACITY {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&re));
    Ok(re)
}

/// Apply find/replace entries in order to bytes. Returns `None` when nothing changed.
fn apply_replacements(
    input: &[u8],
    replacements: &[MatchReplace],
) -> std::result::Result<Option<Vec<u8>>, String> {
    let mut current: Option<Vec<u8>> = None;
    for mr in replacements {
        if mr.find.is_empty() {
            continue;
        }
        let re = replace_regex(mr)?;
        let src: &[u8] = current.as_deref().unwrap_or(input);
        let replaced = if mr.regex {
            re.replace_all(src, mr.replace.as_bytes())
        } else {
            re.replace_all(src, regex::bytes::NoExpand(mr.replace.as_bytes()))
        };
        if let std::borrow::Cow::Owned(v) = replaced {
            current = Some(v);
        }
    }
    Ok(current)
}

// ------------------------------------------------------------------------------------------
// Validation (at rule compile time)
// ------------------------------------------------------------------------------------------

/// RFC 9110 token characters.
fn is_tchar(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

pub(crate) fn validate_header_name(name: &str) -> std::result::Result<(), String> {
    if name.is_empty() || !name.bytes().all(is_tchar) {
        return Err(format!("invalid header name `{name}`"));
    }
    Ok(())
}

pub(crate) fn validate_header_value(name: &str, value: &str) -> std::result::Result<(), String> {
    if value.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
        return Err(format!(
            "invalid value for header `{name}` (contains CR, LF or NUL)"
        ));
    }
    Ok(())
}

pub(crate) fn validate_headers(headers: &Headers) -> std::result::Result<(), String> {
    for (k, v) in headers.iter() {
        validate_header_name(k)?;
        validate_header_value(k, v)?;
    }
    Ok(())
}

fn validate_body(t: &BodyTransform) -> std::result::Result<(), String> {
    match t {
        BodyTransform::Replace { body } => crate::respond::validate_body_source(body),
        BodyTransform::MatchReplace { replacements } => replacements
            .iter()
            .try_for_each(|mr| replace_regex(mr).map(drop)),
        BodyTransform::JsonMerge { .. } => Ok(()),
        BodyTransform::JsonPatch { operations } => validate_patch(operations),
    }
}

/// Validate request/response transforms (regexes, header names, JSON patches...).
pub(crate) fn validate_transforms(
    req: Option<&RequestTransform>,
    res: Option<&ResponseTransform>,
) -> std::result::Result<(), String> {
    if let Some(t) = req {
        if let Some(m) = &t.method
            && (m.is_empty() || !m.bytes().all(is_tchar))
        {
            return Err(format!("invalid method `{m}`"));
        }
        for mr in &t.url_replace {
            replace_regex(mr)?;
        }
        validate_headers(&t.set_headers)?;
        validate_headers(&t.add_headers)?;
        if let Some(b) = &t.body {
            validate_body(b)?;
        }
    }
    if let Some(t) = res {
        if let Some(s) = t.status
            && !(100..=999).contains(&s)
        {
            return Err(format!("invalid HTTP status {s}"));
        }
        if let Some(msg) = &t.status_message {
            validate_header_value("status message", msg)?;
        }
        validate_headers(&t.set_headers)?;
        validate_headers(&t.add_headers)?;
        if let Some(b) = &t.body {
            validate_body(b)?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------
// Header helpers
// ------------------------------------------------------------------------------------------

/// Replace all values of `name` with one value, at the first occurrence's position, using the
/// given casing; append when absent.
pub(crate) fn set_header(headers: &mut Headers, name: &str, value: &str) {
    match headers
        .0
        .iter()
        .position(|(k, _)| k.eq_ignore_ascii_case(name))
    {
        Some(pos) => {
            headers.0[pos] = (name.to_string(), value.to_string());
            let mut i = pos + 1;
            while i < headers.0.len() {
                if headers.0[i].0.eq_ignore_ascii_case(name) {
                    headers.0.remove(i);
                } else {
                    i += 1;
                }
            }
        }
        None => headers.append(name, value),
    }
}

/// `setHeaders` semantics: every header named in `set` replaces all existing values of that
/// name (at the first one's position, with `set`'s casing); a name listed several times in
/// `set` produces several lines, in order.
pub(crate) fn set_headers(headers: &mut Headers, set: &Headers) {
    let mut done: Vec<&str> = Vec::new();
    for (name, value) in set.iter() {
        if done.iter().any(|d| d.eq_ignore_ascii_case(name)) {
            // Further values for a header set here become extra lines.
            let last = headers
                .0
                .iter()
                .rposition(|(k, _)| k.eq_ignore_ascii_case(name));
            let at = last.map_or(headers.0.len(), |p| p + 1);
            headers.0.insert(at, (name.to_string(), value.to_string()));
        } else {
            set_header(headers, name, value);
            done.push(name);
        }
    }
}

fn apply_header_ops(headers: &mut Headers, remove: &[String], set: &Headers, add: &Headers) {
    for name in remove {
        headers.remove(name);
    }
    set_headers(headers, set);
    for (name, value) in add.iter() {
        headers.append(name, value);
    }
}

// ------------------------------------------------------------------------------------------
// Body transforms
// ------------------------------------------------------------------------------------------

/// The body as a number-preserving JSON document (an empty body is `null`).
fn parse_json_body(body: &[u8]) -> Result<Doc> {
    if body.iter().all(u8::is_ascii_whitespace) {
        return Ok(Doc::Null);
    }
    Doc::parse(body).map_err(|e| RulesError::Transform(format!("body is not valid JSON: {e}")))
}

pub(crate) fn apply_body_transform(body: &mut Bytes, t: &BodyTransform) -> Result<()> {
    match t {
        BodyTransform::Replace { body: src } => *body = resolve_body(src)?,
        BodyTransform::MatchReplace { replacements } => {
            if let Some(out) =
                apply_replacements(body, replacements).map_err(RulesError::Transform)?
            {
                *body = Bytes::from(out);
            }
        }
        BodyTransform::JsonMerge { patch } => {
            let mut doc = parse_json_body(body)?;
            merge_patch_doc(&mut doc, patch);
            *body = Bytes::from(doc.to_vec());
        }
        BodyTransform::JsonPatch { operations } => {
            let mut doc = parse_json_body(body)?;
            apply_patch_doc(&mut doc, operations).map_err(RulesError::Transform)?;
            *body = Bytes::from(doc.to_vec());
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------
// Request / response
// ------------------------------------------------------------------------------------------

/// Implementation of [`crate::apply_request_transform`].
pub(crate) fn apply_request(
    head: &mut RequestHead,
    body: &mut Bytes,
    t: &RequestTransform,
) -> Result<()> {
    if let Some(m) = &t.method {
        head.method = m.trim().to_ascii_uppercase();
    }
    if !t.url_replace.is_empty()
        && let Some(new_url) = apply_replacements(head.url.as_bytes(), &t.url_replace)
            .map_err(RulesError::Transform)?
    {
        let new_url = String::from_utf8(new_url).map_err(|_| {
            RulesError::Transform("URL is not valid UTF-8 after replacement".to_string())
        })?;
        set_request_url(head, &new_url, false).map_err(RulesError::Transform)?;
    }
    if !t.set_query.is_empty() {
        let (path, query) = match head.path.split_once('?') {
            Some((p, q)) => (p.to_string(), Some(q.to_string())),
            None => (head.path.clone(), None),
        };
        let new_query = set_query_params(query.as_deref(), &t.set_query);
        head.path = match new_query {
            Some(q) => format!("{path}?{q}"),
            None => path,
        };
        rebuild_url(head);
    }
    apply_header_ops(
        &mut head.headers,
        &t.remove_headers,
        &t.set_headers,
        &t.add_headers,
    );
    if let Some(b) = &t.body {
        apply_body_transform(body, b)?;
    }
    Ok(())
}

/// Implementation of [`crate::apply_response_transform`].
pub(crate) fn apply_response(
    head: &mut ResponseHead,
    body: &mut Bytes,
    t: &ResponseTransform,
) -> Result<()> {
    if let Some(status) = t.status {
        if status != head.status && t.status_message.is_none() {
            canonical_reason(status).clone_into(&mut head.status_message);
        }
        head.status = status;
    }
    if let Some(msg) = &t.status_message {
        msg.clone_into(&mut head.status_message);
    }
    apply_header_ops(
        &mut head.headers,
        &t.remove_headers,
        &t.set_headers,
        &t.add_headers,
    );
    if let Some(b) = &t.body {
        apply_body_transform(body, b)?;
    }
    Ok(())
}

fn encode_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

fn decode_component(s: &str) -> String {
    url::form_urlencoded::parse(format!("{s}=").as_bytes())
        .next()
        .map(|(k, _)| k.into_owned())
        .unwrap_or_default()
}

/// Apply `set_query` edits to a raw query string. Returns `None` when the query ends up empty
/// (and there was none to begin with or everything was removed).
fn set_query_params(query: Option<&str>, edits: &[(String, Option<String>)]) -> Option<String> {
    let mut pairs: Vec<String> = query
        .map(|q| {
            q.split('&')
                .filter(|p| !p.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    for (key, value) in edits {
        let key_of = |p: &str| decode_component(p.split_once('=').map_or(p, |(k, _)| k));
        let first = pairs.iter().position(|p| key_of(p) == *key);
        let encoded = value
            .as_ref()
            .map(|v| format!("{}={}", encode_component(key), encode_component(v)));
        match (first, encoded) {
            (Some(pos), Some(enc)) => {
                pairs[pos] = enc;
                let mut i = pos + 1;
                while i < pairs.len() {
                    if key_of(&pairs[i]) == *key {
                        pairs.remove(i);
                    } else {
                        i += 1;
                    }
                }
            }
            (None, Some(enc)) => pairs.push(enc),
            (_, None) => pairs.retain(|p| key_of(p) != *key),
        }
    }
    if pairs.is_empty() && query.is_none_or(|q| !q.is_empty()) {
        None
    } else {
        Some(pairs.join("&"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_edits() {
        let set = |k: &str, v: Option<&str>| (k.to_string(), v.map(str::to_string));
        assert_eq!(
            set_query_params(Some("a=1&b=2&a=3"), &[set("a", Some("x"))]).as_deref(),
            Some("a=x&b=2")
        );
        assert_eq!(
            set_query_params(Some("a=1&b=2"), &[set("c", Some("x y"))]).as_deref(),
            Some("a=1&b=2&c=x+y")
        );
        assert_eq!(
            set_query_params(Some("a=1&b=2"), &[set("a", None)]).as_deref(),
            Some("b=2")
        );
        assert_eq!(set_query_params(Some("a=1"), &[set("a", None)]), None);
        assert_eq!(
            set_query_params(None, &[set("k", Some("v&w"))]).as_deref(),
            Some("k=v%26w")
        );
        assert_eq!(set_query_params(None, &[set("k", None)]), None);
        // Encoded keys are matched decoded; untouched params keep their exact bytes.
        assert_eq!(
            set_query_params(Some("my%20key=1&z=%7E"), &[set("my key", Some("2"))]).as_deref(),
            Some("my+key=2&z=%7E")
        );
        assert_eq!(
            set_query_params(Some("flag&x=1"), &[set("flag", None)]).as_deref(),
            Some("x=1")
        );
    }

    #[test]
    fn header_ops_order_and_casing() {
        let mut h = Headers(vec![
            ("host".into(), "a".into()),
            ("x-a".into(), "1".into()),
            ("Accept".into(), "*/*".into()),
            ("X-A".into(), "2".into()),
        ]);
        let set = Headers(vec![
            ("X-A".into(), "new".into()),
            ("Set-Cookie".into(), "a=1".into()),
            ("set-cookie".into(), "b=2".into()),
        ]);
        let add = Headers(vec![("Accept".into(), "text/html".into())]);
        apply_header_ops(&mut h, &["HOST".to_string()], &set, &add);
        assert_eq!(
            h.0,
            vec![
                ("X-A".to_string(), "new".to_string()),
                ("Accept".to_string(), "*/*".to_string()),
                ("Set-Cookie".to_string(), "a=1".to_string()),
                ("set-cookie".to_string(), "b=2".to_string()),
                ("Accept".to_string(), "text/html".to_string()),
            ]
        );
    }

    #[test]
    fn header_validation() {
        assert!(validate_header_name("X-Custom_Header").is_ok());
        assert!(validate_header_name("").is_err());
        assert!(validate_header_name("bad name").is_err());
        assert!(validate_header_name(":authority").is_err());
        assert!(validate_header_value("x", "ok value").is_ok());
        assert!(validate_header_value("x", "evil\r\nInjected: 1").is_err());
    }

    #[test]
    fn replacements_are_byte_safe() {
        let input = b"\xff\xfeprefix foo \x00 foo";
        let out = apply_replacements(
            input,
            &[MatchReplace {
                find: "foo".into(),
                replace: "bar".into(),
                regex: false,
                case_sensitive: true,
            }],
        )
        .unwrap()
        .unwrap();
        assert_eq!(out, b"\xff\xfeprefix bar \x00 bar");
        assert!(
            apply_replacements(
                input,
                &[MatchReplace {
                    find: "zzz".into(),
                    replace: "y".into(),
                    regex: false,
                    case_sensitive: true
                }]
            )
            .unwrap()
            .is_none()
        );
    }
}
