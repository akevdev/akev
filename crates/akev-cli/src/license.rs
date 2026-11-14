//! `akev license`: show, set or remove the Akev Pro licence key, or start the trial.
//!
//! Talks to the running daemon when there is one (changes apply right away); otherwise
//! reads and writes `license.json` in the data directory directly.

use crate::client::Daemon;
use crate::style::Paint;
use crate::{GlobalArgs, data_dir};
use anyhow::{Context, anyhow};
use clap::Subcommand;
use reqwest::Method;
use serde_json::{Value, json};
use std::io::Read as _;
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum LicenseCommand {
    /// Show the plan, the licensee and what the license covers.
    Status {
        /// Print the raw license status JSON.
        #[arg(long)]
        json: bool,
    },
    /// Verify and store a license key (reads it from stdin when omitted or `-`).
    Set {
        /// The key from the purchase email (`AKEV1.…`).
        key: Option<String>,
    },
    /// Remove the stored license key (nothing else changes).
    Remove,
    /// Start the free 14-day Pro trial (once).
    Trial,
}

pub async fn run(command: LicenseCommand, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let daemon = Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?;
    let (status, json_out) = match command {
        LicenseCommand::Status { json } => {
            let status = match &daemon {
                Some(d) => d.get_json("/api/license").await?,
                None => offline(&dir, |_| Ok(()))?,
            };
            (status, json)
        }
        LicenseCommand::Set { key } => {
            let key = match key.as_deref() {
                None | Some("-") => read_stdin_key()?,
                Some(k) => k.to_string(),
            };
            let status = match &daemon {
                Some(d) => send(d, Method::PUT, json!({ "key": key })).await?,
                None => offline(&dir, |m| m.set_key(&key).map(drop))?,
            };
            (status, false)
        }
        LicenseCommand::Remove => {
            let status = match &daemon {
                Some(d) => send(d, Method::DELETE, Value::Null).await?,
                None => offline(&dir, |m| m.remove_key().map(drop))?,
            };
            (status, false)
        }
        LicenseCommand::Trial => {
            let status = match &daemon {
                Some(d) => d
                    .send_json(Method::POST, "/api/license/trial", json!({}))
                    .await?
                    .unwrap_or_default(),
                None => offline(&dir, |m| m.start_trial().map(drop))?,
            };
            (status, false)
        }
    };
    if json_out {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        print_status(&status);
    }
    Ok(ExitCode::SUCCESS)
}

async fn send(daemon: &Daemon, method: Method, body: Value) -> anyhow::Result<Value> {
    let resp = if body.is_null() {
        daemon.request(method, "/api/license", None).await?
    } else {
        daemon
            .request(
                method,
                "/api/license",
                Some(crate::client::RequestBody::Json(body)),
            )
            .await?
    };
    resp.json().await.context("invalid JSON from the API")
}

/// Run `edit` on the data directory's licence state (no daemon running) and return the
/// resulting status.
fn offline(
    dir: &std::path::Path,
    edit: impl FnOnce(&akev_api::LicenseManager) -> Result<(), akev_api::ApiError>,
) -> anyhow::Result<Value> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let manager = akev_api::LicenseManager::load(
        dir,
        true,
        None,
        &akev_api::builtin_public_keys(),
        akev_api::default_trial_file(),
    );
    edit(&manager).map_err(|e| anyhow!("{}", e.message))?;
    Ok(serde_json::to_value(manager.status())?)
}

fn read_stdin_key() -> anyhow::Result<String> {
    let mut key = String::new();
    std::io::stdin()
        .read_to_string(&mut key)
        .context("failed to read the license key from stdin")?;
    if key.trim().is_empty() {
        anyhow::bail!("no license key given (pass it as an argument or on stdin)");
    }
    Ok(key)
}

fn print_status(s: &Value) {
    let p = Paint::stdout();
    let label = |x: &str| p.dim(&format!("{x:<12}"));
    let state = s["state"].as_str().unwrap_or("none");
    let days_left = s["trialEndsAt"].as_f64().map(|end| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0.0, |d| d.as_millis() as f64);
        ((end - now) / 86_400_000.0).ceil().max(0.0) as u64
    });
    let headline = match state {
        "active" => format!("{} Akev Pro", p.green("●")),
        "trial" => format!(
            "{} Akev Pro trial — {} day(s) left",
            p.green("●"),
            days_left.unwrap_or(0)
        ),
        "trial-ended" => format!("{} Akev Free — the Pro trial has ended", p.dim("○")),
        "updates-ended" => format!(
            "{} Akev Free — your license doesn't cover this version",
            p.yellow("●")
        ),
        "invalid" => format!("{} Akev Free — the license key can't be used", p.red("●")),
        _ => format!("{} Akev Free", p.dim("○")),
    };
    println!("{headline}");
    if let Some(l) = s.get("license").filter(|l| l.is_object()) {
        let who = match l["name"].as_str() {
            Some(name) => format!("{name} <{}>", l["email"].as_str().unwrap_or("?")),
            None => l["email"].as_str().unwrap_or("?").to_string(),
        };
        println!("  {} {who}", label("Licensed to"));
        println!("  {} {}", label("License"), l["id"].as_str().unwrap_or("?"));
        println!(
            "  {} versions released until {}",
            label("Covers"),
            p.bold(l["updatesUntil"].as_str().unwrap_or("?"))
        );
    }
    println!(
        "  {} {}",
        label("This build"),
        s["releaseDate"].as_str().unwrap_or("?")
    );
    if let Some(err) = s["error"].as_str() {
        println!("  {} {err}", label("Problem"));
    }
    match state {
        "updates-ended" => println!(
            "\nRenew a year of updates at https://akev.dev/account/ (sign in with your checkout email),\nor use a version released before the date above."
        ),
        "active" | "trial" => {}
        _ => {
            if s["trialAvailable"].as_bool() == Some(true) {
                println!("\nTry every Pro feature free for 14 days: `akev license trial`.");
            }
            println!("Get Akev Pro ($49 once, no subscription): https://akev.dev/pricing/");
        }
    }
}
