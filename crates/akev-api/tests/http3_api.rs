//! HTTP/3 through the REST API: the `proxy.http3` settings section (defaults, round trip,
//! live listener start / move / stop reported in `GET /api/status`), and `httpVersion` in
//! Compose's send options.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_api::{AkevHandle, AkevOptions, start};
use serde_json::{Value, json};

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
}

impl Env {
    async fn start() -> Env {
        let handle = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..AkevOptions::default()
        })
        .await
        .expect("akev starts");
        let api = reqwest::Client::builder().no_proxy().build().unwrap();
        Env { handle, api }
    }

    async fn call(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = self
            .api
            .request(method, format!("{}{path}", self.handle.api_url()))
            .bearer_auth(self.handle.token());
        if let Some(b) = body {
            req = req.json(&b);
        }
        let r = req.send().await.unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn get(&self, path: &str) -> Value {
        let (status, v) = self.call(reqwest::Method::GET, path, None).await;
        assert_eq!(status, 200, "{path}: {v}");
        v
    }

    async fn put_settings(&self, settings: Value) -> Value {
        let (status, v) = self
            .call(reqwest::Method::PUT, "/api/settings", Some(settings))
            .await;
        assert_eq!(status, 200, "{v}");
        v
    }
}

#[tokio::test]
async fn http3_settings_and_listener_status() {
    let env = Env::start().await;
    let mut settings = env.get("/api/settings").await;
    assert_eq!(
        settings["proxy"]["http3"],
        json!({"upstream": "off", "listen": false, "listenPort": 0})
    );
    let status = env.get("/api/status").await;
    assert_eq!(status["proxy"]["http3"], json!({"enabled": false}));
    let proxy_port = status["proxy"]["port"].as_u64().unwrap();

    // Enable the listener (on the proxy's port number) and HTTP/3 upstream via Alt-Svc.
    settings["proxy"]["http3"] = json!({"upstream": "alt-svc", "listen": true, "listenPort": 0});
    let saved = env.put_settings(settings.clone()).await;
    assert_eq!(saved["proxy"]["http3"]["upstream"], "alt-svc");
    let status = env.get("/api/status").await;
    let h3 = &status["proxy"]["http3"];
    assert_eq!(h3["enabled"], true);
    assert_eq!(h3["port"].as_u64(), Some(proxy_port));
    assert!(
        h3["address"]
            .as_str()
            .unwrap()
            .ends_with(&format!(":{proxy_port}"))
    );
    assert!(h3.get("error").is_none());

    // Regenerating the CA restarts the proxy: the QUIC listener comes back on the same port
    // (its old socket is released asynchronously; binding is retried).
    let (code, _) = env
        .call(reqwest::Method::POST, "/api/ca/regenerate", None)
        .await;
    assert_eq!(code, 200);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let h3 = env.get("/api/status").await["proxy"]["http3"].clone();
        if h3["port"].as_u64() == Some(proxy_port) {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "listener not back: {h3}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    // A UDP port that's taken: reported, not fatal.
    let busy = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let busy_port = busy.local_addr().unwrap().port();
    settings["proxy"]["http3"]["listenPort"] = json!(busy_port);
    env.put_settings(settings.clone()).await;
    let h3 = env.get("/api/status").await["proxy"]["http3"].clone();
    assert_eq!(h3["enabled"], true);
    assert!(h3.get("port").is_none());
    assert!(
        h3["error"].as_str().unwrap().contains("cannot listen"),
        "{h3}"
    );
    drop(busy);

    // Off again.
    settings["proxy"]["http3"] = json!({"upstream": "prefer", "listen": false, "listenPort": 0});
    env.put_settings(settings).await;
    assert_eq!(
        env.get("/api/status").await["proxy"]["http3"],
        json!({"enabled": false})
    );
    assert_eq!(
        env.get("/api/settings").await["proxy"]["http3"]["upstream"],
        "prefer"
    );

    // Unknown modes are rejected.
    let mut bad = env.get("/api/settings").await;
    bad["proxy"]["http3"]["upstream"] = json!("always");
    let (status, _) = env
        .call(reqwest::Method::PUT, "/api/settings", Some(bad))
        .await;
    assert!(status == 400 || status == 422, "{status}");
}

#[tokio::test]
async fn send_accepts_http_version() {
    let env = Env::start().await;
    // A plain-HTTP origin: `http3` never applies to http:// URLs, the request just works.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let app = axum::Router::new().route("/v", axum::routing::get(|| async { "plain" }));
        let _ = axum::serve(listener, app).await;
    });
    for version in ["auto", "http1", "http2", "http3"] {
        let (status, out) = env
            .call(
                reqwest::Method::POST,
                "/api/send",
                Some(json!({
                    "request": {"method": "GET", "url": format!("http://{addr}/v"), "headers": [], "body": ""},
                    "options": {"httpVersion": version}
                })),
            )
            .await;
        assert_eq!(status, 200, "{version}: {out}");
        assert_eq!(out["response"]["status"], 200, "{version}: {out}");
    }
    let (status, _) = env
        .call(
            reqwest::Method::POST,
            "/api/send",
            Some(json!({
                "request": {"method": "GET", "url": format!("http://{addr}/v"), "headers": [], "body": ""},
                "options": {"httpVersion": "http9"}
            })),
        )
        .await;
    assert!(status == 400 || status == 422, "{status}");
}
