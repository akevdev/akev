//! `akev tail`: follow live traffic of a running daemon, one line per completed exchange
//! (like `tail -f`), optionally filtered server-side with the filter language.

use crate::capture::{is_done, summary_line};
use crate::client::Daemon;
use crate::style::Paint;
use crate::{GlobalArgs, data_dir, shutdown_signal};
use akev_core::ExchangeSummary;
use anyhow::{Context, bail};
use clap::Args;
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::process::ExitCode;
use tokio_tungstenite::tungstenite::Message;

#[derive(Args, Debug, Clone)]
pub struct TailArgs {
    /// Only show exchanges matching this filter (e.g. 'status:>=400 host:api.example.com').
    #[arg(long, short, value_name = "QUERY")]
    pub filter: Option<String>,

    /// Print each completed exchange summary as a JSON line instead of a table row.
    #[arg(long)]
    pub json: bool,

    /// Also print exchanges that completed before `tail` started.
    #[arg(long)]
    pub history: bool,
}

/// What `tail` has printed and which ids match the server-side filter.
#[derive(Debug, Default)]
pub struct TailState {
    /// Ids already printed (or deliberately skipped as history).
    printed: HashSet<u64>,
    /// Matching ids per the server (`None` = everything matches / no filter).
    matching: Option<HashSet<u64>>,
    /// The first `hello` was processed.
    started: bool,
}

impl TailState {
    /// Process one server message; returns the summaries to print now.
    pub fn on_message(&mut self, msg: &Value, history: bool) -> Vec<ExchangeSummary> {
        let mut out = Vec::new();
        match msg["type"].as_str() {
            Some("hello") => {
                let first = !self.started;
                self.started = true;
                for s in summaries(&msg["summaries"]) {
                    if !is_done(&s) {
                        continue;
                    }
                    if first && !history {
                        self.printed.insert(s.id.0);
                    } else {
                        self.consider(s, &mut out);
                    }
                }
            }
            Some("filter-result") if msg.get("error").is_none() => {
                self.matching = msg["ids"]
                    .as_array()
                    .map(|ids| ids.iter().filter_map(Value::as_u64).collect());
            }
            Some("exchanges") => {
                if let (Some(set), Some(delta)) = (self.matching.as_mut(), msg.get("filtered")) {
                    for id in delta["add"].as_array().into_iter().flatten() {
                        if let Some(id) = id.as_u64() {
                            set.insert(id);
                        }
                    }
                    for id in delta["remove"].as_array().into_iter().flatten() {
                        if let Some(id) = id.as_u64() {
                            set.remove(&id);
                        }
                    }
                }
                let changed = summaries(&msg["added"])
                    .into_iter()
                    .chain(summaries(&msg["updated"]));
                for s in changed.filter(is_done) {
                    self.consider(s, &mut out);
                }
            }
            Some("cleared") => {
                if let Some(set) = self.matching.as_mut() {
                    set.clear();
                }
            }
            _ => {}
        }
        out
    }

    fn consider(&mut self, s: ExchangeSummary, out: &mut Vec<ExchangeSummary>) {
        let matches = self.matching.as_ref().is_none_or(|m| m.contains(&s.id.0));
        if matches && self.printed.insert(s.id.0) {
            out.push(s);
        }
    }
}

fn summaries(v: &Value) -> Vec<ExchangeSummary> {
    v.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|s| serde_json::from_value(s.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// `ws://` URL of the daemon's live stream.
fn stream_url(api_url: &str, token: &str) -> anyhow::Result<String> {
    let mut url = url::Url::parse(api_url).context("invalid daemon URL")?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme)
        .map_err(|()| anyhow::anyhow!("cannot derive a WebSocket URL from {api_url}"))?;
    url.set_path("/api/stream");
    url.query_pairs_mut().clear().append_pair("token", token);
    Ok(url.to_string())
}

pub async fn run(args: TailArgs, global: &GlobalArgs) -> anyhow::Result<ExitCode> {
    let dir = data_dir(global)?;
    let Some(daemon) =
        Daemon::connect(&dir, global.url.as_deref(), global.token.as_deref()).await?
    else {
        bail!("Akev is not running (start it with `akev start`)");
    };
    if let Some(q) = args.filter.as_deref().filter(|q| !q.trim().is_empty()) {
        akev_filter::Filter::parse(q).map_err(|e| {
            anyhow::anyhow!("invalid --filter at {}..{}: {}", e.start, e.end, e.message)
        })?;
    }
    let url = stream_url(&daemon.url, &daemon.token)?;
    let (mut ws, _) = tokio_tungstenite::connect_async(url.as_str())
        .await
        .with_context(|| format!("failed to connect to the live stream at {}", daemon.url))?;
    if let Some(q) = args.filter.as_deref().filter(|q| !q.trim().is_empty()) {
        let msg = json!({ "type": "set-filter", "query": q, "seq": 1 }).to_string();
        ws.send(Message::Text(msg.into()))
            .await
            .context("failed to send the filter")?;
    }
    let p = Paint::stdout();
    let err = Paint::stderr();
    eprintln!(
        "{} following {}{} — Ctrl+C to stop",
        err.accent("▲ akev"),
        daemon.url,
        args.filter
            .as_deref()
            .map(|f| format!(" (filter: {f})"))
            .unwrap_or_default()
    );
    let mut state = TailState::default();
    loop {
        tokio::select! {
            _ = shutdown_signal() => break,
            msg = ws.next() => match msg {
                Some(Ok(Message::Text(text))) => {
                    let Ok(value) = serde_json::from_str::<Value>(text.as_str()) else { continue };
                    if value["type"] == "filter-result"
                        && let Some(e) = value.get("error")
                    {
                        bail!("invalid filter: {}", e["message"].as_str().unwrap_or("?"));
                    }
                    for s in state.on_message(&value, args.history) {
                        if args.json {
                            println!("{}", serde_json::to_string(&s).unwrap_or_default());
                        } else {
                            println!("{}", summary_line(&p, &s));
                        }
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    eprintln!("Akev stopped.");
                    break;
                }
                Some(Ok(_)) => {}
                Some(Err(e)) => bail!("live stream error: {e}"),
            },
        }
    }
    let _ = ws.close(None).await;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(id: u64, state: &str, status: Option<u16>) -> Value {
        let mut v = json!({
            "id": id, "kind": "http", "state": state, "startedAt": 0.0, "method": "GET",
            "scheme": "http", "host": "example.com", "port": 80, "path": format!("/{id}"),
            "httpVersion": "HTTP/1.1", "contentKind": "unknown", "requestSize": 0, "responseSize": 0
        });
        if let Some(code) = status {
            v["status"] = json!(code);
        }
        v
    }

    fn ids(v: &[ExchangeSummary]) -> Vec<u64> {
        v.iter().map(|s| s.id.0).collect()
    }

    #[test]
    fn prints_each_completed_exchange_once_skipping_history() {
        let mut t = TailState::default();
        let hello = json!({"type": "hello", "summaries": [s(1, "complete", Some(200)), s(2, "waiting", None)]});
        assert!(t.on_message(&hello, false).is_empty());
        let batch = json!({"type": "exchanges", "added": [s(3, "requesting", None)], "updated": [s(2, "complete", Some(201))]});
        assert_eq!(ids(&t.on_message(&batch, false)), vec![2]);
        let batch = json!({"type": "exchanges", "added": [], "updated": [s(3, "complete", Some(404)), s(2, "complete", Some(201))]});
        assert_eq!(ids(&t.on_message(&batch, false)), vec![3]);
        // A resync hello after lagging prints only what wasn't printed yet.
        let hello = json!({"type": "hello", "summaries": [s(1, "complete", Some(200)), s(2, "complete", Some(201)), s(4, "complete", Some(200))]});
        assert_eq!(ids(&t.on_message(&hello, false)), vec![4]);
    }

    #[test]
    fn history_flag_prints_existing() {
        let mut t = TailState::default();
        let hello = json!({"type": "hello", "summaries": [s(1, "complete", Some(200)), s(2, "waiting", None)]});
        assert_eq!(ids(&t.on_message(&hello, true)), vec![1]);
    }

    #[test]
    fn server_side_filter_gates_output() {
        let mut t = TailState::default();
        t.on_message(&json!({"type": "hello", "summaries": []}), false);
        t.on_message(
            &json!({"type": "filter-result", "seq": 1, "query": "status:404", "ids": []}),
            false,
        );
        let batch = json!({
            "type": "exchanges",
            "added": [s(5, "complete", Some(200)), s(6, "complete", Some(404))],
            "updated": [],
            "filtered": {"add": [6], "remove": []}
        });
        assert_eq!(ids(&t.on_message(&batch, false)), vec![6]);
        // `ids: null` = everything matches.
        t.on_message(
            &json!({"type": "filter-result", "seq": 2, "query": "", "ids": null}),
            false,
        );
        let batch =
            json!({"type": "exchanges", "added": [s(7, "complete", Some(200))], "updated": []});
        assert_eq!(ids(&t.on_message(&batch, false)), vec![7]);
    }

    #[test]
    fn stream_urls() {
        assert_eq!(
            stream_url("http://127.0.0.1:45456", "a b").unwrap(),
            "ws://127.0.0.1:45456/api/stream?token=a+b"
        );
        assert_eq!(
            stream_url("https://host:1/", "t").unwrap(),
            "wss://host:1/api/stream?token=t"
        );
    }
}
