//! Hostname patterns: the one syntax every host list in the settings uses
//! (`tls_passthrough`, `ignore_hosts`, `ignore_upstream_cert_errors`,
//! `client_certificates[].host`, `dns_overrides[].host`, TLS profile overrides, ...).
//!
//! Case-insensitive globs where `*` matches any run of characters (dots included) and `?`
//! exactly one, with an optional `:port` suffix (`[v6]:port` for IPv6 literals). A trailing
//! dot is ignored on both sides. Matching is linear-time. (Rule matchers use the richer
//! glob syntax of `akev-rules`, which also has `**`, classes and `{a,b}`.)

use std::borrow::Cow;

/// Case-insensitive ASCII wildcard match (`*` = any run, `?` = one character).
/// Linear-time greedy matching with single backtrack point (no exponential blowup).
pub fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p].eq_ignore_ascii_case(&text[t])) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(s) = star {
            p = s + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

/// Split `host[:port]`, `[v6]` / `[v6]:port` or a bare IPv6 literal (no port).
pub fn split_host_port(s: &str) -> (&str, Option<u16>) {
    if let Some(rest) = s.strip_prefix('[') {
        if let Some(end) = rest.find(']') {
            let port = rest[end + 1..]
                .strip_prefix(':')
                .and_then(|p| p.parse().ok());
            return (&rest[..end], port);
        }
        return (s, None);
    }
    match s.rfind(':') {
        // Exactly one colon: host:port (several colons = IPv6 literal).
        Some(i) if !s[..i].contains(':') => match s[i + 1..].parse::<u16>() {
            Ok(port) => (&s[..i], Some(port)),
            Err(_) => (s, None),
        },
        _ => (s, None),
    }
}

/// Normalise a hostname for matching / DNS / SNI: strip IPv6 brackets and a trailing dot,
/// lowercase. Borrows when nothing changes (the common case: the inbound side already
/// lowercases hosts).
pub fn normalize_host(host: &str) -> Cow<'_, str> {
    let mut h = host.trim();
    if let Some(inner) = h.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        h = inner;
    }
    let h = h.strip_suffix('.').unwrap_or(h);
    if h.bytes().any(|b| b.is_ascii_uppercase()) {
        Cow::Owned(h.to_ascii_lowercase())
    } else {
        Cow::Borrowed(h)
    }
}

/// A hostname glob with an optional port restriction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostPattern {
    glob: Box<str>,
    port: Option<u16>,
}

impl HostPattern {
    /// Parse a pattern; `None` for empty input.
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim();
        if s.is_empty() {
            return None;
        }
        let (host, port) = split_host_port(s);
        let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }
        Some(HostPattern {
            glob: host.into_boxed_str(),
            port,
        })
    }

    /// Whether the pattern contains wildcards.
    pub fn is_glob(&self) -> bool {
        self.glob.contains(['*', '?'])
    }

    /// The (lowercased) host part.
    pub fn host(&self) -> &str {
        &self.glob
    }

    /// Match a (normalised) host and port.
    pub fn matches(&self, host: &str, port: u16) -> bool {
        if self.port.is_some_and(|p| p != port) {
            return false;
        }
        &*self.glob == "*" || wildcard_match(self.glob.as_bytes(), host.as_bytes())
    }

    /// Match ignoring the port restriction.
    pub fn matches_host(&self, host: &str) -> bool {
        &*self.glob == "*" || wildcard_match(self.glob.as_bytes(), host.as_bytes())
    }
}

/// Parse a list of patterns, skipping empty entries.
pub fn parse_patterns<'a>(items: impl IntoIterator<Item = &'a String>) -> Vec<HostPattern> {
    items
        .into_iter()
        .filter_map(|s| HostPattern::parse(s))
        .collect()
}

/// A list of host patterns (a settings host list), compiled once.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostMatcher {
    patterns: Vec<HostPattern>,
}

impl HostMatcher {
    /// Compile `patterns`, skipping blank ones.
    pub fn new(patterns: &[String]) -> Self {
        HostMatcher {
            patterns: parse_patterns(patterns),
        }
    }

    /// Whether there are no patterns.
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// Whether `host` matches any pattern. A pattern with a port only matches when `port` is
    /// that port.
    pub fn matches(&self, host: &str, port: Option<u16>) -> bool {
        if self.patterns.is_empty() {
            return false;
        }
        let host = normalize_host(host);
        self.patterns.iter().any(|p| match (p.port, port) {
            (Some(want), Some(got)) => want == got && p.matches_host(&host),
            (Some(_), None) => false,
            (None, _) => p.matches_host(&host),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_basics() {
        assert!(wildcard_match(b"*", b""));
        assert!(wildcard_match(b"*", b"anything.example"));
        assert!(wildcard_match(b"*.example.com", b"a.example.com"));
        assert!(wildcard_match(b"*.example.com", b"a.b.example.com"));
        assert!(!wildcard_match(b"*.example.com", b"example.com"));
        assert!(wildcard_match(b"EXAMPLE.com", b"example.COM"));
        assert!(wildcard_match(b"api-?.test", b"api-1.test"));
        assert!(!wildcard_match(b"api-?.test", b"api-12.test"));
        assert!(wildcard_match(b"a*b*c", b"aXXbYYc"));
        assert!(!wildcard_match(b"a*b*c", b"aXXbYY"));
        assert!(wildcard_match(b"*.corp.*", b"git.corp.internal"));
        // Pathological pattern stays linear.
        let text = vec![b'a'; 5000];
        assert!(!wildcard_match(b"*a*a*a*a*a*a*a*b", &text));
    }

    #[test]
    fn host_port_splitting() {
        assert_eq!(split_host_port("example.com"), ("example.com", None));
        assert_eq!(
            split_host_port("example.com:8443"),
            ("example.com", Some(8443))
        );
        assert_eq!(split_host_port("::1"), ("::1", None));
        assert_eq!(split_host_port("[::1]"), ("::1", None));
        assert_eq!(split_host_port("[::1]:8080"), ("::1", Some(8080)));
        assert_eq!(split_host_port("fe80::1:2"), ("fe80::1:2", None));
        assert_eq!(split_host_port("host:notaport"), ("host:notaport", None));
    }

    #[test]
    fn pattern_matching() {
        let p = HostPattern::parse("*.Example.com").unwrap();
        assert!(p.matches("api.example.com", 443));
        assert!(!p.matches("example.com", 443));
        let p = HostPattern::parse("localhost:8443").unwrap();
        assert!(p.matches("localhost", 8443));
        assert!(!p.matches("localhost", 443));
        assert!(p.matches_host("localhost"));
        let p = HostPattern::parse("*").unwrap();
        assert!(p.matches("whatever", 1));
        let p = HostPattern::parse("[::1]:443").unwrap();
        assert!(p.matches("::1", 443));
        assert!(HostPattern::parse("  ").is_none());
        let p = HostPattern::parse("example.com.").unwrap();
        assert!(p.matches("example.com", 80));
    }

    #[test]
    fn host_matcher_lists() {
        let m = HostMatcher::new(&[
            "*.example.com".into(),
            "API.test".into(),
            "pinned.app:8443".into(),
            "[".into(),
            "  ".into(),
        ]);
        assert!(m.matches("a.example.com", Some(443)));
        assert!(m.matches("a.b.example.com", None));
        assert!(!m.matches("example.com", Some(443)));
        assert!(m.matches("api.TEST", None));
        assert!(m.matches("api.test.", None));
        assert!(m.matches("pinned.app", Some(8443)));
        assert!(!m.matches("pinned.app", Some(443)));
        assert!(!m.matches("pinned.app", None));
        assert!(m.matches("[::1]", None) == m.matches("::1", None));
        let empty = HostMatcher::new(&[]);
        assert!(empty.is_empty() && !empty.matches("anything", Some(1)));
    }

    #[test]
    fn host_normalisation() {
        assert_eq!(normalize_host("Example.COM."), "example.com");
        assert_eq!(normalize_host("[::1]"), "::1");
        assert!(matches!(normalize_host("plain.host"), Cow::Borrowed(_)));
    }
}
