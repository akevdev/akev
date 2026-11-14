//! Upstream TLS fingerprint settings through the REST API: the profile catalogue, the
//! `proxy.tlsProfile` settings and the `tls-profile` rule action.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use serde_json::{Value, json};

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
}

async fn setup() -> Env {
    let handle = start(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    })
    .await
    .expect("akev starts");
    Env {
        handle,
        api: reqwest::Client::builder().no_proxy().build().unwrap(),
    }
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    async fn get(&self, path: &str) -> Value {
        let r = self
            .api
            .get(self.url(path))
            .bearer_auth(self.handle.token())
            .send()
            .await
            .unwrap();
        assert!(r.status().is_success(), "GET {path} → {}", r.status());
        r.json().await.unwrap()
    }

    async fn put(&self, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .api
            .put(self.url(path))
            .bearer_auth(self.handle.token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        (status, r.json().await.unwrap_or(Value::Null))
    }
}

#[tokio::test]
async fn catalogue_reports_profiles_and_build_capability() {
    let env = setup().await;
    let body = env.get("/api/tls-profiles").await;
    // Compiled into akev-proxy, whose feature can be on without this crate's.
    let impersonate = akev_proxy::upstream::fingerprint::available();
    assert!(impersonate || !cfg!(feature = "impersonate"));
    assert_eq!(body["available"], impersonate);
    assert_eq!(body["backend"].as_str(), impersonate.then_some("BoringSSL"));
    let profiles = body["profiles"].as_array().unwrap();
    let ids: Vec<&str> = profiles.iter().map(|p| p["id"].as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "native",
            "chrome",
            "firefox",
            "safari",
            "edge",
            "mirror-client"
        ]
    );
    for p in profiles {
        let expected = p["id"] == "native" || impersonate;
        assert_eq!(p["available"], expected, "{p}");
        assert!(!p["label"].as_str().unwrap().is_empty());
        assert!(!p["description"].as_str().unwrap().is_empty());
    }
    let firefox = &profiles[2];
    assert_eq!(firefox["ja4"], "t13d1717h2_5b57614c22b0_3cbfd9057e0d");
    assert_eq!(
        firefox["http2"],
        "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s"
    );
    assert!(firefox["imitates"].as_str().unwrap().starts_with("Firefox"));
    env.handle.shutdown().await;
}

#[tokio::test]
async fn tls_profile_settings_round_trip_and_validate() {
    let env = setup().await;
    let mut settings = env.get("/api/settings").await;
    assert_eq!(
        settings["proxy"]["tlsProfile"],
        json!({ "profile": "native", "overrides": [] })
    );

    settings["proxy"]["tlsProfile"] = json!({
        "profile": "mirror-client",
        "overrides": [
            { "host": "*.shop.example", "profile": "chrome" },
            { "host": "api.example:8443", "profile": "safari" }
        ]
    });
    let (status, saved) = env.put("/api/settings", settings.clone()).await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(
        saved["proxy"]["tlsProfile"],
        settings["proxy"]["tlsProfile"]
    );
    let reread = env.get("/api/settings").await;
    assert_eq!(reread["proxy"]["tlsProfile"]["profile"], "mirror-client");
    assert_eq!(
        reread["proxy"]["tlsProfile"]["overrides"][1]["profile"],
        "safari"
    );

    // Empty override hosts are rejected...
    let mut bad = reread.clone();
    bad["proxy"]["tlsProfile"]["overrides"] = json!([{ "host": " ", "profile": "firefox" }]);
    let (status, body) = env.put("/api/settings", bad).await;
    assert_eq!(status, 400, "{body}");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("tlsProfile"),
        "{body}"
    );
    // ...unknown profile names (from a newer Akev) read as native.
    let mut newer = reread.clone();
    newer["proxy"]["tlsProfile"]["profile"] = json!("chrome-android");
    let (status, body) = env.put("/api/settings", newer).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["proxy"]["tlsProfile"]["profile"], "native");
    env.handle.shutdown().await;
}

#[tokio::test]
async fn tls_profile_rules_are_accepted_and_returned() {
    let env = setup().await;
    let rules = json!({
        "version": 1,
        "items": [{
            "type": "rule",
            "id": "ff",
            "title": "Look like Firefox to the shop",
            "enabled": true,
            "matchers": [{ "type": "host", "pattern": { "op": "equals", "value": "shop.example" } }],
            "action": { "type": "tls-profile", "profile": "firefox" }
        }]
    });
    let (status, body) = env.put("/api/rules", rules).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body["errors"], json!([]));
    let stored = env.get("/api/rules").await;
    assert_eq!(
        stored["items"][0]["action"],
        json!({ "type": "tls-profile", "profile": "firefox" })
    );
    env.handle.shutdown().await;
}
