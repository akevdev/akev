//! Upstream DNS resolution.
//!
//! Order of precedence for a hostname:
//! 1. IP literals are returned as-is (no lookup).
//! 2. `settings.dns_overrides` (exact host first, then globs in order). An override address
//!    may be one or more IPs (`"10.0.0.1, ::1"`) or another hostname (an alias that is then
//!    resolved normally).
//! 3. `localhost` / `*.localhost` resolve locally to `::1` + `127.0.0.1` (RFC 6761) — happy
//!    eyeballs then picks whichever the local server listens on.
//! 4. A TTL-honouring cache.
//! 5. hickory-resolver: the system configuration (`/etc/resolv.conf`, hosts file, registry) by
//!    default, or DNS-over-HTTPS when `settings.doh_url` is set. In system mode, names hickory
//!    can't resolve that may live in other name services (mDNS `.local`, single-label names,
//!    NSS modules) and resolver infrastructure failures fall back to the OS `getaddrinfo`.

use super::UpstreamError;
use super::error::error;
use super::glob::{HostPattern, normalize_host};
use akev_core::ErrorKind;
use akev_core::settings::DnsOverride;
use hickory_resolver::TokioResolver;
use hickory_resolver::config::{LookupIpStrategy, NameServerConfig, ResolverConfig};
use hickory_resolver::net::NetError;
use hickory_resolver::net::runtime::TokioRuntimeProvider;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Resolved addresses (cheaply cloneable).
pub(crate) type Addrs = Arc<[IpAddr]>;

const MAX_CACHE_ENTRIES: usize = 4096;
/// TTL used for results without one (getaddrinfo), like Chrome's host cache.
const FALLBACK_TTL: Duration = Duration::from_secs(60);
const MAX_TTL: Duration = Duration::from_secs(3600);

#[derive(Clone, Debug, PartialEq)]
enum OverrideTarget {
    Ips(Addrs),
    Alias(Box<str>),
}

#[derive(Debug, Default)]
struct Overrides {
    exact: HashMap<Box<str>, OverrideTarget>,
    globs: Vec<(HostPattern, OverrideTarget)>,
}

impl Overrides {
    fn parse(list: &[DnsOverride]) -> Self {
        let mut out = Overrides::default();
        for o in list {
            let Some(pattern) = HostPattern::parse(&o.host) else {
                continue;
            };
            let Some(target) = parse_override_target(&o.address) else {
                tracing::warn!(host = %o.host, address = %o.address, "ignoring invalid DNS override");
                continue;
            };
            if pattern.is_glob() {
                out.globs.push((pattern, target));
            } else {
                // First entry wins, like /etc/hosts.
                out.exact.entry(pattern.host().into()).or_insert(target);
            }
        }
        out
    }

    fn lookup(&self, host: &str) -> Option<&OverrideTarget> {
        if let Some(t) = self.exact.get(host) {
            return Some(t);
        }
        self.globs
            .iter()
            .find(|(p, _)| p.matches_host(host))
            .map(|(_, t)| t)
    }
}

fn parse_override_target(address: &str) -> Option<OverrideTarget> {
    let tokens: Vec<&str> = address
        .split([',', ' ', ';', '\t'])
        .filter(|t| !t.is_empty())
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let ips: Vec<IpAddr> = tokens.iter().filter_map(|t| parse_ip(t)).collect();
    if ips.len() == tokens.len() {
        return Some(OverrideTarget::Ips(ips.into()));
    }
    if tokens.len() == 1
        && tokens[0]
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-._".contains(&b))
    {
        return Some(OverrideTarget::Alias(normalize_host(tokens[0]).into()));
    }
    None
}

/// Parse an IP literal, accepting IPv6 in brackets.
pub(crate) fn parse_ip(s: &str) -> Option<IpAddr> {
    let s = s
        .strip_prefix('[')
        .and_then(|r| r.strip_suffix(']'))
        .unwrap_or(s);
    s.parse().ok()
}

/// `localhost` or a `*.localhost` name.
pub(crate) fn is_localhost_name(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

fn localhost_addrs() -> Addrs {
    Arc::from([
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
    ])
}

/// Names that commonly live outside unicast DNS (mDNS, NSS, search domains).
fn is_local_style_name(name: &str) -> bool {
    !name.contains('.')
        || name.ends_with(".local")
        || name.ends_with(".internal")
        || name.ends_with(".lan")
}

#[derive(Debug)]
struct DohEndpoint {
    url: String,
    host: String,
    port: u16,
    path: String,
}

impl DohEndpoint {
    fn parse(raw: &str) -> Result<Self, String> {
        let url =
            url::Url::parse(raw.trim()).map_err(|e| format!("invalid DoH URL {raw:?}: {e}"))?;
        if url.scheme() != "https" {
            return Err(format!("DoH URL must use https: {raw:?}"));
        }
        let host = url
            .host_str()
            .ok_or_else(|| format!("DoH URL has no host: {raw:?}"))?;
        let host = normalize_host(host).into_owned();
        let mut path = url.path().to_owned();
        if let Some(q) = url.query() {
            path.push('?');
            path.push_str(q);
        }
        if path.is_empty() || path == "/" {
            path = "/dns-query".into();
        }
        Ok(DohEndpoint {
            url: raw.trim().to_owned(),
            host,
            port: url.port().unwrap_or(443),
            path,
        })
    }
}

enum Backend {
    System(OnceLock<Option<TokioResolver>>),
    Doh {
        endpoint: Result<DohEndpoint, String>,
        resolver: tokio::sync::OnceCell<TokioResolver>,
    },
}

struct CacheEntry {
    addrs: Addrs,
    expires: Instant,
}

/// The upstream resolver (one per DNS-settings generation).
pub(crate) struct DnsResolver {
    overrides: Overrides,
    backend: Backend,
    cache: Mutex<HashMap<Box<str>, CacheEntry>>,
}

impl std::fmt::Debug for DnsResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DnsResolver")
            .field("overrides", &self.overrides)
            .finish_non_exhaustive()
    }
}

impl DnsResolver {
    /// Build from settings. Never fails: an invalid DoH URL surfaces as a [`ErrorKind::DnsFailed`]
    /// on lookups that need the network.
    pub(crate) fn new(overrides: &[DnsOverride], doh_url: Option<&str>) -> Self {
        let backend = match doh_url.map(str::trim).filter(|u| !u.is_empty()) {
            Some(url) => Backend::Doh {
                endpoint: DohEndpoint::parse(url),
                resolver: tokio::sync::OnceCell::new(),
            },
            None => Backend::System(OnceLock::new()),
        };
        DnsResolver {
            overrides: Overrides::parse(overrides),
            backend,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Answer without any lookup: IP literal, IP override, localhost. Returns the name to
    /// look up otherwise (the host itself or an override alias).
    fn resolve_local<'a>(&'a self, host: &'a str) -> Result<Addrs, &'a str> {
        if let Some(ip) = parse_ip(host) {
            return Ok(Arc::from([ip]));
        }
        let name = match self.overrides.lookup(host) {
            Some(OverrideTarget::Ips(ips)) => return Ok(ips.clone()),
            Some(OverrideTarget::Alias(alias)) => {
                if let Some(ip) = parse_ip(alias) {
                    return Ok(Arc::from([ip]));
                }
                &**alias
            }
            None => host,
        };
        if is_localhost_name(name) {
            return Ok(localhost_addrs());
        }
        Err(name)
    }

    /// Resolve `host` to one or more addresses (DNS order preserved).
    pub(crate) async fn resolve(&self, host: &str) -> Result<Addrs, UpstreamError> {
        let host = normalize_host(host);
        let name = match self.resolve_local(&host) {
            Ok(addrs) => return Ok(addrs),
            Err(name) => name,
        };
        if name.is_empty() {
            return Err(error(
                ErrorKind::DnsFailed,
                "cannot resolve an empty hostname",
                "ENOTFOUND",
            ));
        }
        if let Some(addrs) = self.cache_get(name) {
            return Ok(addrs);
        }
        let (addrs, ttl) = self.lookup(name).await?;
        self.cache_put(name, addrs.clone(), ttl);
        Ok(addrs)
    }

    /// Blocking resolution (PAC `dnsResolve` runs on its own thread): overrides, cache, then
    /// the OS resolver.
    pub(crate) fn resolve_blocking(&self, host: &str) -> Option<Addrs> {
        let host = normalize_host(host);
        let name = match self.resolve_local(&host) {
            Ok(addrs) => return Some(addrs),
            Err(name) => name,
        };
        if let Some(addrs) = self.cache_get(name) {
            return Some(addrs);
        }
        let addrs = dedup_ips((name, 0u16).to_socket_addrs().ok()?.map(|sa| sa.ip()));
        if addrs.is_empty() {
            return None;
        }
        let addrs: Addrs = addrs.into();
        self.cache_put(name, addrs.clone(), FALLBACK_TTL);
        Some(addrs)
    }

    fn cache_get(&self, name: &str) -> Option<Addrs> {
        let cache = self.cache.lock();
        cache
            .get(name)
            .filter(|e| e.expires > Instant::now())
            .map(|e| e.addrs.clone())
    }

    fn cache_put(&self, name: &str, addrs: Addrs, ttl: Duration) {
        if ttl.is_zero() {
            return;
        }
        let now = Instant::now();
        let mut cache = self.cache.lock();
        if cache.len() >= MAX_CACHE_ENTRIES && !cache.contains_key(name) {
            cache.retain(|_, e| e.expires > now);
            if cache.len() >= MAX_CACHE_ENTRIES {
                cache.clear();
            }
        }
        cache.insert(
            name.into(),
            CacheEntry {
                addrs,
                expires: now + ttl.min(MAX_TTL),
            },
        );
    }

    async fn lookup(&self, name: &str) -> Result<(Addrs, Duration), UpstreamError> {
        match &self.backend {
            Backend::System(cell) => {
                let mut hickory_error = None;
                if let Some(resolver) = cell.get_or_init(build_system_resolver) {
                    match resolver.lookup_ip(name).await {
                        Ok(lookup) => {
                            let ttl = lookup
                                .valid_until()
                                .saturating_duration_since(Instant::now());
                            let addrs = dedup_ips(lookup.iter());
                            if !addrs.is_empty() {
                                return Ok((addrs.into(), ttl));
                            }
                        }
                        Err(e) => {
                            let not_found = e.is_nx_domain() || e.is_no_records_found();
                            if not_found && !is_local_style_name(name) {
                                return Err(map_resolve_error(name, &e));
                            }
                            hickory_error = Some(e);
                        }
                    }
                }
                match getaddrinfo(name).await {
                    Ok(addrs) => Ok((addrs, FALLBACK_TTL)),
                    Err(gai) => Err(match hickory_error {
                        Some(e) => map_resolve_error(name, &e),
                        None => gai,
                    }),
                }
            }
            Backend::Doh { endpoint, resolver } => {
                let endpoint = endpoint
                    .as_ref()
                    .map_err(|e| error(ErrorKind::DnsFailed, e.clone(), "DOH_CONFIG"))?;
                let resolver = resolver
                    .get_or_try_init(|| build_doh_resolver(endpoint))
                    .await?;
                let lookup = resolver
                    .lookup_ip(name)
                    .await
                    .map_err(|e| map_resolve_error(name, &e))?;
                let ttl = lookup
                    .valid_until()
                    .saturating_duration_since(Instant::now());
                let addrs = dedup_ips(lookup.iter());
                if addrs.is_empty() {
                    return Err(not_found(name));
                }
                Ok((addrs.into(), ttl))
            }
        }
    }
}

fn dedup_ips(iter: impl Iterator<Item = IpAddr>) -> Vec<IpAddr> {
    let mut out: Vec<IpAddr> = Vec::with_capacity(4);
    for ip in iter {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    out
}

fn build_system_resolver() -> Option<TokioResolver> {
    let built = TokioResolver::builder_tokio().and_then(|mut builder| {
        builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
        builder.build()
    });
    match built {
        Ok(resolver) => Some(resolver),
        Err(e) => {
            tracing::debug!(error = %e, "system DNS configuration unavailable; using getaddrinfo");
            None
        }
    }
}

async fn build_doh_resolver(endpoint: &DohEndpoint) -> Result<TokioResolver, UpstreamError> {
    let ips: Vec<IpAddr> = match parse_ip(&endpoint.host) {
        Some(ip) => vec![ip],
        None => getaddrinfo(&endpoint.host)
            .await
            .map_err(|e| {
                error(
                    ErrorKind::DnsFailed,
                    format!(
                        "could not resolve DNS-over-HTTPS server {}: {}",
                        endpoint.url, e.message
                    ),
                    "DOH_BOOTSTRAP",
                )
            })?
            .to_vec(),
    };
    let server_name: std::sync::Arc<str> = endpoint.host.as_str().into();
    let path: std::sync::Arc<str> = endpoint.path.as_str().into();
    let servers: Vec<NameServerConfig> = ips
        .into_iter()
        .map(|ip| {
            let mut ns = NameServerConfig::https(ip, server_name.clone(), Some(path.clone()));
            for c in &mut ns.connections {
                c.port = endpoint.port;
            }
            ns
        })
        .collect();
    let config = ResolverConfig::from_parts(None, Vec::new(), servers);
    let tls = super::tls::doh_client_config().map_err(|e| {
        error(
            ErrorKind::DnsFailed,
            format!("could not set up DNS-over-HTTPS with {}: {e}", endpoint.url),
            "DOH_CONFIG",
        )
    })?;
    let mut builder = TokioResolver::builder_with_config(config, TokioRuntimeProvider::default())
        .with_tls_config(tls);
    builder.options_mut().ip_strategy = LookupIpStrategy::Ipv4AndIpv6;
    builder.build().map_err(|e| {
        error(
            ErrorKind::DnsFailed,
            format!("could not set up DNS-over-HTTPS with {}: {e}", endpoint.url),
            "DOH_CONFIG",
        )
    })
}

async fn getaddrinfo(name: &str) -> Result<Addrs, UpstreamError> {
    match tokio::net::lookup_host((name, 0u16)).await {
        Ok(iter) => {
            let addrs = dedup_ips(iter.map(|sa| sa.ip()));
            if addrs.is_empty() {
                Err(not_found(name))
            } else {
                Ok(addrs.into())
            }
        }
        Err(e) => {
            tracing::debug!(host = name, error = %e, "getaddrinfo failed");
            Err(not_found(name))
        }
    }
}

fn not_found(name: &str) -> UpstreamError {
    error(
        ErrorKind::DnsFailed,
        format!("could not resolve host {name}: no such host"),
        "ENOTFOUND",
    )
}

fn map_resolve_error(name: &str, e: &NetError) -> UpstreamError {
    if e.is_nx_domain() || e.is_no_records_found() {
        return not_found(name);
    }
    if matches!(e, NetError::Timeout) {
        return error(
            ErrorKind::DnsFailed,
            format!("DNS lookup for {name} timed out"),
            "EAI_AGAIN",
        );
    }
    error(
        ErrorKind::DnsFailed,
        format!("DNS lookup for {name} failed: {e}"),
        "EAI_FAIL",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov(host: &str, address: &str) -> DnsOverride {
        DnsOverride {
            host: host.into(),
            address: address.into(),
        }
    }

    #[tokio::test]
    async fn literals_overrides_and_localhost() {
        let r = DnsResolver::new(
            &[
                ov("api.test", "10.1.2.3"),
                ov("*.wild.test", "10.9.9.9, ::2"),
                ov("api.test", "10.0.0.0"),
                ov("alias.test", "localhost"),
                ov("bad.test", "not valid !"),
            ],
            None,
        );
        assert_eq!(
            &*r.resolve("1.2.3.4").await.unwrap(),
            &["1.2.3.4".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            &*r.resolve("[::1]").await.unwrap(),
            &["::1".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            &*r.resolve("API.test.").await.unwrap(),
            &["10.1.2.3".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            &*r.resolve("a.b.wild.test").await.unwrap(),
            &[
                "10.9.9.9".parse::<IpAddr>().unwrap(),
                "::2".parse::<IpAddr>().unwrap()
            ]
        );
        let local = r.resolve("localhost").await.unwrap();
        assert_eq!(local.len(), 2);
        assert!(local.iter().all(|ip| ip.is_loopback()));
        assert!(
            r.resolve("my.app.localhost")
                .await
                .unwrap()
                .iter()
                .all(|ip| ip.is_loopback())
        );
        assert!(
            r.resolve("alias.test")
                .await
                .unwrap()
                .iter()
                .all(|ip| ip.is_loopback())
        );
        assert!(r.resolve_blocking("api.test").is_some());
    }

    #[tokio::test]
    async fn nxdomain_is_dns_failed() {
        let r = DnsResolver::new(&[], None);
        let err = r.resolve("does-not-exist.invalid").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::DnsFailed);
    }

    #[test]
    fn cache_respects_ttl_and_bounds() {
        let r = DnsResolver::new(&[], None);
        let addrs: Addrs = Arc::from(["10.0.0.1".parse::<IpAddr>().unwrap()]);
        r.cache_put("a.test", addrs.clone(), Duration::from_secs(30));
        assert_eq!(r.cache_get("a.test").as_deref(), Some(&*addrs));
        r.cache_put("zero.test", addrs.clone(), Duration::ZERO);
        assert!(r.cache_get("zero.test").is_none());
        {
            let mut cache = r.cache.lock();
            cache.get_mut("a.test").unwrap().expires = Instant::now() - Duration::from_secs(1);
        }
        assert!(r.cache_get("a.test").is_none());
        for i in 0..(MAX_CACHE_ENTRIES + 10) {
            r.cache_put(
                &format!("h{i}.test"),
                addrs.clone(),
                Duration::from_secs(30),
            );
        }
        assert!(r.cache.lock().len() <= MAX_CACHE_ENTRIES);
    }

    #[test]
    fn doh_url_parsing() {
        let ep = DohEndpoint::parse("https://dns.example/dns-query?x=1").unwrap();
        assert_eq!(ep.host, "dns.example");
        assert_eq!(ep.port, 443);
        assert_eq!(ep.path, "/dns-query?x=1");
        let ep = DohEndpoint::parse("https://1.1.1.1").unwrap();
        assert_eq!(ep.path, "/dns-query");
        assert!(DohEndpoint::parse("http://insecure.example/").is_err());
        assert!(DohEndpoint::parse("::nonsense").is_err());
    }

    #[tokio::test]
    async fn invalid_doh_url_fails_lookups_but_not_overrides() {
        let r = DnsResolver::new(&[ov("x.test", "10.0.0.5")], Some("ftp://nope"));
        assert!(r.resolve("x.test").await.is_ok());
        let err = r.resolve("example.com").await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::DnsFailed);
        assert_eq!(err.code.as_deref(), Some("DOH_CONFIG"));
    }
}
