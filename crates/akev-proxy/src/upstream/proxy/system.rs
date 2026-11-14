//! OS proxy configuration (best effort), in order of precedence:
//! 1. environment variables (`http_proxy`/`HTTP_PROXY`, `https_proxy`/`HTTPS_PROXY`,
//!    `all_proxy`/`ALL_PROXY`, `no_proxy`/`NO_PROXY`);
//! 2. Linux: KDE `kioslaverc` (on KDE sessions) or GNOME `gsettings org.gnome.system.proxy`;
//!    macOS: `scutil --proxy`; Windows: the WinINET registry settings (`reg query`).
//!
//! Detection spawns processes, so results are cached for [`REFRESH_INTERVAL`] and refreshed
//! in the background (stale-while-revalidate): the request path never waits after the first
//! detection.

use super::ProxyServer;
use super::bypass::Bypass;
use super::pac::PacSource;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long detected settings are used before re-detecting.
const REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Max time an OS tool may take.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
const COMMAND_TIMEOUT: Duration = Duration::from_secs(3);

/// Raw detected configuration (proxy URLs as strings).
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct SystemProxyConfig {
    pub http: Option<String>,
    pub https: Option<String>,
    pub socks: Option<String>,
    pub all: Option<String>,
    pub no_proxy: Vec<String>,
    pub pac_url: Option<String>,
    /// Where it came from (`env`, `gnome`, `kde`, `macos`, `windows`, `none`).
    pub source: &'static str,
}

/// Compiled configuration used for selection.
#[derive(Default)]
pub(crate) struct CompiledSystem {
    pub http: Option<Arc<ProxyServer>>,
    pub https: Option<Arc<ProxyServer>>,
    pub socks: Option<Arc<ProxyServer>>,
    pub all: Option<Arc<ProxyServer>>,
    pub bypass: Bypass,
    pub pac: Option<Arc<PacSource>>,
    raw: SystemProxyConfig,
}

impl CompiledSystem {
    fn compile(raw: SystemProxyConfig, previous: Option<&CompiledSystem>) -> Self {
        let parse = |s: &Option<String>| {
            s.as_deref().filter(|s| !s.trim().is_empty()).and_then(|s| {
                match ProxyServer::parse(s) {
                    Ok(p) => Some(Arc::new(p)),
                    Err(e) => {
                        tracing::warn!(error = %e, "ignoring invalid system proxy setting");
                        None
                    }
                }
            })
        };
        // Keep the PAC source (and its cached script) when the URL didn't change.
        let pac = raw
            .pac_url
            .as_deref()
            .filter(|u| !u.trim().is_empty())
            .map(|url| {
                match previous
                    .and_then(|p| p.pac.as_ref())
                    .filter(|p| p.url() == url.trim())
                {
                    Some(existing) => existing.clone(),
                    None => Arc::new(PacSource::new(url.trim())),
                }
            });
        CompiledSystem {
            http: parse(&raw.http),
            https: parse(&raw.https),
            socks: parse(&raw.socks),
            all: parse(&raw.all),
            bypass: Bypass::new(raw.no_proxy.iter().map(String::as_str)),
            pac,
            raw,
        }
    }
}

/// A detection result and when it was taken.
type Detected = (Arc<CompiledSystem>, Instant);

/// Cached, self-refreshing system proxy configuration.
pub(crate) struct SystemSource {
    state: Arc<Mutex<Option<Detected>>>,
    init: tokio::sync::Mutex<()>,
    refreshing: Arc<AtomicBool>,
}

impl SystemSource {
    pub(crate) fn new() -> Self {
        SystemSource {
            state: Arc::new(Mutex::new(None)),
            init: tokio::sync::Mutex::new(()),
            refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The current configuration (detects on first use; refreshes in the background).
    pub(crate) async fn current(&self) -> Arc<CompiledSystem> {
        let cached = self.state.lock().clone();
        if let Some((cfg, at)) = cached {
            if at.elapsed() >= REFRESH_INTERVAL && !self.refreshing.swap(true, Ordering::AcqRel) {
                self.spawn_refresh(cfg.clone());
            }
            return cfg;
        }
        let _init = self.init.lock().await;
        if let Some((cfg, _)) = self.state.lock().as_ref() {
            return cfg.clone();
        }
        let raw = tokio::task::spawn_blocking(detect)
            .await
            .unwrap_or_default();
        let compiled = Arc::new(CompiledSystem::compile(raw, None));
        *self.state.lock() = Some((compiled.clone(), Instant::now()));
        compiled
    }

    fn spawn_refresh(&self, previous: Arc<CompiledSystem>) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            self.refreshing.store(false, Ordering::Release);
            return;
        };
        let state = self.state.clone();
        let refreshing = self.refreshing.clone();
        runtime.spawn_blocking(move || {
            let raw = detect();
            let compiled = if raw == previous.raw {
                previous
            } else {
                Arc::new(CompiledSystem::compile(raw, Some(&previous)))
            };
            *state.lock() = Some((compiled, Instant::now()));
            refreshing.store(false, Ordering::Release);
        });
    }

    /// A source with a fixed, already detected configuration (tests).
    #[cfg(test)]
    pub(crate) fn fixed(raw: SystemProxyConfig) -> Self {
        let source = SystemSource::new();
        *source.state.lock() = Some((Arc::new(CompiledSystem::compile(raw, None)), Instant::now()));
        source
    }
}

/// Detect the OS configuration (blocking; may spawn processes).
pub(crate) fn detect() -> SystemProxyConfig {
    if let Some(cfg) = from_env(|k| std::env::var(k).ok()) {
        return cfg;
    }
    #[cfg(target_os = "linux")]
    {
        let desktop = std::env::var("XDG_CURRENT_DESKTOP")
            .unwrap_or_default()
            .to_ascii_lowercase();
        if desktop.contains("kde")
            && let Some(path) = dirs_config().map(|d| d.join("kioslaverc"))
            && let Some(cfg) = std::fs::read_to_string(path)
                .ok()
                .and_then(|c| parse_kioslaverc(&c))
        {
            return cfg;
        }
        if let Some(cfg) = run_command("gsettings", &["list-recursively", "org.gnome.system.proxy"])
            .and_then(|out| parse_gsettings(&out))
        {
            return cfg;
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(cfg) = run_command("scutil", &["--proxy"]).and_then(|out| parse_scutil(&out)) {
            return cfg;
        }
    }
    #[cfg(windows)]
    {
        if let Some(cfg) = run_command(
            "reg",
            &[
                "query",
                r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings",
            ],
        )
        .and_then(|out| parse_windows_reg(&out))
        {
            return cfg;
        }
    }
    SystemProxyConfig {
        source: "none",
        ..Default::default()
    }
}

#[cfg(target_os = "linux")]
fn dirs_config() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
}

/// Run a command with a timeout, returning stdout on success.
#[cfg(any(target_os = "linux", target_os = "macos", windows))]
fn run_command(program: &str, args: &[&str]) -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = String::new();
                child.stdout.take()?.read_to_string(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if started.elapsed() < COMMAND_TIMEOUT => {
                std::thread::sleep(Duration::from_millis(10))
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn non_empty(v: Option<String>) -> Option<String> {
    v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// Proxy configuration from environment variables (lowercase names win, like curl).
pub(crate) fn from_env(get: impl Fn(&str) -> Option<String>) -> Option<SystemProxyConfig> {
    let var = |lower: &str, upper: &str| non_empty(get(lower)).or_else(|| non_empty(get(upper)));
    let http = var("http_proxy", "HTTP_PROXY");
    let https = var("https_proxy", "HTTPS_PROXY");
    let all = var("all_proxy", "ALL_PROXY");
    if http.is_none() && https.is_none() && all.is_none() {
        return None;
    }
    let no_proxy = var("no_proxy", "NO_PROXY")
        .map(|v| vec![v])
        .unwrap_or_default();
    Some(SystemProxyConfig {
        http,
        https,
        all,
        no_proxy,
        source: "env",
        ..Default::default()
    })
}

#[cfg(any(target_os = "linux", test))]
fn unquote(v: &str) -> String {
    let v = v.trim();
    v.strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(v)
        .to_owned()
}

#[cfg(any(target_os = "linux", test))]
fn parse_gvariant_list(v: &str) -> Vec<String> {
    let v = v.trim().trim_start_matches("@as").trim();
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or("");
    inner
        .split(',')
        .map(unquote)
        .filter(|s| !s.is_empty())
        .collect()
}

fn proxy_url(scheme: &str, host: &str, port: &str, auth: Option<(&str, &str)>) -> Option<String> {
    let host = host.trim();
    let port: u16 = port.trim().parse().ok()?;
    if host.is_empty() || port == 0 {
        return None;
    }
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let creds = match auth {
        Some((u, p)) if !u.is_empty() => {
            let enc = |s: &str| {
                percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC)
                    .to_string()
            };
            format!("{}:{}@", enc(u), enc(p))
        }
        _ => String::new(),
    };
    Some(format!("{scheme}://{creds}{host}:{port}"))
}

#[cfg(any(target_os = "linux", test))]
/// Parse `gsettings list-recursively org.gnome.system.proxy`.
pub(crate) fn parse_gsettings(output: &str) -> Option<SystemProxyConfig> {
    let mut values = std::collections::HashMap::new();
    for line in output.lines() {
        let mut parts = line.splitn(3, ' ');
        let (Some(schema), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        values.insert(format!("{schema} {key}"), value.trim().to_owned());
    }
    let get = |k: &str| values.get(k).map(|v| unquote(v)).unwrap_or_default();
    let mode = get("org.gnome.system.proxy mode");
    let mut cfg = SystemProxyConfig {
        source: "gnome",
        ..Default::default()
    };
    match mode.as_str() {
        "manual" => {
            let auth =
                (get("org.gnome.system.proxy.http use-authentication") == "true").then(|| {
                    (
                        get("org.gnome.system.proxy.http authentication-user"),
                        get("org.gnome.system.proxy.http authentication-password"),
                    )
                });
            let auth_ref = auth.as_ref().map(|(u, p)| (u.as_str(), p.as_str()));
            cfg.http = proxy_url(
                "http",
                &get("org.gnome.system.proxy.http host"),
                &get("org.gnome.system.proxy.http port"),
                auth_ref,
            );
            cfg.https = proxy_url(
                "http",
                &get("org.gnome.system.proxy.https host"),
                &get("org.gnome.system.proxy.https port"),
                auth_ref,
            );
            cfg.socks = proxy_url(
                "socks5",
                &get("org.gnome.system.proxy.socks host"),
                &get("org.gnome.system.proxy.socks port"),
                None,
            );
            cfg.no_proxy = values
                .get("org.gnome.system.proxy ignore-hosts")
                .map(|v| parse_gvariant_list(v))
                .unwrap_or_default();
        }
        "auto" => {
            cfg.pac_url = non_empty(Some(get("org.gnome.system.proxy autoconfig-url")));
            cfg.no_proxy = values
                .get("org.gnome.system.proxy ignore-hosts")
                .map(|v| parse_gvariant_list(v))
                .unwrap_or_default();
        }
        "none" => {}
        _ => return None,
    }
    Some(cfg)
}

#[cfg(any(target_os = "linux", test))]
/// Parse KDE's `kioslaverc`.
pub(crate) fn parse_kioslaverc(content: &str) -> Option<SystemProxyConfig> {
    let mut in_section = false;
    let mut values = std::collections::HashMap::new();
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[Proxy Settings]";
            continue;
        }
        if in_section && let Some((k, v)) = line.split_once('=') {
            values.insert(k.trim().to_owned(), v.trim().to_owned());
        }
    }
    let kde_url = |key: &str, default_scheme: &str| -> Option<String> {
        let v = values.get(key)?.trim();
        if v.is_empty() {
            return None;
        }
        // "http://host 8080" or "host:8080"
        let v = match v.rsplit_once(' ') {
            Some((h, p)) => format!("{}:{}", h.trim(), p.trim()),
            None => v.to_owned(),
        };
        Some(if v.contains("://") {
            v
        } else {
            format!("{default_scheme}://{v}")
        })
    };
    let mut cfg = SystemProxyConfig {
        source: "kde",
        ..Default::default()
    };
    match values.get("ProxyType").map(String::as_str) {
        Some("1") => {
            cfg.http = kde_url("httpProxy", "http");
            cfg.https = kde_url("httpsProxy", "http");
            cfg.socks =
                kde_url("socksProxy", "socks5").map(|u| u.replacen("socks://", "socks5://", 1));
            cfg.no_proxy = values.get("NoProxyFor").cloned().into_iter().collect();
        }
        Some("2") => {
            cfg.pac_url = values
                .get("Proxy Config Script")
                .cloned()
                .filter(|s| !s.is_empty())
        }
        Some("4") => return from_env(|k| std::env::var(k).ok()),
        Some("0") => {}
        _ => return None,
    }
    Some(cfg)
}

/// Parse `scutil --proxy` (macOS).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn parse_scutil(output: &str) -> Option<SystemProxyConfig> {
    if !output.contains("<dictionary>") {
        return None;
    }
    let mut values = std::collections::HashMap::new();
    let mut exceptions = Vec::new();
    let mut in_exceptions = false;
    for line in output.lines() {
        let line = line.trim();
        if in_exceptions {
            if line.starts_with('}') {
                in_exceptions = false;
            } else if let Some((_, v)) = line.split_once(" : ") {
                exceptions.push(v.trim().to_owned());
            }
            continue;
        }
        if let Some((k, v)) = line.split_once(" : ") {
            if k.trim() == "ExceptionsList" {
                in_exceptions = true;
            } else {
                values.insert(k.trim().to_owned(), v.trim().to_owned());
            }
        }
    }
    let enabled = |k: &str| values.get(k).is_some_and(|v| v == "1");
    let get = |k: &str| values.get(k).cloned().unwrap_or_default();
    let mut cfg = SystemProxyConfig {
        source: "macos",
        ..Default::default()
    };
    if enabled("ProxyAutoConfigEnable") {
        cfg.pac_url = non_empty(values.get("ProxyAutoConfigURLString").cloned());
    }
    if enabled("HTTPEnable") {
        cfg.http = proxy_url("http", &get("HTTPProxy"), &get("HTTPPort"), None);
    }
    if enabled("HTTPSEnable") {
        cfg.https = proxy_url("http", &get("HTTPSProxy"), &get("HTTPSPort"), None);
    }
    if enabled("SOCKSEnable") {
        cfg.socks = proxy_url("socks5", &get("SOCKSProxy"), &get("SOCKSPort"), None);
    }
    if enabled("ExcludeSimpleHostnames") {
        exceptions.push("<local>".into());
    }
    cfg.no_proxy = exceptions;
    Some(cfg)
}

/// Parse `reg query "HKCU\...\Internet Settings"` output (Windows).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_windows_reg(output: &str) -> Option<SystemProxyConfig> {
    let mut values = std::collections::HashMap::new();
    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(kind)) = (parts.next(), parts.next()) else {
            continue;
        };
        if !kind.starts_with("REG_") {
            continue;
        }
        values.insert(name.to_owned(), parts.collect::<Vec<_>>().join(" "));
    }
    if values.is_empty() {
        return None;
    }
    let mut cfg = SystemProxyConfig {
        source: "windows",
        ..Default::default()
    };
    cfg.pac_url = non_empty(values.get("AutoConfigURL").cloned());
    let enabled = values
        .get("ProxyEnable")
        .is_some_and(|v| v.trim() == "0x1" || v.trim() == "1");
    if enabled {
        if let Some(server) = non_empty(values.get("ProxyServer").cloned()) {
            if server.contains('=') {
                for part in server.split(';') {
                    let Some((proto, addr)) = part.split_once('=') else {
                        continue;
                    };
                    let addr = addr.trim();
                    if addr.is_empty() {
                        continue;
                    }
                    let with_scheme = |scheme: &str| {
                        if addr.contains("://") {
                            addr.to_owned()
                        } else {
                            format!("{scheme}://{addr}")
                        }
                    };
                    match proto.trim().to_ascii_lowercase().as_str() {
                        "http" => cfg.http = Some(with_scheme("http")),
                        "https" => cfg.https = Some(with_scheme("http")),
                        "socks" => cfg.socks = Some(with_scheme("socks4a")),
                        _ => {}
                    }
                }
            } else {
                cfg.all = Some(if server.contains("://") {
                    server
                } else {
                    format!("http://{server}")
                });
            }
        }
        if let Some(over) = values.get("ProxyOverride") {
            cfg.no_proxy = over
                .split(';')
                .map(|s| s.trim().to_owned())
                .filter(|s| !s.is_empty())
                .collect();
        }
    }
    Some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_vars() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(from_env(env(&[("NO_PROXY", "x")])).is_none());
        let cfg = from_env(env(&[
            ("HTTP_PROXY", "http://upper:1"),
            ("http_proxy", "http://lower:2"),
            ("HTTPS_PROXY", "http://secure:3"),
            ("no_proxy", "localhost,.corp"),
        ]))
        .unwrap();
        assert_eq!(cfg.http.as_deref(), Some("http://lower:2"));
        assert_eq!(cfg.https.as_deref(), Some("http://secure:3"));
        assert_eq!(cfg.no_proxy, vec!["localhost,.corp".to_string()]);
        let compiled = CompiledSystem::compile(cfg, None);
        assert!(compiled.bypass.matches("git.corp", 443));
        assert_eq!(compiled.http.as_ref().unwrap().port, 2);
    }

    #[test]
    fn gsettings() {
        let out = "org.gnome.system.proxy autoconfig-url ''\n\
org.gnome.system.proxy ignore-hosts ['localhost', '127.0.0.0/8', '::1', '*.corp']\n\
org.gnome.system.proxy mode 'manual'\n\
org.gnome.system.proxy.http authentication-password 'p@ss'\n\
org.gnome.system.proxy.http authentication-user 'bob'\n\
org.gnome.system.proxy.http host 'proxy.example'\n\
org.gnome.system.proxy.http port 8080\n\
org.gnome.system.proxy.http use-authentication true\n\
org.gnome.system.proxy.https host 'secure.example'\n\
org.gnome.system.proxy.https port 8443\n\
org.gnome.system.proxy.socks host ''\n\
org.gnome.system.proxy.socks port 0\n";
        let cfg = parse_gsettings(out).unwrap();
        assert_eq!(
            cfg.http.as_deref(),
            Some("http://bob:p%40ss@proxy.example:8080")
        );
        assert_eq!(
            cfg.https.as_deref(),
            Some("http://bob:p%40ss@secure.example:8443")
        );
        assert!(cfg.socks.is_none());
        assert_eq!(
            cfg.no_proxy,
            vec!["localhost", "127.0.0.0/8", "::1", "*.corp"]
        );
        let compiled = CompiledSystem::compile(cfg, None);
        assert_eq!(
            compiled.http.as_ref().unwrap().password.as_deref(),
            Some("p@ss")
        );

        let auto = parse_gsettings("org.gnome.system.proxy mode 'auto'\norg.gnome.system.proxy autoconfig-url 'http://wpad/wpad.dat'\n").unwrap();
        assert_eq!(auto.pac_url.as_deref(), Some("http://wpad/wpad.dat"));
        let none = parse_gsettings("org.gnome.system.proxy mode 'none'\n").unwrap();
        assert_eq!(
            none,
            SystemProxyConfig {
                source: "gnome",
                ..Default::default()
            }
        );
        assert!(parse_gsettings("garbage").is_none());
    }

    #[test]
    fn kioslaverc() {
        let content = "[General]\nfoo=bar\n[Proxy Settings]\nNoProxyFor=localhost,.lan\nProxyType=1\nhttpProxy=http://proxy.kde 3128\nhttpsProxy=http://proxy.kde 3129\nsocksProxy=socks://socks.kde 1080\n";
        let cfg = parse_kioslaverc(content).unwrap();
        assert_eq!(cfg.http.as_deref(), Some("http://proxy.kde:3128"));
        assert_eq!(cfg.https.as_deref(), Some("http://proxy.kde:3129"));
        assert_eq!(cfg.socks.as_deref(), Some("socks5://socks.kde:1080"));
        assert_eq!(cfg.no_proxy, vec!["localhost,.lan".to_string()]);
        let pac = parse_kioslaverc(
            "[Proxy Settings]\nProxyType=2\nProxy Config Script=http://wpad/x.pac\n",
        )
        .unwrap();
        assert_eq!(pac.pac_url.as_deref(), Some("http://wpad/x.pac"));
    }

    #[test]
    fn scutil() {
        let out = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  ExcludeSimpleHostnames : 1\n  FTPPassive : 1\n  HTTPEnable : 1\n  HTTPPort : 8080\n  HTTPProxy : proxy.mac\n  HTTPSEnable : 0\n  HTTPSPort : 8443\n  HTTPSProxy : secure.mac\n  ProxyAutoConfigEnable : 0\n  SOCKSEnable : 1\n  SOCKSPort : 1080\n  SOCKSProxy : socks.mac\n}\n";
        let cfg = parse_scutil(out).unwrap();
        assert_eq!(cfg.http.as_deref(), Some("http://proxy.mac:8080"));
        assert!(cfg.https.is_none());
        assert_eq!(cfg.socks.as_deref(), Some("socks5://socks.mac:1080"));
        assert_eq!(cfg.no_proxy, vec!["*.local", "169.254/16", "<local>"]);
        let compiled = CompiledSystem::compile(cfg, None);
        assert!(compiled.bypass.matches("169.254.1.1", 80));
        assert!(compiled.bypass.matches("printer", 80));
        assert!(parse_scutil("nothing").is_none());
    }

    #[test]
    fn windows_registry() {
        let out = "\r\nHKEY_CURRENT_USER\\Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings\r\n    ProxyEnable    REG_DWORD    0x1\r\n    ProxyServer    REG_SZ    http=web.proxy:8080;https=secure.proxy:8443;socks=socks.proxy:1080\r\n    ProxyOverride    REG_SZ    <local>;*.corp.example\r\n";
        let cfg = parse_windows_reg(out).unwrap();
        assert_eq!(cfg.http.as_deref(), Some("http://web.proxy:8080"));
        assert_eq!(cfg.https.as_deref(), Some("http://secure.proxy:8443"));
        assert_eq!(cfg.socks.as_deref(), Some("socks4a://socks.proxy:1080"));
        assert_eq!(cfg.no_proxy, vec!["<local>", "*.corp.example"]);

        let single = "    ProxyEnable    REG_DWORD    0x1\n    ProxyServer    REG_SZ    10.0.0.1:3128\n    AutoConfigURL    REG_SZ    http://wpad/proxy.pac\n";
        let cfg = parse_windows_reg(single).unwrap();
        assert_eq!(cfg.all.as_deref(), Some("http://10.0.0.1:3128"));
        assert_eq!(cfg.pac_url.as_deref(), Some("http://wpad/proxy.pac"));
        let disabled = parse_windows_reg(
            "    ProxyEnable    REG_DWORD    0x0\n    ProxyServer    REG_SZ    10.0.0.1:3128\n",
        )
        .unwrap();
        assert!(disabled.all.is_none());
    }

    #[test]
    fn compile_reuses_pac_source() {
        let raw = SystemProxyConfig {
            pac_url: Some("http://wpad/a.pac".into()),
            ..Default::default()
        };
        let first = CompiledSystem::compile(raw.clone(), None);
        let second = CompiledSystem::compile(raw, Some(&first));
        assert!(Arc::ptr_eq(
            first.pac.as_ref().unwrap(),
            second.pac.as_ref().unwrap()
        ));
        assert_eq!(second.raw.pac_url.as_deref(), Some("http://wpad/a.pac"));
    }
}
