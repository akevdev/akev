//! Firefox (and Developer Edition / Nightly).
//!
//! Firefox ignores Chromium-style SPKI pins and keeps its own NSS certificate store, so:
//! * the profile (`<data_dir>/profiles/<id>`, persistent) gets a generated `user.js` with
//!   manual proxy settings and the noise (telemetry, captive portal, first-run pages, remote
//!   settings) switched off;
//! * the CA is added to the profile's `cert9.db` with NSS `certutil` when it's installed;
//! * otherwise the first tab opens `http://akev.cert/ca.crt`, which makes Firefox show its
//!   native "Trust this CA to identify websites" dialog (one click, once per profile).
//!
//! The running instance can't be asked to open a window (`-no-remote`), and it keeps the
//! proxy address and CA it was started with: activating again while it runs with an outdated
//! configuration (the port changed, the CA was regenerated) restarts it.

use crate::browsers::{self, BrowserSpec, Installation};
use crate::launch::{
    LAUNCH_SETTLE, Tracker, cmd_has_flag_value, log_tail, matching_pids, stop_all,
};
use crate::runner::{CommandSpec, ProcessSignal};
use crate::util::{
    Descriptor, PROBE_TTL, ProbeCache, Sys, atomic_write, local_proxy_host, opt_str,
    validate_start_url,
};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// URL served by the Akev proxy that triggers Firefox's CA import dialog.
pub const CA_IMPORT_URL: &str = "http://akev.cert/ca.crt";
/// Certificate nickname inside the profile's NSS database.
const NSS_NICKNAME: &str = "Akev CA";
/// `user.js` comment recording the CA (SPKI fingerprint) the profile was launched for.
const CA_MARKER: &str = "// akev-ca-spki: ";

/// A Firefox preference value.
#[derive(Clone, Debug)]
enum Pref {
    Bool(bool),
    Int(i64),
    Str(String),
}

fn pref_line(name: &str, value: &Pref) -> String {
    let v = match value {
        Pref::Bool(b) => b.to_string(),
        Pref::Int(i) => i.to_string(),
        Pref::Str(s) => serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into()),
    };
    format!(
        "user_pref({}, {v});\n",
        serde_json::to_string(name).unwrap_or_default()
    )
}

/// Generate the profile's `user.js` (applied by Firefox on every start, overriding prefs.js).
pub fn firefox_user_js(proxy_host: &str, proxy_port: u16) -> String {
    use Pref::*;
    let host = proxy_host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = i64::from(proxy_port);
    let prefs: Vec<(&str, Pref)> = vec![
        // Manual proxy for HTTP & HTTPS, nothing bypassed (localhost included).
        ("network.proxy.type", Int(1)),
        ("network.proxy.http", Str(host.clone())),
        ("network.proxy.http_port", Int(port)),
        ("network.proxy.ssl", Str(host)),
        ("network.proxy.ssl_port", Int(port)),
        ("network.proxy.share_proxy_settings", Bool(false)),
        ("network.proxy.socks", Str(String::new())),
        ("network.proxy.no_proxies_on", Str(String::new())),
        ("network.proxy.allow_hijacking_localhost", Bool(true)),
        // DNS-over-HTTPS off: resolution happens in the proxy anyway.
        ("network.trr.mode", Int(5)),
        // Trust CAs from the OS store too (Windows/macOS, and Linux p11-kit on newer builds).
        ("security.enterprise_roots.enabled", Bool(true)),
        // Background noise.
        ("network.captive-portal-service.enabled", Bool(false)),
        ("network.connectivity-service.enabled", Bool(false)),
        (
            "services.settings.server",
            Str("data:,#remote-settings-dummy/v1".into()),
        ),
        ("app.normandy.enabled", Bool(false)),
        ("app.shield.optoutstudies.enabled", Bool(false)),
        ("app.update.auto", Bool(false)),
        ("app.update.checkInstallTime", Bool(false)),
        ("datareporting.healthreport.uploadEnabled", Bool(false)),
        ("datareporting.usage.uploadEnabled", Bool(false)),
        ("datareporting.policy.dataSubmissionEnabled", Bool(false)),
        (
            "datareporting.policy.dataSubmissionPolicyBypassNotification",
            Bool(true),
        ),
        ("toolkit.telemetry.enabled", Bool(false)),
        ("toolkit.telemetry.unified", Bool(false)),
        ("toolkit.telemetry.archive.enabled", Bool(false)),
        ("toolkit.telemetry.reportingpolicy.firstRun", Bool(false)),
        ("browser.ping-centre.telemetry", Bool(false)),
        (
            "browser.newtabpage.activity-stream.feeds.telemetry",
            Bool(false),
        ),
        ("browser.newtabpage.activity-stream.telemetry", Bool(false)),
        (
            "browser.newtabpage.activity-stream.feeds.section.topstories",
            Bool(false),
        ),
        (
            "browser.newtabpage.activity-stream.showSponsored",
            Bool(false),
        ),
        (
            "browser.newtabpage.activity-stream.showSponsoredTopSites",
            Bool(false),
        ),
        ("browser.discovery.enabled", Bool(false)),
        ("extensions.getAddons.showPane", Bool(false)),
        (
            "extensions.htmlaboutaddons.recommendations.enabled",
            Bool(false),
        ),
        // First-run & default-browser nags.
        ("browser.shell.checkDefaultBrowser", Bool(false)),
        (
            "browser.startup.homepage_override.mstone",
            Str("ignore".into()),
        ),
        ("startup.homepage_welcome_url", Str(String::new())),
        (
            "startup.homepage_welcome_url.additional",
            Str(String::new()),
        ),
        ("browser.aboutwelcome.enabled", Bool(false)),
        ("trailhead.firstrun.didSeeAboutWelcome", Bool(true)),
        ("browser.uitour.enabled", Bool(false)),
        ("browser.aboutConfig.showWarning", Bool(false)),
        ("browser.tabs.warnOnClose", Bool(false)),
        ("browser.warnOnQuit", Bool(false)),
        // Akev stops the browser with SIGTERM; don't greet the next start with a crash page.
        ("browser.sessionstore.resume_from_crash", Bool(false)),
    ];
    let mut out = String::from(
        "// Generated by Akev on every launch: edits here are overwritten.\n// Proxy & noise settings for an intercepted Firefox profile.\n",
    );
    for (name, value) in &prefs {
        out.push_str(&pref_line(name, value));
    }
    out
}

/// The profile's `user.js` for `ctx`: [`firefox_user_js`] plus a note of the CA it was
/// written for.
fn profile_user_js(ctx: &InterceptContext) -> String {
    format!(
        "{}{CA_MARKER}{}\n",
        firefox_user_js(&local_proxy_host(ctx), ctx.proxy_port),
        ctx.ca_spki_fingerprint
    )
}

/// Was the Firefox using `profile` started for another proxy address or CA than `ctx`'s?
/// (`user.js` is rewritten on every launch, so it describes the running instance.) `false`
/// when that can't be told.
pub(crate) fn launch_config_outdated(profile: &Path, ctx: &InterceptContext) -> bool {
    let Ok(content) = std::fs::read_to_string(profile.join("user.js")) else {
        return false;
    };
    let host = local_proxy_host(ctx)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string();
    let port = i64::from(ctx.proxy_port);
    let expected = [
        pref_line("network.proxy.http", &Pref::Str(host.clone())),
        pref_line("network.proxy.http_port", &Pref::Int(port)),
        pref_line("network.proxy.ssl", &Pref::Str(host)),
        pref_line("network.proxy.ssl_port", &Pref::Int(port)),
    ];
    if expected
        .iter()
        .any(|line| !content.lines().any(|l| l == line.trim_end()))
    {
        return true;
    }
    content
        .lines()
        .find_map(|l| l.strip_prefix(CA_MARKER))
        .is_some_and(|ca| ca.trim() != ctx.ca_spki_fingerprint)
}

/// Does the profile's NSS database contain `ca_der`? (The DER is stored verbatim in
/// `cert9.db`, so a byte search answers this without linking NSS.)
pub(crate) fn profile_has_ca(profile: &Path, ca_der: &[u8]) -> bool {
    if ca_der.len() < 16 {
        return false;
    }
    ["cert9.db", "cert9.db-wal", "cert9.db-journal"]
        .iter()
        .any(|name| {
            std::fs::read(profile.join(name))
                .map(|db| memchr::memmem::find(&db, ca_der).is_some())
                .unwrap_or(false)
        })
}

/// Interceptor for one Firefox variant.
pub(crate) struct FirefoxInterceptor {
    spec: &'static BrowserSpec,
    sys: Arc<Sys>,
    detected: ProbeCache<Option<Installation>>,
    certutil: ProbeCache<Option<PathBuf>>,
    tracker: Tracker,
}

impl FirefoxInterceptor {
    pub(crate) fn new(spec: &'static BrowserSpec, sys: Arc<Sys>) -> Self {
        FirefoxInterceptor {
            spec,
            sys,
            detected: ProbeCache::new(PROBE_TTL),
            certutil: ProbeCache::new(Duration::from_secs(60)),
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

    /// NSS `certutil` (verified: Windows ships an unrelated `certutil.exe`).
    async fn find_certutil(&self) -> Option<PathBuf> {
        let sys = self.sys.clone();
        self.certutil
            .get(|| async move {
                let env = &sys.env;
                let mut candidates: Vec<PathBuf> =
                    env.find_in_path("certutil").into_iter().collect();
                if env.os == crate::host::Os::MacOs {
                    candidates.push(env.sys_path("/opt/homebrew/opt/nss/bin/certutil"));
                    candidates.push(env.sys_path("/usr/local/opt/nss/bin/certutil"));
                }
                for c in candidates.into_iter().filter(|c| c.is_file()) {
                    let out = sys
                        .runner
                        .output(
                            &CommandSpec::new(&c)
                                .arg("-H")
                                .timeout(Duration::from_secs(3)),
                        )
                        .await;
                    if let Ok(out) = out
                        && (out.stderr.contains("NSS") || out.stdout.contains("NSS"))
                    {
                        return Some(c);
                    }
                }
                None
            })
            .await
    }

    /// Add the CA to the profile's NSS DB with certutil. Returns whether it's now trusted.
    async fn install_ca_with_certutil(
        &self,
        certutil: &Path,
        profile: &Path,
        ctx: &InterceptContext,
    ) -> bool {
        let db = format!("sql:{}", profile.display());
        let run = |args: Vec<String>| {
            let spec = CommandSpec::new(certutil)
                .args(args)
                .timeout(Duration::from_secs(10));
            async move { self.sys.runner.output(&spec).await }
        };
        if !profile.join("cert9.db").exists() {
            let _ = run(vec![
                "-N".into(),
                "-d".into(),
                db.clone(),
                "--empty-password".into(),
            ])
            .await;
        }
        // Drop certificates of a previous (regenerated) Akev CA with the same nickname.
        for _ in 0..3 {
            match run(vec![
                "-D".into(),
                "-d".into(),
                db.clone(),
                "-n".into(),
                NSS_NICKNAME.into(),
            ])
            .await
            {
                Ok(out) if out.success() => continue,
                _ => break,
            }
        }
        crate::envvars::ensure_ca_file(ctx);
        let add = run(vec![
            "-A".into(),
            "-d".into(),
            db,
            "-t".into(),
            "C,,".into(),
            "-n".into(),
            NSS_NICKNAME.into(),
            "-i".into(),
            ctx.ca_path.to_string_lossy().into_owned(),
        ])
        .await;
        match add {
            Ok(out) if out.success() => true,
            Ok(out) => {
                tracing::warn!(
                    "certutil couldn't add the CA to the Firefox profile: {}",
                    out.error_summary()
                );
                false
            }
            Err(e) => {
                tracing::warn!("couldn't run certutil: {e}");
                false
            }
        }
    }

    async fn scanned_pids(&self, profile: &Path) -> Vec<u32> {
        if !profile.exists() {
            return Vec::new();
        }
        let procs = self.sys.processes(false).await;
        let tracked = self.tracker.pids();
        matching_pids(&procs, &tracked, |p| {
            cmd_has_flag_value(&p.cmd, "-profile", profile)
                || cmd_has_flag_value(&p.cmd, "--profile", profile)
        })
    }
}

#[async_trait]
impl Interceptor for FirefoxInterceptor {
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
                unavailable_reason: Some(format!("{} isn't installed", self.spec.name)),
                ..d.info()
            };
        };
        let profile = self.profile_dir(ctx, &inst);
        let (version, scanned, certutil) = tokio::join!(
            browsers::version(&self.sys, &inst, browsers::Engine::Firefox),
            self.scanned_pids(&profile),
            self.find_certutil()
        );
        let mut pids = self.tracker.pids();
        pids.extend(scanned);
        let active = !pids.is_empty();
        InterceptorInfo {
            available: true,
            active,
            version,
            metadata: json!({
                "path": inst.exe,
                "install": inst.kind.as_str(),
                "profileDir": profile,
                "pids": pids,
                "caTrusted": profile_has_ca(&profile, &ctx.ca_der),
                "certutil": certutil,
                // Running with an old proxy address or CA: launching again restarts it.
                "outdated": active && launch_config_outdated(&profile, ctx),
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
        let inst = self.installation().await.ok_or_else(|| {
            InterceptError::Unavailable(format!("{} isn't installed", self.spec.name))
        })?;
        let profile = self.profile_dir(ctx, &inst);

        self.sys.invalidate_processes().await;
        let scanned = self.scanned_pids(&profile).await;
        let running = !self.tracker.alive().is_empty() || !scanned.is_empty();
        let restarted = running && launch_config_outdated(&profile, ctx);
        if restarted {
            // It can't reach Akev (or trust its CA) any more: start it again.
            tracing::info!(
                browser = self.spec.id,
                "restarting a browser started for another proxy address or CA"
            );
            stop_all(&self.sys, &self.tracker, scanned, ProcessSignal::Terminate).await;
        } else if running {
            // Our instance runs with -no-remote, so it can't be asked to open a window.
            return Ok(ActivationResult {
                success: true,
                message: Some(format!(
                    "{} is already running with the Akev profile",
                    self.spec.name
                )),
                metadata: json!({ "profileDir": profile, "alreadyRunning": true }),
            });
        }

        std::fs::create_dir_all(&profile)?;
        atomic_write(&profile.join("user.js"), profile_user_js(ctx).as_bytes())?;

        let mut trusted = profile_has_ca(&profile, &ctx.ca_der);
        let mut used_certutil = false;
        if !trusted && let Some(certutil) = self.find_certutil().await {
            used_certutil = true;
            trusted = self
                .install_ca_with_certutil(&certutil, &profile, ctx)
                .await;
        }

        let mut args: Vec<String> = vec![
            "-profile".into(),
            profile.to_string_lossy().into_owned(),
            "-no-remote".into(),
            "-new-instance".into(),
        ];
        if !trusted {
            args.push(CA_IMPORT_URL.into());
        }
        args.push(url.clone());

        let log = ctx
            .data_dir
            .join("logs")
            .join(format!("{}.log", self.spec.id));
        let child = self
            .sys
            .runner
            .spawn(&CommandSpec::new(&inst.exe).args(args).log_file(&log))
            .map_err(|e| {
                InterceptError::Failed(format!("couldn't start {}: {e}", self.spec.name))
            })?;
        let pid = child.pid();
        if child.wait_timeout(LAUNCH_SETTLE).await {
            let code = child.exit_info().and_then(|e| e.code);
            if code != Some(0) {
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
        } else {
            self.tracker.add(child);
        }
        self.sys.invalidate_processes().await;

        let message = if trusted && restarted {
            format!(
                "Restarted {}: it was running with an outdated proxy address or CA",
                self.spec.name
            )
        } else if trusted {
            format!("Launched {} with an isolated Akev profile", self.spec.name)
        } else {
            format!(
                "Launched {}. Firefox keeps its own certificate store{}: in the first tab tick \
                 \"Trust this CA to identify websites\" and click OK (needed once per profile).",
                self.spec.name,
                if used_certutil {
                    " and certutil failed"
                } else {
                    " and NSS certutil isn't installed"
                }
            )
        };
        Ok(ActivationResult {
            success: true,
            message: Some(message),
            metadata: json!({
                "pid": pid,
                "profileDir": profile,
                "url": url,
                "needsManualTrust": !trusted,
                "caImportUrl": if trusted { serde_json::Value::Null } else { json!(CA_IMPORT_URL) },
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
        stop_all(&self.sys, &self.tracker, extra, ProcessSignal::Terminate).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::Os;
    use crate::host::testutil::{fake_env, make_exe, native};
    use crate::runner::CommandOutput;
    use crate::runner::mock::{MockRunner, line_of};
    use crate::util::testctx::ctx;

    fn firefox_spec() -> &'static BrowserSpec {
        browsers::FIREFOX_BROWSERS
            .iter()
            .find(|s| s.id == "firefox")
            .unwrap()
    }

    fn setup(
        with_certutil: bool,
    ) -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        FirefoxInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        make_exe(&dir.path().join("usr/bin/firefox"), "#!/bin/sh\n");
        let runner = MockRunner::new();
        runner.on_prefix(
            "firefox --version",
            CommandOutput::ok("Mozilla Firefox 132.0.2\n"),
        );
        if with_certutil {
            make_exe(&dir.path().join("usr/bin/certutil"), "");
            runner.on_prefix(
                "certutil -H",
                CommandOutput {
                    code: Some(1),
                    stdout: String::new(),
                    stderr: "certutil - Utility to manipulate NSS certificate databases".into(),
                },
            );
            runner.on_prefix("certutil -N", CommandOutput::ok(""));
            runner.on_prefix(
                "certutil -D",
                CommandOutput::failed("could not find certificate named \"Akev CA\""),
            );
            runner.on_prefix("certutil -A", CommandOutput::ok(""));
        }
        let sys = Sys::new(runner.clone(), env);
        let c = ctx(&dir.path().join("data"));
        (dir, runner, FirefoxInterceptor::new(firefox_spec(), sys), c)
    }

    #[test]
    fn user_js_contents() {
        let js = firefox_user_js("127.0.0.1", 8000);
        for line in [
            "user_pref(\"network.proxy.type\", 1);",
            "user_pref(\"network.proxy.http\", \"127.0.0.1\");",
            "user_pref(\"network.proxy.http_port\", 8000);",
            "user_pref(\"network.proxy.ssl\", \"127.0.0.1\");",
            "user_pref(\"network.proxy.ssl_port\", 8000);",
            "user_pref(\"network.proxy.no_proxies_on\", \"\");",
            "user_pref(\"network.proxy.allow_hijacking_localhost\", true);",
            "user_pref(\"security.enterprise_roots.enabled\", true);",
            "user_pref(\"network.captive-portal-service.enabled\", false);",
            "user_pref(\"browser.shell.checkDefaultBrowser\", false);",
            "user_pref(\"datareporting.policy.dataSubmissionEnabled\", false);",
            "user_pref(\"toolkit.telemetry.enabled\", false);",
        ] {
            assert!(js.contains(line), "missing {line}\n{js}");
        }
        // Every pref line is well-formed.
        for l in js.lines().filter(|l| !l.starts_with("//")) {
            assert!(l.starts_with("user_pref(\"") && l.ends_with(");"), "{l}");
        }
        assert!(
            firefox_user_js("[::1]", 9).contains("user_pref(\"network.proxy.http\", \"::1\");")
        );
    }

    #[test]
    fn nss_db_detection() {
        let dir = tempfile::tempdir().unwrap();
        let der: Vec<u8> = (0..64u8).collect();
        assert!(!profile_has_ca(dir.path(), &der));
        let mut db = vec![0u8; 5000];
        db.extend_from_slice(&der);
        db.extend_from_slice(&[1, 2, 3]);
        std::fs::write(dir.path().join("cert9.db-wal"), &db).unwrap();
        assert!(profile_has_ca(dir.path(), &der));
        assert!(
            !profile_has_ca(dir.path(), &der[..8]),
            "too short to be meaningful"
        );
    }

    #[tokio::test]
    async fn activate_with_certutil() {
        let (_dir, runner, i, c) = setup(true);
        let res = i
            .activate(&c, json!({"url": "https://example.com"}))
            .await
            .unwrap();
        assert!(res.success);
        assert_eq!(res.metadata["needsManualTrust"], json!(false));
        let profile = native(&c.data_dir, "profiles/firefox");
        let user_js = std::fs::read_to_string(profile.join("user.js")).unwrap();
        assert!(user_js.contains("\"network.proxy.http_port\", 8000"));

        let lines = runner.call_lines();
        let add = lines.iter().find(|l| l.starts_with("certutil -A")).unwrap();
        assert!(add.contains(&format!("-d sql:{}", profile.display())));
        assert!(add.contains("-t C,, -n Akev CA -i"));
        assert!(lines.iter().any(|l| l.starts_with("certutil -N")));

        let spawned = runner.spawned_specs();
        assert_eq!(
            line_of(&spawned[0]),
            format!(
                "firefox -profile {} -no-remote -new-instance https://example.com",
                profile.display()
            )
        );
        assert!(i.info(&c).await.active);

        // Second activation while running doesn't start a conflicting instance.
        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(res.metadata["alreadyRunning"], json!(true));
        assert_eq!(runner.spawned_specs().len(), 1);

        i.deactivate_all(&c).await.unwrap();
        assert!(!i.info(&c).await.active);
    }

    #[tokio::test]
    async fn activate_without_certutil_opens_trust_dialog() {
        let (_dir, runner, i, c) = setup(false);
        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(res.metadata["needsManualTrust"], json!(true));
        assert!(
            res.message
                .unwrap()
                .contains("Trust this CA to identify websites")
        );
        let args = runner.spawned_specs()[0].args_lossy();
        assert_eq!(
            &args[args.len() - 2..],
            &[CA_IMPORT_URL.to_string(), "about:blank".to_string()]
        );
        assert!(
            !runner
                .call_lines()
                .iter()
                .any(|l| l.starts_with("certutil"))
        );
    }

    #[tokio::test]
    async fn already_trusted_profile_skips_dialog() {
        let (_dir, runner, i, c) = setup(false);
        let profile = native(&c.data_dir, "profiles/firefox");
        std::fs::create_dir_all(&profile).unwrap();
        let mut db = b"SQLite format 3\0".to_vec();
        db.extend_from_slice(&c.ca_der);
        db.extend_from_slice(&[0; 32]);
        std::fs::write(profile.join("cert9.db"), db).unwrap();
        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(res.metadata["needsManualTrust"], json!(false));
        assert!(
            !runner.spawned_specs()[0]
                .args_lossy()
                .contains(&CA_IMPORT_URL.to_string())
        );
        assert_eq!(i.info(&c).await.metadata["caTrusted"], json!(true));
    }

    fn firefox_process(pid: u32, profile: &Path) -> crate::runner::ProcessEntry {
        crate::runner::ProcessEntry {
            pid,
            name: "firefox".into(),
            cmd: vec![
                "firefox".into(),
                "-profile".into(),
                profile.to_string_lossy().into_owned(),
                "-no-remote".into(),
            ],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn outdated_running_instance_is_restarted() {
        let (_dir, runner, i, c) = setup(true);
        // Launched for port 7000 (before a port change), still running.
        let mut old = c.clone();
        old.proxy_port = 7000;
        let profile = native(&c.data_dir, "profiles/firefox");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(profile.join("user.js"), profile_user_js(&old)).unwrap();
        runner.processes.lock().push(firefox_process(900, &profile));
        assert!(launch_config_outdated(&profile, &c));
        assert_eq!(i.info(&c).await.metadata["outdated"], json!(true));

        let res = i.activate(&c, json!({})).await.unwrap();
        assert_eq!(
            runner.signals.lock().clone(),
            vec![(900, ProcessSignal::Terminate)]
        );
        assert_eq!(res.metadata["restarted"], json!(true));
        assert!(res.message.unwrap().contains("Restarted"));
        assert_eq!(runner.spawned_specs().len(), 1);
        let user_js = std::fs::read_to_string(profile.join("user.js")).unwrap();
        assert!(user_js.contains("\"network.proxy.http_port\", 8000"));
        assert!(!launch_config_outdated(&profile, &c));

        // A regenerated CA makes it outdated too.
        let mut new_ca = c.clone();
        new_ca.ca_spki_fingerprint = "bmV3LWNh".into();
        assert!(launch_config_outdated(&profile, &new_ca));
    }

    #[tokio::test]
    async fn snap_profile_belongs_to_one_data_dir() {
        let dir = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, dir.path());
        // Ubuntu's Firefox is a snap.
        make_exe(&env.sys_path("/snap/bin/firefox"), "");
        let runner = MockRunner::new();
        let sys = Sys::new(runner.clone(), env);
        // Instance A (the desktop app) runs an intercepted Firefox...
        let a = ctx(&dir.path().join("data-a"));
        let firefox = FirefoxInterceptor::new(firefox_spec(), sys);
        let inst = firefox.installation().await.unwrap();
        let profile_a = firefox.profile_dir(&a, &inst);
        assert!(
            profile_a.starts_with(dir.path().join("home/snap/firefox/current/akev-profiles")),
            "{}",
            profile_a.display()
        );
        std::fs::create_dir_all(&profile_a).unwrap();
        runner
            .processes
            .lock()
            .push(firefox_process(4242, &profile_a));

        // ...while instance B (e.g. `akev capture`, ephemeral data dir) comes and goes.
        let b = ctx(&dir.path().join("data-b"));
        let profile_b = firefox.profile_dir(&b, &inst);
        assert_ne!(profile_a, profile_b);
        let info = firefox.info(&b).await;
        assert!(!info.active, "{info:?}");
        let res = firefox.activate(&b, json!({})).await.unwrap();
        assert_ne!(res.metadata["alreadyRunning"], json!(true));
        assert_eq!(runner.spawned_specs().len(), 1);
        runner.exit_all();
        firefox.deactivate_all(&b).await.unwrap();
        assert!(
            runner.signals.lock().iter().all(|(pid, _)| *pid != 4242),
            "B never stops A's browser: {:?}",
            runner.signals.lock()
        );
        assert!(firefox.info(&a).await.active);
    }

    #[tokio::test]
    async fn windows_certutil_is_not_nss() {
        let (dir, runner, i, _c) = setup(false);
        make_exe(&dir.path().join("usr/bin/certutil"), "");
        runner.on_prefix(
            "certutil -H",
            CommandOutput::ok("CertUtil -? Verbs: -dump ..."),
        );
        assert_eq!(i.find_certutil().await, None);
    }
}
