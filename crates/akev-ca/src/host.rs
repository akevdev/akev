//! Hostname normalisation and subjectAltName selection for leaf certificates.

use std::borrow::Cow;
use std::net::{IpAddr, Ipv6Addr};

use rcgen::SanType;

use crate::{CaError, Result};

/// Longest DNS name we accept (RFC 1035 §2.3.4, without the trailing root dot).
const MAX_DNS_NAME_LEN: usize = 253;
/// Longest DNS label (RFC 1035 §2.3.4).
const MAX_LABEL_LEN: usize = 63;
/// X.520 `ub-common-name`: longer names are left out of the subject CN (the SAN is what
/// clients match anyway, and some parsers enforce the upper bound).
const MAX_COMMON_NAME_LEN: usize = 64;
/// Inputs longer than this cannot normalise to a valid name (even a fully non-ASCII name
/// maps to at most 253 A-label characters); rejecting them early bounds the work done on
/// hostile input such as a megabyte-long SNI or CONNECT target.
const MAX_INPUT_LEN: usize = 2048;

/// Second-level labels that, under a two-letter country-code TLD, almost always form a
/// public suffix (`co.uk`, `com.au`, `or.jp`, ...). Wildcards directly below those
/// (`*.co.uk`) are never honoured by browsers, so we don't emit them. This is a deliberately
/// small heuristic, not a Public Suffix List: the exact host SAN is always present, so a
/// missed suffix only produces a harmless, ignored wildcard entry.
const CC_SECOND_LEVEL_SUFFIXES: &[&str] = &[
    "ac", "biz", "co", "com", "ed", "edu", "firm", "gen", "go", "gob", "gov", "govt", "gv", "ind",
    "info", "int", "lg", "ltd", "mil", "mod", "ne", "net", "nhs", "nic", "nom", "or", "org", "plc",
    "police", "priv", "res", "sch", "school", "web",
];

/// The identity a leaf certificate is minted for: a normalised DNS name or an IP address.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum LeafName<'a> {
    /// Lower-case ASCII (IDNA A-label form), no trailing dot.
    Dns(Cow<'a, str>),
    Ip(IpAddr),
}

impl<'a> LeafName<'a> {
    /// Normalise `host` (a DNS name or IP literal, optionally `[bracketed]` IPv6 with a zone
    /// id) into the canonical identity used for certificate SANs and cache keys.
    pub(crate) fn parse(host: &'a str) -> Result<Self> {
        let trimmed = host.trim();
        if trimmed.len() > MAX_INPUT_LEN {
            return Err(CaError::Invalid(format!(
                "cannot issue a certificate for a {}-byte host name",
                trimmed.len()
            )));
        }
        if let Some(inner) = trimmed.strip_prefix('[') {
            let inner = inner
                .strip_suffix(']')
                .ok_or_else(|| invalid_host(host, "unbalanced IPv6 brackets"))?;
            return parse_ipv6(inner)
                .map(|ip| LeafName::Ip(IpAddr::V6(ip)))
                .ok_or_else(|| invalid_host(host, "not a valid IPv6 address"));
        }
        let name = trimmed.strip_suffix('.').unwrap_or(trimmed);
        if let Ok(ip) = name.parse::<IpAddr>() {
            return Ok(LeafName::Ip(ip));
        }
        if name.contains(':') {
            // Only IPv6 literals may contain colons (a `host:port` pair is a caller bug).
            return parse_ipv6(name)
                .map(|ip| LeafName::Ip(IpAddr::V6(ip)))
                .ok_or_else(|| {
                    invalid_host(
                        host,
                        "contains ':' but is not an IPv6 address (strip the port)",
                    )
                });
        }
        let dns: Cow<'a, str> = if name.is_ascii() {
            if name.bytes().any(|b| b.is_ascii_uppercase()) {
                Cow::Owned(name.to_ascii_lowercase())
            } else {
                Cow::Borrowed(name)
            }
        } else {
            let ascii = idna::domain_to_ascii_cow(name.as_bytes(), idna::AsciiDenyList::URL)
                .map_err(|_| invalid_host(host, "not a valid internationalised domain name"))?;
            Cow::Owned(ascii.into_owned())
        };
        validate_dns_name(&dns).map_err(|why| invalid_host(host, why))?;
        Ok(LeafName::Dns(dns))
    }

    /// Key for the leaf cache: identical for every spelling of the same identity.
    pub(crate) fn cache_key(&self) -> Cow<'_, str> {
        match self {
            LeafName::Dns(name) => Cow::Borrowed(name),
            LeafName::Ip(ip) => Cow::Owned(ip.to_string()),
        }
    }

    /// Subject common name, when it fits X.520's 64-character bound.
    pub(crate) fn common_name(&self) -> Option<String> {
        let cn = match self {
            LeafName::Dns(name) => name.to_string(),
            LeafName::Ip(ip) => ip.to_string(),
        };
        (cn.len() <= MAX_COMMON_NAME_LEN).then_some(cn)
    }

    /// The `*.parent` sibling added for DNS names with three or more labels.
    pub(crate) fn wildcard_sibling(&self) -> Option<String> {
        let LeafName::Dns(name) = self else {
            return None;
        };
        let (_, parent) = name.split_once('.')?;
        if !parent.contains('.') || is_likely_public_suffix(parent) {
            return None; // never `*.com`, `*.co.uk`, ...
        }
        Some(format!("*.{parent}"))
    }

    /// subjectAltName entries: the exact identity first, then the wildcard sibling.
    pub(crate) fn subject_alt_names(&self) -> Result<Vec<SanType>> {
        match self {
            LeafName::Ip(ip) => Ok(vec![SanType::IpAddress(*ip)]),
            LeafName::Dns(name) => {
                let mut sans = Vec::with_capacity(2);
                sans.push(SanType::DnsName(ia5(name)?));
                if let Some(wildcard) = self.wildcard_sibling() {
                    sans.push(SanType::DnsName(ia5(&wildcard)?));
                }
                Ok(sans)
            }
        }
    }
}

fn ia5(name: &str) -> Result<rcgen::string::Ia5String> {
    rcgen::string::Ia5String::try_from(name)
        .map_err(|e| CaError::Invalid(format!("host {name:?} cannot be encoded as a DNS SAN: {e}")))
}

/// Parse an IPv6 literal, dropping an RFC 6874 zone id (`fe80::1%eth0`), which has no
/// meaning outside the local host and cannot appear in a certificate.
fn parse_ipv6(text: &str) -> Option<Ipv6Addr> {
    let addr = text.split_once('%').map_or(text, |(addr, _zone)| addr);
    addr.parse().ok()
}

/// Check a lower-cased ASCII DNS name: label lengths and LDH characters, plus `_` which real
/// hosts use (and rustls/webpki accept) even though RFC 952 forbids it.
fn validate_dns_name(name: &str) -> std::result::Result<(), &'static str> {
    if name.is_empty() {
        return Err("empty host name");
    }
    if name.len() > MAX_DNS_NAME_LEN {
        return Err("host name longer than 253 characters");
    }
    for label in name.split('.') {
        if label.is_empty() {
            return Err("empty DNS label");
        }
        if label.len() > MAX_LABEL_LEN {
            return Err("DNS label longer than 63 characters");
        }
        if !label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
        {
            return Err("invalid character in host name");
        }
    }
    Ok(())
}

fn is_likely_public_suffix(parent: &str) -> bool {
    let Some((second_level, tld)) = parent.split_once('.') else {
        return true;
    };
    !tld.contains('.')
        && tld.len() == 2
        && tld.bytes().all(|b| b.is_ascii_lowercase())
        && CC_SECOND_LEVEL_SUFFIXES.contains(&second_level)
}

fn invalid_host(host: &str, why: &str) -> CaError {
    CaError::Invalid(format!(
        "cannot issue a certificate for host {host:?}: {why}"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dns(host: &str) -> String {
        match LeafName::parse(host).unwrap() {
            LeafName::Dns(name) => name.into_owned(),
            other => panic!("{host:?} parsed as {other:?}"),
        }
    }

    fn ip(host: &str) -> IpAddr {
        match LeafName::parse(host).unwrap() {
            LeafName::Ip(ip) => ip,
            other => panic!("{host:?} parsed as {other:?}"),
        }
    }

    fn san_strings(host: &str) -> Vec<String> {
        LeafName::parse(host)
            .unwrap()
            .subject_alt_names()
            .unwrap()
            .into_iter()
            .map(|san| match san {
                SanType::DnsName(name) => format!("DNS:{}", name.as_str()),
                SanType::IpAddress(ip) => format!("IP:{ip}"),
                other => panic!("unexpected SAN {other:?}"),
            })
            .collect()
    }

    #[test]
    fn normalises_case_whitespace_and_trailing_dot() {
        assert_eq!(dns("API.Example.COM."), "api.example.com");
        assert_eq!(dns("  example.com  "), "example.com");
        assert_eq!(dns("localhost"), "localhost");
    }

    #[test]
    fn already_normal_names_are_borrowed() {
        assert!(matches!(
            LeafName::parse("example.com").unwrap(),
            LeafName::Dns(Cow::Borrowed(_))
        ));
    }

    #[test]
    fn parses_ip_literals() {
        assert_eq!(ip("127.0.0.1"), IpAddr::from([127, 0, 0, 1]));
        assert_eq!(ip("127.0.0.1."), IpAddr::from([127, 0, 0, 1]));
        assert_eq!(ip("::1"), "::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip("[::1]"), "::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip("[fe80::1%eth0]"), "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip("fe80::1%25en0"), "fe80::1".parse::<IpAddr>().unwrap());
        assert_eq!(ip("2001:DB8::A"), "2001:db8::a".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn ip_cache_keys_are_canonical() {
        let a = LeafName::parse("[2001:DB8:0:0::1]").unwrap();
        let b = LeafName::parse("2001:db8::1").unwrap();
        assert_eq!(a.cache_key(), b.cache_key());
        assert_eq!(a.cache_key(), "2001:db8::1");
    }

    #[test]
    fn converts_idn_to_punycode() {
        assert_eq!(dns("Bücher.example"), "xn--bcher-kva.example");
        assert_eq!(dns("例え.テスト"), "xn--r8jz45g.xn--zckzah");
        assert_eq!(dns("straße.de"), "xn--strae-oqa.de");
        // Full-width dots / letters are mapped by UTS #46.
        assert_eq!(dns("ＥＸＡＭＰＬＥ．ｃｏｍ"), "example.com");
    }

    #[test]
    fn rejects_invalid_hosts() {
        for bad in [
            "",
            " ",
            ".",
            "..",
            "a..b",
            ".example.com",
            "exa mple.com",
            "*.example.com",
            "example.com:443",
            "1.2.3.4:443",
            "[::1",
            "[not-ip]",
            "foo/bar",
            "a@b.com",
        ] {
            let err = LeafName::parse(bad).expect_err(bad);
            assert!(matches!(err, CaError::Invalid(_)), "{bad:?}: {err}");
        }
        let long_label = format!("{}.com", "a".repeat(64));
        assert!(LeafName::parse(&long_label).is_err());
        let ok_label = format!("{}.com", "a".repeat(63));
        assert!(LeafName::parse(&ok_label).is_ok());
        let long_name = vec!["abcdefghi"; 26].join("."); // 259 chars
        assert!(LeafName::parse(&long_name).is_err());
        let huge = "ü".repeat(100_000);
        let err = LeafName::parse(&huge).unwrap_err().to_string();
        assert!(err.contains("200000-byte"), "{err}");
    }

    #[test]
    fn underscores_are_accepted() {
        assert_eq!(
            san_strings("my_svc.example.com"),
            ["DNS:my_svc.example.com", "DNS:*.example.com"]
        );
        assert_eq!(san_strings("_dmarc.example"), ["DNS:_dmarc.example"]);
    }

    #[test]
    fn wildcard_sibling_rules() {
        assert_eq!(
            san_strings("api.example.com"),
            ["DNS:api.example.com", "DNS:*.example.com"]
        );
        assert_eq!(
            san_strings("a.b.c.example.org"),
            ["DNS:a.b.c.example.org", "DNS:*.b.c.example.org"]
        );
        // Two labels: a wildcard would be `*.com`.
        assert_eq!(san_strings("example.com"), ["DNS:example.com"]);
        assert_eq!(san_strings("localhost"), ["DNS:localhost"]);
        // Public-suffix-looking parents.
        assert_eq!(san_strings("shop.co.uk"), ["DNS:shop.co.uk"]);
        assert_eq!(san_strings("foo.com.au"), ["DNS:foo.com.au"]);
        assert_eq!(
            san_strings("www.shop.co.uk"),
            ["DNS:www.shop.co.uk", "DNS:*.shop.co.uk"]
        );
        // Ordinary two-letter TLD with a non-suffix second level is fine.
        assert_eq!(
            san_strings("api.github.io"),
            ["DNS:api.github.io", "DNS:*.github.io"]
        );
        // IPs never get wildcards.
        assert_eq!(san_strings("10.1.2.3"), ["IP:10.1.2.3"]);
        assert_eq!(san_strings("[::1]"), ["IP:::1"]);
    }

    #[test]
    fn common_name_is_bounded() {
        assert_eq!(
            LeafName::parse("example.com")
                .unwrap()
                .common_name()
                .as_deref(),
            Some("example.com")
        );
        assert_eq!(
            LeafName::parse("[::1]").unwrap().common_name().as_deref(),
            Some("::1")
        );
        let long = format!("{}.{}.com", "a".repeat(40), "b".repeat(40));
        assert_eq!(LeafName::parse(&long).unwrap().common_name(), None);
    }
}
