//! Minimal, *non-normalizing* absolute-URL handling.
//!
//! The `url` crate implements the WHATWG URL standard, which normalizes paths (`/a/../b` →
//! `/b`, `//` handling, percent-encoding of spaces...). An interception proxy must forward
//! exactly what the user asked for (HTTP Toolkit #939: `curl --path-as-is` traversal tests
//! broke because paths were normalized), so URL rewriting here only splits the URL into
//! scheme / authority / path-and-query and never touches the path bytes.

use akev_core::RequestHead;
use akev_core::http::HttpVersion;

/// The parts of an absolute URL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UrlParts {
    /// Lowercased scheme.
    pub scheme: String,
    /// Lowercased host; IPv6 without brackets.
    pub host: String,
    /// Explicit port, if any.
    pub port: Option<u16>,
    /// Path including the query string; always starts with `/` (fragment removed).
    pub path: String,
}

impl UrlParts {
    /// The explicit port or the scheme's default.
    pub fn effective_port(&self) -> Option<u16> {
        self.port.or_else(|| default_port(&self.scheme))
    }
}

/// Default port for the schemes Akev proxies.
pub(crate) fn default_port(scheme: &str) -> Option<u16> {
    match scheme {
        "http" | "ws" => Some(80),
        "https" | "wss" => Some(443),
        _ => None,
    }
}

/// Split an absolute URL (`scheme://[userinfo@]host[:port][/path][?query][#fragment]`).
/// Returns `None` for relative or malformed URLs.
pub(crate) fn split_url(url: &str) -> Option<UrlParts> {
    let (scheme, rest) = url.split_once("://")?;
    let mut sc = scheme.chars();
    if !sc.next()?.is_ascii_alphabetic()
        || !sc.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
    {
        return None;
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let mut path = &rest[auth_end..];
    if let Some(hash) = path.find('#') {
        path = &path[..hash];
    }
    let (host, port) = split_authority(authority)?;
    if host.is_empty() {
        return None;
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    Some(UrlParts {
        scheme: scheme.to_ascii_lowercase(),
        host,
        port,
        path,
    })
}

/// Split `[userinfo@]host[:port]` into a lowercased host (IPv6 unbracketed) and port.
pub(crate) fn split_authority(authority: &str) -> Option<(String, Option<u16>)> {
    let hostport = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    if let Some(inner) = hostport.strip_prefix('[') {
        let close = inner.find(']')?;
        let host = &inner[..close];
        let after = &inner[close + 1..];
        let port = match after {
            "" | ":" => None,
            p => Some(p.strip_prefix(':')?.parse::<u16>().ok()?),
        };
        if !host.contains(':') {
            return None;
        }
        return Some((host.to_ascii_lowercase(), port));
    }
    match hostport.rsplit_once(':') {
        Some((host, port)) => {
            if host.contains(':') {
                // Unbracketed IPv6 literal without a port.
                if hostport
                    .chars()
                    .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.')
                {
                    return Some((hostport.to_ascii_lowercase(), None));
                }
                return None;
            }
            if port.is_empty() {
                return Some((host.to_ascii_lowercase(), None));
            }
            Some((host.to_ascii_lowercase(), Some(port.parse::<u16>().ok()?)))
        }
        None => Some((hostport.to_ascii_lowercase(), None)),
    }
}

/// Rebuild `head.url` from `head.scheme/host/port/path`.
pub(crate) fn rebuild_url(head: &mut RequestHead) {
    head.url = format!("{}://{}{}", head.scheme, head.authority(), head.path);
}

/// Point `head` at a new absolute URL, updating scheme/host/port/path/url consistently and
/// the `Host` / `:authority` header when the authority changed (unless `preserve_host`).
pub(crate) fn set_request_url(
    head: &mut RequestHead,
    url: &str,
    preserve_host: bool,
) -> Result<(), String> {
    let parts = split_url(url).ok_or_else(|| format!("invalid absolute URL `{url}`"))?;
    let port = parts
        .effective_port()
        .ok_or_else(|| format!("unsupported URL scheme `{}`", parts.scheme))?;
    let old_authority = head.authority();
    head.scheme = parts.scheme;
    head.host = parts.host;
    head.port = port;
    head.path = parts.path;
    rebuild_url(head);
    if !preserve_host {
        let new_authority = head.authority();
        if new_authority != old_authority {
            update_host_header(head, &new_authority);
        }
    }
    Ok(())
}

/// Rewrite the `Host` header (and an HTTP/2 `:authority` pseudo-header if present). HTTP/1
/// requests without a Host header get one appended.
pub(crate) fn update_host_header(head: &mut RequestHead, authority: &str) {
    let mut found = false;
    for (name, value) in head.headers.0.iter_mut() {
        if name.eq_ignore_ascii_case("host") || name == ":authority" {
            authority.clone_into(value);
            found = true;
        }
    }
    // Drop duplicate Host headers so exactly one remains.
    let mut seen_host = false;
    head.headers.0.retain(|(name, _)| {
        if name.eq_ignore_ascii_case("host") {
            let keep = !seen_host;
            seen_host = true;
            keep
        } else {
            true
        }
    });
    if !found
        && matches!(
            head.http_version,
            HttpVersion::Http09 | HttpVersion::Http10 | HttpVersion::Http11
        )
    {
        head.headers.append("Host", authority);
    }
}

/// Strip `prefix` from `path` at a segment boundary. Returns the remainder (which starts with
/// `/` or `?`, or is empty) or `None` when the prefix doesn't apply.
pub(crate) fn strip_path_prefix<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    if prefix.is_empty() || prefix == "/" {
        return Some(path);
    }
    let rest = path.strip_prefix(prefix)?;
    if prefix.ends_with('/') || rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') {
        Some(rest)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_urls_without_normalizing() {
        let p = split_url("HTTPS://User:pw@Example.COM:8443/a/../b//c?x=1#frag").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "example.com");
        assert_eq!(p.port, Some(8443));
        assert_eq!(p.path, "/a/../b//c?x=1");
        let p = split_url("http://[::1]:3000").unwrap();
        assert_eq!(
            (p.host.as_str(), p.port, p.path.as_str()),
            ("::1", Some(3000), "/")
        );
        let p = split_url("http://[2001:DB8::1]/x").unwrap();
        assert_eq!(
            (p.host.as_str(), p.effective_port()),
            ("2001:db8::1", Some(80))
        );
        let p = split_url("ws://h?q").unwrap();
        assert_eq!(p.path, "/?q");
        assert!(split_url("/relative").is_none());
        assert!(split_url("http://").is_none());
        assert!(split_url("http://host:99999/").is_none());
        assert!(split_url("http://[::1/").is_none());
        assert!(split_url("1http://x/").is_none());
    }

    #[test]
    fn prefix_stripping_respects_segments() {
        assert_eq!(strip_path_prefix("/api/users", "/api"), Some("/users"));
        assert_eq!(strip_path_prefix("/api", "/api"), Some(""));
        assert_eq!(strip_path_prefix("/api?x", "/api"), Some("?x"));
        assert_eq!(strip_path_prefix("/apiv2/users", "/api"), None);
        assert_eq!(strip_path_prefix("/api/users", "/api/"), Some("users"));
        assert_eq!(strip_path_prefix("/x", ""), Some("/x"));
    }
}
