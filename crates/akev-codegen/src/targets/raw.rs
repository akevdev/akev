//! Raw HTTP/1.1 request text.

use crate::prepare::{Body, Prepared};

/// The request in HTTP/1.1 wire format (CRLF line endings): request line with an origin-form
/// target, `Host` first, the original headers (only pseudo-headers, `Transfer-Encoding`,
/// `Content-Encoding` and `Content-Length` are replaced, since the body is decoded and
/// de-chunked), a fresh `Content-Length`, a blank line and the body. Non-UTF-8 body bytes are
/// shown as U+FFFD.
pub(super) fn generate(p: &Prepared<'_>) -> String {
    let (target, authority) = match &p.parsed {
        Some(url) => {
            let target = match verbatim_target(p.url) {
                Some(target) => target,
                None => {
                    let path = url.path();
                    match url.query() {
                        Some(q) => format!("{path}?{q}"),
                        None => path.to_string(),
                    }
                }
            };
            let host = url.host_str().unwrap_or_default();
            let authority = match url.port() {
                Some(port) => format!("{host}:{port}"),
                None => host.to_string(),
            };
            (target, authority)
        }
        None => (p.url.to_string(), String::new()),
    };
    let target = if target.is_empty() {
        "/".to_string()
    } else {
        target
    };

    let mut out = format!("{} {target} HTTP/1.1\r\n", p.method);
    let host = p
        .raw_headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("host"))
        .map_or(authority.as_str(), |&(_, v)| v);
    if !host.is_empty() {
        out.push_str(&format!("Host: {host}\r\n"));
    }
    for &(name, value) in &p.raw_headers {
        let skip = [
            "host",
            "content-length",
            "transfer-encoding",
            "content-encoding",
        ]
        .iter()
        .any(|h| name.eq_ignore_ascii_case(h));
        if !skip {
            let sep = if value.is_empty() { ":" } else { ": " };
            out.push_str(&format!("{name}{sep}{value}\r\n"));
        }
    }
    let body = p.body.bytes();
    if !body.is_empty() || matches!(p.method, "POST" | "PUT" | "PATCH") {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");
    match &p.body {
        Body::Binary(bytes) => out.push_str(&String::from_utf8_lossy(bytes)),
        other => out.push_str(other.text().unwrap_or_default()),
    }
    out
}

/// The origin-form target exactly as written in the URL (path + query, no fragment), when it
/// is already valid on the wire (visible ASCII only); `None` means "normalise it".
fn verbatim_target(url: &str) -> Option<String> {
    let rest = &url[url.find("://")? + 3..];
    let start = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let target = rest[start..].split('#').next().unwrap_or_default();
    if !target.bytes().all(|b| (0x21..=0x7e).contains(&b)) {
        return None;
    }
    Some(match target.chars().next() {
        None => "/".to_string(),
        Some('?') => format!("/{target}"),
        Some(_) => target.to_string(),
    })
}
