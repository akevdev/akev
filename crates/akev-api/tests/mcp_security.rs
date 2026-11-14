//! What AI agents (MCP) may see and touch: secrets in captured traffic are redacted unless
//! the user opted in, and the REST exports redact on request.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::routing::{get, post};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

const JWT: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
/// Every secret the test exchange carries.
const SECRETS: &[&str] = &[
    "hunter2-pw",
    "AIzaQUERYKEY",
    "s3ssion-c00kie",
    "sk-ant-api03-abcdefghijklmnopqrstuvwx",
    JWT,
];

async fn upstream() -> SocketAddr {
    let app = Router::new()
        .route(
            "/login",
            post(|| async {
                (
                    [
                        ("content-type", "application/json"),
                        ("set-cookie", "sid=s3ssion-c00kie; Path=/; HttpOnly"),
                    ],
                    format!(r#"{{"access_token":"{JWT}","token_type":"bearer"}}"#),
                )
            }),
        )
        .route("/hello", get(|| async { "hello" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

async fn akev(options: AkevOptions) -> AkevHandle {
    start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..options
    })
    .await
    .expect("akev starts")
}

/// Send the secret-laden login through the proxy and wait until it is recorded.
async fn capture_login(handle: &AkevHandle, upstream: SocketAddr) -> u64 {
    let client = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    let resp = client
        .post(format!(
            "http://{upstream}/login?api_key=AIzaQUERYKEY&lang=en"
        ))
        .header(
            "Authorization",
            "Bearer sk-ant-api03-abcdefghijklmnopqrstuvwx",
        )
        .header("Cookie", "sid=s3ssion-c00kie")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("user=me&password=hunter2-pw")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let _ = resp.text().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (_, out) = mcp(
            handle,
            "akev_list_exchanges",
            json!({ "filter": "path:/login", "reveal": false }),
        )
        .await;
        let v: Value = serde_json::from_str(&out).unwrap();
        if let Some(row) = v["exchanges"].as_array().and_then(|r| r.first())
            && row["state"] == "complete"
        {
            return row["id"].as_u64().unwrap();
        }
        assert!(tokio::time::Instant::now() < deadline, "not recorded: {v}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Call an MCP tool in-process: `(is_error, text)`.
async fn mcp(handle: &AkevHandle, name: &str, arguments: Value) -> (bool, String) {
    let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":arguments}});
    let out = handle.mcp(rpc.to_string().as_bytes()).await.unwrap();
    let result = &out["result"];
    (
        result["isError"].as_bool().unwrap_or(false),
        result["content"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
    )
}

fn assert_redacted(what: &str, text: &str) {
    for secret in SECRETS {
        assert!(!text.contains(secret), "{what} leaks {secret}: {text}");
    }
}

#[tokio::test]
async fn mcp_output_is_redacted_by_default() {
    let handle = akev(pro::options()).await;
    let id = capture_login(&handle, upstream().await).await;

    let (err, text) = mcp(&handle, "akev_get_exchange", json!({ "id": id })).await;
    assert!(!err, "{text}");
    assert_redacted("akev_get_exchange", &text);
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        v["request"]["url"]
            .as_str()
            .unwrap()
            .split_once('?')
            .unwrap()
            .1,
        "api_key=[redacted]&lang=en"
    );
    let headers = v["request"]["headers"].as_array().unwrap();
    let auth = headers
        .iter()
        .find(|h| h[0].as_str().unwrap().eq_ignore_ascii_case("authorization"))
        .unwrap();
    assert_eq!(auth[1], "Bearer [redacted]");
    assert_eq!(v["request"]["body"]["text"], "user=me\npassword=[redacted]");
    let response: Value =
        serde_json::from_str(v["response"]["body"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(response["access_token"], "[redacted]");
    assert_eq!(response["token_type"], "bearer", "not a secret");

    for (tool, args) in [
        ("akev_list_exchanges", json!({})),
        ("akev_get_body", json!({ "id": id })),
        ("akev_get_body", json!({ "id": id, "side": "request" })),
        ("akev_get_body", json!({ "id": id, "format": "base64" })),
        ("akev_snippet", json!({ "id": id })),
        ("akev_export_har", json!({})),
        (
            "akev_wait_for",
            json!({ "filter": "path:/login", "includeExisting": true }),
        ),
    ] {
        let (err, text) = mcp(&handle, tool, args.clone()).await;
        assert!(!err, "{tool} {args}: {text}");
        assert_redacted(tool, &text);
        if tool == "akev_get_body" && args.get("format").is_some() {
            let v: Value = serde_json::from_str(&text).unwrap();
            let raw = base64_decode(v["base64"].as_str().unwrap());
            assert!(String::from_utf8(raw).unwrap().contains("[redacted]"));
        }
    }
    let (_, snippet) = mcp(&handle, "akev_snippet", json!({ "id": id })).await;
    assert!(snippet.contains("Bearer [redacted]"), "{snippet}");

    // Search runs over the redacted text: it can't be used to probe a secret.
    let (_, text) = mcp(&handle, "akev_search", json!({ "query": "hunter2" })).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["returned"], 0, "{v}");
    let (_, text) = mcp(&handle, "akev_search", json!({ "query": "Bearer sk-ant" })).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["returned"], 0, "{v}");
    let (_, text) = mcp(&handle, "akev_search", json!({ "query": "lang=en" })).await;
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["returned"], 1, "ordinary text is still found: {v}");
    assert_redacted("akev_search", &text);

    // Revealing needs the user's opt-in.
    let (err, text) = mcp(
        &handle,
        "akev_get_exchange",
        json!({ "id": id, "reveal": true }),
    )
    .await;
    assert!(err, "{text}");
    assert!(text.contains("--mcp-reveal-secrets"), "{text}");

    // The tools advertise `reveal`.
    let list = handle
        .mcp(
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list"})
                .to_string()
                .as_bytes(),
        )
        .await
        .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    let get = tools
        .iter()
        .find(|t| t["name"] == "akev_get_exchange")
        .unwrap();
    assert!(get["inputSchema"]["properties"]["reveal"].is_object());
    let status = tools.iter().find(|t| t["name"] == "akev_status").unwrap();
    assert!(status["inputSchema"]["properties"].get("reveal").is_none());
    handle.shutdown().await;
}

#[tokio::test]
async fn reveal_works_only_after_opt_in() {
    let handle = akev(AkevOptions {
        mcp_reveal_secrets: true,
        ..pro::options()
    })
    .await;
    let id = capture_login(&handle, upstream().await).await;
    // Still redacted unless the call asks.
    let (_, text) = mcp(&handle, "akev_get_exchange", json!({ "id": id })).await;
    assert_redacted("default output", &text);
    let (err, text) = mcp(
        &handle,
        "akev_get_exchange",
        json!({ "id": id, "reveal": true }),
    )
    .await;
    assert!(!err, "{text}");
    for secret in ["hunter2-pw", "AIzaQUERYKEY", "s3ssion-c00kie", JWT] {
        assert!(text.contains(secret), "reveal shows {secret}");
    }
    handle.shutdown().await;
}

#[tokio::test]
async fn rest_exports_redact_on_request() {
    let handle = akev(pro::options()).await;
    capture_login(&handle, upstream().await).await;
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let get = |path: &str| {
        api.get(format!("{}{path}", handle.api_url()))
            .bearer_auth(handle.token())
            .send()
    };
    let plain = get("/api/export/har").await.unwrap().text().await.unwrap();
    assert!(plain.contains("hunter2-pw") && plain.contains(JWT));
    let redacted = get("/api/export/har?redact=1")
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert_redacted("HAR export", &redacted);
    assert!(redacted.contains("[redacted]"));
    let session = get("/api/export/session?redact=true")
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let plain_session = get("/api/export/session")
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_ne!(session, plain_session);
    handle.shutdown().await;
}

fn base64_decode(s: &str) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(s).unwrap()
}

// ---------------------------------------------------------------------------------------
// File access (`--mcp-allow-path`)

fn mock_rule(id: &str, path: &str) -> Value {
    json!({
        "type": "rule", "id": id, "title": format!("mock {id}"), "enabled": true,
        "matchers": [{"type": "url", "pattern": {"op": "contains", "value": format!("/{id}")}}],
        "action": {"type": "mock", "status": 200, "body": {"type": "file", "path": path}}
    })
}

#[tokio::test]
async fn mcp_exports_stay_inside_the_allowed_folders() {
    let data = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let handle = akev(AkevOptions {
        data_dir: Some(data.path().to_path_buf()),
        mcp_allow_paths: vec![allowed.path().to_path_buf()],
        ..pro::options()
    })
    .await;
    capture_login(&handle, upstream().await).await;

    // Outside every root: refused, nothing written.
    let target = outside.path().join("x.har");
    let (err, text) = mcp(&handle, "akev_export_har", json!({ "path": target })).await;
    assert!(err, "{text}");
    assert!(
        text.contains("outside the folders") && text.contains("--mcp-allow-path"),
        "{text}"
    );
    assert!(!target.exists());
    let (err, _) = mcp(
        &handle,
        "akev_export_har",
        json!({ "path": "../settings.json" }),
    )
    .await;
    assert!(err);
    let (err, _) = mcp(
        &handle,
        "akev_export_har",
        json!({ "path": "~/.bashrc", "overwrite": true }),
    )
    .await;
    assert!(err);

    // A relative path lands in <data dir>/exports (created on demand).
    let (err, text) = mcp(&handle, "akev_export_har", json!({ "path": "traffic.har" })).await;
    assert!(!err, "{text}");
    let written = data.path().join("exports").join("traffic.har");
    let har = std::fs::read_to_string(&written).unwrap();
    assert!(har.contains("\"entries\""));
    assert_redacted("exported HAR file", &har);
    // An allowed folder works too.
    let target = allowed.path().join("shared.har");
    let (err, text) = mcp(&handle, "akev_export_har", json!({ "path": target })).await;
    assert!(!err, "{text}");
    assert!(target.exists());

    // akev_status tells the agent where it may write.
    let (_, status) = mcp(&handle, "akev_status", json!({})).await;
    let status: Value = serde_json::from_str(&status).unwrap();
    assert!(
        status["files"]["exportsDir"]
            .as_str()
            .unwrap()
            .ends_with("exports")
    );
    assert_eq!(status["files"]["allowedPaths"].as_array().unwrap().len(), 1);
    handle.shutdown().await;
}

#[tokio::test]
async fn mcp_rules_may_only_read_allowed_files_but_rest_rules_are_unaffected() {
    let data = tempfile::tempdir().unwrap();
    let allowed = tempfile::tempdir().unwrap();
    let fixture = allowed.path().join("fixture.json");
    std::fs::write(&fixture, br#"{"from":"fixture"}"#).unwrap();
    let handle = akev(AkevOptions {
        data_dir: Some(data.path().to_path_buf()),
        mcp_allow_paths: vec![allowed.path().to_path_buf()],
        ..pro::options()
    })
    .await;
    let rules_count = || handle.rules().rules().walk().len();

    // An agent can't serve /etc/passwd (or ~/.ssh through map-local) to itself.
    let rules = json!({ "version": 1, "items": [mock_rule("passwd", "/etc/passwd")] });
    let (err, text) = mcp(&handle, "akev_set_rules", json!({ "rules": rules })).await;
    assert!(err, "{text}");
    assert!(
        text.contains("/etc/passwd") && text.contains("--mcp-allow-path"),
        "{text}"
    );
    assert_eq!(rules_count(), 0, "nothing applied");
    let map_local = json!({ "version": 1, "items": [{
        "type": "rule", "id": "ssh", "enabled": true, "matchers": [],
        "action": {"type": "map-local", "path": "~/.ssh"}
    }]});
    let (err, _) = mcp(&handle, "akev_set_rules", json!({ "rules": map_local })).await;
    assert!(err);
    let (err, _) = mcp(
        &handle,
        "akev_test_rules",
        json!({
            "request": {"url": "http://example.com/passwd"},
            "rules": { "version": 1, "items": [mock_rule("passwd", "/etc/passwd")] }
        }),
    )
    .await;
    assert!(err, "dry runs are checked too");

    // A file inside an allowed folder works.
    let rules = json!({ "version": 1, "items": [mock_rule("fixture", fixture.to_str().unwrap())] });
    let (err, text) = mcp(&handle, "akev_set_rules", json!({ "rules": rules })).await;
    assert!(!err, "{text}");
    assert_eq!(rules_count(), 1);

    // The user can still make any rule through the REST API (the UI)...
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let user_rules = json!({ "version": 1, "items": [
        mock_rule("fixture", fixture.to_str().unwrap()),
        mock_rule("hosts", "/etc/hosts"),
    ]});
    let resp = api
        .put(format!("{}/api/rules", handle.api_url()))
        .bearer_auth(handle.token())
        .json(&user_rules)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success(), "{}", resp.status());
    assert_eq!(rules_count(), 2);
    // ...and an agent may pass that rule back unchanged while adding its own.
    let mut back = user_rules.clone();
    back["items"][1]["enabled"] = json!(false);
    back["items"].as_array_mut().unwrap().push(json!({
        "type": "rule", "id": "agent", "enabled": true, "matchers": [], "action": {"type": "close"}
    }));
    let (err, text) = mcp(&handle, "akev_set_rules", json!({ "rules": back })).await;
    assert!(!err, "{text}");
    assert_eq!(rules_count(), 3);
    // But not repoint it.
    let mut moved = back.clone();
    moved["items"][1]["action"]["body"]["path"] = json!("/etc/shadow");
    let (err, _) = mcp(&handle, "akev_set_rules", json!({ "rules": moved })).await;
    assert!(err);
    handle.shutdown().await;
}
