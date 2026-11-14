//! Upstream proxy selection: direct, manual (`http://`, `https://`, `socks4[a]://`,
//! `socks5[h]://`), system (env vars + OS settings) and PAC, with bypass lists.

pub(crate) mod bypass;
pub(crate) mod pac;
pub(crate) mod system;
pub(crate) mod tunnel;

use super::UpstreamError;
use super::dns::DnsResolver;
use super::error::error;
use super::glob::normalize_host;
use akev_core::ErrorKind;
use akev_core::settings::{UpstreamProxy, UpstreamProxyMode};
use base64::Engine as _;
use bypass::Bypass;
use futures::future::BoxFuture;
use std::sync::Arc;

/// Upstream proxy protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ProxyKind {
    Http,
    /// HTTP proxy reached over TLS.
    Https,
    Socks4,
    /// SOCKS4 with proxy-side DNS.
    Socks4a,
    Socks5,
    /// SOCKS5 with proxy-side DNS.
    Socks5h,
}

impl ProxyKind {
    pub(crate) fn is_http(self) -> bool {
        matches!(self, ProxyKind::Http | ProxyKind::Https)
    }

    fn scheme(self) -> &'static str {
        match self {
            ProxyKind::Http => "http",
            ProxyKind::Https => "https",
            ProxyKind::Socks4 => "socks4",
            ProxyKind::Socks4a => "socks4a",
            ProxyKind::Socks5 => "socks5",
            ProxyKind::Socks5h => "socks5h",
        }
    }

    fn default_port(self) -> u16 {
        match self {
            ProxyKind::Http => 80,
            ProxyKind::Https => 443,
            _ => 1080,
        }
    }
}

/// A configured upstream proxy server.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ProxyServer {
    pub kind: ProxyKind,
    /// Normalised host (IPv6 without brackets).
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// `scheme://host:port` (no credentials) — shown as `UpstreamInfo::via_proxy`.
    pub display: Arc<str>,
    /// Identity for pool keys (includes the username, never the password).
    pub id: Arc<str>,
}

/// `host:port` with brackets for IPv6 literals.
pub(crate) fn host_port(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

impl ProxyServer {
    pub(crate) fn new(
        kind: ProxyKind,
        host: &str,
        port: u16,
        username: Option<String>,
        password: Option<String>,
    ) -> Self {
        let host = normalize_host(host).into_owned();
        let display: Arc<str> = format!("{}://{}", kind.scheme(), host_port(&host, port)).into();
        let id: Arc<str> = match &username {
            Some(u) => format!("{}://{}@{}", kind.scheme(), u, host_port(&host, port)).into(),
            None => display.clone(),
        };
        ProxyServer {
            kind,
            host,
            port,
            username,
            password,
            display,
            id,
        }
    }

    /// Parse a proxy URL (`http://user:pass@host:port`, `socks5h://host`, bare `host:port`
    /// = HTTP).
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err("empty upstream proxy URL".into());
        }
        let with_scheme = if raw.contains("://") {
            raw.to_owned()
        } else {
            format!("http://{raw}")
        };
        let url = url::Url::parse(&with_scheme)
            .map_err(|e| format!("invalid upstream proxy URL {raw:?}: {e}"))?;
        let kind = match url.scheme() {
            "http" => ProxyKind::Http,
            "https" => ProxyKind::Https,
            "socks4" => ProxyKind::Socks4,
            "socks4a" => ProxyKind::Socks4a,
            "socks" | "socks5" => ProxyKind::Socks5,
            "socks5h" => ProxyKind::Socks5h,
            other => {
                return Err(format!(
                    "unsupported upstream proxy scheme {other:?} in {raw:?}"
                ));
            }
        };
        let host = url
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| format!("upstream proxy URL {raw:?} has no host"))?;
        let port = url.port().unwrap_or(kind.default_port());
        let decode = |s: &str| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        };
        let username = (!url.username().is_empty()).then(|| decode(url.username()));
        let password = url.password().map(decode);
        Ok(ProxyServer::new(kind, host, port, username, password))
    }

    /// `Proxy-Authorization` value for HTTP proxies with credentials.
    pub(crate) fn basic_auth(&self) -> Option<String> {
        let user = self.username.as_deref()?;
        let pass = self.password.as_deref().unwrap_or("");
        Some(format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"))
        ))
    }
}

/// How to reach a target.
#[derive(Clone, Debug)]
pub(crate) enum Route {
    Direct,
    Proxy(Arc<ProxyServer>),
}

impl Route {
    /// Pool-key identity of the proxy, if any.
    pub(crate) fn proxy_id(&self) -> Option<Arc<str>> {
        match self {
            Route::Direct => None,
            Route::Proxy(p) => Some(p.id.clone()),
        }
    }
}

/// What proxy selection needs to know about a request.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ProxyTarget<'a> {
    /// `http`, `https`, `ws`, `wss`.
    pub scheme: &'a str,
    /// Normalised host.
    pub host: &'a str,
    pub port: u16,
    /// Full URL (PAC scripts get it; trimmed to the origin for TLS targets).
    pub url: &'a str,
}

impl ProxyTarget<'_> {
    fn is_tls(&self) -> bool {
        matches!(self.scheme, "https" | "wss")
    }
}

/// Fetches PAC scripts (implemented by the client, which owns the network stack).
pub(crate) trait PacFetcher: Send + Sync {
    fn fetch_pac<'a>(&'a self, url: &'a str) -> BoxFuture<'a, Result<String, UpstreamError>>;
}

/// Context for selection.
pub(crate) struct SelectEnv<'a> {
    pub pac: &'a pac::PacEngine,
    pub fetcher: &'a dyn PacFetcher,
    pub dns: &'a Arc<DnsResolver>,
}

enum Mode {
    Direct,
    Manual {
        server: Result<Arc<ProxyServer>, String>,
        bypass: Bypass,
    },
    System {
        source: system::SystemSource,
        bypass: Bypass,
    },
    Pac {
        source: Arc<pac::PacSource>,
        bypass: Bypass,
    },
}

/// Compiled `settings.upstream_proxy`.
pub(crate) struct ProxySelector {
    mode: Mode,
}

impl std::fmt::Debug for ProxySelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mode = match &self.mode {
            Mode::Direct => "direct",
            Mode::Manual { .. } => "manual",
            Mode::System { .. } => "system",
            Mode::Pac { .. } => "pac",
        };
        f.debug_struct("ProxySelector")
            .field("mode", &mode)
            .finish()
    }
}

impl ProxySelector {
    pub(crate) fn new(cfg: &UpstreamProxy) -> Self {
        let bypass = Bypass::new(cfg.no_proxy.iter().map(String::as_str));
        let mode = match cfg.mode {
            UpstreamProxyMode::Direct => Mode::Direct,
            UpstreamProxyMode::Manual if cfg.url.trim().is_empty() => Mode::Direct,
            UpstreamProxyMode::Manual => Mode::Manual {
                server: ProxyServer::parse(&cfg.url).map(Arc::new),
                bypass,
            },
            UpstreamProxyMode::System => Mode::System {
                source: system::SystemSource::new(),
                bypass,
            },
            UpstreamProxyMode::Pac => Mode::Pac {
                source: Arc::new(pac::PacSource::new(cfg.url.trim())),
                bypass,
            },
        };
        ProxySelector { mode }
    }

    /// Whether every request connects directly.
    pub(crate) fn is_direct(&self) -> bool {
        matches!(self.mode, Mode::Direct)
    }

    /// Ordered candidate routes for a request (later entries are fail-over alternatives).
    pub(crate) async fn select(
        &self,
        target: &ProxyTarget<'_>,
        env: &SelectEnv<'_>,
    ) -> Result<Vec<Route>, UpstreamError> {
        match &self.mode {
            Mode::Direct => Ok(vec![Route::Direct]),
            Mode::Manual { server, bypass } => {
                if bypass.matches(target.host, target.port) {
                    return Ok(vec![Route::Direct]);
                }
                match server {
                    Ok(s) => Ok(vec![Route::Proxy(s.clone())]),
                    Err(e) => Err(error(
                        ErrorKind::UpstreamProxyFailed,
                        e.clone(),
                        "PROXY_CONFIG",
                    )),
                }
            }
            Mode::Pac { source, bypass } => {
                if bypass.matches(target.host, target.port) {
                    return Ok(vec![Route::Direct]);
                }
                Ok(source.select(target, env).await)
            }
            Mode::System { source, bypass } => {
                if bypass.matches(target.host, target.port) {
                    return Ok(vec![Route::Direct]);
                }
                let sys = source.current().await;
                if sys.bypass.matches(target.host, target.port) {
                    return Ok(vec![Route::Direct]);
                }
                if let Some(pac) = &sys.pac {
                    return Ok(pac.select(target, env).await);
                }
                let server = if target.is_tls() {
                    sys.https.as_ref()
                } else {
                    sys.http.as_ref()
                }
                .or(sys.all.as_ref())
                .or(sys.socks.as_ref());
                Ok(vec![
                    server.map_or(Route::Direct, |s| Route::Proxy(s.clone())),
                ])
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proxy_urls() {
        let p = ProxyServer::parse("http://user:p%40ss@proxy.test:3128").unwrap();
        assert_eq!(p.kind, ProxyKind::Http);
        assert_eq!(p.host, "proxy.test");
        assert_eq!(p.port, 3128);
        assert_eq!(p.username.as_deref(), Some("user"));
        assert_eq!(p.password.as_deref(), Some("p@ss"));
        assert_eq!(&*p.display, "http://proxy.test:3128");
        assert_eq!(&*p.id, "http://user@proxy.test:3128");
        assert_eq!(p.basic_auth().unwrap(), "Basic dXNlcjpwQHNz");

        let p = ProxyServer::parse("proxy.local:8080").unwrap();
        assert_eq!((p.kind, p.port), (ProxyKind::Http, 8080));
        let p = ProxyServer::parse("socks5h://[::1]").unwrap();
        assert_eq!(
            (p.kind, p.host.as_str(), p.port),
            (ProxyKind::Socks5h, "::1", 1080)
        );
        assert_eq!(&*p.display, "socks5h://[::1]:1080");
        let p = ProxyServer::parse("https://secure-proxy.test").unwrap();
        assert_eq!((p.kind, p.port), (ProxyKind::Https, 443));
        assert!(ProxyServer::parse("ftp://nope:21").is_err());
        assert!(ProxyServer::parse("").is_err());
        assert!(ProxyServer::parse("http://").is_err());
    }

    struct NoFetch;
    impl PacFetcher for NoFetch {
        fn fetch_pac<'a>(&'a self, _url: &'a str) -> BoxFuture<'a, Result<String, UpstreamError>> {
            Box::pin(async { Err(super::super::error::internal("no network in tests")) })
        }
    }

    #[tokio::test]
    async fn manual_selection_with_bypass() {
        let sel = ProxySelector::new(&UpstreamProxy {
            mode: UpstreamProxyMode::Manual,
            url: "http://proxy.test:3128".into(),
            no_proxy: vec!["internal.test".into()],
        });
        let engine = pac::PacEngine::new();
        let dns = Arc::new(DnsResolver::new(&[], None));
        let env = SelectEnv {
            pac: &engine,
            fetcher: &NoFetch,
            dns: &dns,
        };
        let t = |host| ProxyTarget {
            scheme: "https",
            host,
            port: 443,
            url: "https://x/",
        };
        assert!(matches!(
            sel.select(&t("example.com"), &env).await.unwrap()[..],
            [Route::Proxy(_)]
        ));
        assert!(matches!(
            sel.select(&t("api.internal.test"), &env).await.unwrap()[..],
            [Route::Direct]
        ));
        assert!(matches!(
            sel.select(&t("localhost"), &env).await.unwrap()[..],
            [Route::Direct]
        ));

        let bad = ProxySelector::new(&UpstreamProxy {
            mode: UpstreamProxyMode::Manual,
            url: "gopher://x".into(),
            no_proxy: vec![],
        });
        let err = bad.select(&t("example.com"), &env).await.unwrap_err();
        assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);

        // PAC unavailable → DIRECT (browser behaviour).
        let pac_sel = ProxySelector::new(&UpstreamProxy {
            mode: UpstreamProxyMode::Pac,
            url: "http://wpad.invalid/proxy.pac".into(),
            no_proxy: vec![],
        });
        assert!(matches!(
            pac_sel.select(&t("example.com"), &env).await.unwrap()[..],
            [Route::Direct]
        ));
    }

    fn proxy_of(routes: &[Route]) -> Option<(ProxyKind, String, u16)> {
        match routes {
            [Route::Proxy(p)] => Some((p.kind, p.host.clone(), p.port)),
            [Route::Direct] => None,
            other => panic!("unexpected routes {other:?}"),
        }
    }

    #[tokio::test]
    async fn system_selection_by_scheme_with_fallbacks_and_bypass() {
        let engine = pac::PacEngine::new();
        let dns = Arc::new(DnsResolver::new(&[], None));
        let env = SelectEnv {
            pac: &engine,
            fetcher: &NoFetch,
            dns: &dns,
        };
        let target = |scheme, host| ProxyTarget {
            scheme,
            host,
            port: if scheme == "https" { 443 } else { 80 },
            url: "http://x/",
        };
        let system = |raw: system::SystemProxyConfig, no_proxy: &[&str]| ProxySelector {
            mode: Mode::System {
                source: system::SystemSource::fixed(raw),
                bypass: Bypass::new(no_proxy.iter().copied()),
            },
        };

        let sel = system(
            system::SystemProxyConfig {
                http: Some("http://web.proxy:3128".into()),
                https: Some("http://secure.proxy:3129".into()),
                no_proxy: vec!["*.corp".into()],
                source: "env",
                ..Default::default()
            },
            &["settings.bypass"],
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("https", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            Some((ProxyKind::Http, "secure.proxy".into(), 3129))
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("wss", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            Some((ProxyKind::Http, "secure.proxy".into(), 3129))
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("http", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            Some((ProxyKind::Http, "web.proxy".into(), 3128))
        );
        // OS bypass list, settings bypass list and the implicit loopback bypass.
        assert_eq!(
            proxy_of(
                &sel.select(&target("https", "git.corp"), &env)
                    .await
                    .unwrap()
            ),
            None
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("https", "api.settings.bypass"), &env)
                    .await
                    .unwrap()
            ),
            None
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("http", "localhost"), &env)
                    .await
                    .unwrap()
            ),
            None
        );

        // ALL_PROXY (and then a SOCKS proxy) apply when there's no per-scheme proxy.
        let sel = system(
            system::SystemProxyConfig {
                https: Some("http://secure.proxy:3129".into()),
                all: Some("socks5h://all.proxy:1080".into()),
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("http", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            Some((ProxyKind::Socks5h, "all.proxy".into(), 1080))
        );
        let sel = system(
            system::SystemProxyConfig {
                socks: Some("socks5://socks.proxy:1081".into()),
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("https", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            Some((ProxyKind::Socks5, "socks.proxy".into(), 1081))
        );

        // Nothing configured (or an invalid entry) → direct.
        let sel = system(
            system::SystemProxyConfig {
                http: Some("gopher://bad".into()),
                source: "none",
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("http", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            None
        );

        // A system PAC URL that can't be fetched → direct (browser behaviour).
        let sel = system(
            system::SystemProxyConfig {
                pac_url: Some("http://wpad.invalid/wpad.dat".into()),
                ..Default::default()
            },
            &[],
        );
        assert_eq!(
            proxy_of(
                &sel.select(&target("https", "example.com"), &env)
                    .await
                    .unwrap()
            ),
            None
        );
    }
}
