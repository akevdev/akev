//! Destination & URL handling: `host[:port]` parsing (IPv6-safe), absolute-form URL parsing that
//! keeps the path byte-exact, and the reconstruction of a request's absolute URL from the
//! request target, `Host`/`:authority`, the innermost tunnel target and the TLS SNI
//! (mockttp's algorithm, minus its IPv6 bugs).

use akev_core::ProxyMode;
use std::fmt;
use std::net::{IpAddr, Ipv6Addr};

/// A destination host (lowercase, IPv6 without brackets) and port.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct HostPort {
    /// Hostname or IP literal (lowercase, no brackets).
    pub host: String,
    pub port: u16,
}

impl HostPort {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        HostPort {
            host: host.into(),
            port,
        }
    }

    /// True if the host is an IP literal.
    pub fn is_ip(&self) -> bool {
        self.host.parse::<IpAddr>().is_ok()
    }
}

impl fmt::Display for HostPort {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// Default port for a URL scheme (`https`/`wss` → 443, everything else → 80).
pub fn default_port(scheme: &str) -> u16 {
    if scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("wss") {
        443
    } else {
        80
    }
}

/// Whether `host` is acceptable as a URL host (reg-name or IP literal). Rejects whitespace,
/// control characters and URL delimiters that would make the reconstructed URL ambiguous.
pub fn valid_host(host: &str) -> bool {
    if host.is_empty() || host.len() > 255 {
        return false;
    }
    if host.parse::<IpAddr>().is_ok() {
        return true;
    }
    host.bytes().all(|b| {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.'
                    | b'_'
                    | b'~'
                    | b'%'
                    | b'!'
                    | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
            )
    })
}

/// Normalise a host: lowercase; IPv6 literals in canonical compressed form.
fn normalize_host(host: &str) -> String {
    match host.parse::<Ipv6Addr>() {
        Ok(v6) => v6.to_string(),
        Err(_) => host.to_ascii_lowercase(),
    }
}

/// Parse `host`, `host:port`, `[v6]`, `[v6]:port` or a bare (unbracketed) IPv6 literal.
/// `default_port` is used when no port is present (`None` = port required).
pub fn parse_authority(s: &str, default_port: Option<u16>) -> Option<HostPort> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let end = rest.find(']')?;
        let host = &rest[..end];
        host.parse::<Ipv6Addr>().ok()?;
        let after = &rest[end + 1..];
        let port = if after.is_empty() {
            default_port?
        } else {
            let p = after.strip_prefix(':')?;
            if p.is_empty() {
                default_port?
            } else {
                p.parse::<u16>().ok()?
            }
        };
        (host, port)
    } else if s.bytes().filter(|&b| b == b':').count() > 1 {
        // Bare IPv6 literal without brackets: no port possible.
        s.parse::<Ipv6Addr>().ok()?;
        (s, default_port?)
    } else {
        match s.split_once(':') {
            Some((h, p)) => {
                let port = if p.is_empty() {
                    default_port?
                } else {
                    p.parse::<u16>().ok()?
                };
                (h, port)
            }
            None => (s, default_port?),
        }
    };
    if !valid_host(host) {
        return None;
    }
    Some(HostPort {
        host: normalize_host(host),
        port,
    })
}

/// A parsed absolute-form URL (`scheme://authority/path?query`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AbsoluteUrl {
    /// Lowercase scheme (`http`, `https`, `ws`, `wss`).
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Path + query exactly as written (`/` when empty; fragment removed).
    pub path: String,
}

/// Parse an absolute URL with an `http`, `https`, `ws` or `wss` scheme, keeping the path
/// byte-for-byte (no normalisation), dropping userinfo and fragment.
pub fn parse_absolute_url(s: &str) -> Option<AbsoluteUrl> {
    let (scheme, rest) = s.split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ws" | "wss") {
        return None;
    }
    let auth_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let mut authority = &rest[..auth_end];
    if let Some(at) = authority.rfind('@') {
        authority = &authority[at + 1..];
    }
    let hp = parse_authority(authority, Some(default_port(&scheme)))?;
    let mut path = &rest[auth_end..];
    if let Some(hash) = path.find('#') {
        path = &path[..hash];
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    Some(AbsoluteUrl {
        scheme,
        host: hp.host,
        port: hp.port,
        path,
    })
}

/// `host[:port]` with the port omitted when it is the scheme default and IPv6 bracketed.
pub fn authority_for(scheme: &str, host: &str, port: u16) -> String {
    let h = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == default_port(scheme) {
        h
    } else {
        format!("{h}:{port}")
    }
}

/// Build the absolute URL `scheme://authority/path`.
pub fn build_url(scheme: &str, host: &str, port: u16, path: &str) -> String {
    let authority = authority_for(scheme, host, port);
    if path.starts_with('/') {
        format!("{scheme}://{authority}{path}")
    } else if path == "*" {
        format!("{scheme}://{authority}")
    } else {
        format!("{scheme}://{authority}/{path}")
    }
}

/// The request target as sent on the request line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RequestTarget {
    /// `http://host/path` (proxy request).
    Absolute(AbsoluteUrl),
    /// `/path?query`.
    Origin(String),
    /// `*` (server-wide OPTIONS).
    Asterisk,
}

impl RequestTarget {
    /// Classify a raw request-target (non-CONNECT requests).
    pub fn parse(raw: &str) -> Option<RequestTarget> {
        if raw == "*" {
            return Some(RequestTarget::Asterisk);
        }
        if raw.starts_with('/') {
            return Some(RequestTarget::Origin(raw.to_string()));
        }
        if raw.contains("://") {
            return parse_absolute_url(raw).map(RequestTarget::Absolute);
        }
        None
    }
}

/// Connection facts that influence how a relative request target is resolved.
#[derive(Clone, Debug, Default)]
pub struct TargetContext<'a> {
    /// Innermost tunnel target (CONNECT / SOCKS), if any.
    pub tunnel: Option<&'a HostPort>,
    /// How the tunnel was established (Connect / Socks); ignored without a tunnel.
    pub tunnel_via: ProxyMode,
    /// Whether the innermost hop was TLS-decrypted by us.
    pub tls: bool,
    /// SNI sent by the client on the innermost TLS hop.
    pub sni: Option<&'a str>,
    /// Local port the client connected to (used for host-less direct requests).
    pub local_port: u16,
}

/// The resolved destination of a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub path: String,
    pub mode: ProxyMode,
}

/// Why a request target couldn't be resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TargetError {
    InvalidTarget,
    InvalidHost(String),
}

impl fmt::Display for TargetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TargetError::InvalidTarget => f.write_str("invalid request target"),
            TargetError::InvalidHost(h) => write!(f, "invalid Host header: {h:?}"),
        }
    }
}

/// Resolve where a request is going and what its absolute URL looks like.
///
/// * Absolute-form targets are proxy requests: the URL wins, tunnel info is ignored.
/// * Relative targets use the tunnel's port; the URL host prefers the tunnel *name*, but when
///   the tunnel target is a bare IP, a hostname from `Host`/`:authority` or SNI is preferred.
/// * With no information at all, the request is addressed to the proxy itself
///   (`localhost:<local port>`).
///
/// `scheme_hint` is HTTP/2's `:scheme`; `websocket` maps http(s) → ws(s).
pub fn resolve_target(
    ctx: &TargetContext<'_>,
    target: &RequestTarget,
    host_header: Option<&str>,
    scheme_hint: Option<&str>,
    websocket: bool,
) -> Result<ResolvedTarget, TargetError> {
    let ws = |scheme: &str| -> String {
        match (websocket, scheme) {
            (true, "http") => "ws".to_string(),
            (true, "https") => "wss".to_string(),
            (_, s) => s.to_string(),
        }
    };
    if let RequestTarget::Absolute(url) = target {
        let scheme = match url.scheme.as_str() {
            "ws" | "wss" => url.scheme.clone(),
            s => ws(s),
        };
        return Ok(ResolvedTarget {
            scheme,
            host: url.host.clone(),
            port: url.port,
            path: url.path.clone(),
            mode: ProxyMode::HttpProxy,
        });
    }
    let path = match target {
        RequestTarget::Origin(p) => p.clone(),
        RequestTarget::Asterisk => "*".to_string(),
        RequestTarget::Absolute(_) => return Err(TargetError::InvalidTarget),
    };
    let base_scheme = match scheme_hint.map(|s| s.to_ascii_lowercase()) {
        Some(s) if s == "http" || s == "https" => s,
        _ => {
            if ctx.tls {
                "https".to_string()
            } else {
                "http".to_string()
            }
        }
    };
    let dport = default_port(&base_scheme);
    let host_hdr = match host_header.map(str::trim).filter(|h| !h.is_empty()) {
        Some(h) => Some(
            parse_authority(h, Some(dport))
                .ok_or_else(|| TargetError::InvalidHost(h.to_string()))?,
        ),
        None => None,
    };
    let sni = ctx
        .sni
        .filter(|s| valid_host(s))
        .map(|s| s.to_ascii_lowercase());

    let (host, port, mode) = match ctx.tunnel {
        Some(tunnel) => {
            let host = if tunnel.is_ip() {
                host_hdr
                    .as_ref()
                    .map(|h| h.host.clone())
                    .or(sni)
                    .unwrap_or_else(|| tunnel.host.clone())
            } else {
                tunnel.host.clone()
            };
            (host, tunnel.port, ctx.tunnel_via)
        }
        None => match (host_hdr, sni) {
            (Some(h), _) => (h.host, h.port, ProxyMode::Direct),
            (None, Some(sni)) => (sni, dport, ProxyMode::Direct),
            (None, None) => ("localhost".to_string(), ctx.local_port, ProxyMode::Direct),
        },
    };
    Ok(ResolvedTarget {
        scheme: ws(&base_scheme),
        host,
        port,
        path,
        mode,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_parsing() {
        assert_eq!(
            parse_authority("Example.COM:8080", Some(80)),
            Some(HostPort::new("example.com", 8080))
        );
        assert_eq!(
            parse_authority("example.com", Some(443)),
            Some(HostPort::new("example.com", 443))
        );
        assert_eq!(
            parse_authority("example.com:", Some(443)),
            Some(HostPort::new("example.com", 443))
        );
        assert_eq!(
            parse_authority("[::1]:8000", None),
            Some(HostPort::new("::1", 8000))
        );
        assert_eq!(
            parse_authority("[::1]", Some(80)),
            Some(HostPort::new("::1", 80))
        );
        // Bare IPv6 without a port (mockttp mis-parses this).
        assert_eq!(
            parse_authority("2001:DB8::1", Some(443)),
            Some(HostPort::new("2001:db8::1", 443))
        );
        assert_eq!(
            parse_authority("[0:0:0:0:0:0:0:1]:1", None),
            Some(HostPort::new("::1", 1))
        );
        assert_eq!(parse_authority("example.com", None), None);
        assert_eq!(parse_authority("a^1:2", Some(80)), None);
        assert_eq!(parse_authority("host:99999", Some(80)), None);
        assert_eq!(parse_authority("[nope]:1", None), None);
        assert_eq!(parse_authority("", Some(80)), None);
        assert_eq!(HostPort::new("::1", 80).to_string(), "[::1]:80");
    }

    #[test]
    fn absolute_url_parsing_keeps_path_exact() {
        let u =
            parse_absolute_url("HTTP://user:pw@Example.com:8080/a%2Fb//c?x=1&y=%zz#frag").unwrap();
        assert_eq!(u.scheme, "http");
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 8080);
        assert_eq!(u.path, "/a%2Fb//c?x=1&y=%zz");
        let u = parse_absolute_url("https://example.com?q=1").unwrap();
        assert_eq!((u.port, u.path.as_str()), (443, "/?q=1"));
        let u = parse_absolute_url("http://[::1]:3000").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("::1", 3000, "/")
        );
        assert!(parse_absolute_url("ftp://example.com/").is_none());
        assert!(parse_absolute_url("http:///nohost").is_none());
    }

    #[test]
    fn url_building() {
        assert_eq!(
            build_url("https", "example.com", 443, "/a?b"),
            "https://example.com/a?b"
        );
        assert_eq!(build_url("http", "::1", 8080, "/"), "http://[::1]:8080/");
        assert_eq!(build_url("ws", "h", 80, "*"), "ws://h");
    }

    #[test]
    fn resolve_direct_and_proxy_requests() {
        let ctx = TargetContext {
            local_port: 8000,
            ..Default::default()
        };
        let t = RequestTarget::parse("http://example.com/x").unwrap();
        let r = resolve_target(&ctx, &t, Some("ignored.com"), None, false).unwrap();
        assert_eq!(
            (r.host.as_str(), r.port, r.mode),
            ("example.com", 80, ProxyMode::HttpProxy)
        );

        let t = RequestTarget::parse("/x").unwrap();
        let r = resolve_target(&ctx, &t, Some("Example.com:81"), None, true).unwrap();
        assert_eq!(
            (r.scheme.as_str(), r.host.as_str(), r.port, r.mode),
            ("ws", "example.com", 81, ProxyMode::Direct)
        );

        let r = resolve_target(&ctx, &t, None, None, false).unwrap();
        assert_eq!((r.host.as_str(), r.port), ("localhost", 8000));

        assert!(matches!(
            resolve_target(&ctx, &t, Some("bad host"), None, false),
            Err(TargetError::InvalidHost(_))
        ));
    }

    #[test]
    fn resolve_tunnelled_requests() {
        let tunnel = HostPort::new("example.com", 8443);
        let ctx = TargetContext {
            tunnel: Some(&tunnel),
            tunnel_via: ProxyMode::Connect,
            tls: true,
            sni: Some("other.com"),
            local_port: 8000,
        };
        let t = RequestTarget::Origin("/p".into());
        // Tunnel name wins over Host and SNI; port always follows the tunnel.
        let r = resolve_target(&ctx, &t, Some("host-header.com"), None, false).unwrap();
        assert_eq!(
            (r.scheme.as_str(), r.host.as_str(), r.port, r.mode),
            ("https", "example.com", 8443, ProxyMode::Connect)
        );

        // Tunnel to an IP: prefer a name (Host, then SNI), keep the tunnel port.
        let ip = HostPort::new("10.0.0.1", 444);
        let ctx = TargetContext {
            tunnel: Some(&ip),
            tunnel_via: ProxyMode::Socks,
            ..ctx
        };
        let r = resolve_target(&ctx, &t, Some("named.com:1234"), None, false).unwrap();
        assert_eq!(
            (r.host.as_str(), r.port, r.mode),
            ("named.com", 444, ProxyMode::Socks)
        );
        let r = resolve_target(&ctx, &t, None, None, false).unwrap();
        assert_eq!(r.host, "other.com");
        let ctx = TargetContext { sni: None, ..ctx };
        let r = resolve_target(&ctx, &t, None, None, false).unwrap();
        assert_eq!(r.host, "10.0.0.1");
    }
}
