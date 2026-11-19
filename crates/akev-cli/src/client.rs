//! Talking to a running daemon: discovery through `daemon.json` (or `--url`/`--token`) and a
//! small authenticated HTTP client.

use akev_api::DaemonInfo;
use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Method, StatusCode};
use serde_json::Value;
use std::path::Path;
use std::time::Duration;

/// HTTP client for the local API. Never uses a proxy: the user's `HTTP(S)_PROXY` may well
/// point at Akev itself (e.g. inside an intercepted terminal).
pub fn http_client(timeout: Option<Duration>) -> Result<reqwest::Client> {
    akev_api::install_default_crypto_provider();
    let mut b = reqwest::Client::builder()
        .no_proxy()
        .user_agent(concat!("akev-cli/", env!("CARGO_PKG_VERSION")));
    if let Some(t) = timeout {
        b = b.timeout(t);
    }
    b.build().context("failed to build HTTP client")
}

/// A reachable, authenticated daemon.
pub struct Daemon {
    pub url: String,
    pub token: String,
    pub pid: Option<u32>,
    http: reqwest::Client,
}

impl Daemon {
    /// Find a running daemon. `Ok(None)` when none is running (no/stale `daemon.json`).
    pub async fn connect(
        data_dir: &Path,
        url: Option<&str>,
        token: Option<&str>,
    ) -> Result<Option<Daemon>> {
        let info = DaemonInfo::read(data_dir);
        let (url, token, pid) = match (url, token, info) {
            (Some(u), Some(t), info) => {
                let pid = info.filter(|i| i.api_url == u).map(|i| i.pid);
                (u.to_string(), t.to_string(), pid)
            }
            (Some(u), None, Some(i)) => (u.to_string(), i.token, Some(i.pid)),
            (Some(_), None, None) => bail!(
                "--url requires --token (no daemon.json found in {})",
                data_dir.display()
            ),
            (None, t, Some(i)) => (
                i.api_url,
                t.map(str::to_string).unwrap_or(i.token),
                Some(i.pid),
            ),
            (None, _, None) => return Ok(None),
        };
        let daemon = Daemon {
            url: url.trim_end_matches('/').to_string(),
            token,
            pid,
            http: http_client(Some(Duration::from_secs(600)))?,
        };
        let probe = daemon
            .http
            .get(format!("{}/api/status", daemon.url))
            .bearer_auth(&daemon.token)
            .timeout(Duration::from_secs(3))
            .send()
            .await;
        match probe {
            Ok(r) if r.status().is_success() => Ok(Some(daemon)),
            Ok(r) if r.status() == StatusCode::UNAUTHORIZED => {
                bail!(
                    "the Akev daemon at {} rejected the API token (pass --token)",
                    daemon.url
                )
            }
            Ok(r) => bail!("unexpected response from {}: {}", daemon.url, r.status()),
            Err(e) if e.is_connect() || e.is_timeout() => Ok(None),
            Err(e) => Err(anyhow!(
                "failed to reach the Akev daemon at {}: {e}",
                daemon.url
            )),
        }
    }

    /// The authenticated UI URL. Never pass it on a command line (see `start::open_ui`).
    pub fn ui_url(&self) -> String {
        akev_api::ui_url_for(&self.url, &self.token)
    }

    /// Send a request; non-2xx responses become errors carrying the API's message.
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        body: Option<RequestBody>,
    ) -> Result<reqwest::Response> {
        let mut req = self
            .http
            .request(method.clone(), format!("{}{path}", self.url))
            .bearer_auth(&self.token);
        match body {
            Some(RequestBody::Json(v)) => req = req.json(&v),
            Some(RequestBody::Bytes(b, ct)) => {
                req = req.header(reqwest::header::CONTENT_TYPE, ct).body(b)
            }
            None => {}
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("{method} {path} failed"))?;
        if resp.status().is_success() {
            return Ok(resp);
        }
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        Err(anyhow!("{}", api_error_message(status, &text)))
    }

    pub async fn get_json(&self, path: &str) -> Result<Value> {
        let resp = self.request(Method::GET, path, None).await?;
        resp.json().await.context("invalid JSON from the API")
    }

    /// POST/PUT JSON; returns `None` for `204 No Content`.
    pub async fn send_json(
        &self,
        method: Method,
        path: &str,
        body: Value,
    ) -> Result<Option<Value>> {
        let resp = self
            .request(method, path, Some(RequestBody::Json(body)))
            .await?;
        if resp.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        let bytes = resp.bytes().await?;
        if bytes.is_empty() {
            return Ok(None);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .context("invalid JSON from the API")
    }

    pub async fn get_bytes(&self, path: &str) -> Result<bytes::Bytes> {
        let resp = self.request(Method::GET, path, None).await?;
        resp.bytes().await.context("failed to read the response")
    }

    /// Raw POST of an MCP payload: `Ok(None)` for `202 Accepted`, `Ok(Some(json))` for a
    /// JSON-RPC response (including JSON-RPC errors sent with a 4xx status).
    pub async fn post_mcp(&self, payload: String) -> Result<Option<String>, McpPostError> {
        let resp = self
            .http
            .post(format!("{}/mcp", self.url))
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .body(payload)
            .send()
            .await
            .map_err(|e| {
                if e.is_connect() || e.is_timeout() || e.is_request() {
                    McpPostError::Unreachable(e.to_string())
                } else {
                    McpPostError::Failed(e.to_string())
                }
            })?;
        let status = resp.status();
        if status == StatusCode::ACCEPTED || status == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        if status == StatusCode::UNAUTHORIZED {
            return Err(McpPostError::Unauthorized);
        }
        let text = resp
            .text()
            .await
            .map_err(|e| McpPostError::Unreachable(e.to_string()))?;
        if status.is_success() || is_jsonrpc(&text) {
            return Ok(Some(text));
        }
        Err(McpPostError::Failed(api_error_message(status, &text)))
    }
}

/// Whether `text` is a JSON-RPC message (or batch).
fn is_jsonrpc(text: &str) -> bool {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(o)) => o.contains_key("jsonrpc"),
        Ok(Value::Array(items)) => items.iter().all(|i| i.get("jsonrpc").is_some()),
        _ => false,
    }
}

/// Failure forwarding an MCP message to the daemon.
#[derive(Debug, Clone, PartialEq)]
pub enum McpPostError {
    /// The daemon couldn't be reached (stopped or restarted elsewhere).
    Unreachable(String),
    /// The daemon rejected the token (it restarted with a new one).
    Unauthorized,
    /// Any other failure.
    Failed(String),
}

impl std::fmt::Display for McpPostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            McpPostError::Unreachable(e) => write!(f, "the Akev daemon is unreachable: {e}"),
            McpPostError::Unauthorized => f.write_str("the Akev daemon rejected the API token"),
            McpPostError::Failed(e) => f.write_str(e),
        }
    }
}

/// Request body variants.
pub enum RequestBody {
    Json(Value),
    Bytes(Vec<u8>, &'static str),
}

/// Extract `{"error": {"message"}}` from an API error response.
pub fn api_error_message(status: StatusCode, body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
        .map(|m| format!("{m} ({status})"))
        .unwrap_or_else(|| {
            if body.trim().is_empty() {
                status.to_string()
            } else {
                format!("{status}: {}", body.trim())
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert_eq!(
            api_error_message(
                StatusCode::NOT_FOUND,
                r#"{"error":{"code":"not-found","message":"exchange 3 not found"}}"#
            ),
            "exchange 3 not found (404 Not Found)"
        );
        assert_eq!(
            api_error_message(StatusCode::BAD_GATEWAY, ""),
            "502 Bad Gateway"
        );
        assert_eq!(
            api_error_message(StatusCode::BAD_REQUEST, "oops"),
            "400 Bad Request: oops"
        );
    }

    #[test]
    fn jsonrpc_detection() {
        assert!(is_jsonrpc(
            r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32600,"message":"x"}}"#
        ));
        assert!(is_jsonrpc(r#"[{"jsonrpc":"2.0","id":1,"result":{}}]"#));
        assert!(!is_jsonrpc(
            r#"{"error":{"code":"forbidden-origin","message":"no"}}"#
        ));
        assert!(!is_jsonrpc("not json"));
    }

    #[tokio::test]
    async fn no_daemon_without_discovery_file() {
        let dir = tempfile::tempdir().unwrap();
        assert!(
            Daemon::connect(dir.path(), None, None)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            Daemon::connect(dir.path(), Some("http://127.0.0.1:1"), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stale_discovery_file_means_not_running() {
        let dir = tempfile::tempdir().unwrap();
        // A port nobody listens on.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        DaemonInfo {
            pid: 1,
            api_url: format!("http://127.0.0.1:{port}"),
            token: "t".into(),
            proxy_port: 8000,
            version: "0".into(),
        }
        .write(dir.path())
        .unwrap();
        assert!(
            Daemon::connect(dir.path(), None, None)
                .await
                .unwrap()
                .is_none()
        );
    }
}
