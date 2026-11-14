//! Locally generated responses (mock, map-local) and forwarding targets.

use crate::urlutil::{
    default_port, rebuild_url, split_authority, split_url, strip_path_prefix, update_host_header,
};
use crate::{MapLocalOutcome, Result, RulesError};
use akev_core::http::canonical_reason;
use akev_core::rules::{Action, BodySource};
use akev_core::{Headers, RequestHead, ResponseHead};
use base64::Engine;
use bytes::Bytes;
use percent_encoding::percent_decode_str;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

// ------------------------------------------------------------------------------------------
// Body sources
// ------------------------------------------------------------------------------------------

/// Expand a leading `~/` to the user's home directory.
pub(crate) fn expand_home(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/").or_else(|| path.strip_prefix("~\\")) {
        let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
        if let Some(home) = home {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// Decode base64 leniently: standard or URL-safe alphabet, with or without padding, ignoring
/// whitespace.
pub(crate) fn decode_base64(data: &str) -> std::result::Result<Vec<u8>, String> {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD};
    let cleaned: String = data.chars().filter(|c| !c.is_ascii_whitespace()).collect();
    [STANDARD, STANDARD_NO_PAD, URL_SAFE, URL_SAFE_NO_PAD]
        .iter()
        .find_map(|engine| engine.decode(&cleaned).ok())
        .ok_or_else(|| "invalid base64 body".to_string())
}

pub(crate) fn validate_body_source(src: &BodySource) -> std::result::Result<(), String> {
    match src {
        BodySource::Base64 { data } => decode_base64(data).map(drop),
        BodySource::File { path } if path.trim().is_empty() => {
            Err("body file path is empty".to_string())
        }
        _ => Ok(()),
    }
}

/// Implementation of [`crate::resolve_body`].
pub(crate) fn resolve(src: &BodySource) -> Result<Bytes> {
    Ok(match src {
        BodySource::Empty => Bytes::new(),
        BodySource::Text { text } => Bytes::copy_from_slice(text.as_bytes()),
        BodySource::Base64 { data } => {
            Bytes::from(decode_base64(data).map_err(RulesError::Transform)?)
        }
        BodySource::Json { value } => Bytes::from(
            serde_json::to_vec(value)
                .map_err(|e| RulesError::Transform(format!("JSON serialization failed: {e}")))?,
        ),
        BodySource::File { path } => Bytes::from(std::fs::read(expand_home(path))?),
    })
}

// ------------------------------------------------------------------------------------------
// Mock responses
// ------------------------------------------------------------------------------------------

fn http_date(t: SystemTime) -> String {
    httpdate::fmt_http_date(t)
}

/// Finish a locally generated response: `Date`, `Content-Length` (always correct unless the
/// headers ask for chunked transfer-encoding), and no body for `HEAD` / 1xx / 204 / 304.
pub(crate) fn finalize_local_response(
    status: u16,
    status_message: Option<&str>,
    headers: Headers,
    body: Bytes,
    request: &RequestHead,
) -> (ResponseHead, Bytes) {
    let (head, send_body) =
        finalize_local_head(status, status_message, headers, body.len() as u64, request);
    (head, if send_body { body } else { Bytes::new() })
}

/// The head of a locally generated response whose body is `body_len` bytes (see
/// [`finalize_local_response`]), and whether the body is sent at all.
pub(crate) fn finalize_local_head(
    status: u16,
    status_message: Option<&str>,
    mut headers: Headers,
    body_len: u64,
    request: &RequestHead,
) -> (ResponseHead, bool) {
    if !headers.contains("date") {
        headers.append("date", http_date(SystemTime::now()));
    }
    let bodiless_status = (100..200).contains(&status) || status == 204 || status == 304;
    let chunked = headers
        .get_all("transfer-encoding")
        .any(|v| v.to_ascii_lowercase().contains("chunked"));
    let mut send_body = true;
    if bodiless_status {
        if status != 304 {
            headers.remove("content-length");
        }
        send_body = false;
    } else if chunked {
        headers.remove("content-length");
    } else {
        let name = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .map_or_else(|| "content-length".to_string(), |(k, _)| k.to_string());
        crate::transform::set_header(&mut headers, &name, &body_len.to_string());
    }
    if request.method.eq_ignore_ascii_case("HEAD") {
        send_body = false;
    }
    let head = ResponseHead {
        status,
        status_message: status_message
            .map_or_else(|| canonical_reason(status).to_string(), str::to_string),
        http_version: request.http_version,
        headers,
    };
    (head, send_body)
}

/// Implementation of [`crate::mock_response`].
pub(crate) fn mock(action: &Action, request: &RequestHead) -> Result<(ResponseHead, Bytes)> {
    let (status, status_message, headers, body_src) = match action {
        Action::Mock {
            status,
            status_message,
            headers,
            body,
        } => (*status, status_message.as_deref(), headers, body),
        Action::WsReject { status, body } => (*status, None, &Headers::new(), body),
        other => {
            return Err(RulesError::Transform(format!(
                "not a mock action: {}",
                other.type_name()
            )));
        }
    };
    let mut headers = headers.clone();
    let body = resolve(body_src)?;
    if !headers.contains("content-type") {
        match body_src {
            BodySource::Json { .. } => headers.append("content-type", "application/json"),
            BodySource::File { path } => {
                if let Some(mime) = mime_guess::from_path(path).first() {
                    headers.append("content-type", with_charset(mime.essence_str()));
                }
            }
            _ => {}
        }
    }
    Ok(finalize_local_response(
        status,
        status_message,
        headers,
        body,
        request,
    ))
}

/// Add `; charset=utf-8` to textual MIME types.
fn with_charset(essence: &str) -> String {
    let textual = essence.starts_with("text/")
        || matches!(
            essence,
            "application/javascript" | "application/json" | "application/xml" | "image/svg+xml"
        );
    if textual {
        format!("{essence}; charset=utf-8")
    } else {
        essence.to_string()
    }
}

// ------------------------------------------------------------------------------------------
// Map local
// ------------------------------------------------------------------------------------------

const DEFAULT_INDEX: &str = "index.html";

/// Why a request couldn't be mapped to a file.
enum Miss {
    /// Ordinary miss (file absent) — fallback allowed.
    NotFound,
    /// Path traversal / invalid path — always a 404, never forwarded.
    Rejected,
}

/// Implementation of [`crate::map_local`].
pub(crate) fn map_local(action: &Action, request: &RequestHead) -> Result<MapLocalOutcome> {
    let Action::MapLocal {
        path,
        strip_prefix,
        index,
        fallback_to_upstream,
        headers,
    } = action
    else {
        return Err(RulesError::Transform(format!(
            "not a map-local action: {}",
            action.type_name()
        )));
    };
    let root = expand_home(path);
    let index = index
        .as_deref()
        .filter(|i| !i.is_empty())
        .unwrap_or(DEFAULT_INDEX);
    match locate(&root, request, strip_prefix.as_deref(), index) {
        Ok(path) => {
            let file = match std::fs::File::open(&path) {
                Ok(f) => f,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(miss(request, *fallback_to_upstream, false));
                }
                Err(e) => return Err(e.into()),
            };
            let meta = file.metadata()?;
            let mut out = Headers::new();
            let mime = mime_guess::from_path(&path).first_or_octet_stream();
            out.append("content-type", with_charset(mime.essence_str()));
            if let Ok(modified) = meta.modified() {
                out.append("last-modified", http_date(modified));
            }
            // The action's headers replace generated ones of the same name; repeated names
            // (Set-Cookie, Link, ...) keep every value.
            crate::transform::set_headers(&mut out, headers);
            if meta.len() >= crate::MAP_LOCAL_STREAM_MIN {
                // Large files are streamed from disk rather than read into memory.
                let (head, send_body) = finalize_local_head(200, None, out, meta.len(), request);
                return Ok(MapLocalOutcome::FoundFile {
                    head,
                    file: send_body.then_some(file),
                    len: meta.len(),
                });
            }
            let mut data = Vec::with_capacity(usize::try_from(meta.len()).unwrap_or(0));
            std::io::Read::read_to_end(&mut &file, &mut data)?;
            let (head, body) = finalize_local_response(200, None, out, Bytes::from(data), request);
            Ok(MapLocalOutcome::Found(head, body))
        }
        Err(Miss::NotFound) => Ok(miss(request, *fallback_to_upstream, false)),
        Err(Miss::Rejected) => Ok(miss(request, false, true)),
    }
}

fn miss(request: &RequestHead, fallback: bool, rejected: bool) -> MapLocalOutcome {
    if fallback {
        return MapLocalOutcome::FallbackToUpstream;
    }
    let mut headers = Headers::new();
    headers.append("content-type", "text/plain; charset=utf-8");
    headers.append("cache-control", "no-store");
    let reason = if rejected {
        "invalid path"
    } else {
        "no such file"
    };
    let body = format!(
        "404 Not Found\n\nAkev map-local: {reason} for {}\n",
        request.path_only()
    );
    let (head, body) = finalize_local_response(404, None, headers, Bytes::from(body), request);
    MapLocalOutcome::NotFound(head, body)
}

/// Resolve the file to serve. The result is canonical and guaranteed to be inside `root`
/// (symlinks included).
fn locate(
    root: &Path,
    request: &RequestHead,
    strip_prefix: Option<&str>,
    index: &str,
) -> std::result::Result<PathBuf, Miss> {
    let meta = std::fs::metadata(root).map_err(|_| Miss::NotFound)?;
    if meta.is_file() {
        return root.canonicalize().map_err(|_| Miss::NotFound);
    }
    let canonical_root = root.canonicalize().map_err(|_| Miss::NotFound)?;
    let raw_path = request.path_only();
    let rel = match strip_prefix {
        Some(prefix) => strip_path_prefix(raw_path, prefix).unwrap_or(raw_path),
        None => raw_path,
    };
    let mut target = canonical_root.clone();
    for segment in rel.split('/') {
        if segment.is_empty() {
            continue;
        }
        let decoded = percent_decode_str(segment)
            .decode_utf8()
            .map_err(|_| Miss::Rejected)?;
        if decoded.contains(['/', '\\', '\0']) || decoded == ".." {
            return Err(Miss::Rejected);
        }
        if decoded == "." {
            continue;
        }
        let mut comps = Path::new(decoded.as_ref()).components();
        match (comps.next(), comps.next()) {
            (Some(Component::Normal(c)), None) => target.push(c),
            _ => return Err(Miss::Rejected),
        }
    }
    if rel.ends_with('/') || rel.is_empty() || target.is_dir() {
        target.push(index);
    }
    let canonical = target.canonicalize().map_err(|_| Miss::NotFound)?;
    if !canonical.starts_with(&canonical_root) {
        return Err(Miss::Rejected);
    }
    if !canonical.is_file() {
        return Err(Miss::NotFound);
    }
    Ok(canonical)
}

// ------------------------------------------------------------------------------------------
// Forwarding
// ------------------------------------------------------------------------------------------

/// A parsed `Action::Forward` target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForwardTarget {
    /// `None` = keep the request's scheme (target given as `host[:port][/path]`).
    pub scheme: Option<String>,
    pub host: String,
    pub port: Option<u16>,
    /// Base path without trailing slash (`""` = none).
    pub base_path: String,
    pub query: Option<String>,
}

/// Parse a forward target: `scheme://host[:port][/base/path][?query]` or `host[:port][/path]`.
pub(crate) fn parse_forward_target(target: &str) -> std::result::Result<ForwardTarget, String> {
    let target = target.trim();
    if target.is_empty() {
        return Err("forward target is empty".to_string());
    }
    let (scheme, host, port, path) = if target.contains("://") {
        let parts =
            split_url(target).ok_or_else(|| format!("invalid forward target `{target}`"))?;
        if default_port(&parts.scheme).is_none() {
            return Err(format!(
                "unsupported forward target scheme `{}`",
                parts.scheme
            ));
        }
        (Some(parts.scheme), parts.host, parts.port, parts.path)
    } else {
        let end = target.find(['/', '?']).unwrap_or(target.len());
        let (host, port) = split_authority(&target[..end])
            .ok_or_else(|| format!("invalid forward target `{target}`"))?;
        if host.is_empty() {
            return Err(format!("invalid forward target `{target}`"));
        }
        let rest = &target[end..];
        let path = if rest.starts_with('?') {
            format!("/{rest}")
        } else {
            rest.to_string()
        };
        (None, host, port, path)
    };
    if host.contains(['/', ' ', '@']) {
        return Err(format!("invalid host in forward target `{target}`"));
    }
    let (base, query) = match path.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (path, None),
    };
    Ok(ForwardTarget {
        scheme,
        host,
        port,
        base_path: base.trim_end_matches('/').to_string(),
        query,
    })
}

/// Implementation of [`crate::forward_request`].
pub(crate) fn forward(action: &Action, request: &RequestHead) -> Result<RequestHead> {
    let Action::Forward {
        target,
        strip_prefix,
        preserve_host,
        ..
    } = action
    else {
        return Err(RulesError::Transform(format!(
            "not a forward action: {}",
            action.type_name()
        )));
    };
    let t = parse_forward_target(target).map_err(RulesError::Transform)?;
    let mut head = request.clone();
    let old_authority = head.authority();
    let websocket = crate::matchers::effective_protocol(request).starts_with("ws");
    let scheme = match t.scheme.as_deref() {
        None => request.scheme.to_ascii_lowercase(),
        Some("http") if websocket => "ws".to_string(),
        Some("https") if websocket => "wss".to_string(),
        Some(s) => s.to_string(),
    };
    let port = t
        .port
        .or_else(|| default_port(&scheme))
        .ok_or_else(|| RulesError::Transform(format!("unsupported scheme `{scheme}`")))?;

    let path_only = request.path_only();
    let remainder = match strip_prefix.as_deref() {
        Some(prefix) => strip_path_prefix(path_only, prefix).unwrap_or(path_only),
        None => path_only,
    };
    let mut path = String::with_capacity(t.base_path.len() + remainder.len() + 1);
    path.push_str(&t.base_path);
    if remainder.is_empty() {
        if path.is_empty() {
            path.push('/');
        }
    } else {
        if !remainder.starts_with('/') {
            path.push('/');
        }
        path.push_str(remainder);
    }
    let query = match (t.query.as_deref(), request.query()) {
        (Some(a), Some(b)) if !a.is_empty() && !b.is_empty() => Some(format!("{a}&{b}")),
        (Some(a), None) => Some(a.to_string()),
        (Some(a), Some("")) => Some(a.to_string()),
        (_, b) => b.map(str::to_string),
    };
    if let Some(q) = query {
        path.push('?');
        path.push_str(&q);
    }

    head.scheme = scheme;
    head.host = t.host;
    head.port = port;
    head.path = path;
    rebuild_url(&mut head);
    if !*preserve_host {
        let new_authority = head.authority();
        if new_authority != old_authority || !head.headers.contains("host") {
            update_host_header(&mut head, &new_authority);
        }
    }
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_target_parsing() {
        let t = parse_forward_target("https://staging.example.com:8443").unwrap();
        assert_eq!(
            t,
            ForwardTarget {
                scheme: Some("https".into()),
                host: "staging.example.com".into(),
                port: Some(8443),
                base_path: "".into(),
                query: None
            }
        );
        let t = parse_forward_target("http://localhost:3000/api/").unwrap();
        assert_eq!((t.port, t.base_path.as_str()), (Some(3000), "/api"));
        let t = parse_forward_target("localhost:3000").unwrap();
        assert_eq!(
            (t.scheme, t.host.as_str(), t.port),
            (None, "localhost", Some(3000))
        );
        let t = parse_forward_target("[::1]:8080/x?y=1").unwrap();
        assert_eq!(
            (t.host.as_str(), t.base_path.as_str(), t.query.as_deref()),
            ("::1", "/x", Some("y=1"))
        );
        assert!(parse_forward_target("").is_err());
        assert!(parse_forward_target("ftp://x").is_err());
        assert!(parse_forward_target("http://").is_err());
        assert!(parse_forward_target("host:notaport").is_err());
    }

    #[test]
    fn base64_is_lenient() {
        assert_eq!(decode_base64("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVsbG8").unwrap(), b"hello");
        assert_eq!(decode_base64("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(decode_base64("-_8").unwrap(), vec![0xfb, 0xff]);
        assert!(decode_base64("***").is_err());
    }

    #[test]
    fn charset_for_text_types() {
        assert_eq!(with_charset("text/html"), "text/html; charset=utf-8");
        assert_eq!(
            with_charset("application/json"),
            "application/json; charset=utf-8"
        );
        assert_eq!(with_charset("image/png"), "image/png");
    }
}
