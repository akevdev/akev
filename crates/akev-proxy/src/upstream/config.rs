//! Compiled settings snapshot, swapped atomically on `update_settings`.

use super::dns::DnsResolver;
use super::fingerprint::ProfileSelector;
use super::proxy::ProxySelector;
use super::tls::TlsContext;
use akev_core::settings::ProxySettings;
use std::sync::Arc;
use std::time::Duration;

/// Everything the upstream client derives from [`ProxySettings`].
#[derive(Debug)]
pub(crate) struct Config {
    /// Bumped whenever DNS, TLS or proxy settings change; pooled connections from older
    /// generations are discarded.
    pub generation: u64,
    pub settings: ProxySettings,
    pub dns: Arc<DnsResolver>,
    pub tls: Arc<TlsContext>,
    pub proxy: Arc<ProxySelector>,
    /// `None` = no client-side connect timeout (`connect_timeout_ms == 0`).
    pub connect_timeout: Option<Duration>,
    /// Wait for the response head (`response_timeout_ms`, `None` when 0).
    pub response_timeout: Option<Duration>,
    /// Longest gap between response body chunks (`body_idle_timeout_ms`, `None` when 0).
    pub body_idle_timeout: Option<Duration>,
    /// Normalise HTTP/1 header names to Title-Case (`preserve_header_case == false`).
    pub title_case: bool,
    /// Upstream TLS fingerprint choice (`tls_profile`). Not part of the generation: the
    /// fingerprint is in the pool key.
    pub fingerprint: Arc<ProfileSelector>,
}

fn same_tls(a: &ProxySettings, b: &ProxySettings) -> bool {
    a.ignore_upstream_cert_errors == b.ignore_upstream_cert_errors
        && a.trusted_ca_paths == b.trusted_ca_paths
        && a.client_certificates == b.client_certificates
}

fn same_dns(a: &ProxySettings, b: &ProxySettings) -> bool {
    a.dns_overrides == b.dns_overrides && a.doh_url == b.doh_url
}

impl Config {
    /// Compile `settings`, reusing the unchanged components of `previous`.
    pub(crate) fn build(settings: &ProxySettings, previous: Option<&Config>) -> Config {
        let dns = match previous {
            Some(p) if same_dns(&p.settings, settings) => p.dns.clone(),
            _ => Arc::new(DnsResolver::new(
                &settings.dns_overrides,
                settings.doh_url.as_deref(),
            )),
        };
        let tls = match previous {
            Some(p) if same_tls(&p.settings, settings) => p.tls.clone(),
            _ => Arc::new(TlsContext::new(settings)),
        };
        let proxy = match previous {
            Some(p) if p.settings.upstream_proxy == settings.upstream_proxy => p.proxy.clone(),
            _ => Arc::new(ProxySelector::new(&settings.upstream_proxy)),
        };
        let generation = match previous {
            None => 1,
            Some(p)
                if Arc::ptr_eq(&p.dns, &dns)
                    && Arc::ptr_eq(&p.tls, &tls)
                    && Arc::ptr_eq(&p.proxy, &proxy) =>
            {
                p.generation
            }
            Some(p) => p.generation + 1,
        };
        Config {
            generation,
            settings: settings.clone(),
            dns,
            tls,
            proxy,
            connect_timeout: (settings.connect_timeout_ms > 0)
                .then(|| Duration::from_millis(settings.connect_timeout_ms)),
            response_timeout: (settings.response_timeout_ms > 0)
                .then(|| Duration::from_millis(settings.response_timeout_ms)),
            body_idle_timeout: (settings.body_idle_timeout_ms > 0)
                .then(|| Duration::from_millis(settings.body_idle_timeout_ms)),
            title_case: !settings.preserve_header_case,
            fingerprint: match previous {
                Some(p) if p.settings.tls_profile == settings.tls_profile => p.fingerprint.clone(),
                _ => Arc::new(ProfileSelector::new(&settings.tls_profile)),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::settings::{DnsOverride, UpstreamProxy, UpstreamProxyMode};

    #[test]
    fn generation_tracks_transport_changes() {
        let base = ProxySettings::default();
        let a = Config::build(&base, None);
        assert_eq!(a.generation, 1);
        assert_eq!(a.connect_timeout, Some(Duration::from_millis(15_000)));
        assert_eq!(a.response_timeout, Some(Duration::from_secs(120)));
        assert_eq!(a.body_idle_timeout, Some(Duration::from_secs(300)));
        assert!(!a.title_case);

        // Timeouts only: same generation, components reused.
        let b = Config::build(
            &ProxySettings {
                response_timeout_ms: 5_000,
                body_idle_timeout_ms: 0,
                ..base.clone()
            },
            Some(&a),
        );
        assert_eq!(b.generation, 1);
        assert!(
            Arc::ptr_eq(&a.dns, &b.dns)
                && Arc::ptr_eq(&a.tls, &b.tls)
                && Arc::ptr_eq(&a.proxy, &b.proxy)
        );
        assert_eq!(b.response_timeout, Some(Duration::from_secs(5)));
        assert_eq!(b.body_idle_timeout, None, "0 = no limit");

        let c = Config::build(
            &ProxySettings {
                dns_overrides: vec![DnsOverride {
                    host: "a".into(),
                    address: "10.0.0.1".into(),
                }],
                ..base.clone()
            },
            Some(&b),
        );
        assert_eq!(c.generation, 2);
        assert!(Arc::ptr_eq(&b.tls, &c.tls));
        assert!(!Arc::ptr_eq(&b.dns, &c.dns));

        let d = Config::build(
            &ProxySettings {
                upstream_proxy: UpstreamProxy {
                    mode: UpstreamProxyMode::Manual,
                    url: "http://p:1".into(),
                    no_proxy: vec![],
                },
                ..c.settings.clone()
            },
            Some(&c),
        );
        assert_eq!(d.generation, 3);
        let e = Config::build(
            &ProxySettings {
                ignore_upstream_cert_errors: vec!["*".into()],
                ..d.settings.clone()
            },
            Some(&d),
        );
        assert_eq!(e.generation, 4);
        let f = Config::build(
            &ProxySettings {
                connect_timeout_ms: 0,
                preserve_header_case: false,
                ..e.settings.clone()
            },
            Some(&e),
        );
        assert_eq!(f.generation, 4);
        assert_eq!(f.connect_timeout, None);
        assert!(f.title_case);
    }

    #[test]
    fn tls_profile_changes_keep_the_pools() {
        use akev_core::settings::{TlsProfile, TlsProfileSettings};
        let a = Config::build(&ProxySettings::default(), None);
        let same = Config::build(&ProxySettings::default(), Some(&a));
        assert!(Arc::ptr_eq(&a.fingerprint, &same.fingerprint));
        // The fingerprint is part of the pool key, so connections don't need to be dropped.
        let b = Config::build(
            &ProxySettings {
                tls_profile: TlsProfileSettings {
                    profile: TlsProfile::Firefox,
                    overrides: Vec::new(),
                },
                ..Default::default()
            },
            Some(&a),
        );
        assert_eq!(b.generation, a.generation);
        assert!(!Arc::ptr_eq(&a.fingerprint, &b.fingerprint));
        assert!(Arc::ptr_eq(&a.tls, &b.tls));
    }
}
