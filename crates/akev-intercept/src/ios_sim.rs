//! iOS Simulator (macOS): trust the Akev CA in booted simulators with one click.
//!
//! `xcrun simctl keychain <udid> add-root-cert <ca.pem>` adds the CA as a trusted root inside
//! the simulator. Simulators use the Mac's network settings, so their traffic reaches Akev
//! through the system proxy (the `system-proxy` interceptor) or a proxy the app sets itself.
//! `simctl` can't remove one root certificate again: deactivating only forgets the device (the
//! CA stays trusted in that simulator until its keychain is reset or it is erased).

use crate::host::Os;
use crate::runner::CommandSpec;
use crate::util::{Descriptor, PROBE_TTL, ProbeCache, Sys, opt_str, tags};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

const SIMCTL_TIMEOUT: Duration = Duration::from_secs(20);

/// One booted simulator.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Simulator {
    udid: String,
    name: String,
    /// e.g. `iOS 18.1`.
    runtime: String,
}

pub(crate) struct IosSimulatorInterceptor {
    sys: Arc<Sys>,
    scan: ProbeCache<std::result::Result<Vec<Simulator>, String>>,
    trusted: Mutex<BTreeSet<String>>,
}

impl IosSimulatorInterceptor {
    const DESCRIPTOR: Descriptor = Descriptor {
        id: "ios-simulator",
        name: "iOS Simulator",
        description: "Trust the Akev CA in booted iOS simulators (Xcode); they use your Mac's proxy settings",
        category: InterceptorCategory::Mobile,
        tags: tags::IOS,
        icon: "ios",
        needs_config: false,
    };

    pub(crate) fn new(sys: Arc<Sys>) -> Self {
        IosSimulatorInterceptor {
            sys,
            scan: ProbeCache::new(PROBE_TTL),
            trusted: Mutex::new(BTreeSet::new()),
        }
    }

    async fn booted(&self) -> std::result::Result<Vec<Simulator>, String> {
        let sys = self.sys.clone();
        self.scan
            .get(|| async move {
                let spec = CommandSpec::new("xcrun")
                    .args(["simctl", "list", "devices", "booted", "-j"])
                    .timeout(SIMCTL_TIMEOUT);
                match sys.runner.output(&spec).await {
                    Ok(out) if out.success() => Ok(parse_booted(&out.stdout)),
                    Ok(out) => Err(out
                        .stderr
                        .lines()
                        .next()
                        .unwrap_or("simctl failed")
                        .to_string()),
                    Err(_) => {
                        Err("Xcode's command-line tools (xcrun simctl) aren't installed".into())
                    }
                }
            })
            .await
    }
}

/// Booted devices from `simctl list devices booted -j`.
fn parse_booted(json: &str) -> Vec<Simulator> {
    let Ok(v) = serde_json::from_str::<Value>(json) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(runtimes) = v.get("devices").and_then(Value::as_object) {
        for (runtime, devices) in runtimes {
            // `com.apple.CoreSimulator.SimRuntime.iOS-18-1` → `iOS 18.1`.
            let short = runtime.rsplit('.').next().unwrap_or(runtime);
            let pretty = match short.split_once('-') {
                Some((os, version)) => format!("{os} {}", version.replace('-', ".")),
                None => short.to_string(),
            };
            for d in devices.as_array().into_iter().flatten() {
                if d.get("state").and_then(Value::as_str) != Some("Booted") {
                    continue;
                }
                if let Some(udid) = d.get("udid").and_then(Value::as_str) {
                    out.push(Simulator {
                        udid: udid.to_string(),
                        name: d
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("Simulator")
                            .to_string(),
                        runtime: pretty.clone(),
                    });
                }
            }
        }
    }
    out
}

#[async_trait]
impl Interceptor for IosSimulatorInterceptor {
    fn id(&self) -> &str {
        Self::DESCRIPTOR.id
    }

    async fn info(&self, _ctx: &InterceptContext) -> InterceptorInfo {
        let d = Self::DESCRIPTOR;
        if self.sys.env.os != Os::MacOs {
            return d.unsupported(self.sys.env.os.as_str());
        }
        let mut info = d.info();
        match self.booted().await {
            Ok(sims) => {
                info.available = !sims.is_empty();
                if sims.is_empty() {
                    info.unavailable_reason =
                        Some("No simulator is running: start one in Xcode or Simulator.app".into());
                }
                let trusted = self.trusted.lock().clone();
                info.active = sims.iter().any(|s| trusted.contains(&s.udid));
                info.metadata = json!({
                    "simulators": sims.iter().map(|s| json!({
                        "udid": s.udid,
                        "name": s.name,
                        "runtime": s.runtime,
                        "trusted": trusted.contains(&s.udid),
                    })).collect::<Vec<_>>(),
                    "notes": [
                        "Simulators use your Mac's network settings: turn on the System proxy interceptor so their traffic goes through Akev.",
                        "Apps with certificate pinning still reject Akev's certificates.",
                    ],
                });
            }
            Err(e) => info.unavailable_reason = Some(e),
        }
        info
    }

    async fn activate(&self, ctx: &InterceptContext, options: Value) -> Result<ActivationResult> {
        if self.sys.env.os != Os::MacOs {
            return Err(InterceptError::Unavailable(
                "The iOS Simulator only runs on macOS".into(),
            ));
        }
        crate::envvars::ensure_ca_file(ctx);
        self.scan.invalidate().await;
        let sims = self.booted().await.map_err(InterceptError::Unavailable)?;
        let wanted: Vec<&Simulator> = match opt_str(&options, "udid") {
            Some(id) => sims.iter().filter(|s| s.udid == id).collect(),
            None => sims.iter().collect(),
        };
        if wanted.is_empty() {
            return Err(InterceptError::Unavailable(
                "No matching simulator is running: start one in Xcode or Simulator.app".into(),
            ));
        }
        let mut done = Vec::new();
        for sim in wanted {
            let spec = CommandSpec::new("xcrun")
                .args(["simctl", "keychain", &sim.udid, "add-root-cert"])
                .arg(&ctx.ca_path)
                .timeout(SIMCTL_TIMEOUT);
            let out = self
                .sys
                .runner
                .output(&spec)
                .await
                .map_err(|e| InterceptError::Failed(format!("simctl: {e}")))?;
            if !out.success() {
                return Err(InterceptError::Failed(format!(
                    "Couldn't add the certificate to {}: {}",
                    sim.name,
                    out.stderr.lines().next().unwrap_or("simctl failed")
                )));
            }
            self.trusted.lock().insert(sim.udid.clone());
            done.push(format!("{} ({})", sim.name, sim.runtime));
        }
        Ok(ActivationResult {
            success: true,
            message: Some(format!(
                "The Akev CA is trusted in {}. Simulators use your Mac's proxy settings: turn on the System proxy interceptor so their traffic goes through Akev, then restart the apps you test.",
                done.join(", ")
            )),
            metadata: json!({ "simulators": done }),
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, options: Value) -> Result<()> {
        match opt_str(&options, "udid") {
            Some(id) => {
                self.trusted.lock().remove(id);
            }
            None => self.trusted.lock().clear(),
        }
        Ok(())
    }

    async fn deactivate_all(&self, ctx: &InterceptContext) -> Result<()> {
        self.deactivate(ctx, json!({})).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::fake_env;
    use crate::runner::CommandOutput;
    use crate::runner::mock::MockRunner;

    const BOOTED: &str = r#"{"devices":{"com.apple.CoreSimulator.SimRuntime.iOS-18-1":[{"udid":"A1B2","name":"iPhone 16","state":"Booted"},{"udid":"OFF","name":"iPad","state":"Shutdown"}],"com.apple.CoreSimulator.SimRuntime.watchOS-11-0":[]}}"#;

    fn setup(
        os: Os,
    ) -> (
        tempfile::TempDir,
        Arc<MockRunner>,
        IosSimulatorInterceptor,
        InterceptContext,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let runner = MockRunner::new();
        runner.on_prefix(
            "xcrun simctl list devices booted -j",
            CommandOutput::ok(BOOTED),
        );
        runner.on_prefix("xcrun simctl keychain", CommandOutput::ok(""));
        let i = IosSimulatorInterceptor::new(Sys::new(runner.clone(), fake_env(os, dir.path())));
        let c = crate::util::testctx::ctx(&dir.path().join("data"));
        (dir, runner, i, c)
    }

    #[test]
    fn parses_booted_simulators() {
        let sims = parse_booted(BOOTED);
        assert_eq!(
            sims,
            vec![Simulator {
                udid: "A1B2".into(),
                name: "iPhone 16".into(),
                runtime: "iOS 18.1".into()
            }]
        );
        assert!(parse_booted("nope").is_empty());
    }

    #[tokio::test]
    async fn trusts_the_ca_in_booted_simulators() {
        let (_dir, runner, i, c) = setup(Os::MacOs);
        let info = i.info(&c).await;
        assert!(info.available && !info.active);
        let res = i.activate(&c, json!({})).await.unwrap();
        assert!(res.message.unwrap().contains("iPhone 16 (iOS 18.1)"));
        assert!(runner.call_lines().iter().any(|l| {
            l.starts_with("xcrun simctl keychain A1B2 add-root-cert ") && l.ends_with("ca.pem")
        }));
        assert!(i.info(&c).await.active);
        i.deactivate_all(&c).await.unwrap();
        assert!(!i.info(&c).await.active);
        assert!(matches!(
            i.activate(&c, json!({"udid": "OFF"})).await,
            Err(InterceptError::Unavailable(_))
        ));
    }

    #[tokio::test]
    async fn only_on_macos() {
        let (_dir, _runner, i, c) = setup(Os::Linux);
        assert!(!i.info(&c).await.supported);
        assert!(i.activate(&c, json!({})).await.is_err());
    }
}
