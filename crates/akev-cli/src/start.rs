//! `akev` / `akev start`: run the daemon, print the banner, open the UI.

use crate::client::Daemon;
use crate::style::Paint;
use crate::{GlobalArgs, StartArgs, data_dir, shutdown_gracefully, shutdown_signal};
use akev_api::{AkevHandle, AkevOptions};
use akev_core::license::{LicenseState, LicenseStatus};
use std::path::Path;
use std::process::ExitCode;

pub async fn run(args: StartArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let explicit_dir = global.data_dir.clone();
    // A second `akev` just opens the UI of the running instance.
    if !args.ephemeral {
        let dir = data_dir(global)?;
        if let Some(daemon) = Daemon::connect(&dir, None, None).await.unwrap_or(None) {
            let p = Paint::stdout();
            println!(
                "{} Akev is already running{} — {}",
                p.green("●"),
                daemon
                    .pid
                    .map(|pid| format!(" (pid {pid})"))
                    .unwrap_or_default(),
                p.cyan(&daemon.url)
            );
            if !args.no_open {
                open_ui(&dir, &daemon.ui_url());
                println!(
                    "  {}",
                    p.dim("opening the UI in your browser — if it can't open the launcher page, `akev status` shows the link")
                );
            }
            return Ok(ExitCode::SUCCESS);
        }
    }

    let defaults = AkevOptions::default();
    let options = AkevOptions {
        data_dir: explicit_dir,
        proxy_port: args.proxy_port,
        proxy_bind: args.proxy_bind.clone(),
        api_port: args.api_port.unwrap_or(defaults.api_port),
        api_bind: args.api_bind.clone().unwrap_or(defaults.api_bind),
        token: global.token.clone(),
        ephemeral: args.ephemeral,
        rules_file: args.rules.clone(),
        allowed_origins: args.allow_origin.clone(),
        mcp_allow_launch: args.mcp_allow_launch,
        mcp_reveal_secrets: args.mcp_reveal_secrets,
        mcp_allow_paths: args.mcp_allow_path.clone(),
        license: global.license.clone(),
        trial_file: akev_api::default_trial_file(),
        ..defaults
    };
    let handle = akev_api::start(options).await?;
    print_banner(&handle, args.ephemeral, !args.no_open);
    if !args.no_open {
        open_ui(handle.data_dir(), &handle.ui_url());
    }
    tokio::select! {
        _ = shutdown_signal() => {}
        _ = handle.wait() => {}
    }
    eprintln_safe!("Shutting down…");
    shutdown_gracefully(&handle).await;
    Ok(ExitCode::SUCCESS)
}

/// Open the web UI in the default browser without putting the API token on a command line
/// (other local users can read process arguments, e.g. `/proc/<pid>/cmdline`): the browser
/// is handed a private (0600) launcher page in the data directory that redirects to the
/// login URL.
fn open_ui(data_dir: &Path, ui_url: &str) {
    match browser_target(data_dir, ui_url) {
        Ok(target) => {
            if let Err(e) = open::that_detached(&target) {
                tracing::warn!(
                    "could not open a browser ({e}); open the Web UI link (`akev status` shows it) manually"
                );
            }
        }
        Err(e) => tracing::warn!(
            "could not write the browser launcher page ({e}); open the Web UI link (`akev status` shows it) manually"
        ),
    }
}

/// What to hand to the browser launcher for `ui_url`: the `file://` URL of the launcher page.
fn browser_target(data_dir: &Path, ui_url: &str) -> std::io::Result<String> {
    let page = akev_api::write_ui_launcher(data_dir, ui_url)?;
    Ok(akev_api::ui_launcher_url(&page))
}

fn print_banner(handle: &AkevHandle, ephemeral: bool, opening: bool) {
    let p = Paint::stdout();
    let status = handle.status();
    let proxy = status
        .proxy
        .addresses
        .first()
        .cloned()
        .unwrap_or_else(|| handle.proxy_addr().to_string());
    let others: Vec<&String> = status.proxy.addresses.iter().skip(1).collect();
    let label = |s: &str| p.dim(&format!("{s:<10}"));
    println!();
    println!(
        "  {} {}  {}",
        p.accent("▲ Akev"),
        p.bold(akev_core::VERSION),
        p.dim("HTTP(S) interceptor & debugger")
    );
    println!();
    let mut proxy_line = format!(
        "  {} {}",
        label("Proxy"),
        p.bold(&format!("http://{proxy}"))
    );
    if !others.is_empty() {
        let list: Vec<&str> = others.iter().map(|s| s.as_str()).collect();
        proxy_line.push_str(&p.dim(&format!("  (LAN: {})", list.join(", "))));
    }
    println!("{proxy_line}");
    println!("  {} {}", label("Web UI"), p.cyan(&handle.ui_url()));
    if let Some(network) = handle.network_url() {
        println!(
            "  {} {}",
            label("Network"),
            p.cyan(&akev_api::ui_url_for(&network, handle.token()))
        );
        println!(
            "  {} {}",
            label(""),
            p.dim("other devices use HTTPS: the certificate is issued by the Akev CA, so trust it on the device first")
        );
    }
    if opening {
        println!(
            "  {} {}",
            label(""),
            p.dim("opening in your browser — if it can't open the launcher page (e.g. a snap/flatpak browser), use this link")
        );
    }
    println!("  {} {}", label("CA cert"), handle.ca_path().display());
    let data = if ephemeral {
        format!("{} (ephemeral)", handle.data_dir().display())
    } else {
        handle.data_dir().display().to_string()
    };
    println!("  {} {}", label("Data"), p.dim(&data));
    if let Some(licensee) = pro_licensee(&status.license) {
        println!(
            "  {} {} {}",
            label("License"),
            p.accent("Akev Pro"),
            p.dim(&format!("· {licensee}"))
        );
    }
    if let Some(warning) = status.ca.expiry_warning("with `akev ca regenerate`") {
        println!("  {} {}", label("Warning"), p.yellow(&warning));
    }
    println!();
    println!("  {}", p.bold("Quick tips"));
    println!(
        "    {} Route a terminal: {}",
        p.dim("•"),
        p.green(&format!(
            "export HTTP_PROXY=http://{proxy} HTTPS_PROXY=http://{proxy} SSL_CERT_FILE={}",
            handle.ca_path().display()
        ))
    );
    println!(
        "    {} Or launch a pre-configured browser/terminal from the UI's Connect page",
        p.dim("•")
    );
    println!(
        "    {} Phones: browse to {} through the proxy for setup",
        p.dim("•"),
        p.green("http://akev.cert")
    );
    println!(
        "    {} AI agents: {}",
        p.dim("•"),
        p.green("claude mcp add akev -- akev mcp")
    );
    println!("    {} Press {} to stop", p.dim("•"), p.bold("Ctrl+C"));
    println!();
}

/// The licensee to name in the banner: only while a licence unlocks this build. The name alone
/// (the email only without one): the banner ends up in pasted logs and bug reports.
fn pro_licensee(status: &LicenseStatus) -> Option<&str> {
    match (&status.state, &status.license) {
        (LicenseState::Active, Some(license)) => Some(license.licensee()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::license::{LicenseInfo, Plan};

    #[test]
    fn the_banner_names_the_licensee_only_while_pro_is_active() {
        let license = LicenseInfo {
            id: "AKV-1".into(),
            email: "ada@example.com".into(),
            name: Some("Ada Lovelace".into()),
            issued: "2026-11-05".into(),
            updates_until: "2027-11-05".into(),
            seats: None,
        };
        let mut status = LicenseStatus {
            plan: Plan::Pro,
            state: LicenseState::Active,
            features: vec![],
            license: Some(license),
            trial_ends_at: None,
            trial_available: false,
            error: None,
            release_date: "2026-11-11".into(),
        };
        assert_eq!(pro_licensee(&status), Some("Ada Lovelace"));
        status.state = LicenseState::UpdatesEnded;
        assert_eq!(pro_licensee(&status), None);
        status.state = LicenseState::Trial;
        status.license = None;
        assert_eq!(pro_licensee(&status), None);
    }

    #[test]
    fn the_browser_never_gets_the_token_on_its_command_line() {
        let dir = tempfile::tempdir().unwrap();
        let token = "0123456789abcdef0123456789abcdef";
        let ui_url = akev_api::ui_url_for("http://127.0.0.1:45456", token);
        let target = browser_target(dir.path(), &ui_url).unwrap();
        assert!(target.starts_with("file://"), "{target}");
        assert!(!target.contains(token), "{target}");
        let page = akev_api::ui_launcher_path(dir.path());
        let html = std::fs::read_to_string(&page).unwrap();
        assert!(html.contains(&ui_url));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&page).unwrap().permissions().mode();
            assert_eq!(mode & 0o077, 0, "only the user may read the launcher page");
        }
    }
}
