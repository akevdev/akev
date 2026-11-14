//! Small, dependency-free helpers shared by the store modules: host globs, address parsing,
//! cookie parsing, HTTP-date conversion and URL splitting.

/// A compiled hostname glob (the settings host-pattern syntax, see
/// [`akev_core::hostpat`]), matched case-insensitively against the whole hostname.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostGlob(akev_core::hostpat::HostPattern);

impl HostGlob {
    /// Compile a pattern; blank patterns are rejected (they would only match an empty host).
    pub(crate) fn new(pattern: &str) -> Option<Self> {
        akev_core::hostpat::HostPattern::parse(pattern).map(HostGlob)
    }

    pub(crate) fn matches(&self, host: &str) -> bool {
        self.0
            .matches_host(&akev_core::hostpat::normalize_host(host))
    }
}

/// The IP part of a socket address string: `1.2.3.4:5` → `1.2.3.4`, `[::1]:5` → `::1`.
/// A bare IP (no port) is returned unchanged. Empty input → `None`.
pub(crate) fn ip_of(addr: &str) -> Option<&str> {
    let addr = addr.trim();
    if addr.is_empty() {
        return None;
    }
    if let Some(rest) = addr.strip_prefix('[') {
        return rest
            .split_once(']')
            .map(|(ip, _)| ip)
            .filter(|ip| !ip.is_empty());
    }
    match addr.matches(':').count() {
        1 => addr
            .split_once(':')
            .map(|(ip, _)| ip)
            .filter(|ip| !ip.is_empty()),
        // Zero colons: IPv4/hostname without port. Several: bare IPv6 without port.
        _ => Some(addr),
    }
}

/// The port of a socket address string (`1.2.3.4:5` / `[::1]:5`), if present.
pub(crate) fn port_of(addr: &str) -> Option<u16> {
    let addr = addr.trim();
    if let Some(rest) = addr.strip_prefix('[') {
        return rest.split_once("]:").and_then(|(_, p)| p.parse().ok());
    }
    if addr.matches(':').count() == 1 {
        return addr.split_once(':').and_then(|(_, p)| p.parse().ok());
    }
    None
}

/// Format `ip:port`, bracketing IPv6 addresses.
pub(crate) fn socket_addr_string(ip: &str, port: u16) -> String {
    if ip.contains(':') {
        format!("[{ip}]:{port}")
    } else {
        format!("{ip}:{port}")
    }
}

/// `host[:port]` with the port omitted when it is the scheme's default.
pub(crate) fn authority(scheme: &str, host: &str, port: u16) -> String {
    let default = matches!((scheme, port), ("http" | "ws", 80) | ("https" | "wss", 443));
    let host = if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if default || port == 0 {
        host
    } else {
        format!("{host}:{port}")
    }
}

/// Parse a `Cookie` request header into ordered `(name, value)` pairs.
pub(crate) fn parse_cookie_header(value: &str) -> impl Iterator<Item = (&str, &str)> {
    value.split(';').filter_map(|part| {
        let part = part.trim();
        if part.is_empty() {
            return None;
        }
        Some(match part.split_once('=') {
            Some((n, v)) => (n.trim(), v.trim()),
            None => ("", part),
        })
    })
}

/// A parsed `Set-Cookie` header (HAR cookie shape).
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct SetCookie<'a> {
    pub name: &'a str,
    pub value: &'a str,
    pub path: Option<&'a str>,
    pub domain: Option<&'a str>,
    /// Raw `Expires` attribute value.
    pub expires: Option<&'a str>,
    pub http_only: bool,
    pub secure: bool,
    pub same_site: Option<&'a str>,
}

/// Parse one `Set-Cookie` header value. Values containing `=` are kept whole (unlike HTTP
/// Toolkit, which truncated them). Returns `None` for an empty header.
pub(crate) fn parse_set_cookie(value: &str) -> Option<SetCookie<'_>> {
    let mut parts = value.split(';');
    let first = parts.next()?.trim();
    if first.is_empty() {
        return None;
    }
    let (name, val) = match first.split_once('=') {
        Some((n, v)) => (n.trim(), v.trim()),
        None => ("", first),
    };
    let mut cookie = SetCookie {
        name,
        value: val,
        ..SetCookie::default()
    };
    for attr in parts {
        let attr = attr.trim();
        let (key, v) = match attr.split_once('=') {
            Some((k, v)) => (k.trim(), Some(v.trim())),
            None => (attr, None),
        };
        if key.eq_ignore_ascii_case("path") {
            cookie.path = v;
        } else if key.eq_ignore_ascii_case("domain") {
            cookie.domain = v;
        } else if key.eq_ignore_ascii_case("expires") {
            cookie.expires = v;
        } else if key.eq_ignore_ascii_case("httponly") {
            cookie.http_only = true;
        } else if key.eq_ignore_ascii_case("secure") {
            cookie.secure = true;
        } else if key.eq_ignore_ascii_case("samesite") {
            cookie.same_site = v;
        }
    }
    Some(cookie)
}

/// Convert an HTTP date (IMF-fixdate `Sun, 06 Nov 1994 08:49:37 GMT`, RFC 850
/// `Sunday, 06-Nov-94 08:49:37 GMT` or asctime `Sun Nov  6 08:49:37 1994`) into an RFC 3339
/// UTC timestamp (`1994-11-06T08:49:37.000Z`), as HAR cookie `expires` expects.
pub(crate) fn http_date_to_iso(s: &str) -> Option<String> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let month_num = |m: &str| {
        let m = m.get(..3)?.to_ascii_lowercase();
        MONTHS.iter().position(|x| *x == m).map(|i| i as u32 + 1)
    };
    let s = s.trim();
    // Drop the weekday ("Sun," / "Sunday,") when present.
    let rest = match s.split_once(',') {
        Some((_, r)) => r.trim(),
        None => s,
    };
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let (day, month, mut year, time) = match tokens.as_slice() {
        // IMF-fixdate: 06 Nov 1994 08:49:37 GMT
        [d, m, y, t, ..] if d.chars().all(|c| c.is_ascii_digit()) => (
            d.parse::<u32>().ok()?,
            month_num(m)?,
            y.parse::<i32>().ok()?,
            *t,
        ),
        // RFC 850: 06-Nov-94 08:49:37 GMT
        [dmy, t, ..] if dmy.contains('-') => {
            let mut it = dmy.split('-');
            let d = it.next()?.parse::<u32>().ok()?;
            let m = month_num(it.next()?)?;
            let y = it.next()?.parse::<i32>().ok()?;
            (d, m, y, *t)
        }
        // asctime: Sun Nov  6 08:49:37 1994 (weekday still present: no comma)
        [_, m, d, t, y] => (
            d.parse::<u32>().ok()?,
            month_num(m)?,
            y.parse::<i32>().ok()?,
            *t,
        ),
        _ => return None,
    };
    if year < 100 {
        year += if year < 70 { 2000 } else { 1900 };
    }
    let mut hms = time.split(':').map(|p| p.parse::<u32>().ok());
    let (h, mi, se) = (hms.next()??, hms.next()??, hms.next()??);
    if !(1..=31).contains(&day) || h > 23 || mi > 59 || se > 60 || !(0..=9999).contains(&year) {
        return None;
    }
    Some(format!(
        "{year:04}-{month:02}-{day:02}T{h:02}:{mi:02}:{se:02}.000Z"
    ))
}

/// Components of an absolute URL, with the path taken verbatim from the input string (the
/// `url` crate would normalise percent-encoding, which breaks wire fidelity).
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct UrlParts {
    pub scheme: String,
    /// Lowercase, IPv6 without brackets.
    pub host: String,
    pub port: u16,
    /// Path + query exactly as written (`/` when empty), fragment removed.
    pub path: String,
}

pub(crate) fn split_url(raw: &str) -> Option<UrlParts> {
    let raw = raw.trim();
    let parsed = url::Url::parse(raw).ok()?;
    let scheme = parsed.scheme().to_ascii_lowercase();
    let host = match parsed.host()? {
        url::Host::Domain(d) => d.to_ascii_lowercase(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    };
    let port = parsed
        .port_or_known_default()
        .unwrap_or(match scheme.as_str() {
            "https" | "wss" => 443,
            _ => 80,
        });
    // Verbatim path: everything after the authority, up to the fragment.
    let after_scheme = raw.split_once("://").map(|(_, r)| r).unwrap_or(raw);
    let path_start = after_scheme.find(['/', '?']).unwrap_or(after_scheme.len());
    let mut path = &after_scheme[path_start..];
    if let Some(i) = path.find('#') {
        path = &path[..i];
    }
    let path = if path.is_empty() {
        "/".to_string()
    } else if path.starts_with('?') {
        format!("/{path}")
    } else {
        path.to_string()
    };
    Some(UrlParts {
        scheme,
        host,
        port,
        path,
    })
}

/// Lowercase an ASCII header name into `buf` without allocating (names longer than the
/// buffer fall back to an owned string).
pub(crate) fn lower_name<'b>(name: &str, buf: &'b mut [u8; 64]) -> std::borrow::Cow<'b, str> {
    if name.len() <= buf.len() && name.is_ascii() {
        let out = &mut buf[..name.len()];
        out.copy_from_slice(name.as_bytes());
        out.make_ascii_lowercase();
        // ASCII in, ASCII out: always valid UTF-8.
        std::borrow::Cow::Borrowed(std::str::from_utf8(out).unwrap_or_default())
    } else {
        std::borrow::Cow::Owned(name.to_lowercase())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs() {
        let g = HostGlob::new("*.Example.com").unwrap();
        assert!(g.matches("api.example.com"));
        assert!(g.matches("a.b.EXAMPLE.com"));
        assert!(!g.matches("example.com"));
        assert!(!g.matches("example.com.evil"));
        let g = HostGlob::new("ex?mple.*").unwrap();
        assert!(g.matches("example.org"));
        assert!(!g.matches("exaample.org"));
        assert!(HostGlob::new("*").unwrap().matches("anything"));
        assert!(HostGlob::new("  ").is_none());
        assert!(HostGlob::new("**a*b").unwrap().matches("xxaxxb"));
        assert!(!HostGlob::new("a*b").unwrap().matches("ab-"));
    }

    #[test]
    fn addresses() {
        assert_eq!(ip_of("10.0.0.1:5000"), Some("10.0.0.1"));
        assert_eq!(ip_of("[::1]:8080"), Some("::1"));
        assert_eq!(ip_of("::1"), Some("::1"));
        assert_eq!(ip_of("10.0.0.1"), Some("10.0.0.1"));
        assert_eq!(ip_of(""), None);
        assert_eq!(port_of("10.0.0.1:5000"), Some(5000));
        assert_eq!(port_of("[::1]:8080"), Some(8080));
        assert_eq!(port_of("::1"), None);
        assert_eq!(socket_addr_string("::1", 80), "[::1]:80");
        assert_eq!(authority("https", "a.com", 443), "a.com");
        assert_eq!(authority("http", "::1", 8080), "[::1]:8080");
    }

    #[test]
    fn cookies() {
        let c: Vec<_> = parse_cookie_header("a=1; b=x=y ;flag").collect();
        assert_eq!(c, vec![("a", "1"), ("b", "x=y"), ("", "flag")]);
        let sc = parse_set_cookie(
            "sid=abc=def; Path=/; Domain=.x.com; Expires=Wed, 21 Oct 2015 07:28:00 GMT; HttpOnly; Secure; SameSite=Lax",
        )
        .unwrap();
        assert_eq!(sc.name, "sid");
        assert_eq!(sc.value, "abc=def");
        assert_eq!(sc.path, Some("/"));
        assert_eq!(sc.domain, Some(".x.com"));
        assert!(sc.http_only && sc.secure);
        assert_eq!(sc.same_site, Some("Lax"));
        assert_eq!(
            http_date_to_iso(sc.expires.unwrap()).as_deref(),
            Some("2015-10-21T07:28:00.000Z")
        );
        assert!(parse_set_cookie("  ").is_none());
    }

    #[test]
    fn http_dates() {
        assert_eq!(
            http_date_to_iso("Sunday, 06-Nov-94 08:49:37 GMT").as_deref(),
            Some("1994-11-06T08:49:37.000Z")
        );
        assert_eq!(
            http_date_to_iso("Sun Nov  6 08:49:37 1994").as_deref(),
            Some("1994-11-06T08:49:37.000Z")
        );
        assert_eq!(http_date_to_iso("garbage"), None);
        assert_eq!(http_date_to_iso("Sun, 32 Nov 1994 08:49:37 GMT"), None);
    }

    #[test]
    fn urls() {
        let u = split_url("https://Example.COM/a%2Fb?q=1#frag").unwrap();
        assert_eq!(
            u,
            UrlParts {
                scheme: "https".into(),
                host: "example.com".into(),
                port: 443,
                path: "/a%2Fb?q=1".into()
            }
        );
        let u = split_url("http://[::1]:8080?x").unwrap();
        assert_eq!(
            (u.host.as_str(), u.port, u.path.as_str()),
            ("::1", 8080, "/?x")
        );
        assert_eq!(split_url("wss://h.com").unwrap().port, 443);
        assert!(split_url("not a url").is_none());
    }

    #[test]
    fn lowercase_names() {
        let mut buf = [0u8; 64];
        assert_eq!(lower_name("Content-Type", &mut buf), "content-type");
        let long = "X".repeat(100);
        assert_eq!(lower_name(&long, &mut buf), "x".repeat(100));
    }
}
