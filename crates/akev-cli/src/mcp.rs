//! `akev mcp`: MCP over stdio. Reads newline-delimited JSON-RPC from stdin and forwards each
//! message to the running daemon's `POST /mcp` (discovered via `daemon.json`, or `--url` /
//! `--token`); when no daemon runs, an embedded headless instance is started instead.
//! Responses are written to stdout, one per line; logs go to stderr. Requests are handled
//! concurrently (long-running tools such as `akev_wait_for` don't block others).

use crate::client::{Daemon, McpPostError};
use crate::{GlobalArgs, data_dir};
use akev_api::{AkevHandle, AkevOptions};
use clap::Args;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{RwLock, mpsc};

/// `akev mcp` options.
#[derive(Args, Debug, Clone, Default, PartialEq)]
pub struct McpArgs {
    /// Let the agent activate interceptors that start arbitrary programs (`electron`). Only
    /// applies to the embedded instance started when no daemon runs; a running daemon decides
    /// itself (`akev start --mcp-allow-launch`).
    #[arg(long)]
    pub allow_launch: bool,
    /// Let the agent see secrets in captured traffic when it asks (`reveal`); otherwise they
    /// are shown as [redacted]. Only applies to the embedded instance; a running daemon
    /// decides itself (`akev start --mcp-reveal-secrets`).
    #[arg(long, alias = "mcp-reveal-secrets")]
    pub reveal_secrets: bool,
    /// A folder the agent may make Akev read and write files in, besides the data
    /// directory's `exports` folder (HAR exports, files served by its rules). Repeatable. Only
    /// applies to the embedded instance; a running daemon decides itself (`akev start
    /// --mcp-allow-path`).
    #[arg(long = "allow-path", alias = "mcp-allow-path", value_name = "DIR")]
    pub allow_path: Vec<PathBuf>,
}

enum Backend {
    Remote(Remote),
    Embedded(AkevHandle),
}

/// A daemon reached over HTTP; re-discovered once when it goes away or rotates its token
/// (e.g. the desktop app was restarted while the agent session kept running).
struct Remote {
    daemon: RwLock<Daemon>,
    data_dir: PathBuf,
    url: Option<String>,
    token: Option<String>,
}

impl Remote {
    async fn forward(&self, line: &str) -> Result<Option<String>, McpPostError> {
        let daemon = self.daemon.read().await;
        daemon.post_mcp(line.to_string()).await
    }

    async fn handle(&self, line: String) -> Option<String> {
        let result = match self.forward(&line).await {
            Err(e @ (McpPostError::Unreachable(_) | McpPostError::Unauthorized)) => {
                if self.rediscover().await {
                    self.forward(&line).await
                } else {
                    Err(match e {
                        McpPostError::Unreachable(m) => McpPostError::Unreachable(format!(
                            "{m} (is Akev still running? start it with `akev start`)"
                        )),
                        other => other,
                    })
                }
            }
            other => other,
        };
        match result {
            Ok(resp) => resp,
            Err(e) => error_response(&line, &e.to_string()),
        }
    }

    async fn rediscover(&self) -> bool {
        match Daemon::connect(&self.data_dir, self.url.as_deref(), self.token.as_deref()).await {
            Ok(Some(daemon)) => {
                eprintln_safe!("[akev mcp] reconnected to {}", daemon.url);
                *self.daemon.write().await = daemon;
                true
            }
            _ => false,
        }
    }
}

impl Backend {
    async fn handle(&self, line: String) -> Option<String> {
        match self {
            Backend::Remote(remote) => remote.handle(line).await,
            Backend::Embedded(handle) => handle.mcp(line.as_bytes()).await.map(|v| v.to_string()),
        }
    }
}

/// JSON-RPC error for a request line (None for notifications or unparseable input without id).
fn error_response(line: &str, message: &str) -> Option<String> {
    let id = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("id").cloned())?;
    Some(
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": message } })
            .to_string(),
    )
}

pub async fn run(args: McpArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let backend = match Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref())
        .await?
    {
        Some(daemon) => {
            eprintln_safe!("[akev mcp] connected to {}", daemon.url);
            if args.allow_launch {
                eprintln_safe!(
                    "[akev mcp] --allow-launch has no effect on a running daemon; start it with `akev start --mcp-allow-launch`"
                );
            }
            if !args.allow_path.is_empty() {
                eprintln_safe!(
                    "[akev mcp] --allow-path has no effect on a running daemon; start it with `akev start --mcp-allow-path <dir>`"
                );
            }
            if args.reveal_secrets {
                eprintln_safe!(
                    "[akev mcp] --reveal-secrets has no effect on a running daemon; start it with `akev start --mcp-reveal-secrets`"
                );
            }
            Backend::Remote(Remote {
                daemon: RwLock::new(daemon),
                data_dir: dir.clone(),
                url: global.url.clone(),
                token: global.token.clone(),
            })
        }
        None if global.url.is_some() => {
            anyhow::bail!(
                "no Akev daemon reachable at {}",
                global.url.as_deref().unwrap_or_default()
            )
        }
        None => {
            let handle = akev_api::start(AkevOptions {
                data_dir: global.data_dir.clone(),
                token: global.token.clone(),
                mcp_allow_launch: args.allow_launch,
                license: global.license.clone(),
                trial_file: akev_api::default_trial_file(),
                mcp_reveal_secrets: args.reveal_secrets,
                mcp_allow_paths: args.allow_path.clone(),
                ..AkevOptions::default()
            })
            .await?;
            eprintln_safe!(
                "[akev mcp] started an embedded Akev: proxy http://{} · UI {}",
                handle
                    .status()
                    .proxy
                    .addresses
                    .first()
                    .cloned()
                    .unwrap_or_default(),
                handle.ui_url()
            );
            Backend::Embedded(handle)
        }
    };
    let backend = Arc::new(backend);

    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    let writer = tokio::spawn(async move {
        let mut stdout = tokio::io::stdout();
        while let Some(mut line) = rx.recv().await {
            line.push('\n');
            if stdout.write_all(line.as_bytes()).await.is_err() || stdout.flush().await.is_err() {
                break;
            }
        }
    });

    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    let mut tasks = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            line = lines.next_line() => match line {
                Ok(Some(line)) => {
                    let line = line.trim().to_string();
                    if line.is_empty() {
                        continue;
                    }
                    let backend = backend.clone();
                    let tx = tx.clone();
                    tasks.spawn(async move {
                        if let Some(resp) = backend.handle(line).await {
                            let _ = tx.send(resp);
                        }
                    });
                }
                Ok(None) => break,
                Err(e) => {
                    eprintln_safe!("[akev mcp] stdin error: {e}");
                    break;
                }
            },
            _ = crate::shutdown_signal() => break,
            Some(_) = tasks.join_next(), if !tasks.is_empty() => {}
        }
    }
    // stdin closed: let in-flight requests finish briefly.
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        while tasks.join_next().await.is_some() {}
    })
    .await;
    tasks.abort_all();
    drop(tx);
    let _ = tokio::time::timeout(Duration::from_secs(2), writer).await;
    if let Ok(Backend::Embedded(handle)) = Arc::try_unwrap(backend) {
        handle.shutdown().await;
    }
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_responses_only_for_requests() {
        let r =
            error_response(r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#, "down").unwrap();
        let v: Value = serde_json::from_str(&r).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["error"]["code"], -32603);
        assert_eq!(
            error_response(
                r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                "down"
            ),
            None
        );
        assert_eq!(error_response("garbage", "down"), None);
    }
}
