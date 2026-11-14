//! `akev ca path|show|export|regenerate` — works with or without a running daemon.

use crate::client::Daemon;
use crate::style::Paint;
use crate::{GlobalArgs, data_dir};
use akev_ca::CertificateAuthority;
use anyhow::Context;
use clap::Subcommand;
use reqwest::Method;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Subcommand, Debug)]
pub enum CaCommand {
    /// Print the path of the CA certificate (PEM).
    Path,
    /// Show certificate details (subject, fingerprints, validity).
    Show {
        #[arg(long)]
        json: bool,
    },
    /// Write the CA certificate to a file (PEM, or DER with --der).
    Export {
        /// Destination file.
        file: PathBuf,
        /// DER encoding (`.crt`/`.cer`, e.g. for Android/Windows) instead of PEM.
        #[arg(long)]
        der: bool,
    },
    /// Generate a new CA certificate (clients must trust the new one).
    Regenerate,
}

fn load_local(dir: &Path) -> anyhow::Result<CertificateAuthority> {
    ensure_dir(dir)?;
    CertificateAuthority::load_or_generate(dir).context("failed to load or generate the CA")
}

fn ensure_dir(dir: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))
}

fn print_info(info: &Value, path: &Path, json: bool) {
    if json {
        let mut v = info.clone();
        v["path"] = Value::String(path.display().to_string());
        println!("{}", serde_json::to_string_pretty(&v).unwrap_or_default());
        return;
    }
    let p = Paint::stdout();
    let field = |k: &str| {
        info[k]
            .as_str()
            .map(str::to_string)
            .unwrap_or_else(|| info[k].to_string())
    };
    let label = |s: &str| p.dim(&format!("{s:<20}"));
    println!("{} {}", label("Subject"), p.bold(&field("subject")));
    println!("{} {}", label("Path"), path.display());
    println!("{} {}", label("SHA-256"), field("sha256Fingerprint"));
    println!("{} {}", label("SPKI (base64)"), field("spkiFingerprint"));
    println!("{} {}", label("subject_hash_old"), field("subjectHashOld"));
    println!("{} {}", label("Valid from"), field("notBefore"));
    let days = info["daysRemaining"].as_i64().unwrap_or(0);
    let expiry = info["expiry"].as_str().unwrap_or("valid");
    let until = if expiry == "expired" {
        format!(
            "{} (expired: regenerate with `akev ca regenerate`)",
            field("notAfter")
        )
    } else {
        format!("{} ({days} days left)", field("notAfter"))
    };
    println!(
        "{} {}",
        label("Valid until"),
        if expiry == "valid" {
            until
        } else {
            p.yellow(&until)
        }
    );
}

pub async fn run(command: CaCommand, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let daemon = Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?;
    // A running daemon may use another data directory (e.g. `--url` or an ephemeral run).
    let ca_path = match &daemon {
        Some(d) => d
            .get_json("/api/status")
            .await
            .ok()
            .and_then(|s| {
                s["dataDir"]
                    .as_str()
                    .map(|d| PathBuf::from(d).join("ca.pem"))
            })
            .unwrap_or_else(|| dir.join("ca.pem")),
        None => dir.join("ca.pem"),
    };
    match command {
        CaCommand::Path => {
            if !ca_path.exists() && daemon.is_none() {
                load_local(&dir)?;
            }
            println!("{}", ca_path.display());
        }
        CaCommand::Show { json } => {
            let info = match &daemon {
                Some(d) => d.get_json("/api/ca").await?,
                None => {
                    let dir2 = dir.clone();
                    tokio::task::spawn_blocking(move || {
                        load_local(&dir2).map(|ca| serde_json::to_value(ca.info()))
                    })
                    .await??
                    .context("failed to serialize CA info")?
                }
            };
            print_info(&info, &ca_path, json);
        }
        CaCommand::Export { file, der } => {
            let bytes = match &daemon {
                Some(d) => d
                    .get_bytes(if der {
                        "/api/ca/cert.der"
                    } else {
                        "/api/ca/cert.pem"
                    })
                    .await?
                    .to_vec(),
                None => {
                    let ca = load_local(&dir)?;
                    if der {
                        ca.cert_der().to_vec()
                    } else {
                        ca.cert_pem().as_bytes().to_vec()
                    }
                }
            };
            std::fs::write(&file, &bytes)
                .with_context(|| format!("failed to write {}", file.display()))?;
            eprintln!(
                "Wrote the Akev CA certificate ({}) to {}",
                if der { "DER" } else { "PEM" },
                file.display()
            );
        }
        CaCommand::Regenerate => {
            let info = match &daemon {
                Some(d) => d
                    .send_json(Method::POST, "/api/ca/regenerate", Value::Null)
                    .await?
                    .unwrap_or(Value::Null),
                None => {
                    let dir2 = dir.clone();
                    tokio::task::spawn_blocking(move || regenerate_local(&dir2)).await??
                }
            };
            print_info(&info, &ca_path, false);
            let p = Paint::stderr();
            eprintln!(
                "{} A new CA was generated. Re-trust it on every device and browser that trusted the old one.",
                p.yellow("!")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Regenerate the CA files in `dir` (no daemon running). The previous pair is kept as
/// `ca.pem.bak` / `ca.key.bak` (and restored if the new one can't be written).
fn regenerate_local(dir: &Path) -> anyhow::Result<Value> {
    ensure_dir(dir)?;
    let ca = CertificateAuthority::regenerate(dir).context("failed to generate a new CA")?;
    serde_json::to_value(ca.info()).context("failed to serialize CA info")
}
