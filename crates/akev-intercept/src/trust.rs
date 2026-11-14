//! Trusting the Akev CA in the operating system's certificate stores: detect whether it is
//! trusted, install it, remove it.
//!
//! Nothing here runs on its own: every install or removal is an explicit user action (the REST
//! API only; MCP agents can't reach it), and the operating system asks the user to confirm:
//!
//! * **macOS**: the login keychain, as a trusted root for the current user
//!   (`security add-trusted-cert`; macOS shows its password dialog).
//! * **Windows**: the current user's Trusted Root store (`certutil -user -addstore Root`; Windows
//!   shows its security warning).
//! * **Linux**: the distribution's CA bundle through polkit (`pkexec`, which shows the desktop's
//!   password dialog), with p11-kit's `trust anchor` or Debian's `update-ca-certificates`; and
//!   the NSS database Chrome and Chromium read (`~/.pki/nssdb`, no password, needs `certutil`).
//!
//! Firefox keeps its own store; its interceptor handles that per profile.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;
use sha1::{Digest, Sha1};

use crate::InterceptContext;
use crate::host::Os;
use crate::runner::{CommandOutput, CommandSpec};
use crate::util::Sys;

/// Long enough for the user to read and answer the operating system's confirmation dialog.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(180);
/// Detection commands are quick.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the CA can be trusted on this computer, and whether it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustReport {
    /// Whether the store most apps use trusts the CA (`None`: couldn't tell).
    pub trusted: Option<bool>,
    /// Every store Akev can manage here, the main one first.
    pub stores: Vec<TrustStore>,
}

/// One certificate store.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustStore {
    /// `user`, `system` or `nss`.
    pub id: String,
    pub label: String,
    /// Who reads this store.
    pub description: String,
    /// Whether it trusts the CA (`None`: couldn't tell).
    pub trusted: Option<bool>,
    /// Akev can install and remove the CA here.
    pub can_change: bool,
    /// Changing it asks for an administrator password.
    pub needs_admin: bool,
    /// Why it can't be changed from Akev (with what to do instead).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

/// Why trusting or untrusting the CA failed: a sentence for the user.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum TrustError {
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Failed(String),
}

/// Manages the CA in the operating system's stores.
pub struct CaTrust {
    sys: Arc<Sys>,
}

impl CaTrust {
    /// For this computer.
    pub fn system() -> Self {
        CaTrust { sys: Sys::system() }
    }

    /// With a custom command runner and host environment (tests).
    pub fn with_sys(sys: Arc<Sys>) -> Self {
        CaTrust { sys }
    }

    fn os(&self) -> Os {
        self.sys.env.os
    }

    /// Which stores exist here and whether they trust the CA.
    pub async fn report(&self, ctx: &InterceptContext) -> TrustReport {
        let stores = match self.os() {
            Os::MacOs => vec![self.macos_user(ctx).await],
            Os::Windows => vec![self.windows_user(ctx).await],
            Os::Linux => {
                let mut stores = vec![self.linux_system(ctx)];
                if let Some(nss) = self.linux_nss(ctx).await {
                    stores.push(nss);
                }
                stores
            }
            Os::Other => Vec::new(),
        };
        TrustReport {
            trusted: stores.first().and_then(|s| s.trusted),
            stores,
        }
    }

    /// Install the CA in `store`. The operating system asks the user to confirm.
    pub async fn install(&self, ctx: &InterceptContext, store: &str) -> Result<(), TrustError> {
        let spec = match (self.os(), store) {
            (Os::MacOs, "user") => {
                let keychain = self.login_keychain()?;
                CommandSpec::new("security")
                    .args(["add-trusted-cert", "-r", "trustRoot", "-k"])
                    .arg(keychain)
                    .arg(&ctx.ca_path)
            }
            (Os::Windows, "user") => CommandSpec::new("certutil")
                .args(["-user", "-addstore", "Root"])
                .arg(&ctx.ca_path),
            (Os::Linux, "system") => self.linux_system_command(ctx, true)?,
            (Os::Linux, "nss") => return self.nss_install(ctx).await,
            _ => return Err(unknown_store(store)),
        };
        self.confirm(spec, "trust the Akev certificate").await
    }

    /// Remove the CA from `store`. The operating system asks the user to confirm.
    pub async fn remove(&self, ctx: &InterceptContext, store: &str) -> Result<(), TrustError> {
        match (self.os(), store) {
            (Os::MacOs, "user") => {
                let keychain = self.login_keychain()?;
                self.confirm(
                    CommandSpec::new("security")
                        .arg("remove-trusted-cert")
                        .arg(&ctx.ca_path),
                    "remove the Akev certificate's trust setting",
                )
                .await?;
                // Also drop the certificate itself; fine if it isn't there.
                let delete = CommandSpec::new("security")
                    .args(["delete-certificate", "-Z", &sha1_hex(&ctx.ca_der)])
                    .arg(keychain)
                    .timeout(CONFIRM_TIMEOUT);
                let _ = self.sys.runner.output(&delete).await;
                Ok(())
            }
            (Os::Windows, "user") => {
                self.confirm(
                    CommandSpec::new("certutil").args([
                        "-user",
                        "-delstore",
                        "Root",
                        &sha1_hex(&ctx.ca_der),
                    ]),
                    "remove the Akev certificate",
                )
                .await
            }
            (Os::Linux, "system") => {
                let spec = self.linux_system_command(ctx, false)?;
                self.confirm(spec, "remove the Akev certificate").await
            }
            (Os::Linux, "nss") => self.nss_remove(ctx).await,
            _ => Err(unknown_store(store)),
        }
    }

    async fn confirm(&self, spec: CommandSpec, what: &str) -> Result<(), TrustError> {
        let spec = spec.timeout(CONFIRM_TIMEOUT);
        match self.sys.runner.output(&spec).await {
            Ok(out) if out.success() => Ok(()),
            Ok(out) => Err(TrustError::Failed(failure(what, &out))),
            Err(e) if e.kind() == std::io::ErrorKind::TimedOut => Err(TrustError::Failed(format!(
                "Couldn't {what}: the confirmation wasn't answered in time."
            ))),
            Err(e) => Err(TrustError::Failed(format!("Couldn't {what}: {e}"))),
        }
    }

    async fn succeeds(&self, spec: CommandSpec) -> Option<bool> {
        match self.sys.runner.output(&spec.timeout(PROBE_TIMEOUT)).await {
            Ok(out) => Some(out.success()),
            Err(_) => None,
        }
    }

    // ------------------------------------------------------------------------- macOS

    fn login_keychain(&self) -> Result<PathBuf, TrustError> {
        self.sys
            .env
            .home_path("Library/Keychains/login.keychain-db")
            .ok_or_else(|| TrustError::Unavailable("Your home folder couldn't be found.".into()))
    }

    async fn macos_user(&self, ctx: &InterceptContext) -> TrustStore {
        // Verifying the CA as a certificate succeeds only when a trust setting makes it a root.
        let trusted = self
            .succeeds(
                CommandSpec::new("security")
                    .args(["verify-cert", "-L", "-p", "ssl", "-c"])
                    .arg(&ctx.ca_path),
            )
            .await;
        TrustStore {
            id: "user".into(),
            label: "macOS keychain (your account)".into(),
            description: "Safari, Chrome, Edge, curl and apps that use the system's TLS".into(),
            trusted,
            can_change: true,
            needs_admin: true,
            unavailable_reason: None,
        }
    }

    // ------------------------------------------------------------------------ Windows

    async fn windows_user(&self, ctx: &InterceptContext) -> TrustStore {
        let trusted = self
            .succeeds(CommandSpec::new("certutil").args([
                "-user",
                "-store",
                "Root",
                &sha1_hex(&ctx.ca_der),
            ]))
            .await;
        TrustStore {
            id: "user".into(),
            label: "Windows certificate store (your account)".into(),
            description: "Edge, Chrome, .NET and most Windows apps".into(),
            trusted,
            can_change: true,
            needs_admin: false,
            unavailable_reason: None,
        }
    }

    // -------------------------------------------------------------------------- Linux

    /// Bundles the distributions build from their trust stores.
    const LINUX_BUNDLES: [&'static str; 4] = [
        "/etc/ssl/certs/ca-certificates.crt",
        "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
        "/etc/pki/tls/certs/ca-bundle.crt",
        "/etc/ssl/ca-bundle.pem",
    ];

    fn linux_system(&self, ctx: &InterceptContext) -> TrustStore {
        let wanted = pem_body(&ctx.ca_pem);
        let mut seen_bundle = false;
        let mut trusted = false;
        for bundle in Self::LINUX_BUNDLES {
            if let Ok(text) = std::fs::read_to_string(self.sys.env.sys_path(bundle)) {
                seen_bundle = true;
                if pem_bodies(&text).any(|b| b == wanted) {
                    trusted = true;
                    break;
                }
            }
        }
        let tool = self.linux_tool();
        let pkexec = self.sys.env.find_in_path("pkexec").is_some();
        let unavailable_reason = match (&tool, pkexec) {
            (None, _) => Some(
                "Neither p11-kit's `trust` nor `update-ca-certificates` was found; add the certificate with your distribution's tools.".into(),
            ),
            (Some(_), false) => Some(
                "`pkexec` (polkit) isn't installed, so Akev can't ask for your password; use the commands below in a terminal.".into(),
            ),
            _ => None,
        };
        TrustStore {
            id: "system".into(),
            label: "System certificates".into(),
            description:
                "curl, wget, Python, Go, Node with --use-openssl-ca and most command-line tools"
                    .into(),
            trusted: seen_bundle.then_some(trusted),
            can_change: unavailable_reason.is_none(),
            needs_admin: true,
            unavailable_reason,
        }
    }

    fn linux_tool(&self) -> Option<LinuxTool> {
        if self.sys.env.find_in_path("trust").is_some() {
            Some(LinuxTool::P11Kit)
        } else if self
            .sys
            .env
            .find_in_path("update-ca-certificates")
            .is_some()
        {
            Some(LinuxTool::Debian)
        } else {
            None
        }
    }

    fn linux_system_command(
        &self,
        ctx: &InterceptContext,
        install: bool,
    ) -> Result<CommandSpec, TrustError> {
        if self.sys.env.find_in_path("pkexec").is_none() {
            return Err(TrustError::Unavailable(
                "`pkexec` (polkit) isn't installed; run the commands shown in a terminal instead."
                    .into(),
            ));
        }
        let ca = ctx.ca_path.to_string_lossy().into_owned();
        Ok(match (self.linux_tool(), install) {
            (Some(LinuxTool::P11Kit), true) => {
                CommandSpec::new("pkexec").args(["trust", "anchor", "--store", &ca])
            }
            (Some(LinuxTool::P11Kit), false) => {
                CommandSpec::new("pkexec").args(["trust", "anchor", "--remove", &ca])
            }
            (Some(LinuxTool::Debian), true) => CommandSpec::new("pkexec").args([
                "sh",
                "-c",
                "cp \"$1\" /usr/local/share/ca-certificates/akev-ca.crt && update-ca-certificates",
                "sh",
                &ca,
            ]),
            (Some(LinuxTool::Debian), false) => CommandSpec::new("pkexec").args([
                "sh",
                "-c",
                "rm -f /usr/local/share/ca-certificates/akev-ca.crt && update-ca-certificates --fresh",
            ]),
            (None, _) => {
                return Err(TrustError::Unavailable(
                    "Neither p11-kit's `trust` nor `update-ca-certificates` was found.".into(),
                ));
            }
        })
    }

    fn nss_db(&self) -> Option<PathBuf> {
        self.sys.env.home_path(".pki/nssdb")
    }

    /// Chrome/Chromium's NSS database, when `certutil` (nss-tools) is installed.
    async fn linux_nss(&self, ctx: &InterceptContext) -> Option<TrustStore> {
        let db = self.nss_db()?;
        let has_certutil = self.sys.env.find_in_path("certutil").is_some();
        let trusted = if has_certutil && db.join("cert9.db").is_file() {
            self.succeeds(
                CommandSpec::new("certutil")
                    .arg("-d")
                    .arg(sql_db(&db))
                    .args(["-L", "-n", &nss_nickname(&ctx.ca_der)]),
            )
            .await
        } else if db.join("cert9.db").is_file() {
            None
        } else {
            Some(false)
        };
        Some(TrustStore {
            id: "nss".into(),
            label: "Chrome and Chromium (NSS)".into(),
            description: "Chrome, Chromium, Brave, Edge and Vivaldi on Linux".into(),
            trusted,
            can_change: has_certutil,
            needs_admin: false,
            unavailable_reason: (!has_certutil).then(|| {
                "Install `certutil` (nss-tools on Fedora, libnss3-tools on Debian and Ubuntu) to manage it from Akev.".into()
            }),
        })
    }

    async fn nss_install(&self, ctx: &InterceptContext) -> Result<(), TrustError> {
        let db = self
            .nss_db()
            .ok_or_else(|| TrustError::Unavailable("Your home folder couldn't be found.".into()))?;
        if !db.join("cert9.db").is_file() {
            std::fs::create_dir_all(&db).map_err(|e| {
                TrustError::Failed(format!("Couldn't create {}: {e}", db.display()))
            })?;
            self.confirm(
                CommandSpec::new("certutil")
                    .arg("-d")
                    .arg(sql_db(&db))
                    .args(["-N", "--empty-password"]),
                "create the NSS database",
            )
            .await?;
        }
        self.confirm(
            CommandSpec::new("certutil")
                .arg("-d")
                .arg(sql_db(&db))
                .args(["-A", "-t", "C,,", "-n", &nss_nickname(&ctx.ca_der), "-i"])
                .arg(&ctx.ca_path),
            "add the Akev certificate to the NSS database",
        )
        .await
    }

    async fn nss_remove(&self, ctx: &InterceptContext) -> Result<(), TrustError> {
        let db = self
            .nss_db()
            .ok_or_else(|| TrustError::Unavailable("Your home folder couldn't be found.".into()))?;
        self.confirm(
            CommandSpec::new("certutil")
                .arg("-d")
                .arg(sql_db(&db))
                .args(["-D", "-n", &nss_nickname(&ctx.ca_der)]),
            "remove the Akev certificate from the NSS database",
        )
        .await
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxTool {
    /// Fedora, RHEL, Arch, openSUSE: `trust anchor`.
    P11Kit,
    /// Debian, Ubuntu: `update-ca-certificates`.
    Debian,
}

fn sql_db(db: &Path) -> String {
    format!("sql:{}", db.display())
}

fn unknown_store(store: &str) -> TrustError {
    TrustError::Unavailable(format!(
        "There's no certificate store called {store:?} on this computer."
    ))
}

fn failure(what: &str, out: &CommandOutput) -> String {
    let detail = out
        .stderr
        .lines()
        .chain(out.stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("it was cancelled or refused");
    // pkexec exits 126 when the user dismisses the dialog, 127 when not authorised.
    if matches!(out.code, Some(126 | 127)) {
        return format!("Couldn't {what}: the password prompt was cancelled.");
    }
    format!("Couldn't {what}: {detail}")
}

/// Upper-case hex SHA-1 of the certificate: its Windows thumbprint.
/// Nickname of this CA in the user's NSS database (`Akev CA <first 8 hex digits of its SHA-1>`),
/// so a regenerated CA doesn't collide with the old one.
pub(crate) fn nss_nickname(ca_der: &[u8]) -> String {
    let sha = sha1_hex(ca_der);
    format!("Akev CA {}", &sha[..8.min(sha.len())])
}

fn sha1_hex(der: &[u8]) -> String {
    hex::encode_upper(Sha1::digest(der))
}

/// The base64 body of the first PEM block, without whitespace.
fn pem_body(pem: &str) -> String {
    pem_bodies(pem).next().unwrap_or_default()
}

/// The base64 bodies of every `CERTIFICATE` block, without whitespace. Equal DER encodes to
/// equal base64, so bodies compare like certificates.
fn pem_bodies(text: &str) -> impl Iterator<Item = String> + '_ {
    const BEGIN: &str = "-----BEGIN CERTIFICATE-----";
    const END: &str = "-----END CERTIFICATE-----";
    text.split(BEGIN).skip(1).filter_map(|rest| {
        let body = rest.split(END).next()?;
        Some(body.chars().filter(|c| !c.is_whitespace()).collect())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host::HostEnv;
    use crate::host::testutil::{fake_env, make_exe};
    use crate::runner::mock::MockRunner;

    const PEM: &str = "-----BEGIN CERTIFICATE-----\nQUJD\nREVG\n-----END CERTIFICATE-----\n";

    fn ctx(dir: &Path) -> InterceptContext {
        let mut c = crate::util::testctx::ctx(dir);
        c.ca_pem = PEM.into();
        c.ca_der = b"ABCDEF".to_vec();
        c.ca_path = dir.join("ca.pem");
        c
    }

    fn trust(env: HostEnv) -> (CaTrust, Arc<MockRunner>) {
        let runner = MockRunner::new();
        (CaTrust::with_sys(Sys::new(runner.clone(), env)), runner)
    }

    #[test]
    fn pem_bodies_compare_like_certificates() {
        let bundle =
            format!("junk\n{PEM}\n-----BEGIN CERTIFICATE-----\nWFla\n-----END CERTIFICATE-----\n");
        let bodies: Vec<_> = pem_bodies(&bundle).collect();
        assert_eq!(bodies, ["QUJDREVG", "WFla"]);
        assert_eq!(pem_body(PEM), "QUJDREVG");
        assert_eq!(sha1_hex(b"abc"), "A9993E364706816ABA3E25717850C26C9CD0D89D");
    }

    #[tokio::test]
    async fn linux_detects_the_ca_in_the_system_bundle() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, root.path());
        make_exe(&root.path().join("usr/bin/trust"), "");
        make_exe(&root.path().join("usr/bin/pkexec"), "");
        let bundle = root
            .path()
            .join("etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem");
        std::fs::create_dir_all(bundle.parent().unwrap()).unwrap();
        std::fs::write(
            &bundle,
            "-----BEGIN CERTIFICATE-----\nWFla\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let (t, _) = trust(env);
        let c = ctx(root.path());
        let report = t.report(&c).await;
        assert_eq!(report.trusted, Some(false));
        assert_eq!(report.stores[0].id, "system");
        assert!(report.stores[0].can_change);
        std::fs::write(&bundle, PEM).unwrap();
        assert_eq!(t.report(&c).await.trusted, Some(true));
    }

    #[tokio::test]
    async fn linux_installs_through_pkexec() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, root.path());
        make_exe(&root.path().join("usr/bin/trust"), "");
        make_exe(&root.path().join("usr/bin/pkexec"), "");
        let (t, runner) = trust(env);
        runner.on_prefix("pkexec", CommandOutput::ok(""));
        let c = ctx(root.path());
        t.install(&c, "system").await.unwrap();
        t.remove(&c, "system").await.unwrap();
        let lines = runner.call_lines();
        assert!(
            lines[0].starts_with("pkexec trust anchor --store "),
            "{lines:?}"
        );
        assert!(
            lines[1].starts_with("pkexec trust anchor --remove "),
            "{lines:?}"
        );
        // A dismissed password prompt reads as such.
        runner.on_prefix(
            "pkexec",
            CommandOutput {
                code: Some(126),
                ..Default::default()
            },
        );
        let err = t.install(&c, "system").await.unwrap_err().to_string();
        assert!(err.contains("cancelled"), "{err}");
    }

    #[tokio::test]
    async fn linux_debian_uses_update_ca_certificates() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, root.path());
        make_exe(&root.path().join("usr/bin/update-ca-certificates"), "");
        make_exe(&root.path().join("usr/bin/pkexec"), "");
        let (t, runner) = trust(env);
        runner.on_prefix("pkexec", CommandOutput::ok(""));
        t.install(&ctx(root.path()), "system").await.unwrap();
        let line = &runner.call_lines()[0];
        assert!(line.contains("update-ca-certificates"), "{line}");
        assert!(line.ends_with("ca.pem"), "{line}");
    }

    #[tokio::test]
    async fn linux_without_pkexec_explains() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, root.path());
        make_exe(&root.path().join("usr/bin/trust"), "");
        let (t, _) = trust(env);
        let report = t.report(&ctx(root.path())).await;
        assert!(!report.stores[0].can_change);
        assert!(
            report.stores[0]
                .unavailable_reason
                .as_deref()
                .unwrap()
                .contains("pkexec")
        );
        assert!(t.install(&ctx(root.path()), "system").await.is_err());
    }

    #[tokio::test]
    async fn linux_nss_database() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Linux, root.path());
        make_exe(&root.path().join("usr/bin/certutil"), "");
        let (t, runner) = trust(env);
        runner.on_prefix("certutil", CommandOutput::ok(""));
        let c = ctx(root.path());
        let report = t.report(&c).await;
        let nss = report.stores.iter().find(|s| s.id == "nss").unwrap();
        assert_eq!(nss.trusted, Some(false), "no database yet");
        assert!(nss.can_change);
        t.install(&c, "nss").await.unwrap();
        let lines = runner.call_lines();
        assert!(
            lines.iter().any(|l| l.contains(" -N --empty-password")),
            "{lines:?}"
        );
        assert!(
            lines.iter().any(|l| l.contains(" -A -t C,, -n Akev CA ")),
            "{lines:?}"
        );
    }

    #[tokio::test]
    async fn macos_login_keychain() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::MacOs, root.path());
        let (t, runner) = trust(env);
        runner.on_prefix("security verify-cert", CommandOutput::failed("not trusted"));
        runner.on_prefix("security add-trusted-cert", CommandOutput::ok(""));
        runner.on_prefix("security remove-trusted-cert", CommandOutput::ok(""));
        runner.on_prefix("security delete-certificate", CommandOutput::ok(""));
        let c = ctx(root.path());
        let report = t.report(&c).await;
        assert_eq!(report.trusted, Some(false));
        t.install(&c, "user").await.unwrap();
        t.remove(&c, "user").await.unwrap();
        let lines = runner.call_lines();
        assert!(
            lines.iter().any(
                |l| l.starts_with("security add-trusted-cert -r trustRoot -k ")
                    && l.contains("login.keychain-db")
            ),
            "{lines:?}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.contains(&format!("delete-certificate -Z {}", sha1_hex(b"ABCDEF")))),
            "{lines:?}"
        );
        runner.on_prefix("security verify-cert", CommandOutput::ok("ok"));
        assert_eq!(t.report(&c).await.trusted, Some(true));
    }

    #[tokio::test]
    async fn windows_current_user_root_store() {
        let root = tempfile::tempdir().unwrap();
        let env = fake_env(Os::Windows, root.path());
        let (t, runner) = trust(env);
        let thumb = sha1_hex(b"ABCDEF");
        runner.on_prefix(
            "certutil -user -store Root",
            CommandOutput::failed("not found"),
        );
        runner.on_prefix("certutil -user -addstore Root", CommandOutput::ok(""));
        runner.on_prefix("certutil -user -delstore Root", CommandOutput::ok(""));
        let c = ctx(root.path());
        assert_eq!(t.report(&c).await.trusted, Some(false));
        t.install(&c, "user").await.unwrap();
        t.remove(&c, "user").await.unwrap();
        let lines = runner.call_lines();
        assert!(
            lines
                .iter()
                .any(|l| l == &format!("certutil -user -delstore Root {thumb}")),
            "{lines:?}"
        );
        assert!(t.install(&c, "system").await.is_err(), "unknown store");
    }
}
