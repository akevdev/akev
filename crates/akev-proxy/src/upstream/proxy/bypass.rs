//! `no_proxy` / bypass-list matching.
//!
//! Supported entry forms (case-insensitive, comma/space/semicolon separated when they come
//! from env vars or OS settings):
//! * `*` — bypass everything;
//! * `example.com`, `.example.com`, `*.example.com` — the domain and all its subdomains
//!   (curl semantics; HTTP Toolkit treats `*.` the same way);
//! * other globs (`*.corp.*`, `api-?.test`) — wildcard match on the hostname;
//! * `10.0.0.1`, `::1`, `[::1]` — exact IP (IPv4-mapped IPv6 normalised);
//! * `10.0.0.0/8`, `fe80::/10`, `169.254/16` (macOS short form) — CIDR ranges;
//! * any of the above with `:port` (`[v6]:port` for IPv6) — also require the port;
//! * `<local>` — plain hostnames without dots (Windows semantics) plus loopback;
//! * `<-loopback>` — disable the implicit loopback bypass.
//!
//! Like Chrome, `localhost`, `*.localhost` and loopback IPs bypass the proxy implicitly
//! (sending them to a remote proxy can never reach the local service) unless `<-loopback>` is
//! present.

use crate::upstream::dns::{is_localhost_name, parse_ip};
use crate::upstream::glob::{split_host_port, wildcard_match};
use std::net::IpAddr;

#[derive(Clone, Debug, PartialEq)]
enum Rule {
    All,
    Local,
    Domain {
        suffix: String,
        port: Option<u16>,
    },
    Glob {
        pattern: String,
        port: Option<u16>,
    },
    Ip {
        ip: IpAddr,
        port: Option<u16>,
    },
    Cidr {
        net: IpAddr,
        prefix: u8,
        port: Option<u16>,
    },
}

/// A compiled bypass list.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Bypass {
    rules: Vec<Rule>,
    implicit_loopback: bool,
}

impl Default for Bypass {
    fn default() -> Self {
        Bypass {
            rules: Vec::new(),
            implicit_loopback: true,
        }
    }
}

impl Bypass {
    /// Compile entries (each entry may itself be a comma/semicolon/space separated list).
    pub(crate) fn new<'a>(entries: impl IntoIterator<Item = &'a str>) -> Self {
        let mut bypass = Bypass::default();
        for raw in entries {
            for entry in raw.split([',', ';', ' ', '\t', '\n']) {
                let entry = entry.trim();
                if entry.is_empty() {
                    continue;
                }
                if entry.eq_ignore_ascii_case("<-loopback>") {
                    bypass.implicit_loopback = false;
                    continue;
                }
                if let Some(rule) = parse_rule(entry) {
                    bypass.rules.push(rule);
                }
            }
        }
        bypass
    }

    /// Whether a (normalised, lowercase) host/port bypasses the proxy.
    pub(crate) fn matches(&self, host: &str, port: u16) -> bool {
        let ip = parse_ip(host).map(canonical_ip);
        if self.implicit_loopback && is_loopback(host, ip) {
            return true;
        }
        self.rules.iter().any(|rule| match rule {
            Rule::All => true,
            Rule::Local => (ip.is_none() && !host.contains('.')) || is_loopback(host, ip),
            Rule::Domain { suffix, port: p } => {
                port_ok(*p, port)
                    && ip.is_none()
                    && (host == suffix
                        || (host.len() > suffix.len()
                            && host.ends_with(suffix.as_str())
                            && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'))
            }
            Rule::Glob { pattern, port: p } => {
                port_ok(*p, port) && wildcard_match(pattern.as_bytes(), host.as_bytes())
            }
            Rule::Ip {
                ip: rule_ip,
                port: p,
            } => port_ok(*p, port) && ip == Some(*rule_ip),
            Rule::Cidr {
                net,
                prefix,
                port: p,
            } => port_ok(*p, port) && ip.is_some_and(|ip| in_cidr(ip, *net, *prefix)),
        })
    }
}

fn port_ok(rule: Option<u16>, port: u16) -> bool {
    rule.is_none_or(|p| p == port)
}

fn is_loopback(host: &str, ip: Option<IpAddr>) -> bool {
    match ip {
        Some(ip) => ip.is_loopback(),
        None => is_localhost_name(host),
    }
}

/// IPv4-mapped IPv6 (`::ffff:1.2.3.4`) → IPv4.
pub(crate) fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Whether `ip` is inside `net/prefix`.
pub(crate) fn in_cidr(ip: IpAddr, net: IpAddr, prefix: u8) -> bool {
    match (canonical_ip(ip), canonical_ip(net)) {
        (IpAddr::V4(a), IpAddr::V4(b)) => {
            let prefix = u32::from(prefix.min(32));
            let mask = if prefix == 0 {
                0
            } else {
                u32::MAX << (32 - prefix)
            };
            (u32::from(a) & mask) == (u32::from(b) & mask)
        }
        (IpAddr::V6(a), IpAddr::V6(b)) => {
            let prefix = u32::from(prefix.min(128));
            let mask = if prefix == 0 {
                0
            } else {
                u128::MAX << (128 - prefix)
            };
            (u128::from(a) & mask) == (u128::from(b) & mask)
        }
        _ => false,
    }
}

/// Parse `a.b.c.d/n`, `a.b/n` (missing octets are zero), `v6/n`.
pub(crate) fn parse_cidr(s: &str) -> Option<(IpAddr, u8)> {
    let (addr, prefix) = s.split_once('/')?;
    let prefix: u8 = prefix.trim().parse().ok()?;
    let addr = addr.trim().trim_start_matches('[').trim_end_matches(']');
    let ip = match addr.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(_) if !addr.contains(':') => {
            let mut octets = [0u8; 4];
            let parts: Vec<&str> = addr.split('.').collect();
            if parts.is_empty() || parts.len() > 4 {
                return None;
            }
            for (i, p) in parts.iter().enumerate() {
                octets[i] = p.parse().ok()?;
            }
            IpAddr::from(octets)
        }
        Err(_) => return None,
    };
    let max = if ip.is_ipv4() { 32 } else { 128 };
    (prefix <= max).then_some((ip, prefix))
}

fn parse_rule(entry: &str) -> Option<Rule> {
    if entry == "*" {
        return Some(Rule::All);
    }
    if entry.eq_ignore_ascii_case("<local>") {
        return Some(Rule::Local);
    }
    let lower = entry.to_ascii_lowercase();
    // Strip a URL scheme some tools put in bypass lists (`http://example.com`).
    let lower = lower
        .split_once("://")
        .map(|(_, rest)| rest.to_owned())
        .unwrap_or(lower);
    if lower.contains('/') {
        // CIDR, optionally followed by nothing else (ports on CIDRs are not meaningful).
        let (net, prefix) = parse_cidr(&lower)?;
        return Some(Rule::Cidr {
            net,
            prefix,
            port: None,
        });
    }
    let (host, port) = split_host_port(&lower);
    if let Some(ip) = parse_ip(host) {
        return Some(Rule::Ip {
            ip: canonical_ip(ip),
            port,
        });
    }
    let host = host.strip_suffix('.').unwrap_or(host);
    if let Some(suffix) = host.strip_prefix("*.").or_else(|| host.strip_prefix('.'))
        && !suffix.is_empty()
        && !suffix.contains(['*', '?'])
    {
        return Some(Rule::Domain {
            suffix: suffix.to_owned(),
            port,
        });
    }
    if host.contains(['*', '?']) {
        return Some(Rule::Glob {
            pattern: host.to_owned(),
            port,
        });
    }
    if host.is_empty() {
        return None;
    }
    Some(Rule::Domain {
        suffix: host.to_owned(),
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domains_and_subdomains() {
        let b = Bypass::new(["example.com, .corp.test;*.svc.local", "internal:8080"]);
        assert!(b.matches("example.com", 443));
        assert!(b.matches("api.example.com", 443));
        assert!(!b.matches("notexample.com", 443));
        assert!(b.matches("corp.test", 80));
        assert!(b.matches("git.corp.test", 80));
        assert!(b.matches("svc.local", 80));
        assert!(b.matches("db.svc.local", 80));
        assert!(b.matches("internal", 8080));
        assert!(!b.matches("internal", 80));
    }

    #[test]
    fn globs_ips_and_cidrs() {
        let b = Bypass::new([
            "*.corp.*",
            "api-?.test",
            "10.0.0.0/8",
            "192.168.1.5",
            "fe80::/10",
            "[2001:db8::1]:8443",
            "169.254/16",
        ]);
        assert!(b.matches("git.corp.internal", 443));
        assert!(b.matches("api-1.test", 443));
        assert!(!b.matches("api-12.test", 443));
        assert!(b.matches("10.20.30.40", 80));
        assert!(!b.matches("11.0.0.1", 80));
        assert!(b.matches("192.168.1.5", 80));
        assert!(!b.matches("192.168.1.6", 80));
        assert!(b.matches("::ffff:192.168.1.5", 80));
        assert!(b.matches("fe80::1234", 80));
        assert!(b.matches("2001:db8::1", 8443));
        assert!(!b.matches("2001:db8::1", 443));
        assert!(b.matches("169.254.10.10", 80));
        // Hostnames never match IP rules (no DNS lookups for bypass decisions).
        assert!(!b.matches("ten.example", 80));
    }

    #[test]
    fn local_and_loopback() {
        let b = Bypass::new(["<local>"]);
        assert!(b.matches("intranet", 80));
        assert!(!b.matches("intranet.example", 80));
        assert!(!b.matches("10.1.1.1", 80));
        let empty = Bypass::new([]);
        assert!(empty.matches("localhost", 80));
        assert!(empty.matches("app.localhost", 80));
        assert!(empty.matches("127.0.0.1", 80));
        assert!(empty.matches("::1", 80));
        assert!(!empty.matches("example.com", 80));
        let no_loop = Bypass::new(["<-loopback>"]);
        assert!(!no_loop.matches("localhost", 80));
        assert!(!no_loop.matches("127.0.0.1", 80));
    }

    #[test]
    fn match_all_and_garbage() {
        assert!(Bypass::new(["*"]).matches("anything", 1));
        let b = Bypass::new(["", " , ", "bad/cidr", "10.0.0.0/99"]);
        assert!(!b.matches("bad", 80));
        assert!(!b.matches("10.0.0.1", 80));
    }

    #[test]
    fn cidr_math() {
        assert!(in_cidr(
            "10.1.2.3".parse().unwrap(),
            "10.0.0.0".parse().unwrap(),
            8
        ));
        assert!(in_cidr(
            "1.2.3.4".parse().unwrap(),
            "0.0.0.0".parse().unwrap(),
            0
        ));
        assert!(!in_cidr(
            "10.1.2.3".parse().unwrap(),
            "fe80::".parse().unwrap(),
            8
        ));
        assert_eq!(
            parse_cidr("172.16/12"),
            Some(("172.16.0.0".parse().unwrap(), 12))
        );
        assert_eq!(parse_cidr("::/0"), Some(("::".parse().unwrap(), 0)));
    }
}
