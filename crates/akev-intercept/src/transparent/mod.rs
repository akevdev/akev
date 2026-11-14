//! "Transparent capture (Linux)": captures programs that ignore proxy settings by having the
//! kernel redirect their connections to Akev's transparent listener — per app (cgroup v2
//! slice), per user, or machine-wide.
//!
//! Akev never touches the firewall itself (that needs root): activating this interceptor
//! returns the exact commands to run ([`plan::generate`]), and its status reports whether
//! redirected connections are actually arriving (from the proxy, through
//! [`InterceptContext::transparent`](crate::InterceptContext::transparent)).

pub mod facts;
pub mod plan;

use crate::host::Os;
use crate::util::{Descriptor, Sys};
use crate::{InterceptContext, InterceptError, Interceptor, Result};
use akev_core::interceptor::{ActivationResult, InterceptorCategory, InterceptorInfo};
use akev_core::transparent::{TransparentStatus, now_ms};
use async_trait::async_trait;
pub use facts::SystemFacts;
pub use plan::{Plan, PlanContext, PlanError, PlanRequest};
use std::sync::Arc;

/// Interceptor id.
pub const ID: &str = "transparent-linux";

const DESCRIPTOR: Descriptor = Descriptor {
    id: ID,
    name: "Transparent capture (Linux)",
    description: "Capture apps that ignore proxy settings: the firewall redirects their connections to Akev — per app, per user or machine-wide.",
    category: InterceptorCategory::System,
    tags: &[
        "transparent",
        "redirect",
        "nftables",
        "nft",
        "iptables",
        "firewall",
        "cgroup",
        "per-app",
        "linux",
        "everything",
        "ignores proxy",
        "system",
    ],
    icon: "linux",
    needs_config: true,
};

/// Variables that make a launched program trust Akev's CA (no proxy variables: its
/// connections are redirected instead).
const CA_ENV_VARS: &[&str] = &[
    "SSL_CERT_FILE",
    "REQUESTS_CA_BUNDLE",
    "CURL_CA_BUNDLE",
    "GIT_SSL_CAINFO",
    "AWS_CA_BUNDLE",
    "DENO_CERT",
    "NODE_EXTRA_CA_CERTS",
];

/// Why transparent capture isn't available on other systems.
pub const UNSUPPORTED_REASON: &str = "Transparent capture needs Linux netfilter. macOS (pf `rdr` + DIOCNATLOOK) and Windows (WinDivert) aren't supported yet.";

/// Live listener status from the context, or "not running" when the embedder has no proxy.
pub fn listener_status(ctx: &InterceptContext) -> TransparentStatus {
    ctx.transparent
        .as_ref()
        .map(|p| p.status())
        .unwrap_or_default()
}

/// Build the [`PlanContext`] for `req` on this machine.
pub fn plan_context(
    ctx: &InterceptContext,
    sys: &Sys,
    facts: SystemFacts,
    req: &PlanRequest,
) -> PlanContext {
    let status = listener_status(ctx);
    let ca_env = if req.trust_ca && req.mode == plan::CaptureMode::App {
        crate::envvars::local_env_vars(ctx, &sys.env)
            .into_iter()
            .filter(|(k, _)| CA_ENV_VARS.contains(&k.as_str()))
            // The commands run from any directory: never hand out relative paths.
            .map(|(k, v)| {
                let abs = std::path::absolute(&v)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(v);
                (k, abs)
            })
            .collect()
    } else {
        Vec::new()
    };
    let target_user = req
        .user
        .as_deref()
        .filter(|u| !u.trim().is_empty())
        .and_then(|u| facts::resolve_user(&sys.env, u));
    PlanContext {
        facts,
        port: status.port.unwrap_or(status.requested_port),
        listening: status.listening,
        mark: status.mark,
        mark_error: status.mark_error,
        ca_env,
        target_user,
    }
}

/// Generate the commands for `req` (`POST /api/transparent/plan`).
pub fn generate_plan(
    ctx: &InterceptContext,
    sys: &Sys,
    req: &PlanRequest,
) -> std::result::Result<Plan, PlanError> {
    let facts = SystemFacts::detect(&sys.env);
    plan::generate(req, &plan_context(ctx, sys, facts, req))
}

/// The `transparent-linux` interceptor.
pub struct TransparentInterceptor {
    sys: Arc<Sys>,
}

impl TransparentInterceptor {
    /// Interceptor for the machine described by `sys`.
    pub fn new(sys: Arc<Sys>) -> Self {
        TransparentInterceptor { sys }
    }

    fn supported(&self) -> bool {
        self.sys.env.os == Os::Linux
    }
}

#[async_trait]
impl Interceptor for TransparentInterceptor {
    fn id(&self) -> &str {
        ID
    }

    async fn info(&self, ctx: &InterceptContext) -> InterceptorInfo {
        if !self.supported() {
            return InterceptorInfo {
                unavailable_reason: Some(UNSUPPORTED_REASON.into()),
                ..DESCRIPTOR.unsupported(self.sys.env.os.as_str())
            };
        }
        let status = listener_status(ctx);
        let facts = SystemFacts::detect(&self.sys.env);
        let mut info = DESCRIPTOR.info();
        info.available = true;
        info.active = status.redirect_active(now_ms());
        info.metadata = serde_json::json!({
            "listener": status,
            "system": facts,
            "slice": plan::CAPTURE_SLICE,
        });
        info
    }

    async fn activate(
        &self,
        ctx: &InterceptContext,
        options: serde_json::Value,
    ) -> Result<ActivationResult> {
        if !self.supported() {
            return Err(InterceptError::Unavailable(UNSUPPORTED_REASON.into()));
        }
        let req: PlanRequest = serde_json::from_value(options)
            .map_err(|e| InterceptError::Failed(format!("invalid options: {e}")))?;
        let plan = generate_plan(ctx, &self.sys, &req).map_err(|e| InterceptError::Failed(e.0))?;
        Ok(ActivationResult {
            success: true,
            message: Some(
                "Run the setup commands in a terminal (they need root: Akev never changes your firewall itself)."
                    .into(),
            ),
            metadata: serde_json::to_value(&plan).unwrap_or_default(),
        })
    }

    async fn deactivate(&self, _ctx: &InterceptContext, _options: serde_json::Value) -> Result<()> {
        Err(InterceptError::Failed(
            "Akev can't remove firewall rules itself (that needs root): run the teardown commands from the Transparent capture panel (sudo nft delete table inet akev).".into(),
        ))
    }

    async fn deactivate_all(&self, _ctx: &InterceptContext) -> Result<()> {
        // Nothing of ours to undo: the rules are the user's (see `deactivate`).
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::testutil::fake_env;
    use crate::runner::mock::MockRunner;
    use crate::util::testctx::ctx;
    use akev_core::transparent::{TransparentProbe, TransparentProbeHandle};

    struct Listener(TransparentStatus);
    impl TransparentProbe for Listener {
        fn transparent_status(&self) -> TransparentStatus {
            self.0.clone()
        }
    }

    fn sys(os: Os, root: &std::path::Path) -> Arc<Sys> {
        Sys::new(MockRunner::new(), fake_env(os, root))
    }

    fn listening(redirected_at: Option<f64>) -> TransparentProbeHandle {
        TransparentProbeHandle::new(Listener(TransparentStatus {
            enabled: true,
            supported: true,
            listening: true,
            port: Some(8001),
            requested_port: 8001,
            addresses: vec!["127.0.0.1:8001".into()],
            last_redirect_at: redirected_at,
            redirected: u64::from(redirected_at.is_some()),
            ..Default::default()
        }))
    }

    #[tokio::test]
    async fn info_reports_listener_and_redirect_activity() {
        let dir = tempfile::tempdir().unwrap();
        let i = TransparentInterceptor::new(sys(Os::Linux, dir.path()));
        let mut c = ctx(&dir.path().join("data"));
        // No proxy attached: available (the panel explains), not active.
        let info = i.info(&c).await;
        assert!(info.supported && info.available && !info.active && info.needs_config);
        assert_eq!(info.id, ID);
        assert_eq!(info.metadata["listener"]["listening"], false);
        assert_eq!(info.metadata["system"]["platform"], "linux");
        assert_eq!(info.metadata["slice"], "akev-capture.slice");

        c.transparent = Some(listening(None));
        let info = i.info(&c).await;
        assert!(!info.active, "listening but nothing redirected yet");
        assert_eq!(info.metadata["listener"]["port"], 8001);

        c.transparent = Some(listening(Some(now_ms() - 1000.0)));
        assert!(i.info(&c).await.active);
        c.transparent = Some(listening(Some(now_ms() - 3_600_000.0)));
        assert!(!i.info(&c).await.active, "an hour-old redirect is stale");
    }

    #[tokio::test]
    async fn unsupported_off_linux() {
        let dir = tempfile::tempdir().unwrap();
        let c = ctx(&dir.path().join("data"));
        for os in [Os::MacOs, Os::Windows] {
            let i = TransparentInterceptor::new(sys(os, dir.path()));
            let info = i.info(&c).await;
            assert!(!info.supported && !info.available);
            assert!(info.unavailable_reason.unwrap().contains("WinDivert"));
            assert!(matches!(
                i.activate(&c, serde_json::json!({})).await,
                Err(InterceptError::Unavailable(_))
            ));
        }
    }

    #[tokio::test]
    async fn activation_returns_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sys/fs/cgroup")).unwrap();
        std::fs::write(root.join("sys/fs/cgroup/cgroup.controllers"), "").unwrap();
        let i = TransparentInterceptor::new(sys(Os::Linux, root));
        let mut c = ctx(&root.join("data"));
        c.transparent = Some(listening(None));
        let r = i.activate(&c, serde_json::json!({})).await.unwrap();
        assert!(r.success);
        let plan: Plan = serde_json::from_value(r.metadata).unwrap();
        assert_eq!(plan.port, 8001);
        assert!(plan.rules.contains("socket cgroupv2"), "{}", plan.rules);
        // The launch makes programs trust the CA, without proxy variables.
        let launch = plan.launch.unwrap().command;
        assert!(
            launch.contains("SSL_CERT_FILE=") && launch.contains("NODE_EXTRA_CA_CERTS="),
            "{launch}"
        );
        assert!(!launch.contains("HTTP_PROXY"), "{launch}");

        let r = i
            .activate(
                &c,
                serde_json::json!({"mode": "machine", "backend": "iptables"}),
            )
            .await
            .unwrap();
        assert!(
            r.metadata["rules"]
                .as_str()
                .unwrap()
                .contains("iptables -t nat")
        );

        assert!(matches!(
            i.activate(&c, serde_json::json!({"mode": "bogus"})).await,
            Err(InterceptError::Failed(m)) if m.contains("invalid options")
        ));
        assert!(matches!(
            i.activate(&c, serde_json::json!({"mode": "user", "user": "nobody-here"})).await,
            Err(InterceptError::Failed(m)) if m.contains("no user")
        ));
        // Deactivation is the user's job (root), and says how.
        assert!(matches!(
            i.deactivate(&c, serde_json::json!({})).await,
            Err(InterceptError::Failed(m)) if m.contains("nft delete table inet akev")
        ));
        i.deactivate_all(&c).await.unwrap();
    }
}
