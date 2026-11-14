//! Chromium-family browsers (Chrome, Chromium, Brave, Edge, Opera, Vivaldi, Arc).
//!
//! Each activation launches a *separate* browser instance with its own persistent profile
//! (`<data_dir>/profiles/<id>`, so logins survive between sessions), proxied through Akev and
//! trusting the Akev CA by SPKI pin (no system trust store changes needed).
//!
//! A browser already running with the profile receives a second launch through Chrome's
//! process singleton, which ignores the new switches. When the running instance was started
//! for another proxy address or CA (the port changed, the CA was regenerated), activation
//! therefore restarts it instead of opening a window that can't reach Akev.

use crate::browsers::{self, BrowserSpec, Installation};
use crate::launch::{
    LAUNCH_SETTLE, Tracker, cmd_has_flag_value, log_tail, matching_pids, stop_all,
};
use crate::runner::{CommandSpec, ProcessEntry, ProcessSignal};
use crate::util::{
    Descriptor, PROBE_TTL, ProbeCache, Sys, local_proxy_addr, opt_bool, opt_str, validate_start_url,
};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Features disabled to cut first-run UI and background model/hint downloads (noise in the
/// traffic list) without breaking extensions.
const DISABLED_FEATURES: &[&str] = &[
    "ChromeWhatsNewUI",
    "OptimizationGuideModelDownloading",
    "OptimizationHintsFetching",
    "OptimizationTargetPrediction",
    "OptimizationHints",
    "MediaRouter",
];

/// Inputs for [`chromium_args`].
#[derive(Clone, Debug)]
pub struct ChromiumLaunch<'a> {
    /// Browser id (`chrome`, `brave`...).
    pub browser_id: &'a str,
    /// `--user-data-dir`.
    pub profile_dir: &'a Path,
    /// Proxy `host:port`.
    pub proxy_addr: &'a str,
    /// Base64 SHA-256 SPKI fingerprint of the CA.
    pub spki_fingerprint: &'a str,
    /// Start URL.
    pub url: &'a str,
    /// Also disable background networking, component updates and sync (quietest traffic;
    /// extensions still install but don't auto-update).
    pub quiet: bool,
}

/// Command-line arguments for an isolated, proxied Chromium-family instance.
pub fn chromium_args(l: &ChromiumLaunch<'_>) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", l.profile_dir.display()),
        format!("--proxy-server=http://{}", l.proxy_addr),
        // Remove the implicit loopback bypass so localhost traffic is captured too.
        "--proxy-bypass-list=<-loopback>".to_string(),
        format!(
            "--ignore-certificate-errors-spki-list={}",
            l.spki_fingerprint
        ),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        // Hides the "unsupported command-line flag" infobar the SPKI flag triggers.
        "--test-type".to_string(),
        format!("--disable-features={}", DISABLED_FEATURES.join(",")),
        "--disable-domain-reliability".to_string(),
        "--metrics-recording-only".to_string(),
    ];
    if l.quiet {
        args.push("--disable-background-networking".to_string());
        args.push("--disable-sync".to_string());
        // Brave updates its ad-block lists through the component updater.
        if l.browser_id != "brave" {
            args.push("--disable-component-update".to_string());
        }
    }
    // If our instance is already running, the launch is forwarded to it: open a window.
    args.push("--new-window".to_string());
    args.push(l.url.to_string());
    args
}

/// The switches that tie a Chromium instance to one proxy address and CA.
fn instance_flags(proxy_addr: &str, spki_fingerprint: &str) -> [String; 2] {
    [
        format!("--proxy-server=http://{proxy_addr}"),
        format!("--ignore-certificate-errors-spki-list={spki_fingerprint}"),
    ]
}

/// Was this running browser started for another proxy address or CA than `flags`? Only a
/// switch present with a different value counts (a command line without them can't tell).
fn is_outdated(p: &ProcessEntry, flags: &[String]) -> bool {
    flags.iter().any(|flag| {
        let name = flag.split_once('=').map_or(flag.as_str(), |(n, _)| n);
        let prefix = format!("{name}=");
        let values: Vec<&String> = p.cmd.iter().filter(|a| a.starts_with(&prefix)).collect();
        !values.is_empty() && !values.contains(&flag)
    })
}

/// Interceptor for one Chromium-family browser.
pub(crate) struct ChromiumInterceptor {
    spec: &'static BrowserSpec,
    sys: Arc<Sys>,
    detected: ProbeCache<Option<Installation>>,
    tracker: Tracker,
}

impl ChromiumInterceptor {
    pub(crate) fn new(spec: &'static BrowserSpec, sys: Arc<Sys>) -> Self {
        ChromiumInterceptor {
            spec,
            sys,
            detected: ProbeCache::new(PROBE_TTL),
            tracker: Tracker::default(),
        }
    }

    fn descriptor(&self) -> Descriptor {
        Descriptor {
            id: self.spec.id,
            name: self.spec.name,
            description: self.spec.description,
            category: InterceptorCategory::Browser,
            tags: crate::util::tags::BROWSER,
            icon: self.spec.icon,
            needs_config: false,
        }
    }

    async fn installation(&self) -> Option<Installation> {
        let spec = self.spec;
        let env = &self.sys.env;
        self.detected
            .get(|| async move { browsers::detect(spec, env) })
            .await
    }

    fn profile_dir(&self, ctx: &InterceptContext, inst: &Installation) -> PathBuf {
        let default = ctx.data_dir.join("profiles").join(self.spec.id);
        browsers::confined_profile_dir(inst, &self.sys.env, &ctx.data_dir, &default)
    }

    /// Running main browser processes using our profile (launched by us or not).
    async fn profile_processes(&self, profile: &Path) -> Vec<ProcessEntry> {
        if !profile.exists() {
            return Vec::new();
        }
        let procs = self.sys.processes(false).await;
        let ours = matching_pids(&procs, &[], |p| {
            cmd_has_flag_value(&p.cmd, "--user-data-dir", profile)
                && !p.cmd.iter().any(|a| a.starts_with("--type="))
        });
        procs
            .iter()
            .filter(|p| ours.contains(&p.pid))
            .cloned()
            .collect()
    }

    /// Pids of running main browser processes using our profile that we didn't spawn.
    async fn scanned_pids(&self, profile: &Path) -> Vec<u32> {
        let tracked = self.tracker.pids();
        self.profile_processes(profile)
            .await
            .into_iter()
            .map(|p| p.pid)
            .filter(|pid| !tracked.contains(pid))
            .collect()
    }

    fn not_installed_reason(&self) -> String {
        let where_ = match self.sys.env.os {
            crate::host::Os::Linux => "PATH, standard install locations, Snap and Flatpak",
            crate::host::Os::MacOs => "/Applications and ~/Applications",
            crate::host::Os::Windows => "Program Files and AppData",
            crate::host::Os::Other => "this system",
        };
        format!("{} isn't installed (looked in {where_})", self.spec.name)
    }
}

#[async_trait]
impl Interceptor for ChromiumInterceptor {
    fn id(&self) -> &str {
        self.spec.id
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        let d = self.descriptor();
        if !self.spec.supported_on(self.sys.env.os) {
            return d.unsupported(self.sys.env.os.as_str());
        }
        let Some(inst) = self.installation().await else {
            return InterceptorInfo {
                unavailable_reason: Some(self.not_installed_reason()),
                ..d.info()
            };
        };
        let profile = self.profile_dir(ctx, &inst);
        let (version, running) = tokio::join!(
            browsers::version(&self.sys, &inst, browsers::Engine::Chromium),
            self.profile_processes(&profile)
        );
        let mut pids = self.tracker.pids();
        for p in &running {
            if !pids.contains(&p.pid) {
                pids.push(p.pid);
            }
        }
        let flags = instance_flags(&local_proxy_addr(ctx), &ctx.ca_spki_fingerprint);
        InterceptorInfo {
            available: true,
            active: !pids.is_empty(),
            version,
            metadata: json!({
                "path": inst.exe,
                "install": inst.kind.as_str(),
                "profileDir": profile,
                "pids": pids,
                // Running with an old proxy address or CA: launching again restarts it.
                "outdated": running.iter().any(|p| is_outdated(p, &flags)),
            }),
            ..d.info()
        }
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        if !self.spec.supported_on(self.sys.env.os) {
            return Err(InterceptError::Unavailable(format!(
                "{} is not supported on this OS",
                self.spec.name
            )));
        }
        let url = validate_start_url(opt_str(&options, "url").unwrap_or("about:blank"))?;
        self.detected.invalidate().await;
        let inst = self
            .installation()
            .await
            .ok_or_else(|| InterceptError::Unavailable(self.not_installed_reason()))?;
        let profile = self.profile_dir(ctx, &inst);
        std::fs::create_dir_all(&profile)?;
        let log = ctx
            .data_dir
            .join("logs")
            .join(format!("{}.log", self.spec.id));
        let proxy_addr = local_proxy_addr(ctx);

        // A launch would be forwarded to an instance already using the profile, which keeps its
        // own switches: if it was started for another proxy address or CA it can't reach Akev
        // (or fails every HTTPS pin), so restart it.
        self.sys.invalidate_processes().await;
        let running = self.profile_processes(&profile).await;
        let flags = instance_flags(&proxy_addr, &ctx.ca_spki_fingerprint);
        let restarted = running.iter().any(|p| is_outdated(p, &flags));
        if restarted {
            let tracked = self.tracker.pids();
            let foreign: Vec<u32> = running
                .iter()
                .map(|p| p.pid)
                .filter(|pid| !tracked.contains(pid))
                .collect();
            tracing::info!(
                browser = self.spec.id,
                "restarting a browser started for another proxy address or CA"
            );
            stop_all(&self.sys, &self.tracker, foreign, ProcessSignal::Terminate).await;
        }

        let args = chromium_args(&ChromiumLaunch {
            browser_id: self.spec.id,
            profile_dir: &profile,
            proxy_addr: &proxy_addr,
            spki_fingerprint: &ctx.ca_spki_fingerprint,
            url: &url,
            quiet: opt_bool(&options, "quiet").unwrap_or(false),
        });
        let child = self
            .sys
            .runner
            .spawn(&CommandSpec::new(&inst.exe).args(args).log_file(&log))
            .map_err(|e| {
                InterceptError::Failed(format!(
                    "couldn't start {} ({}): {e}",
                    self.spec.name,
                    inst.exe.display()
                ))
            })?;
        let pid = child.pid();

        let message = if child.wait_timeout(LAUNCH_SETTLE).await {
            match child.exit_info().and_then(|e| e.code) {
                Some(0) if restarted => format!(
                    "Restarted {}: it was running with an outdated proxy address or CA",
                    self.spec.name
                ),
                Some(0) => format!(
                    "Opened a new {} window with the Akev profile",
                    self.spec.name
                ),
                code => {
                    let tail = log_tail(&log, 600);
                    return Err(InterceptError::Failed(format!(
                        "{} exited immediately (code {}){}{}",
                        self.spec.name,
                        code.map(|c| c.to_string())
                            .unwrap_or_else(|| "unknown".into()),
                        if tail.is_empty() { "" } else { ": " },
                        tail
                    )));
                }
            }
        } else {
            self.tracker.add(child);
            if restarted {
                format!(
                    "Restarted {}: it was running with an outdated proxy address or CA",
                    self.spec.name
                )
            } else {
                format!("Launched {} with an isolated Akev profile", self.spec.name)
            }
        };
        self.sys.invalidate_processes().await;
        Ok(ActivationResult {
            success: true,
            message: Some(message),
            metadata: json!({
                "pid": pid,
                "profileDir": profile,
                "url": url,
                "path": inst.exe,
                "restarted": restarted,
            }),
        })
    }

    async fn deactivate(&self, ctx: &InterceptContext, _options: serde_json::Value) -> Result<()> {
        self.deactivate_all(ctx).await
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        let extra = match self.installation().await {
            Some(inst) => {
                self.sys.invalidate_processes().await;
                self.scanned_pids(&self.profile_dir(ctx, &inst)).await
            }
            None => Vec::new(),
        };
        let stopped = stop_all(&self.sys, &self.tracker, extra, ProcessSignal::Terminate).await;
        if stopped > 0 {
            tracing::info!(
                browser = self.spec.id,
                stopped,
                "closed intercepted browser instances"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Os;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::ProcessEntry;
    use crate::runner::mock::MockRunner;
    use crate::util::testctx::ctx;

    fn chrome_spec() -> &'static BrowserSpec {
        browsers::CHROMIUM_BROWSERS
            .iter()
            .find(|s| s.id == "chrome")
            .unwrap()
    }

    fn setup() -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        ChromiumInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        make_exe(&dir.path().join("usr/bin/google-chrome"), "#!/bin/sh\n");
        let runner = MockRunner::new();
        runner.on_prefix(
            "google-chrome --version",
            crate::runner::CommandOutput::ok("Google Chrome 131.0.6778.85\n"),
        );
        let sys = Sys::new(runner.clone(), env);
        let i = ChromiumInterceptor::new(chrome_spec(), sys);
        let c = ctx(&dir.path().join("data"));
        (dir, runner, i, c)
    }

    #[test]
    fn args_construction() {
        let args = chromium_args(&ChromiumLaunch {
            browser_id: "chrome",
            profile_dir: Path::new("/d/profiles/chrome"),
            proxy_addr: "127.0.0.1:8000",
            spki_fingerprint: "abc=",
            url: "https://example.com",
            quiet: false,
        });
        assert_eq!(args[0], "--user-data-dir=/d/profiles/chrome");
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:8000".to_string()));
        assert!(args.contains(&"--proxy-bypass-list=<-loopback>".to_string()));
        assert!(args.contains(&"--ignore-certificate-errors-spki-list=abc=".to_string()));
        assert!(args.contains(&"--no-first-run".to_string()));
        assert!(args.contains(&"--no-default-browser-check".to_string()));
        assert!(args.contains(&"--test-type".to_string()));
        assert!(
            !args.contains(&"--disable-background-networking".to_string()),
            "extensions must keep updating by default"
        );
        assert_eq!(args.last().unwrap(), "https://example.com");

        let quiet = chromium_args(&ChromiumLaunch {
            browser_id: "brave",
            profile_dir: Path::new("/p"),
            proxy_addr: "[::1]:8000",
            spki_fingerprint: "x",
            url: "about:blank",
            quiet: true,
        });
        assert!(quiet.contains(&"--disable-background-networking".to_string()));
        assert!(quiet.contains(&"--proxy-server=http://[::1]:8000".to_string()));
        // Brave needs the component updater for its ad-block lists.
        assert!(!quiet.contains(&"--disable-component-update".to_string()));
    }

    #[tokio::test]
    async fn info_reports_installation_and_activity() {
        let (dir, runner, i, c) = setup();
        let info = i.info(&c).await;
        assert!(info.supported && info.available && !info.active);
        assert_eq!(info.id, "chrome");
        assert_eq!(info.version.as_deref(), Some("131.0.6778.85"));
        assert_eq!(
            info.metadata["path"],
            json!(dir.path().join("usr/bin/google-chrome"))
        );
        assert_eq!(
            info.metadata["profileDir"],
            json!(c.data_dir.join("profiles/chrome"))
        );

        // A browser left over from a previous Akev run counts as active.
        std::fs::create_dir_all(c.data_dir.join("profiles/chrome")).unwrap();
        let profile_arg = format!(
            "--user-data-dir={}",
            c.data_dir.join("profiles/chrome").display()
        );
        runner.processes.lock().extend([
            ProcessEntry {
                pid: 900,
                name: "chrome".into(),
                cmd: vec!["chrome".into(), profile_arg.clone()],
                ..Default::default()
            },
            ProcessEntry {
                pid: 901,
                name: "chrome".into(),
                cmd: vec!["chrome".into(), "--type=renderer".into(), profile_arg],
                ..Default::default()
            },
        ]);
        i.sys.invalidate_processes().await;
        let info = i.info(&c).await;
        assert!(info.active);
        assert_eq!(info.metadata["pids"], json!([900]));

        // Deactivation terminates it (renderers follow the main process).
        i.deactivate_all(&c).await.unwrap();
        assert_eq!(
            runner.signals.lock().first(),
            Some(&(900, ProcessSignal::Terminate))
        );
    }

    #[tokio::test]
    async fn activate_launches_tracks_and_deactivates() {
        let (_dir, runner, i, c) = setup();
        let res = i
            .activate(&c, json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert!(res.success);
        let specs = runner.spawned_specs();
        assert_eq!(specs.len(), 1);
        let args = specs[0].args_lossy();
        assert!(args.contains(&format!(
            "--user-data-dir={}",
            c.data_dir.join("profiles/chrome").display()
        )));
        assert!(args.contains(&"--proxy-server=http://127.0.0.1:8000".to_string()));
        assert!(args.contains(&format!(
            "--ignore-certificate-errors-spki-list={}",
            c.ca_spki_fingerprint
        )));
        assert_eq!(args.last().unwrap(), "https://example.com");
        assert!(c.data_dir.join("profiles/chrome").is_dir());
        assert!(i.info(&c).await.active);

        i.deactivate(&c, json!({})).await.unwrap();
        let signals = runner.spawned.lock()[0].signals.lock().clone();
        assert_eq!(signals, vec![ProcessSignal::Terminate]);
        assert!(!i.info(&c).await.active);
    }

    #[tokio::test]
    async fn stubborn_browser_gets_killed() {
        let (_dir, runner, i, c) = setup();
        *runner.stubborn.lock() = true;
        i.activate(&c, json!({})).await.unwrap();
        i.deactivate_all(&c).await.unwrap();
        let signals = runner.spawned.lock()[0].signals.lock().clone();
        assert_eq!(signals, vec![ProcessSignal::Terminate, ProcessSignal::Kill]);
    }

    #[tokio::test]
    async fn activate_errors() {
        let (_dir, runner, i, c) = setup();
        let err = i
            .activate(&c, json!({"url": "--disable-web-security"}))
            .await
            .unwrap_err();
        assert!(matches!(err, InterceptError::Failed(_)));
        assert!(runner.spawned_specs().is_empty());

        // Immediate crash → error with the exit code.
        *runner.spawn_exit_code.lock() = Some(127);
        let err = i.activate(&c, json!({})).await.unwrap_err();
        assert!(err.to_string().contains("code 127"), "{err}");

        // Exit 0 straight away = forwarded to the running instance: success, not tracked.
        *runner.spawn_exit_code.lock() = Some(0);
        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.message.unwrap().contains("new Chrome window"));

        // Not installed.
        let dir = tempfile::tempdir().unwrap();
        let sys = Sys::new(MockRunner::new(), fake_env(Os::Linux, dir.path()));
        let i = ChromiumInterceptor::new(chrome_spec(), sys);
        let info = i.info(&c).await;
        assert!(info.supported && !info.available);
        assert!(info.unavailable_reason.unwrap().contains("isn't installed"));
        assert!(matches!(
            i.activate(&c, json!({})).await,
            Err(InterceptError::Unavailable(_))
        ));
    }

    fn browser_process(pid: u32, profile: &Path, proxy: &str, spki: &str) -> ProcessEntry {
        ProcessEntry {
            pid,
            name: "chrome".into(),
            cmd: vec![
                "chrome".into(),
                format!("--user-data-dir={}", profile.display()),
                format!("--proxy-server=http://{proxy}"),
                "--proxy-bypass-list=<-loopback>".into(),
                format!("--ignore-certificate-errors-spki-list={spki}"),
            ],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn outdated_running_instance_is_restarted() {
        let (_dir, runner, i, c) = setup();
        let profile = c.data_dir.join("profiles/chrome");
        std::fs::create_dir_all(&profile).unwrap();
        // Still running from before the proxy port changed (8000 → now 8000; it uses 7000).
        runner.processes.lock().push(browser_process(
            900,
            &profile,
            "127.0.0.1:7000",
            &c.ca_spki_fingerprint,
        ));
        let info = i.info(&c).await;
        assert!(info.active);
        assert_eq!(info.metadata["outdated"], json!(true));

        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(
            runner.signals.lock().first(),
            Some(&(900, ProcessSignal::Terminate)),
            "the stale instance is stopped"
        );
        assert_eq!(runner.spawned_specs().len(), 1, "and a fresh one launched");
        assert!(res.message.unwrap().contains("Restarted"));
        assert_eq!(res.metadata["restarted"], json!(true));

        // Same after a CA regeneration (new SPKI pin), even if the launch is forwarded.
        runner.signals.lock().clear();
        runner.exit_all();
        runner
            .processes
            .lock()
            .push(browser_process(901, &profile, "127.0.0.1:8000", "b2xkLWNh"));
        *runner.spawn_exit_code.lock() = Some(0);
        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(
            runner.signals.lock().clone(),
            vec![(901, ProcessSignal::Terminate)]
        );
        assert!(res.message.unwrap().contains("Restarted"));
    }

    #[test]
    fn outdated_means_another_value() {
        let flags = instance_flags("127.0.0.1:8000", "pin=");
        let p = |cmd: &[&str]| ProcessEntry {
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        };
        assert!(!is_outdated(
            &p(&[
                "chrome",
                "--proxy-server=http://127.0.0.1:8000",
                "--ignore-certificate-errors-spki-list=pin="
            ]),
            &flags
        ));
        assert!(is_outdated(
            &p(&["chrome", "--proxy-server=http://127.0.0.1:7000"]),
            &flags
        ));
        assert!(is_outdated(
            &p(&["chrome", "--ignore-certificate-errors-spki-list=old="]),
            &flags
        ));
        assert!(!is_outdated(&p(&["chrome", "--user-data-dir=/p"]), &flags));
    }

    #[tokio::test]
    async fn up_to_date_running_instance_gets_a_new_window() {
        let (_dir, runner, i, c) = setup();
        let profile = c.data_dir.join("profiles/chrome");
        std::fs::create_dir_all(&profile).unwrap();
        runner.processes.lock().push(browser_process(
            900,
            &profile,
            "127.0.0.1:8000",
            &c.ca_spki_fingerprint,
        ));
        assert_eq!(i.info(&c).await.metadata["outdated"], json!(false));
        // Chrome forwards the launch to the running instance and exits 0.
        *runner.spawn_exit_code.lock() = Some(0);
        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(runner.signals.lock().is_empty());
        assert!(res.message.unwrap().contains("new Chrome window"));
        assert_eq!(res.metadata["restarted"], json!(false));
    }

    #[tokio::test]
    async fn arc_unsupported_on_linux() {
        let dir = tempfile::tempdir().unwrap();
        let sys = Sys::new(MockRunner::new(), fake_env(Os::Linux, dir.path()));
        let arc = browsers::CHROMIUM_BROWSERS
            .iter()
            .find(|s| s.id == "arc")
            .unwrap();
        let i = ChromiumInterceptor::new(arc, sys);
        let info = i.info(&ctx(dir.path())).await;
        assert!(!info.supported && !info.available);
    }
}
