//! WireGuard mode through the REST API: enable it live, tunnel a request from a WireGuard
//! client (boringtun + smoltcp), see it recorded, read the configuration, regenerate keys,
//! and handle a busy port.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use akev_wireguard::WireguardKeys;
use akev_wireguard::testing::TestClient;
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const T: Duration = Duration::from_secs(20);

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
}

impl Env {
    async fn new() -> Env {
        let handle = start(AkevOptions {
            ephemeral: true,
            proxy_port: Some(0),
            api_port: 0,
            ..pro::options()
        })
        .await
        .expect("akev starts");
        let api = reqwest::Client::builder().no_proxy().build().unwrap();
        Env { handle, api }
    }

    async fn send(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> reqwest::Response {
        let mut req = self
            .api
            .request(method, format!("{}{path}", self.handle.api_url()))
            .bearer_auth(self.handle.token());
        if let Some(b) = body {
            req = req.json(&b);
        }
        req.send().await.unwrap()
    }

    async fn json(&self, method: reqwest::Method, path: &str, body: Option<Value>) -> Value {
        let resp = self.send(method, path, body).await;
        assert!(resp.status().is_success(), "{path}: {}", resp.status());
        resp.json().await.unwrap()
    }

    async fn info(&self) -> Value {
        self.json(reqwest::Method::GET, "/api/wireguard", None)
            .await
    }
}

fn free_udp_port() -> u16 {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

async fn origin() -> SocketAddr {
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = l.accept().await {
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let mut got = Vec::new();
                while !got.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => got.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\nConnection: close\r\n\r\nfrom phone")
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn wireguard_mode_via_the_api() {
    let env = Env::new().await;

    // Off by default; the configuration is available anyway.
    let info = env.info().await;
    assert_eq!(info["enabled"], false);
    assert_eq!(info["running"], false);
    assert_eq!(info["port"], 51820);
    let config = info["config"].as_str().unwrap();
    assert!(config.starts_with("[Interface]\nPrivateKey = "), "{config}");
    assert!(config.contains("Address = 10.0.0.2/32\nDNS = 10.0.0.53\n"));
    assert!(config.contains("AllowedIPs = 0.0.0.0/0, ::/0"));
    assert_eq!(info["qrPayload"], info["config"]);
    assert_eq!(info["configFileName"], "akev.conf");
    let server_key = info["serverPublicKey"].as_str().unwrap().to_string();
    assert!(config.contains(&format!("PublicKey = {server_key}")));

    // Keys are persisted in the data directory (owner-only).
    let keys = WireguardKeys::load_or_generate(env.handle.data_dir()).unwrap();
    assert_eq!(keys.server.public_key().to_base64(), server_key);

    // Enable it live on a free loopback port.
    let port = free_udp_port();
    let info = env
        .json(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "enabled": true, "port": port, "bindAddress": "127.0.0.1" })),
        )
        .await;
    assert_eq!(info["running"], true, "{info}");
    assert_eq!(info["listenAddr"], format!("127.0.0.1:{port}"));
    assert_eq!(info["endpoints"], json!(["127.0.0.1"]));
    assert_eq!(info["endpoint"], format!("127.0.0.1:{port}"));
    let settings = env.json(reqwest::Method::GET, "/api/settings", None).await;
    assert_eq!(
        settings["proxy"]["wireguard"],
        json!({ "enabled": true, "port": port, "bindAddress": "127.0.0.1" })
    );
    let interceptors = env
        .json(reqwest::Method::GET, "/api/interceptors", None)
        .await;
    let wg = interceptors
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "wireguard")
        .expect("wireguard interceptor listed");
    assert_eq!(wg["active"], true);
    assert_eq!(wg["category"], "mobile");
    assert_eq!(wg["needsConfig"], true);

    // A device connects and browses.
    let upstream = origin().await;
    let device = TestClient::start(format!("127.0.0.1:{port}").parse().unwrap(), &keys)
        .await
        .unwrap();
    device.handshake(T).await.expect("handshake");
    let mut conn = device.connect(upstream).await.unwrap();
    conn.write_all(format!("GET /api-check HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut resp = Vec::new();
    tokio::time::timeout(T, conn.read_to_end(&mut resp))
        .await
        .unwrap()
        .unwrap();
    assert!(String::from_utf8_lossy(&resp).ends_with("from phone"));

    let deadline = tokio::time::Instant::now() + T;
    let summary = loop {
        let list = env.json(reqwest::Method::GET, "/api/exchanges", None).await;
        if let Some(s) = list
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["path"] == "/api-check" && s["status"] == 200)
        {
            break s.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "not recorded: {list}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(summary["source"], "wireguard");
    let detail = env
        .json(
            reqwest::Method::GET,
            &format!("/api/exchanges/{}", summary["id"]),
            None,
        )
        .await;
    assert_eq!(detail["client"]["via"], "wireguard");
    assert_eq!(detail["client"]["interceptor"], "wireguard");
    let filtered = env
        .json(
            reqwest::Method::POST,
            "/api/filter/query",
            Some(json!({ "query": "source:wireguard" })),
        )
        .await;
    assert!(
        filtered["ids"].as_array().unwrap().contains(&summary["id"]),
        "{filtered}"
    );

    // Live handshake status.
    tokio::time::sleep(Duration::from_millis(400)).await;
    let info = env.info().await;
    assert_eq!(info["peer"]["connected"], true, "{info}");
    assert!(info["peer"]["lastHandshakeAt"].as_u64().unwrap() > 0);
    assert!(info["peer"]["lastSeenAt"].as_u64().unwrap() > 0);
    assert!(info["stats"]["tcpTotal"].as_u64().unwrap() >= 1);

    // Configuration download for another address.
    let resp = env
        .send(
            reqwest::Method::GET,
            "/api/wireguard/config?host=192.168.5.5",
            None,
        )
        .await;
    assert_eq!(resp.status(), 200);
    assert!(
        resp.headers()["content-disposition"]
            .to_str()
            .unwrap()
            .contains("akev.conf")
    );
    let text = resp.text().await.unwrap();
    assert!(
        text.contains(&format!("Endpoint = 192.168.5.5:{port}")),
        "{text}"
    );
    let bad = env
        .send(reqwest::Method::GET, "/api/wireguard?host=a%20b", None)
        .await;
    assert_eq!(bad.status(), 400);

    // New keys: the old configuration stops working, the server keeps running.
    let info = env
        .json(reqwest::Method::POST, "/api/wireguard/regenerate", None)
        .await;
    assert_ne!(info["serverPublicKey"], server_key);
    assert_eq!(info["running"], true);
    let stale = TestClient::start(format!("127.0.0.1:{port}").parse().unwrap(), &keys)
        .await
        .unwrap();
    assert!(stale.handshake(Duration::from_secs(1)).await.is_err());
    let fresh = WireguardKeys::load_or_generate(env.handle.data_dir()).unwrap();
    let device = TestClient::start(format!("127.0.0.1:{port}").parse().unwrap(), &fresh)
        .await
        .unwrap();
    device.handshake(T).await.expect("new keys work");

    // Disable.
    let info = env
        .json(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(info["running"], false);
    assert!(info["stats"].is_null());
    env.handle.shutdown().await;
}

#[tokio::test]
async fn busy_port_and_invalid_settings() {
    let env = Env::new().await;
    let blocker = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = blocker.local_addr().unwrap().port();
    let info = env
        .json(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "enabled": true, "port": port, "bindAddress": "127.0.0.1" })),
        )
        .await;
    assert_eq!(info["enabled"], true);
    assert_eq!(info["running"], false);
    assert!(
        info["error"].as_str().unwrap().contains("could not start"),
        "{info}"
    );
    let interceptors = env
        .json(reqwest::Method::GET, "/api/interceptors", None)
        .await;
    let wg = interceptors
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "wireguard")
        .unwrap();
    assert_eq!(wg["active"], false);
    assert_eq!(wg["available"], false);
    assert!(wg["unavailableReason"].as_str().is_some());

    // Freeing the port and enabling again (same settings) retries and starts it.
    drop(blocker);
    let info = env
        .json(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "enabled": true })),
        )
        .await;
    assert_eq!(info["running"], true, "{info}");
    assert_eq!(info["listenAddr"], format!("127.0.0.1:{port}"));
    assert!(info["error"].is_null());
    // Another port moves it.
    let other = free_udp_port();
    let info = env
        .json(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "port": other })),
        )
        .await;
    assert_eq!(info["listenAddr"], format!("127.0.0.1:{other}"));

    let mut settings = env.json(reqwest::Method::GET, "/api/settings", None).await;
    settings["proxy"]["wireguard"]["bindAddress"] = json!("not-an-ip");
    let resp = env
        .send(reqwest::Method::PUT, "/api/settings", Some(settings))
        .await;
    assert_eq!(resp.status(), 400);
    let resp = env
        .send(
            reqwest::Method::PUT,
            "/api/wireguard",
            Some(json!({ "port": 0 })),
        )
        .await;
    assert_eq!(resp.status(), 400);

    // The interceptor endpoints toggle the same setting.
    let r = env
        .send(
            reqwest::Method::POST,
            "/api/interceptors/wireguard/deactivate",
            Some(json!({})),
        )
        .await;
    assert_eq!(r.status(), 204);
    assert_eq!(env.info().await["running"], false);
    let r = env
        .json(
            reqwest::Method::POST,
            "/api/interceptors/wireguard/activate",
            Some(json!({})),
        )
        .await;
    assert_eq!(r["success"], true, "{r}");
    assert_eq!(env.info().await["running"], true);
    env.handle.shutdown().await;
}
