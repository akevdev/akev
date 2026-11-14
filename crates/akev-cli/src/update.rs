//! `akev update`: is a newer version out? Asks the running daemon when there is one (sharing
//! its cached result), otherwise checks the release manifest directly. Installing is up to the
//! user's package manager or the desktop app's updater.

use std::process::ExitCode;

use crate::client::Daemon;
use crate::style::Paint;
use crate::{GlobalArgs, data_dir};
use anyhow::Context;
use reqwest::Method;
use serde_json::{Value, json};

pub async fn run(json_out: bool, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let daemon = Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?;
    let info: Value = match &daemon {
        Some(d) => d
            .send_json(Method::POST, "/api/update/check", json!({}))
            .await?
            .unwrap_or_default(),
        None => serde_json::to_value(
            akev_api::updates::UpdateChecker::new(akev_api::updates::MANIFEST_URL)
                .check()
                .await,
        )
        .context("update check result")?,
    };
    if json_out {
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(ExitCode::SUCCESS);
    }
    let p = Paint::stdout();
    let current = info["current"].as_str().unwrap_or(akev_core::VERSION);
    if let Some(error) = info["error"].as_str() {
        eprintln!("{} couldn't check for updates: {error}", p.red("✗"));
        return Ok(ExitCode::FAILURE);
    }
    match (info["available"].as_bool(), info["latest"].as_str()) {
        (Some(true), Some(latest)) => {
            println!(
                "{} Akev {latest} is available (you have {current}).",
                p.green("●")
            );
            if let Some(notes) = info["notes"].as_str() {
                println!();
                for line in notes.lines().take(20) {
                    println!("  {line}");
                }
            }
            println!(
                "\n  Download: {}",
                info["downloadUrl"].as_str().unwrap_or_default()
            );
            println!(
                "  {}",
                p.dim("Installed with a package manager? Update with it (brew upgrade akev, winget upgrade Akev, …).")
            );
        }
        _ => println!("{} Akev {current} is the latest version.", p.green("✓")),
    }
    Ok(ExitCode::SUCCESS)
}
