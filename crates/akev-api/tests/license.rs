//! Akev Pro licensing end to end: what a Free instance refuses (402 `pro-required`, MCP tool
//! errors), that Pro configuration is kept but inactive, and that a licence key or the trial
//! unlocks everything live.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::routing::get;
use futures::StreamExt;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio_tungstenite::tungstenite::Message;

const TIMEOUT: Duration = Duration::from_secs(10);

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
    proxied: reqwest::Client,
    upstream: SocketAddr,
}

async fn setup(options: AkevOptions) -> Env {
    let handle = start(AkevOptions {
        proxy_port: Some(0),
        api_port: 0,
        ..options
    })
    .await
    .expect("akev starts");
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    let app = Router::new().route("/hello", get(|| async { "hello" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Env {
        handle,
        api,
        proxied,
        upstream,
    }
}

fn ephemeral_free() -> AkevOptions {
    AkevOptions {
        ephemeral: true,
        ..pro::free()
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = self
            .api
            .request(method, self.url(path))
            .bearer_auth(self.handle.token());
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn license(&self) -> Value {
        self.call(reqwest::Method::GET, "/api/license", None)
            .await
            .1
    }

    async fn mcp(&self, tool: &str, arguments: Value) -> Value {
        let rpc = json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":tool,"arguments":arguments}});
        self.handle.mcp(rpc.to_string().as_bytes()).await.unwrap()
    }

    /// The `x-script` header the test script rule adds to responses, if any.
    async fn script_header(&self) -> Option<String> {
        let resp = self
            .proxied
            .get(format!("http://{}/hello", self.upstream))
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        resp.headers()
            .get("x-script")
            .map(|v| v.to_str().unwrap().to_string())
    }
}

fn assert_pro_required((status, body): (u16, Value), feature: &str) {
    assert_eq!(status, 402, "{body}");
    assert_eq!(body["error"]["code"], "pro-required", "{body}");
    assert_eq!(body["error"]["feature"], feature, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Akev Pro"),
        "{body}"
    );
}

fn script_rules() -> Value {
    json!({ "version": 1, "items": [{
        "type": "rule",
        "id": "add-header",
        "matchers": [],
        "action": { "type": "script", "source": "function onResponse(req, res) { res.headers['x-script'] = 'yes'; return res; }" }
    }]})
}

#[tokio::test]
async fn free_refuses_pro_features_and_keeps_their_configuration() {
    use reqwest::Method;
    let env = setup(ephemeral_free()).await;

    let license = env.license().await;
    assert_eq!(license["plan"], "free");
    assert_eq!(license["state"], "none");
    assert_eq!(license["features"], json!([]));
    // Ephemeral sessions (CI, throwaway containers) never start the trial.
    assert_eq!(license["trialAvailable"], false);
    assert_eq!(license["releaseDate"], akev_core::RELEASE_DATE);
    let (_, status) = env.call(Method::GET, "/api/status", None).await;
    assert_eq!(status["license"], license);

    // Script rules are accepted and kept, reported as inactive, and don't run.
    let (code, saved) = env
        .call(Method::PUT, "/api/rules", Some(script_rules()))
        .await;
    assert_eq!(code, 200, "{saved}");
    let errors = saved["errors"].to_string();
    assert!(
        errors.contains("add-header") && errors.contains("Pro"),
        "{saved}"
    );
    let (_, rules) = env.call(Method::GET, "/api/rules", None).await;
    assert_eq!(rules["items"][0]["action"]["type"], "script");
    assert_eq!(rules["items"][0]["enabled"], true);
    assert_eq!(env.script_header().await, None);

    // Turning Pro features on is refused.
    assert_pro_required(
        env.call(
            Method::PUT,
            "/api/wireguard",
            Some(json!({ "enabled": true })),
        )
        .await,
        "wireguard",
    );
    assert_pro_required(
        env.call(
            Method::POST,
            "/api/interceptors/wireguard/activate",
            Some(json!({})),
        )
        .await,
        "wireguard",
    );
    assert_pro_required(
        env.call(
            Method::POST,
            "/api/interceptors/jvm/activate",
            Some(json!({})),
        )
        .await,
        "jvm-attach",
    );
    assert_pro_required(
        env.call(
            Method::POST,
            "/api/transparent/plan",
            Some(json!({ "mode": "user" })),
        )
        .await,
        "transparent",
    );
    assert_pro_required(
        env.call(Method::PUT, "/api/protos/mappings", Some(json!([])))
            .await,
        "proto-schemas",
    );
    assert_pro_required(
        env.call(Method::GET, "/api/exchanges/1/llm", None).await,
        "llm-lens",
    );
    let (_, mut settings) = env.call(Method::GET, "/api/settings", None).await;
    settings["proxy"]["tlsProfile"]["profile"] = json!("chrome");
    assert_pro_required(
        env.call(Method::PUT, "/api/settings", Some(settings.clone()))
            .await,
        "tls-impersonation",
    );
    let (_, tls) = env.call(Method::GET, "/api/tls-profiles", None).await;
    assert_eq!(tls["licensed"], false);
    // Free settings still save.
    settings["proxy"]["tlsProfile"]["profile"] = json!("native");
    settings["capture"]["maxExchanges"] = json!(1234);
    let (code, body) = env.call(Method::PUT, "/api/settings", Some(settings)).await;
    assert_eq!(code, 200, "{body}");

    // MCP: reading is free, changing state needs Pro.
    let out = env.mcp("akev_list_exchanges", json!({})).await;
    assert_ne!(out["result"]["isError"], true, "{out}");
    let out = env
        .mcp(
            "akev_add_mock",
            json!({ "url": "http://x.test/", "status": 200 }),
        )
        .await;
    assert_eq!(out["result"]["isError"], true, "{out}");
    let text = out["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        text.contains("MCP tools that change state needs Akev Pro"),
        "{text}"
    );
    let out = env
        .mcp("akev_deactivate_interceptor", json!({ "id": "jvm" }))
        .await;
    assert!(
        !out.to_string().contains("Akev Pro"),
        "deactivating stays free: {out}"
    );
}

#[tokio::test]
async fn a_license_key_unlocks_pro_live_and_removing_it_keeps_data() {
    use reqwest::Method;
    let env = setup(ephemeral_free()).await;
    let url = format!(
        "ws://{}/api/stream?token={}",
        env.handle.api_addr(),
        env.handle.token()
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    let (code, _) = env
        .call(Method::PUT, "/api/rules", Some(script_rules()))
        .await;
    assert_eq!(code, 200);
    assert_eq!(env.script_header().await, None);

    let (code, body) = env
        .call(
            Method::PUT,
            "/api/license",
            Some(json!({ "key": "AKEV1.bad.key" })),
        )
        .await;
    assert_eq!(code, 400, "{body}");
    assert_eq!(body["error"]["code"], "invalid-license");

    let (code, license) = env
        .call(
            Method::PUT,
            "/api/license",
            Some(json!({ "key": format!("  {}\n", pro::TEST_KEY) })),
        )
        .await;
    assert_eq!(code, 200, "{license}");
    assert_eq!(license["plan"], "pro");
    assert_eq!(license["state"], "active");
    assert_eq!(license["license"]["email"], "tests@example.com");
    assert_eq!(license["license"]["name"], "Akev tests");
    assert_eq!(license["features"].as_array().unwrap().len(), 8);

    // The stream announces the change.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    loop {
        let msg = tokio::time::timeout_at(deadline, ws.next())
            .await
            .expect("license message in time")
            .unwrap()
            .unwrap();
        if let Message::Text(t) = msg {
            let v: Value = serde_json::from_str(t.as_str()).unwrap();
            if v["type"] == "license" {
                assert_eq!(v["license"]["state"], "active");
                break;
            }
        }
    }

    // The script rule runs now, without being saved again.
    assert_eq!(env.script_header().await.as_deref(), Some("yes"));
    let (_, tls) = env.call(Method::GET, "/api/tls-profiles", None).await;
    assert_eq!(tls["licensed"], true);
    let out = env
        .mcp("akev_set_recording", json!({ "recording": true }))
        .await;
    assert_ne!(out["result"]["isError"], true, "{out}");

    // Removing the key: back to Free, the rule is still there but inactive.
    let (code, license) = env.call(Method::DELETE, "/api/license", None).await;
    assert_eq!(code, 200);
    assert_eq!(license["state"], "none");
    assert_eq!(env.script_header().await, None);
    let (_, rules) = env.call(Method::GET, "/api/rules", None).await;
    assert_eq!(rules["items"][0]["id"], "add-header");
}

#[tokio::test]
async fn the_trial_runs_once_and_is_stored() {
    use reqwest::Method;
    let dir = tempfile::tempdir().unwrap();
    let options = || AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        ..pro::free()
    };
    {
        let env = setup(options()).await;
        let (code, license) = env.call(Method::POST, "/api/license/trial", None).await;
        assert_eq!(code, 200, "{license}");
        assert_eq!(license["state"], "trial");
        assert_eq!(license["plan"], "pro");
        assert_eq!(license["trialAvailable"], false);
        assert!(license["trialEndsAt"].as_f64().unwrap() > 0.0);
        let (code, body) = env.call(Method::POST, "/api/license/trial", None).await;
        assert_eq!(code, 409, "{body}");
        assert_eq!(body["error"]["code"], "trial-unavailable");
        env.handle.shutdown().await;
    }
    let stored = std::fs::read_to_string(dir.path().join("license.json")).unwrap();
    assert!(stored.contains("trialStartedAt"), "{stored}");
    let env = setup(options()).await;
    assert_eq!(env.license().await["state"], "trial");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn a_run_license_is_used_but_not_stored() {
    let dir = tempfile::tempdir().unwrap();
    let env = setup(AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        ..pro::options()
    })
    .await;
    assert_eq!(env.license().await["state"], "active");
    assert!(!dir.path().join("license.json").exists());
    env.handle.shutdown().await;
}
