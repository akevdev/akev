//! Commands that control a running daemon: `status`, `stop`, `intercept`, `export`, `rules`.

use crate::client::{Daemon, RequestBody};
use crate::style::{self, Paint};
use crate::{GlobalArgs, data_dir};
use anyhow::{Context, bail};
use clap::{Args, Subcommand};
use reqwest::Method;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

/// Connect or explain how to start Akev.
async fn require_daemon(global: &GlobalArgs) -> anyhow::Result<Daemon> {
    let dir = data_dir(global)?;
    match Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await? {
        Some(d) => Ok(d),
        None => bail!("Akev is not running (start it with `akev start`)"),
    }
}

/// Percent-encode a query/path component.
fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ---------- status / stop ----------

pub async fn status(global: &GlobalArgs, json_out: bool) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let Some(daemon) =
        Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?
    else {
        if json_out {
            println!("{}", json!({ "running": false }));
        } else {
            println!("Akev is not running.");
        }
        return Ok(ExitCode::from(1));
    };
    let s = daemon.get_json("/api/status").await?;
    if json_out {
        let mut v = s.clone();
        v["running"] = Value::Bool(true);
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(ExitCode::SUCCESS);
    }
    let p = Paint::stdout();
    let label = |x: &str| p.dim(&format!("{x:<10}"));
    let addresses: Vec<String> = s["proxy"]["addresses"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    println!(
        "{} Akev {} is running{}",
        p.green("●"),
        s["version"].as_str().unwrap_or("?"),
        daemon
            .pid
            .map(|pid| format!(" (pid {pid})"))
            .unwrap_or_default()
    );
    if let Some((first, rest)) = addresses.split_first() {
        let mut line = format!(
            "  {} {}",
            label("Proxy"),
            p.bold(&format!("http://{first}"))
        );
        if !rest.is_empty() {
            line.push_str(&p.dim(&format!("  (LAN: {})", rest.join(", "))));
        }
        println!("{line}");
    }
    println!("  {} {}", label("Web UI"), p.cyan(&daemon.ui_url()));
    if let Some(network) = s["networkUrl"].as_str() {
        println!(
            "  {} {}  {}",
            label("Network"),
            p.cyan(&akev_api::ui_url_for(network, &daemon.token)),
            p.dim("(HTTPS for other devices: they must trust the Akev CA)")
        );
    }
    let ca = &s["ca"];
    println!(
        "  {} {}  {}",
        label("CA"),
        dir_of(&s).join("ca.pem").display(),
        p.dim(&format!(
            "(expires {}, {} days)",
            ca["notAfter"].as_str().unwrap_or("?"),
            ca["daysRemaining"].as_i64().unwrap_or(0)
        ))
    );
    let st = &s["stats"];
    println!(
        "  {} {} exchanges · {} bodies in memory · {} spilled · {} active connection(s)",
        label("Traffic"),
        st["exchanges"].as_u64().unwrap_or(0),
        style::bytes(st["memoryBodyBytes"].as_u64().unwrap_or(0)),
        style::bytes(st["spilledBodyBytes"].as_u64().unwrap_or(0)),
        st["activeConnections"].as_u64().unwrap_or(0)
    );
    Ok(ExitCode::SUCCESS)
}

fn dir_of(status: &Value) -> PathBuf {
    PathBuf::from(status["dataDir"].as_str().unwrap_or("."))
}

pub async fn stop(global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let Some(daemon) =
        Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?
    else {
        println!("Akev is not running.");
        return Ok(ExitCode::SUCCESS);
    };
    daemon
        .send_json(Method::POST, "/api/shutdown", Value::Null)
        .await?;
    // Wait for the daemon to go away (it removes daemon.json when done).
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let probe = crate::client::http_client(Some(Duration::from_millis(500)))?
            .get(format!("{}/api/status", daemon.url))
            .bearer_auth(&daemon.token)
            .send()
            .await;
        if probe.is_err() {
            println!("Akev stopped.");
            return Ok(ExitCode::SUCCESS);
        }
    }
    bail!("Akev did not stop within 10 seconds")
}

// ---------- intercept ----------

#[derive(Args, Debug)]
pub struct InterceptArgs {
    /// `list`, `stop`, or an interceptor id to activate (e.g. `chrome`, `fresh-terminal`).
    pub action: String,
    /// Interceptor id (for `stop`).
    pub id: Option<String>,
    /// Interceptor options as JSON, e.g. '{"url":"https://example.com"}'.
    #[arg(long, value_name = "JSON")]
    pub options: Option<String>,
}

fn parse_options(raw: Option<&str>) -> anyhow::Result<Value> {
    match raw {
        None => Ok(Value::Null),
        Some(s) => {
            serde_json::from_str(s).with_context(|| format!("--options is not valid JSON: {s}"))
        }
    }
}

pub async fn intercept(args: InterceptArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let daemon = require_daemon(global).await?;
    let p = Paint::stdout();
    match args.action.as_str() {
        "list" | "ls" => {
            let list = daemon.get_json("/api/interceptors").await?;
            let items = list.as_array().cloned().unwrap_or_default();
            let width = items
                .iter()
                .filter_map(|i| i["id"].as_str())
                .map(str::len)
                .max()
                .unwrap_or(10);
            for i in items {
                let id = i["id"].as_str().unwrap_or("?");
                let state = if i["active"].as_bool() == Some(true) {
                    p.green("active")
                } else if i["available"].as_bool() == Some(true) {
                    p.cyan("available")
                } else {
                    p.dim("unavailable")
                };
                let mut line = format!(
                    "  {:<width$}  {:<11}  {}",
                    p.bold(id),
                    state,
                    i["name"].as_str().unwrap_or("")
                );
                if let Some(reason) = i["unavailableReason"].as_str() {
                    line.push_str(&p.dim(&format!(" — {reason}")));
                }
                println!("{line}");
            }
        }
        "stop" | "deactivate" => {
            let Some(id) = args.id.as_deref() else {
                bail!("usage: akev intercept stop <id>")
            };
            let body = json!({ "options": parse_options(args.options.as_deref())? });
            daemon
                .send_json(
                    Method::POST,
                    &format!("/api/interceptors/{}/deactivate", url_encode(id)),
                    body,
                )
                .await?;
            println!("{} deactivated {id}", p.green("✓"));
        }
        id => {
            let body = json!({ "options": parse_options(args.options.as_deref())? });
            let result = daemon
                .send_json(
                    Method::POST,
                    &format!("/api/interceptors/{}/activate", url_encode(id)),
                    body,
                )
                .await?
                .unwrap_or(Value::Null);
            if result["success"].as_bool() == Some(true) {
                println!("{} activated {id}", p.green("✓"));
                if let Some(m) = result["message"].as_str() {
                    println!("  {m}");
                }
                if !result["metadata"].is_null() {
                    println!("{}", serde_json::to_string_pretty(&result["metadata"])?);
                }
            } else {
                bail!(
                    "could not activate {id}: {}",
                    result["message"].as_str().unwrap_or("unknown error")
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

// ---------- export ----------

#[derive(Args, Debug)]
pub struct ExportArgs {
    /// Write a HAR 1.2 file.
    #[arg(long, value_name = "FILE", required_unless_present = "session")]
    pub har: Option<PathBuf>,
    /// Write a native `.akev` session file.
    #[arg(long, value_name = "FILE", conflicts_with = "har")]
    pub session: Option<PathBuf>,
    /// Only exchanges matching this filter.
    #[arg(long, value_name = "QUERY")]
    pub filter: Option<String>,
    /// Replace secrets (credential headers and cookies, secret query parameters, tokens in
    /// text bodies) with `[redacted]`, e.g. before sharing the file.
    #[arg(long)]
    pub redact: bool,
}

pub async fn export(args: ExportArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let daemon = require_daemon(global).await?;
    let mut params = Vec::new();
    if let Some(f) = args.filter.as_deref() {
        params.push(format!("filter={}", url_encode(f)));
    }
    if args.redact {
        params.push("redact=1".to_string());
    }
    let query = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };
    let (path, file) = match (&args.har, &args.session) {
        (Some(f), _) => (format!("/api/export/har{query}"), f),
        (None, Some(f)) => (format!("/api/export/session{query}"), f),
        (None, None) => bail!("pass --har FILE or --session FILE"),
    };
    let bytes = daemon.get_bytes(&path).await?;
    std::fs::write(file, &bytes).with_context(|| format!("failed to write {}", file.display()))?;
    let count = if args.har.is_some() {
        serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|v| v["log"]["entries"].as_array().map(Vec::len))
    } else {
        None
    };
    let p = Paint::stdout();
    match count {
        Some(n) => println!(
            "{} wrote {n} exchange(s) to {}",
            p.green("✓"),
            file.display()
        ),
        None => println!(
            "{} wrote {} to {}",
            p.green("✓"),
            style::bytes(bytes.len() as u64),
            file.display()
        ),
    }
    Ok(ExitCode::SUCCESS)
}

// ---------- rules ----------

#[derive(Subcommand, Debug)]
pub enum RulesCommand {
    /// Print the rule set as JSON (or write it to a file).
    Get {
        #[arg(short, long, value_name = "FILE")]
        output: Option<PathBuf>,
    },
    /// Replace the rule set from a file (Akev rules JSON or an HTTP Toolkit rules export).
    Set { file: PathBuf },
}

pub async fn rules(command: RulesCommand, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let daemon = require_daemon(global).await?;
    match command {
        RulesCommand::Get { output } => {
            let rules = daemon.get_json("/api/rules").await?;
            let text = serde_json::to_string_pretty(&rules)? + "\n";
            match output {
                Some(file) => {
                    std::fs::write(&file, text)
                        .with_context(|| format!("failed to write {}", file.display()))?;
                    eprintln!("Wrote rules to {}", file.display());
                }
                None => print!("{text}"),
            }
        }
        RulesCommand::Set { file } => {
            let bytes = std::fs::read(&file)
                .with_context(|| format!("failed to read {}", file.display()))?;
            serde_json::from_slice::<Value>(&bytes)
                .with_context(|| format!("{} is not valid JSON", file.display()))?;
            // Normalize/convert (accepts HTTP Toolkit exports) then apply.
            let converted = daemon
                .request(
                    Method::POST,
                    "/api/rules/import",
                    Some(RequestBody::Bytes(bytes, "application/json")),
                )
                .await?
                .json::<Value>()
                .await
                .context("invalid response from /api/rules/import")?;
            let result = daemon
                .send_json(Method::PUT, "/api/rules", converted.clone())
                .await?
                .unwrap_or(Value::Null);
            let count = count_rules(&converted);
            let errors = result["errors"].as_array().cloned().unwrap_or_default();
            let p = Paint::stdout();
            println!(
                "{} applied {count} rule(s) from {}",
                p.green("✓"),
                file.display()
            );
            for e in &errors {
                println!(
                    "  {} rule {}: {}",
                    p.yellow("!"),
                    e[0].as_str().unwrap_or("?"),
                    e[1].as_str().unwrap_or("")
                );
            }
            if !errors.is_empty() {
                return Ok(ExitCode::from(2));
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn count_rules(rules: &Value) -> usize {
    fn walk(items: &[Value]) -> usize {
        items
            .iter()
            .map(|i| match i["type"].as_str() {
                Some("group") => walk(i["items"].as_array().map(Vec::as_slice).unwrap_or(&[])),
                _ => 1,
            })
            .sum()
    }
    walk(rules["items"].as_array().map(Vec::as_slice).unwrap_or(&[]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding() {
        assert_eq!(
            url_encode("status:>=400 host:a.b"),
            "status%3A%3E%3D400%20host%3Aa.b"
        );
        assert_eq!(url_encode("fresh-chrome"), "fresh-chrome");
    }

    #[test]
    fn rule_counting() {
        let rules = json!({"version": 1, "items": [
            {"type": "rule", "id": "a"},
            {"type": "group", "id": "g", "items": [{"type": "rule", "id": "b"}, {"type": "rule", "id": "c"}]}
        ]});
        assert_eq!(count_rules(&rules), 3);
        assert_eq!(count_rules(&json!({})), 0);
    }

    #[test]
    fn options_parsing() {
        assert_eq!(parse_options(None).unwrap(), Value::Null);
        assert_eq!(parse_options(Some(r#"{"a":1}"#)).unwrap(), json!({"a": 1}));
        assert!(parse_options(Some("{")).is_err());
    }
}
