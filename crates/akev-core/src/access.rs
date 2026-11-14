//! Who may use the proxy (`proxy.access`).
//!
//! The proxy accepts clients on the loopback interface by default; bound to another address
//! (phones, containers, `--proxy-bind 0.0.0.0`) it would otherwise be an open relay that
//! also reaches this machine's own loopback services. So:
//!
//! * **Client policy**: [`ClientPolicy::Private`] (default) admits loopback, RFC 1918,
//!   link-local, IPv6 unique-local and CGNAT `100.64.0.0/10` (Docker, Tailscale) clients;
//!   [`ClientPolicy::Allowlist`] loopback plus the listed addresses / CIDRs;
//!   [`ClientPolicy::Any`] everyone. Clients in `block` are always refused (loopback never
//!   is). Refused connections are closed without a response.
//! * **Credentials** (optional): with a username set, clients on other machines must
//!   authenticate (HTTP `Proxy-Authorization: Basic`, SOCKS5 username/password). Loopback,
//!   WireGuard and transparent-mode clients are exempt.
//! * **Loopback targets**: clients on other machines can't make the proxy connect to this
//!   machine's loopback or unspecified addresses (`127.0.0.0/8`, `::1`, `0.0.0.0`,
//!   `localhost`, also after DNS) unless `allow_loopback_targets` is on.
//! * **PROXY protocol** headers are honoured only with `trust_proxy_protocol` (the peer is a
//!   load balancer that vouches for the client address).

use serde::{Deserialize, Serialize};
use std::net::IpAddr;

/// Which clients may connect ([`ProxyAccessSettings::policy`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ClientPolicy {
    /// This machine and private networks (LAN, link-local, IPv6 ULA, CGNAT/Tailscale).
    #[default]
    Private,
    /// This machine and the addresses in [`ProxyAccessSettings::allow`].
    Allowlist,
    /// Anyone who can reach the port.
    Any,
}

/// `proxy.access`: client policy, blocked clients, credentials and what remote clients may
/// reach. Every field has a default, so a partial file loads.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProxyAccessSettings {
    /// Which clients may connect.
    pub policy: ClientPolicy,
    /// IP addresses or CIDR ranges admitted by [`ClientPolicy::Allowlist`].
    pub allow: Vec<String>,
    /// IP addresses or CIDR ranges always refused (never loopback).
    pub block: Vec<String>,
    /// Proxy username; empty = no credentials. When set, clients on other machines must
    /// authenticate.
    pub username: String,
    /// Proxy password (with `username`).
    pub password: String,
    /// Let clients on other machines reach this machine's loopback addresses through the
    /// proxy (e.g. a dev server on `localhost`).
    pub allow_loopback_targets: bool,
    /// Honour PROXY protocol (v1/v2) headers: the peer is a trusted load balancer.
    pub trust_proxy_protocol: bool,
}

impl ProxyAccessSettings {
    /// Configured `(username, password)`, if credentials are required.
    pub fn credentials(&self) -> Option<(&str, &str)> {
        (!self.username.is_empty()).then_some((self.username.as_str(), self.password.as_str()))
    }

    /// Check the settings; the message names the field and says what to do.
    pub fn validate(&self) -> Result<(), String> {
        for (field, list) in [("allow", &self.allow), ("block", &self.block)] {
            for entry in list {
                IpRange::parse(entry).map_err(|e| format!("proxy.access.{field}: {e}"))?;
            }
        }
        if self.username.contains(':') {
            return Err("proxy.access.username must not contain ':'".into());
        }
        if self
            .username
            .chars()
            .chain(self.password.chars())
            .any(char::is_control)
        {
            return Err("proxy.access credentials must not contain control characters".into());
        }
        if self.username.len() > 255 || self.password.len() > 255 {
            return Err(
                "proxy.access credentials must be at most 255 bytes each (SOCKS5 limit)".into(),
            );
        }
        Ok(())
    }
}

/// An IP address or CIDR range (`192.168.1.23`, `10.0.0.0/8`, `fd00::/8`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IpRange {
    network: IpAddr,
    prefix: u8,
}

impl IpRange {
    /// Parse `ip` or `ip/prefix` (IPv4-mapped IPv6 addresses count as IPv4).
    pub fn parse(s: &str) -> Result<IpRange, String> {
        let s = s.trim();
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a.trim(), Some(p.trim())),
            None => (s, None),
        };
        let ip: IpAddr = addr
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .map_err(|_| format!("{s:?} is not an IP address or CIDR range"))?;
        let ip = canonical_ip(ip);
        let max = if ip.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            None => max,
            Some(p) => p
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or_else(|| format!("{s:?} has an invalid prefix length (0-{max})"))?,
        };
        Ok(IpRange {
            network: ip,
            prefix,
        })
    }

    /// Whether `ip` is in the range.
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, canonical_ip(ip)) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 addresses (dual-stack listeners) as IPv4.
pub fn canonical_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    }
}

/// Loopback (`127.0.0.0/8`, `::1`, also IPv4-mapped).
pub fn is_loopback(ip: IpAddr) -> bool {
    canonical_ip(ip).is_loopback()
}

/// Addresses that reach this machine itself when connected to: loopback and unspecified
/// (`0.0.0.0/8`, `::`).
pub fn is_loopback_target(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => v4.is_loopback() || v4.octets()[0] == 0,
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

/// Loopback, RFC 1918, link-local, IPv6 unique-local and CGNAT (`100.64.0.0/10`) addresses.
pub fn is_private_network(ip: IpAddr) -> bool {
    match canonical_ip(ip) {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64)
        }
        IpAddr::V6(v6) => {
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xFE00) == 0xFC00 || (first & 0xFFC0) == 0xFE80
        }
    }
}

/// Compiled [`ProxyAccessSettings`].
#[derive(Clone, Debug, Default)]
pub struct ClientAccess {
    policy: ClientPolicy,
    allow: Vec<IpRange>,
    block: Vec<IpRange>,
    credentials: Option<(String, String)>,
    /// See [`ProxyAccessSettings::allow_loopback_targets`].
    pub allow_loopback_targets: bool,
    /// See [`ProxyAccessSettings::trust_proxy_protocol`].
    pub trust_proxy_protocol: bool,
}

impl ClientAccess {
    /// Compile the settings (invalid list entries are skipped: the API validates them).
    pub fn new(settings: &ProxyAccessSettings) -> Self {
        let ranges = |list: &[String]| -> Vec<IpRange> {
            list.iter().filter_map(|s| IpRange::parse(s).ok()).collect()
        };
        ClientAccess {
            policy: settings.policy,
            allow: ranges(&settings.allow),
            block: ranges(&settings.block),
            credentials: settings
                .credentials()
                .map(|(u, p)| (u.to_string(), p.to_string())),
            allow_loopback_targets: settings.allow_loopback_targets,
            trust_proxy_protocol: settings.trust_proxy_protocol,
        }
    }

    /// Whether a client at `ip` may connect.
    pub fn admits(&self, ip: IpAddr) -> bool {
        let ip = canonical_ip(ip);
        if ip.is_loopback() {
            return true;
        }
        if self.block.iter().any(|r| r.contains(ip)) {
            return false;
        }
        match self.policy {
            ClientPolicy::Any => true,
            ClientPolicy::Private => is_private_network(ip),
            ClientPolicy::Allowlist => self.allow.iter().any(|r| r.contains(ip)),
        }
    }

    /// Whether a client at `ip` must authenticate (credentials configured, not loopback).
    pub fn needs_auth(&self, ip: IpAddr) -> bool {
        self.credentials.is_some() && !is_loopback(ip)
    }

    /// Whether credentials are configured.
    pub fn has_credentials(&self) -> bool {
        self.credentials.is_some()
    }

    /// Check a username and password (constant time; `false` without configured credentials).
    pub fn check(&self, username: &[u8], password: &[u8]) -> bool {
        match &self.credentials {
            Some((u, p)) => {
                // Both compared, always: no early exit telling which one was wrong.
                let user_ok = constant_time_eq(username, u.as_bytes());
                let pass_ok = constant_time_eq(password, p.as_bytes());
                user_ok & pass_ok
            }
            None => false,
        }
    }

    /// Check a `Proxy-Authorization` header value (`Basic base64(user:password)`).
    pub fn check_basic(&self, header: &str) -> bool {
        match parse_basic(header) {
            Some((user, password)) => self.check(user.as_bytes(), password.as_bytes()),
            None => false,
        }
    }
}

/// `(user, password)` from `Basic base64(user:password)`.
pub fn parse_basic(header: &str) -> Option<(String, String)> {
    use base64::Engine as _;
    let (scheme, value) = header.trim().split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("basic") {
        return None;
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value.trim())
        .ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let (user, password) = text.split_once(':')?;
    Some((user.to_string(), password.to_string()))
}

/// Byte comparison whose time depends only on the lengths.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let mut diff = a.len() ^ b.len();
    for i in 0..a.len().max(b.len()) {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        diff |= usize::from(x ^ y);
    }
    std::hint::black_box(diff) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn settings_default_and_round_trip() {
        let s = ProxyAccessSettings::default();
        assert_eq!(s.policy, ClientPolicy::Private);
        assert!(s.credentials().is_none());
        let json = serde_json::to_value(&s).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "policy": "private", "allow": [], "block": [], "username": "", "password": "",
                "allowLoopbackTargets": false, "trustProxyProtocol": false
            })
        );
        let parsed: ProxyAccessSettings =
            serde_json::from_str(r#"{"policy":"allowlist","allow":["10.0.0.0/8"]}"#).unwrap();
        assert_eq!(parsed.policy, ClientPolicy::Allowlist);
        assert_eq!(parsed.allow, ["10.0.0.0/8"]);
    }

    #[test]
    fn validation() {
        let mut s = ProxyAccessSettings {
            allow: vec![
                "192.168.1.0/24".into(),
                "fd00::1".into(),
                "::ffff:10.0.0.1".into(),
            ],
            block: vec!["10.0.0.7".into()],
            ..Default::default()
        };
        assert!(s.validate().is_ok());
        s.block.push("not-an-ip".into());
        assert!(s.validate().unwrap_err().contains("proxy.access.block"));
        s.block.pop();
        s.allow.push("10.0.0.0/33".into());
        assert!(s.validate().unwrap_err().contains("prefix"));
        s.allow.pop();
        s.username = "a:b".into();
        assert!(s.validate().is_err());
        s.username = "user".into();
        s.password = "x".repeat(300);
        assert!(s.validate().is_err());
    }

    #[test]
    fn ranges() {
        let r = IpRange::parse("192.168.1.0/24").unwrap();
        assert!(r.contains(ip("192.168.1.200")));
        assert!(r.contains(ip("::ffff:192.168.1.5")));
        assert!(!r.contains(ip("192.168.2.1")));
        assert!(!r.contains(ip("fd00::1")));
        let single = IpRange::parse("10.0.0.7").unwrap();
        assert!(single.contains(ip("10.0.0.7")));
        assert!(!single.contains(ip("10.0.0.8")));
        let all = IpRange::parse("0.0.0.0/0").unwrap();
        assert!(all.contains(ip("8.8.8.8")));
        let v6 = IpRange::parse("[fd00::]/8").unwrap();
        assert!(v6.contains(ip("fd12::1")));
        assert!(!v6.contains(ip("fe80::1")));
    }

    #[test]
    fn address_classes() {
        for private in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.23",
            "169.254.1.1",
            "100.64.0.1",
            "100.127.255.254",
            "::1",
            "fd7a:115c:a1e0::1",
            "fe80::1",
            "::ffff:192.168.0.1",
        ] {
            assert!(is_private_network(ip(private)), "{private}");
        }
        for public in ["8.8.8.8", "100.128.0.1", "172.32.0.1", "2001:db8::1"] {
            assert!(!is_private_network(ip(public)), "{public}");
        }
        for target in [
            "127.0.0.1",
            "127.9.9.9",
            "0.0.0.0",
            "::1",
            "::",
            "::ffff:127.0.0.1",
        ] {
            assert!(is_loopback_target(ip(target)), "{target}");
        }
        assert!(!is_loopback_target(ip("192.168.1.1")));
    }

    #[test]
    fn policies() {
        let private = ClientAccess::new(&ProxyAccessSettings::default());
        assert!(private.admits(ip("127.0.0.1")));
        assert!(private.admits(ip("192.168.1.23")));
        assert!(!private.admits(ip("8.8.8.8")));

        let allowlist = ClientAccess::new(&ProxyAccessSettings {
            policy: ClientPolicy::Allowlist,
            allow: vec!["192.168.1.23".into()],
            ..Default::default()
        });
        assert!(allowlist.admits(ip("::1")), "loopback always");
        assert!(allowlist.admits(ip("192.168.1.23")));
        assert!(!allowlist.admits(ip("192.168.1.24")));

        let any = ClientAccess::new(&ProxyAccessSettings {
            policy: ClientPolicy::Any,
            block: vec!["203.0.113.0/24".into(), "127.0.0.1".into()],
            ..Default::default()
        });
        assert!(any.admits(ip("8.8.8.8")));
        assert!(!any.admits(ip("203.0.113.9")), "blocked");
        assert!(any.admits(ip("127.0.0.1")), "loopback is never blocked");
    }

    #[test]
    fn credentials() {
        let open = ClientAccess::new(&ProxyAccessSettings::default());
        assert!(!open.needs_auth(ip("192.168.1.2")));
        assert!(!open.check(b"", b""));

        let locked = ClientAccess::new(&ProxyAccessSettings {
            username: "phone".into(),
            password: "s3cret".into(),
            ..Default::default()
        });
        assert!(locked.needs_auth(ip("192.168.1.2")));
        assert!(!locked.needs_auth(ip("127.0.0.1")));
        assert!(locked.check(b"phone", b"s3cret"));
        assert!(!locked.check(b"phone", b"s3cre"));
        assert!(!locked.check(b"phon", b"s3cret"));
        use base64::Engine as _;
        let header = format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode("phone:s3cret")
        );
        assert!(locked.check_basic(&header));
        assert!(!locked.check_basic("Bearer x"));
        assert!(!locked.check_basic("Basic !!!"));
        assert_eq!(
            parse_basic(&header),
            Some(("phone".to_string(), "s3cret".to_string()))
        );
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
    }
}
