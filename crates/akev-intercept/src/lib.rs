//! # akev-intercept
//!
//! One-click setup of clients so their traffic flows through Akev: launching pre-configured
//! browsers, terminals with proxy/CA env vars, Android devices over ADB, Docker containers,
//! running JVMs (Java agent attach), the OS-wide system proxy, and manual setup helpers.
//! Each is an [`Interceptor`]; the [`InterceptorRegistry`] owns them all.
//!
//! Design notes:
//! * **Never blocks the runtime**: every external command runs through
//!   [`runner::CommandRunner`] (`tokio::process` with hard timeouts).
//! * **Fast listing**: availability probes are filesystem checks cached for 5 s; versions are
//!   read from metadata files and cached until the installation changes; process-table
//!   scans are shared between interceptors.
//! * **Testable everywhere**: platform differences are decided at runtime from
//!   [`host::HostEnv`], so macOS/Windows logic is unit-tested on any OS with fake
//!   directories and a mock runner. Only real OS APIs are `cfg`-gated.
//! * **Crash-safe**: the system proxy and Android device settings are journaled before being
//!   changed and restored on the next start if Akev died. Journals live in a stable per-user
//!   location (the platform data directory, or its `recovery/` directory for any other data
//!   directory), so they survive ephemeral and custom data directories.

use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod android;
mod browsers;
mod chromium;
mod docker;
mod electron;
mod envvars;
mod firefox;
pub mod host;
mod ios_sim;
mod journal;
mod jvm;
mod launch;
mod manual;
pub mod runner;
mod scripts;
mod sysproxy;
mod terminal;
pub mod transparent;
pub mod trust;
mod util;

pub use chromium::{ChromiumLaunch, chromium_args};
pub use envvars::{APPEND_VARS, ENV_VAR_NAMES, ca_bundle_path, java_truststore_path};
pub use firefox::{CA_IMPORT_URL, firefox_user_js};
pub use scripts::Shell;
pub use trust::{CaTrust, TrustError, TrustReport, TrustStore};
pub use util::Sys;

/// Errors from interceptor operations.
#[derive(Debug, thiserror::Error)]
pub enum InterceptError {
    /// No interceptor with this id.
    #[error("interceptor not found: {0}")]
    NotFound(String),
    /// The interceptor can't be used right now (not installed, no device, unsupported OS).
    #[error("not available: {0}")]
    Unavailable(String),
    /// Activation/deactivation was attempted and failed (or the options were invalid).
    #[error("activation failed: {0}")]
    Failed(String),
    /// Filesystem error.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, InterceptError>;

/// Everything an interceptor needs to configure a client.
#[derive(Clone, Debug)]
pub struct InterceptContext {
    /// Proxy host clients should use (usually `127.0.0.1`; LAN IP for devices).
    pub proxy_host: String,
    /// Proxy port.
    pub proxy_port: u16,
    /// This machine's LAN IPv4 addresses (reachable from other devices), for mobile setup.
    pub lan_addresses: Vec<String>,
    /// CA certificate PEM & DER.
    pub ca_pem: String,
    /// CA certificate DER.
    pub ca_der: Vec<u8>,
    /// Path where the CA PEM is written (for env vars like SSL_CERT_FILE).
    pub ca_path: PathBuf,
    /// Base64 SHA-256 SPKI fingerprint of the CA (Chrome's --ignore-certificate-errors-spki-list).
    pub ca_spki_fingerprint: String,
    /// OpenSSL subject_hash_old of the CA (Android system store file name).
    pub ca_subject_hash_old: String,
    /// Akev data directory (profiles, overrides...).
    pub data_dir: PathBuf,
    /// Base URL of the Akev API (for terminal setup snippets), e.g. `http://127.0.0.1:45457`.
    pub api_url: String,
    /// API auth token (embedded in setup snippets that call back into the API).
    pub api_token: String,
    /// Live status of the proxy's transparent capture listener (`None` without a proxy).
    pub transparent: Option<akev_core::transparent::TransparentProbeHandle>,
    /// `(username, password)` clients on other machines (containers) must send when the proxy
    /// requires credentials (Settings → Proxy → Who can connect). Loopback clients don't.
    pub proxy_credentials: Option<(String, String)>,
}

/// A way of getting a client's traffic into Akev.
#[async_trait]
pub trait Interceptor: Send + Sync + 'static {
    /// Stable id (used by the UI and the API).
    fn id(&self) -> &str;
    /// Current info (availability is re-detected on each call; keep it fast — cache expensive
    /// probes for a few seconds).
    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo;
    /// Activate with interceptor-specific options (e.g. `{"deviceId": "..."}` for Android,
    /// `{"url": "https://example.com"}` for browsers).
    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult>;
    /// Deactivate (close launched browsers, remove device config, restore system proxy...).
    async fn deactivate(&self, ctx: &InterceptContext, options: serde_json::Value) -> Result<()>;
    /// Deactivate everything this interceptor started (called on shutdown).
    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()>;
}

/// Interceptor ids only listed when they're available (installed), to keep the catalogue
/// free of variants most people don't have.
const LISTED_ONLY_WHEN_AVAILABLE: &[&str] = &["firefox-developer", "firefox-nightly"];

/// Upper bound for one interceptor's `info()` inside [`InterceptorRegistry::list`].
const INFO_TIMEOUT: Duration = Duration::from_secs(8);
/// Upper bound for one interceptor's `deactivate_all()` during shutdown.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);

/// Owns all interceptors.
pub struct InterceptorRegistry {
    interceptors: Vec<Arc<dyn Interceptor>>,
}

impl Default for InterceptorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl InterceptorRegistry {
    /// Registry with all built-in interceptors for this platform.
    pub fn new() -> Self {
        Self::with(builtin_interceptors(Sys::system()))
    }

    /// Registry with exactly these interceptors (in listing order).
    pub fn with(interceptors: Vec<Arc<dyn Interceptor>>) -> Self {
        let mut unique: Vec<Arc<dyn Interceptor>> = Vec::with_capacity(interceptors.len());
        for i in interceptors {
            if unique.iter().any(|u| u.id() == i.id()) {
                tracing::warn!(id = i.id(), "duplicate interceptor id ignored");
                continue;
            }
            unique.push(i);
        }
        InterceptorRegistry {
            interceptors: unique,
        }
    }

    /// Registry with the built-in interceptors using a custom command runner and host
    /// environment (tests, sandboxes).
    pub fn with_sys(sys: Arc<Sys>) -> Self {
        Self::with(builtin_interceptors(sys))
    }

    /// The interceptor with this id.
    pub fn get(&self, id: &str) -> Option<&Arc<dyn Interceptor>> {
        self.interceptors.iter().find(|i| i.id() == id)
    }

    /// Ids in listing order.
    pub fn ids(&self) -> Vec<String> {
        self.interceptors
            .iter()
            .map(|i| i.id().to_string())
            .collect()
    }

    /// Info for every interceptor, probed concurrently (registry order).
    ///
    /// Interceptors that can't exist on this OS (e.g. Arc on Linux) are left out, as are
    /// rarely installed browser variants that aren't installed; both stay registered, so
    /// [`InterceptorRegistry::activate`] still explains why they can't be used.
    pub async fn list(&self, ctx: &InterceptContext) -> Vec<InterceptorInfo> {
        let infos = futures::future::join_all(self.interceptors.iter().map(|i| async move {
            match tokio::time::timeout(INFO_TIMEOUT, i.info(ctx)).await {
                Ok(info) => info,
                Err(_) => {
                    tracing::warn!(id = i.id(), "interceptor detection timed out");
                    timed_out_info(i.id())
                }
            }
        }))
        .await;
        infos
            .into_iter()
            .filter(|info| info.supported || info.active)
            .filter(|info| {
                info.available
                    || info.active
                    || !LISTED_ONLY_WHEN_AVAILABLE.contains(&info.id.as_str())
            })
            .collect()
    }

    /// Activate interceptor `id`.
    pub async fn activate(
        &self,
        id: &str,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        let interceptor = self
            .get(id)
            .ok_or_else(|| InterceptError::NotFound(id.to_string()))?;
        let options = if options.is_null() {
            serde_json::json!({})
        } else {
            options
        };
        interceptor.activate(ctx, options).await
    }

    /// Deactivate interceptor `id`.
    pub async fn deactivate(
        &self,
        id: &str,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<()> {
        let interceptor = self
            .get(id)
            .ok_or_else(|| InterceptError::NotFound(id.to_string()))?;
        let options = if options.is_null() {
            serde_json::json!({})
        } else {
            options
        };
        interceptor.deactivate(ctx, options).await
    }

    /// Deactivate all (on shutdown). Never fails; logs errors.
    pub async fn shutdown(&self, ctx: &InterceptContext) {
        futures::future::join_all(self.interceptors.iter().map(|i| async move {
            match tokio::time::timeout(SHUTDOWN_TIMEOUT, i.deactivate_all(ctx)).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => tracing::warn!(id = i.id(), "interceptor shutdown failed: {e}"),
                Err(_) => tracing::warn!(id = i.id(), "interceptor shutdown timed out"),
            }
        }))
        .await;
    }
}

fn timed_out_info(id: &str) -> InterceptorInfo {
    InterceptorInfo {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        category: InterceptorCategory::Manual,
        tags: Vec::new(),
        icon: id.to_string(),
        supported: true,
        available: false,
        active: false,
        unavailable_reason: Some("detection timed out".into()),
        version: None,
        metadata: serde_json::Value::Null,
        needs_config: false,
    }
}

/// Ids of all built-in interceptors, in catalogue order (stable: the UI and API use them).
pub const BUILTIN_IDS: &[&str] = &[
    "chrome",
    "firefox",
    "firefox-developer",
    "firefox-nightly",
    "chromium",
    "brave",
    "edge",
    "opera",
    "vivaldi",
    "arc",
    "fresh-terminal",
    "existing-terminal",
    "android-adb",
    "ios",
    "ios-simulator",
    "docker",
    "system-proxy",
    "electron",
    "jvm",
    "manual",
    "transparent-linux",
];

/// All built-in interceptors, in catalogue order ([`BUILTIN_IDS`]).
///
/// Every id is registered on every OS so activation errors stay meaningful; interceptors
/// that can't work here report `supported: false` (and are left out of
/// [`InterceptorRegistry::list`]). Called inside a Tokio runtime, this also starts restoring
/// a system proxy left behind by a crashed Akev (see the `system-proxy` interceptor).
pub fn builtin_interceptors(sys: Arc<Sys>) -> Vec<Arc<dyn Interceptor>> {
    let mut out: Vec<Arc<dyn Interceptor>> = Vec::new();
    for spec in browsers::CHROMIUM_BROWSERS {
        out.push(Arc::new(chromium::ChromiumInterceptor::new(
            spec,
            sys.clone(),
        )));
        if spec.id == "chrome" {
            // Keep Firefox right after Chrome in the catalogue.
            for ff in browsers::FIREFOX_BROWSERS {
                out.push(Arc::new(firefox::FirefoxInterceptor::new(ff, sys.clone())));
            }
        }
    }
    out.push(Arc::new(terminal::FreshTerminalInterceptor::new(
        sys.clone(),
    )));
    out.push(Arc::new(terminal::ExistingTerminalInterceptor::new(
        sys.clone(),
    )));
    out.push(Arc::new(android::AndroidAdbInterceptor::new(sys.clone())));
    out.push(Arc::new(manual::IosInterceptor));
    out.push(Arc::new(ios_sim::IosSimulatorInterceptor::new(sys.clone())));
    out.push(Arc::new(docker::DockerInterceptor::new(sys.clone())));
    out.push(Arc::new(sysproxy::SystemProxyInterceptor::new(sys.clone())));
    out.push(Arc::new(electron::ElectronInterceptor::new(sys.clone())));
    out.push(Arc::new(jvm::JvmInterceptor::new(sys.clone())));
    out.push(Arc::new(manual::ManualInterceptor::new(sys.clone())));
    out.push(Arc::new(transparent::TransparentInterceptor::new(sys)));
    out
}

/// Environment variables that make most CLI tools & runtimes use the proxy and trust the CA.
/// Shared by the terminal interceptors and the "existing terminal" setup script.
///
/// Proxy variables (`HTTP_PROXY`/`HTTPS_PROXY` in both cases, npm/yarn/global-agent/Node ≥ 24
/// variants), an empty `NO_PROXY`, trust-store variables pointing at the combined bundle
/// `<data_dir>/ca-bundle.pem` (system roots + Akev CA, written on demand),
/// `NODE_EXTRA_CA_CERTS` (Akev CA only), `JAVA_TOOL_OPTIONS` (proxy properties plus a
/// truststore once one has been generated with `keytool`) and `AKEV_ACTIVE=1`.
/// `ALL_PROXY` is intentionally not set (it breaks tools that expect SOCKS or use it for
/// non-HTTP protocols). See [`ENV_VAR_NAMES`] for the full list.
pub fn proxy_env_vars(ctx: &InterceptContext) -> Vec<(String, String)> {
    let sys = Sys::system();
    // The Java truststore needs `keytool` (seconds): start building it for next time.
    terminal::prepare_java_truststore(ctx, &sys);
    envvars::local_env_vars(ctx, &sys.env)
}

/// Shell script snippet for configuring an already-open terminal
/// (`bash`/`zsh`/`fish`/`powershell`/`cmd`).
///
/// Unknown shell names get the POSIX (bash) version. The script saves previous values,
/// defines `akev-off` to restore them (a function; in `cmd` a doskey macro calling
/// `<data_dir>/terminal/akev-off.cmd`, written on demand), and prints a one-line
/// confirmation; the `cmd` version uses CRLF line endings.
pub fn existing_terminal_script(ctx: &InterceptContext, shell: &str) -> String {
    let shell = Shell::parse(shell);
    let cmd_reset = match shell {
        Shell::Cmd => scripts::ensure_cmd_reset_file(&ctx.data_dir),
        _ => None,
    };
    scripts::setup_script(
        shell,
        &proxy_env_vars(ctx),
        &util::local_proxy_url(ctx),
        cmd_reset.as_deref(),
    )
}

/// Shell snippet to reset a terminal configured by `existing_terminal_script`.
///
/// Restores every variable to its value before setup (unset if it was unset); does nothing
/// (apart from printing a note) in a terminal that was never set up.
pub fn reset_terminal_script(shell: &str) -> String {
    scripts::reset_script(Shell::parse(shell))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::testctx::ctx;
    use akev_core::interceptor::InterceptorCategory;
    use parking_lot::Mutex;

    struct Fake {
        id: &'static str,
        available: bool,
        fail: bool,
        log: Arc<Mutex<Vec<String>>>,
        slow: bool,
    }

    #[async_trait]
    impl Interceptor for Fake {
        fn id(&self) -> &str {
            self.id
        }
        async fn info(&self, _ctx: &InterceptContext) -> InterceptorInfo {
            if self.slow {
                tokio::time::sleep(Duration::from_secs(3600)).await;
            }
            InterceptorInfo {
                id: self.id.into(),
                name: self.id.into(),
                description: String::new(),
                category: InterceptorCategory::Browser,
                tags: vec![],
                icon: "x".into(),
                supported: true,
                available: self.available,
                active: false,
                unavailable_reason: None,
                version: None,
                metadata: serde_json::Value::Null,
                needs_config: false,
            }
        }
        async fn activate(
            &self,
            _ctx: &InterceptContext,
            options: serde_json::Value,
        ) -> Result<ActivationResult> {
            self.log
                .lock()
                .push(format!("activate {} {options}", self.id));
            if self.fail {
                return Err(InterceptError::Failed("boom".into()));
            }
            Ok(ActivationResult {
                success: true,
                message: None,
                metadata: serde_json::Value::Null,
            })
        }
        async fn deactivate(
            &self,
            _ctx: &InterceptContext,
            _options: serde_json::Value,
        ) -> Result<()> {
            self.log.lock().push(format!("deactivate {}", self.id));
            Ok(())
        }
        async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
            self.log.lock().push(format!("deactivate_all {}", self.id));
            if self.fail {
                Err(InterceptError::Failed("nope".into()))
            } else {
                Ok(())
            }
        }
    }

    fn fake(id: &'static str, log: &Arc<Mutex<Vec<String>>>) -> Arc<dyn Interceptor> {
        Arc::new(Fake {
            id,
            available: true,
            fail: false,
            log: log.clone(),
            slow: false,
        })
    }

    #[tokio::test]
    async fn registry_list_activate_and_errors() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let log = Arc::new(Mutex::new(Vec::new()));
        let reg = InterceptorRegistry::with(vec![
            fake("a", &log),
            Arc::new(Fake {
                id: "bad",
                available: true,
                fail: true,
                log: log.clone(),
                slow: false,
            }),
            fake("a", &log), // duplicate: ignored
            Arc::new(Fake {
                id: "firefox-nightly",
                available: false,
                fail: false,
                log: log.clone(),
                slow: false,
            }),
        ]);
        assert_eq!(reg.ids(), vec!["a", "bad", "firefox-nightly"]);
        let list = reg.list(&c).await;
        assert_eq!(
            list.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["a", "bad"],
            "unavailable variants hidden"
        );

        assert!(
            reg.activate("a", &c, serde_json::Value::Null)
                .await
                .unwrap()
                .success
        );
        assert!(
            log.lock().contains(&"activate a {}".to_string()),
            "null options become {{}}"
        );
        assert!(
            matches!(reg.activate("nope", &c, serde_json::json!({})).await, Err(InterceptError::NotFound(id)) if id == "nope")
        );
        assert!(matches!(
            reg.deactivate("nope", &c, serde_json::json!({})).await,
            Err(InterceptError::NotFound(_))
        ));
        assert!(matches!(
            reg.activate("bad", &c, serde_json::json!({})).await,
            Err(InterceptError::Failed(_))
        ));
        reg.deactivate("a", &c, serde_json::json!({}))
            .await
            .unwrap();

        // Shutdown reaches everyone and swallows errors.
        reg.shutdown(&c).await;
        let log = log.lock();
        assert!(log.contains(&"deactivate_all a".to_string()));
        assert!(log.contains(&"deactivate_all bad".to_string()));
    }

    #[tokio::test]
    async fn registry_list_survives_hanging_detection() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let log = Arc::new(Mutex::new(Vec::new()));
        let reg = InterceptorRegistry::with(vec![
            fake("a", &log),
            Arc::new(Fake {
                id: "hang",
                available: true,
                fail: false,
                log: log.clone(),
                slow: true,
            }),
        ]);
        let list = tokio::time::timeout(INFO_TIMEOUT + Duration::from_secs(5), reg.list(&c))
            .await
            .unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(
            list[1].unavailable_reason.as_deref(),
            Some("detection timed out")
        );
    }

    fn fake_machine(dir: &std::path::Path) -> (Arc<runner::mock::MockRunner>, Arc<Sys>) {
        let runner = runner::mock::MockRunner::new();
        runner.processes.lock().push(runner::ProcessEntry {
            pid: std::process::id(),
            name: "akev-test".into(),
            ..Default::default()
        });
        let env = host::testutil::fake_env(host::Os::Linux, dir);
        let sys = Sys::new(runner.clone(), env);
        (runner, sys)
    }

    #[tokio::test]
    async fn builtin_catalogue_on_a_bare_linux_machine() {
        let dir = tempfile::tempdir().unwrap();
        let (runner, sys) = fake_machine(dir.path());
        let reg = InterceptorRegistry::with_sys(sys);
        assert_eq!(
            reg.ids(),
            BUILTIN_IDS
                .iter()
                .map(|s| s.to_string())
                .collect::<Vec<_>>()
        );

        let c = ctx(&dir.path().join("data"));
        let list = reg.list(&c).await;
        let ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
        // Arc doesn't exist on Linux; uninstalled Firefox variants are hidden.
        assert!(!ids.contains(&"arc"));
        assert!(!ids.contains(&"firefox-developer") && !ids.contains(&"firefox-nightly"));
        for id in [
            "chrome",
            "firefox",
            "fresh-terminal",
            "existing-terminal",
            "android-adb",
            "ios",
            "docker",
            "system-proxy",
            "electron",
            "jvm",
            "manual",
        ] {
            assert!(ids.contains(&id), "{id} missing from {ids:?}");
        }
        for info in &list {
            assert!(
                !info.name.is_empty() && !info.description.is_empty() && !info.icon.is_empty(),
                "{info:?}"
            );
            assert!(
                info.available || info.unavailable_reason.is_some(),
                "{} needs a reason",
                info.id
            );
            assert!(!info.active, "{} active on a bare machine", info.id);
        }
        let by_id = |id: &str| list.iter().find(|i| i.id == id).unwrap();
        assert!(
            by_id("manual").available
                && by_id("existing-terminal").available
                && by_id("electron").available
        );
        assert!(
            by_id("ios").available,
            "LAN address present in the test context"
        );
        assert!(
            !by_id("chrome").available
                && !by_id("docker").available
                && !by_id("system-proxy").available
        );
        assert!(
            by_id("android-adb").needs_config
                && by_id("docker").needs_config
                && by_id("electron").needs_config
                && by_id("jvm").needs_config
        );
        assert!(!by_id("jvm").available, "no Java on a bare machine");
        // Nothing was launched to build the catalogue.
        assert!(runner.spawned_specs().is_empty());

        // Registered-but-unsupported ids explain themselves.
        assert!(matches!(
            reg.activate("arc", &c, serde_json::json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
        assert!(matches!(
            reg.activate("chrome", &c, serde_json::json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
        assert!(matches!(
            reg.activate("android-adb", &c, serde_json::json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
        assert!(matches!(
            reg.activate("no-such-thing", &c, serde_json::json!({}))
                .await,
            Err(InterceptError::NotFound(_))
        ));
        // Shutdown with nothing active is quiet and doesn't run anything.
        reg.shutdown(&c).await;
        assert!(runner.spawned_specs().is_empty());
    }

    #[tokio::test]
    async fn registry_construction_restores_stale_system_proxy() {
        let dir = tempfile::tempdir().unwrap();
        host::testutil::make_exe(&dir.path().join("usr/bin/gsettings"), "");
        let (runner, sys) = fake_machine(dir.path());
        let sets = Arc::new(Mutex::new(Vec::new()));
        let log = sets.clone();
        let written = Arc::new(Mutex::new(
            std::collections::HashMap::<String, String>::new(),
        ));
        runner.on(move |spec| {
            if spec.program_name() != "gsettings" {
                return None;
            }
            let args = spec.args_lossy();
            let key = format!("{} {}", args[1], args[2]);
            if args[0] == "get" {
                // Still pointing at the crashed Akev, unless set since.
                let value = written.lock().get(&key).cloned().unwrap_or_else(|| {
                    match args[2].as_str() {
                        "mode" => "'manual'",
                        "host" => "'127.0.0.1'",
                        _ => "8000",
                    }
                    .to_string()
                });
                return Some(Ok(runner::CommandOutput::ok(format!("{value}\n"))));
            }
            written.lock().insert(key, args[3].clone());
            log.lock().push(args.join(" "));
            Some(Ok(runner::CommandOutput::ok("")))
        });
        let data = dir.path().join("home/.local/share/akev");
        std::fs::create_dir_all(&data).unwrap();
        let backup = serde_json::json!({
            "version": 1, "instance": "crashed-run", "pid": 999_999, "processName": "akev",
            "createdAt": 1, "proxy": "127.0.0.1:8000",
            "snapshots": [{"backend": "gnome", "values": [
                {"schema": "org.gnome.system.proxy", "key": "mode", "value": "'none'"},
                {"schema": "org.gnome.system.proxy.http", "key": "host", "value": "''"}
            ]}]
        });
        std::fs::write(sysproxy::backup_path(&data), backup.to_string()).unwrap();

        let _reg = InterceptorRegistry::with_sys(sys);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while sysproxy::backup_path(&data).exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !sysproxy::backup_path(&data).exists(),
            "stale backup restored and removed"
        );
        let sets = sets.lock().clone();
        assert_eq!(
            sets,
            vec![
                "set org.gnome.system.proxy mode 'none'",
                "set org.gnome.system.proxy.http host ''"
            ]
        );
    }

    #[test]
    fn errors_display() {
        assert_eq!(
            InterceptError::NotFound("x".into()).to_string(),
            "interceptor not found: x"
        );
        assert_eq!(
            InterceptError::Unavailable("y".into()).to_string(),
            "not available: y"
        );
    }

    #[test]
    fn public_script_helpers() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(dir.path());
        let bash = existing_terminal_script(&c, "bash");
        assert!(bash.contains("export HTTP_PROXY='http://127.0.0.1:8000'"));
        assert!(bash.contains(&format!(
            "export SSL_CERT_FILE='{}'",
            ca_bundle_path(&c.data_dir).display()
        )));
        assert!(existing_terminal_script(&c, "fish").contains("set -gx HTTP_PROXY"));
        assert!(existing_terminal_script(&c, "PowerShell").contains("$env:HTTP_PROXY"));
        assert!(existing_terminal_script(&c, "cmd").contains("set \"HTTP_PROXY="));
        assert!(reset_terminal_script("zsh").contains("unset HTTP_PROXY"));
        let vars = proxy_env_vars(&c);
        assert!(vars.iter().any(|(k, v)| k == "AKEV_ACTIVE" && v == "1"));
    }
}
