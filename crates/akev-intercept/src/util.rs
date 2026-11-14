//! Shared plumbing: the [`Sys`] context every interceptor holds, TTL probe caches, process
//! snapshots, proxy address helpers and small file utilities.

use crate::host::HostEnv;
use crate::runner::{ChildHandle, CommandRunner, ProcessEntry, ProcessSignal, SystemRunner};
use crate::{InterceptContext, InterceptError, Result};
use akev_core::interceptor::{InterceptorCategory, InterceptorInfo};
use std::future::Future;
use std::io;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long availability probes are cached.
pub(crate) const PROBE_TTL: Duration = Duration::from_secs(5);
/// How long process-table snapshots are reused.
const PROCESS_TTL: Duration = Duration::from_secs(2);
/// Grace period between a polite terminate and a forceful kill.
pub(crate) const GRACEFUL_STOP: Duration = if cfg!(test) {
    Duration::from_millis(200)
} else {
    Duration::from_secs(5)
};

/// Shared context for all interceptors: how to run commands and what machine we're on.
pub struct Sys {
    /// Command runner (real or mock).
    pub runner: Arc<dyn CommandRunner>,
    /// Host environment snapshot.
    pub env: HostEnv,
    processes: ProcessCache,
}

impl Sys {
    /// Context for this machine with the real [`SystemRunner`].
    pub fn system() -> Arc<Self> {
        Self::new(Arc::new(SystemRunner), HostEnv::current())
    }

    /// Context with a custom runner and environment.
    pub fn new(runner: Arc<dyn CommandRunner>, env: HostEnv) -> Arc<Self> {
        Arc::new(Sys {
            runner,
            env,
            processes: ProcessCache::default(),
        })
    }

    /// A (cached, ≤2s old) process-table snapshot. Failures yield an empty list.
    pub(crate) async fn processes(&self, with_environ: bool) -> Arc<Vec<ProcessEntry>> {
        self.processes.get(self.runner.as_ref(), with_environ).await
    }

    /// Drop cached process snapshots (after launching or killing something).
    pub(crate) async fn invalidate_processes(&self) {
        self.processes.invalidate().await;
    }
}

/// A single-flight TTL cache: concurrent callers share one probe, results are reused for
/// the TTL.
pub(crate) struct ProbeCache<T> {
    ttl: Duration,
    slot: tokio::sync::Mutex<Option<(Instant, T)>>,
}

impl<T: Clone> ProbeCache<T> {
    pub(crate) fn new(ttl: Duration) -> Self {
        ProbeCache {
            ttl,
            slot: tokio::sync::Mutex::new(None),
        }
    }

    /// Cached value, or run `probe` (holding the lock, so concurrent callers wait for it).
    pub(crate) async fn get<F, Fut>(&self, probe: F) -> T
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = T>,
    {
        let mut slot = self.slot.lock().await;
        if let Some((at, value)) = slot.as_ref()
            && at.elapsed() < self.ttl
        {
            return value.clone();
        }
        let value = probe().await;
        *slot = Some((Instant::now(), value.clone()));
        value
    }

    pub(crate) async fn invalidate(&self) {
        *self.slot.lock().await = None;
    }
}

#[derive(Default)]
struct ProcessCache {
    plain: tokio::sync::Mutex<Option<(Instant, Arc<Vec<ProcessEntry>>)>>,
    environ: tokio::sync::Mutex<Option<(Instant, Arc<Vec<ProcessEntry>>)>>,
}

impl ProcessCache {
    async fn get(&self, runner: &dyn CommandRunner, with_environ: bool) -> Arc<Vec<ProcessEntry>> {
        let mut slot = if with_environ {
            self.environ.lock().await
        } else {
            self.plain.lock().await
        };
        if let Some((at, procs)) = slot.as_ref()
            && at.elapsed() < PROCESS_TTL
        {
            return procs.clone();
        }
        let procs = match runner.processes(with_environ).await {
            Ok(p) => Arc::new(p),
            Err(e) => {
                tracing::debug!("process scan failed: {e}");
                Arc::new(Vec::new())
            }
        };
        *slot = Some((Instant::now(), procs.clone()));
        procs
    }

    async fn invalidate(&self) {
        *self.plain.lock().await = None;
        *self.environ.lock().await = None;
    }
}

/// Static description of an interceptor (the parts of [`InterceptorInfo`] that never change).
#[derive(Clone, Copy, Debug)]
pub(crate) struct Descriptor {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: InterceptorCategory,
    pub tags: &'static [&'static str],
    pub icon: &'static str,
    pub needs_config: bool,
}

impl Descriptor {
    /// Base info: supported, unavailable, inactive, no metadata.
    pub(crate) fn info(&self) -> InterceptorInfo {
        InterceptorInfo {
            id: self.id.to_string(),
            name: self.name.to_string(),
            description: self.description.to_string(),
            category: self.category,
            tags: self.tags.iter().map(|t| t.to_string()).collect(),
            icon: self.icon.to_string(),
            supported: true,
            available: false,
            active: false,
            unavailable_reason: None,
            version: None,
            metadata: serde_json::Value::Null,
            needs_config: self.needs_config,
        }
    }

    /// Info for an interceptor that doesn't exist on this OS.
    pub(crate) fn unsupported(&self, os_name: &str) -> InterceptorInfo {
        InterceptorInfo {
            supported: false,
            unavailable_reason: Some(format!("{} is not supported on {os_name}", self.name)),
            ..self.info()
        }
    }
}

/// Tag groups (mirroring the HTTP Toolkit catalogue so searches behave the same).
pub(crate) mod tags {
    pub const BROWSER: &[&str] = &["browsers", "web", "pwa", "browser"];
    pub const TERMINAL: &[&str] = &[
        "terminal",
        "command line",
        "cli",
        "bash",
        "zsh",
        "fish",
        "powershell",
        "cmd",
        "shell",
        "curl",
        "node",
        "js",
        "python",
        "ruby",
        "php",
        "go",
        "rust",
        "jvm",
        "java",
        "kotlin",
        "scala",
        "clojure",
        "groovy",
    ];
    pub const ANDROID: &[&str] = &[
        "mobile", "phone", "app", "android", "samsung", "galaxy", "pixel", "google", "nokia", "lg",
        "motorola", "emulator", "adb", "usb", "jvm", "java", "kotlin",
    ];
    pub const IOS: &[&str] = &[
        "mobile", "phone", "app", "apple", "ios", "iphone", "ipad", "qr",
    ];
    pub const DOCKER: &[&str] = &[
        "docker",
        "podman",
        "container",
        "containers",
        "bridge",
        "services",
        "images",
    ];
    pub const SYSTEM: &[&str] = &[
        "everything",
        "system",
        "global",
        "local",
        "machine",
        "me",
        "os",
    ];
    pub const ELECTRON: &[&str] = &["electron", "desktop", "app", "postman", "slack", "vscode"];
    pub const MANUAL: &[&str] = &["manual", "anything", "proxy", "pac", "certificate", "ca"];
}

/// The proxy host local clients should use: `ctx.proxy_host`, with unspecified bind
/// addresses (`0.0.0.0`, `::`) and empty values mapped to loopback.
pub(crate) fn local_proxy_host(ctx: &InterceptContext) -> String {
    let host = ctx
        .proxy_host
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']');
    match host.parse::<IpAddr>() {
        Ok(ip) if ip.is_unspecified() => {
            if ip.is_ipv4() {
                "127.0.0.1".into()
            } else {
                "::1".into()
            }
        }
        _ if host.is_empty() => "127.0.0.1".into(),
        _ => host.to_string(),
    }
}

/// `host` ready to embed in a URL / `host:port` (IPv6 literals get brackets).
pub(crate) fn host_for_url(host: &str) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    }
}

/// `host:port` of the proxy for local clients.
pub(crate) fn local_proxy_addr(ctx: &InterceptContext) -> String {
    format!(
        "{}:{}",
        host_for_url(&local_proxy_host(ctx)),
        ctx.proxy_port
    )
}

/// `http://host:port` of the proxy for local clients.
pub(crate) fn local_proxy_url(ctx: &InterceptContext) -> String {
    format!("http://{}", local_proxy_addr(ctx))
}

/// Read a string option from an activation options object.
pub(crate) fn opt_str<'a>(options: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    options
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Read a bool option.
pub(crate) fn opt_bool(options: &serde_json::Value, key: &str) -> Option<bool> {
    options.get(key).and_then(|v| v.as_bool())
}

/// Validate a start URL for a browser: http(s), about: or file: only, and never something
/// that could be parsed as a command-line flag.
pub(crate) fn validate_start_url(url: &str) -> Result<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    let ok_scheme = ["http://", "https://", "about:", "file://"]
        .iter()
        .any(|s| lower.starts_with(s));
    if url.is_empty() || url.starts_with('-') || !ok_scheme || url.chars().any(char::is_control) {
        return Err(InterceptError::Failed(format!(
            "invalid start URL `{url}` (expected an http://, https://, about: or file:// URL)"
        )));
    }
    Ok(url.to_string())
}

/// Write `contents` to `path` atomically (temp file + rename), creating parent directories.
pub(crate) fn atomic_write(path: &Path, contents: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{file_name}.{}.tmp", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Write only if the content differs (avoids needless disk writes on every refresh).
pub(crate) fn write_if_changed(path: &Path, contents: &[u8]) -> io::Result<()> {
    match std::fs::read(path) {
        Ok(existing) if existing == contents => Ok(()),
        _ => atomic_write(path, contents),
    }
}

/// Mark a file as executable (no-op off Unix).
pub(crate) fn make_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Short lowercase hex SHA-256 of some bytes (cache keys, file names).
pub(crate) fn short_hash(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(data)[..8])
}

/// Stop processes politely, then forcefully: `Terminate` (or `Hangup` for shells) all
/// tracked children and the given foreign pids, wait up to [`GRACEFUL_STOP`], then `Kill`
/// whatever is left.
pub(crate) async fn stop_processes(
    sys: &Sys,
    children: &[ChildHandle],
    foreign_pids: &[u32],
    polite: ProcessSignal,
) {
    for child in children {
        child.signal(polite);
    }
    for pid in foreign_pids {
        if let Err(e) = sys.runner.signal(*pid, polite).await {
            tracing::debug!(pid, "signal failed: {e}");
        }
    }

    let deadline = Instant::now() + GRACEFUL_STOP;
    for child in children {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if !child.wait_timeout(remaining).await {
            tracing::debug!(pid = ?child.pid(), "process ignored polite stop; killing");
            child.signal(ProcessSignal::Kill);
            child.wait_timeout(Duration::from_secs(2)).await;
        }
    }

    if !foreign_pids.is_empty() {
        // Poll the process table until they're gone or the deadline passes.
        loop {
            sys.invalidate_processes().await;
            let procs = sys.processes(false).await;
            let alive: Vec<u32> = foreign_pids
                .iter()
                .copied()
                .filter(|pid| procs.iter().any(|p| p.pid == *pid))
                .collect();
            if alive.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                for pid in alive {
                    let _ = sys.runner.signal(pid, ProcessSignal::Kill).await;
                }
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    sys.invalidate_processes().await;
}

/// Send `signal` to processes Akev didn't start itself (e.g. the shell of a terminal it
/// opened), without escalating: one that ignores it is the user's to keep.
pub(crate) async fn signal_foreign(sys: &Sys, pids: &[u32], signal: ProcessSignal) {
    for pid in pids {
        if let Err(e) = sys.runner.signal(*pid, signal).await {
            tracing::debug!(pid, "signal failed: {e}");
        }
    }
    sys.invalidate_processes().await;
}

#[cfg(test)]
pub(crate) mod testctx {
    use super::*;

    /// A test context with a real (throwaway) CA PEM and the data dir in `dir`.
    pub fn ctx(dir: &Path) -> InterceptContext {
        let ca_path = dir.join("ca.pem");
        let pem = "-----BEGIN CERTIFICATE-----\nMIIBszCCAVmgAwIBAgIUAKEVTESTCERT\n-----END CERTIFICATE-----\n";
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(&ca_path, pem).unwrap();
        InterceptContext {
            proxy_host: "127.0.0.1".into(),
            proxy_port: 8000,
            lan_addresses: vec!["192.168.1.20".into()],
            ca_pem: pem.into(),
            ca_der: [0x30, 0x82, 0x01, 0xb3]
                .into_iter()
                .chain(0xA0..0xC0u8)
                .collect(),
            ca_path,
            ca_spki_fingerprint: "dGVzdC1zcGtpLWZpbmdlcnByaW50LWFrZXZvLWNhLTEyMzQ=".into(),
            ca_subject_hash_old: "c8750f0d".into(),
            data_dir: dir.to_path_buf(),
            api_url: "http://127.0.0.1:45456".into(),
            api_token: "tok123".into(),
            transparent: None,
            proxy_credentials: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testctx::ctx;
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn probe_cache_reuses_and_invalidates() {
        let cache = ProbeCache::new(Duration::from_secs(60));
        let calls = AtomicUsize::new(0);
        let probe = || async {
            calls.fetch_add(1, Ordering::SeqCst);
            42
        };
        assert_eq!(cache.get(probe).await, 42);
        assert_eq!(cache.get(probe).await, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        cache.invalidate().await;
        assert_eq!(cache.get(probe).await, 42);
        assert_eq!(calls.load(Ordering::SeqCst), 2);

        let short = ProbeCache::new(Duration::from_millis(0));
        short.get(probe).await;
        short.get(probe).await;
        assert_eq!(calls.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn proxy_hosts() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = ctx(dir.path());
        assert_eq!(local_proxy_url(&c), "http://127.0.0.1:8000");
        c.proxy_host = "0.0.0.0".into();
        assert_eq!(local_proxy_host(&c), "127.0.0.1");
        c.proxy_host = "::".into();
        assert_eq!(local_proxy_url(&c), "http://[::1]:8000");
        c.proxy_host = "::1".into();
        assert_eq!(local_proxy_addr(&c), "[::1]:8000");
        c.proxy_host = "".into();
        assert_eq!(local_proxy_host(&c), "127.0.0.1");
        c.proxy_host = "localhost".into();
        assert_eq!(local_proxy_url(&c), "http://localhost:8000");
    }

    #[test]
    fn start_url_validation() {
        assert!(validate_start_url("https://example.com/a?b=c").is_ok());
        assert!(validate_start_url("about:blank").is_ok());
        assert!(validate_start_url("HTTP://EXAMPLE.COM").is_ok());
        assert!(validate_start_url("--disable-web-security").is_err());
        assert!(validate_start_url("javascript:alert(1)").is_err());
        assert!(validate_start_url("chrome://settings").is_err());
        assert!(validate_start_url("").is_err());
        assert!(validate_start_url("http://a\nb").is_err());
    }

    #[test]
    fn options_helpers() {
        let o = serde_json::json!({"a": " x ", "b": true, "l": ["1", " ", 2, "3"], "e": ""});
        assert_eq!(opt_str(&o, "a"), Some("x"));
        assert_eq!(opt_str(&o, "missing"), None);
        assert_eq!(opt_str(&o, "e"), None);
        assert_eq!(opt_bool(&o, "b"), Some(true));
        assert_eq!(opt_str(&serde_json::Value::Null, "a"), None);
    }

    #[test]
    fn atomic_and_conditional_writes() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("sub/file.txt");
        atomic_write(&p, b"one").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        let mtime = std::fs::metadata(&p).unwrap().modified().unwrap();
        std::thread::sleep(Duration::from_millis(20));
        write_if_changed(&p, b"one").unwrap();
        assert_eq!(std::fs::metadata(&p).unwrap().modified().unwrap(), mtime);
        write_if_changed(&p, b"two").unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        // No temp files left behind.
        assert_eq!(
            std::fs::read_dir(dir.path().join("sub")).unwrap().count(),
            1
        );
    }

    #[test]
    fn hashes_are_stable() {
        assert_eq!(short_hash(b"abc"), "ba7816bf8f01cfea");
    }
}
