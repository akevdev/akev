//! End-to-end: transparent capture through the public API — listener status, settings, the
//! redirect check, command generation and the `transparent-linux` interceptor. A real Akev
//! instance; the kernel's original-destination answer is simulated through the proxy's lookup
//! hook (firewall redirects need root).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const TIMEOUT: Duration = Duration::from_secs(10);

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
            ..pro::options()
        })
        .await
        .expect("akev starts");
        Env {
            handle,
            api: reqwest::Client::builder().no_proxy().build().unwrap(),
        }
    }

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

    async fn send(&self, method: reqwest::Method, path: &str, body: Value) -> (u16, Value) {
        let r = self
            .api
            .request(method, self.url(path))
            .bearer_auth(self.handle.token())
            .json(&body)
            .send()
            .await
            .unwrap();
        let status = r.status().as_u16();
        let text = r.text().await.unwrap();
        (status, serde_json::from_str(&text).unwrap_or(Value::Null))
    }

    async fn set_transparent(&self, transparent: Value) -> (u16, Value) {
        let mut settings = self.get("/api/settings").await;
        settings["proxy"]["transparent"] = transparent;
        self.send(reqwest::Method::PUT, "/api/settings", settings)
            .await
    }
}

async fn upstream() -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let _ = s
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 8\r\nConnection: close\r\n\r\nupstream")
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn transparent_capture_status_check_and_commands() {
    let env = Env::start().await;

    // Off by default; the system facts the commands depend on are reported.
    let info = env.get("/api/transparent").await;
    assert_eq!(info["listener"]["enabled"], false);
    assert_eq!(info["listener"]["listening"], false);
    assert_eq!(info["listener"]["requestedPort"], 8001);
    assert_eq!(info["active"], false);
    assert_eq!(info["slice"], "akev-capture.slice");
    let linux = cfg!(target_os = "linux");
    assert_eq!(info["supported"], linux);
    assert_eq!(
        info["system"]["platform"],
        if linux {
            "linux"
        } else {
            info["system"]["platform"].as_str().unwrap()
        }
    );
    if linux {
        assert!(info["system"]["uid"].is_u64(), "{info}");
        assert!(info.get("unsupportedReason").is_none());
    }

    // The interceptor is listed on Linux and needs configuration.
    let interceptors = env.get("/api/interceptors").await;
    let card = interceptors
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "transparent-linux")
        .cloned();
    if !linux {
        assert!(card.is_none());
        return;
    }
    let card = card.expect("transparent-linux listed");
    assert_eq!(card["needsConfig"], true);
    assert_eq!(card["available"], true);
    assert_eq!(card["active"], false);
    assert_eq!(card["category"], "system");

    // Same port as the main proxy port: rejected.
    let settings = env.get("/api/settings").await;
    let main_port = settings["proxy"]["port"].clone();
    let (code, body) = env
        .set_transparent(json!({"enabled": true, "port": main_port, "mark": 0}))
        .await;
    assert_eq!(code, 400, "{body}");
    assert!(body.to_string().contains("must differ"), "{body}");

    // Turn the listener on (any free port): the settings response already reflects it.
    let (code, saved) = env
        .set_transparent(json!({"enabled": true, "port": 0, "mark": 0}))
        .await;
    assert_eq!(code, 200, "{saved}");
    assert_eq!(saved["proxy"]["transparent"]["enabled"], true);
    let info = env.get("/api/transparent").await;
    assert_eq!(info["listener"]["listening"], true, "{info}");
    let port = info["listener"]["port"].as_u64().unwrap() as u16;
    assert!(port > 0);
    assert_eq!(info["active"], false, "nothing redirected yet");

    // Simulate the kernel: connections on the transparent port were going to 127.0.0.2:<up>.
    let up = upstream().await;
    let original = SocketAddr::from(([127, 0, 0, 2], up.port()));
    env.handle.proxy().set_original_destination_lookup(Arc::new(
        move |_peer: SocketAddr, _local: SocketAddr| Ok(Some(original)),
    ));
    let mut tcp = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tcp.write_all(b"GET /redirected HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let mut response = Vec::new();
    tokio::time::timeout(TIMEOUT, tcp.read_to_end(&mut response))
        .await
        .expect("response in time")
        .unwrap();
    let response = String::from_utf8_lossy(&response);
    assert!(
        response.starts_with("HTTP/1.1 200") && response.ends_with("upstream"),
        "{response}"
    );

    // Recorded like any exchange, with `via: transparent` and the original port.
    let deadline = tokio::time::Instant::now() + TIMEOUT;
    let summary = loop {
        let list = env.get("/api/exchanges").await;
        if let Some(s) = list
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["path"] == "/redirected" && s["state"] == "complete")
        {
            break s.clone();
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "no exchange: {list}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    let id = match &summary["id"] {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    let detail = env.get(&format!("/api/exchanges/{id}")).await;
    assert_eq!(detail["client"]["via"], "transparent", "{detail}");
    assert_eq!(
        detail["request"]["url"],
        format!("http://127.0.0.1:{}/redirected", up.port())
    );

    // The check: redirection is detected and the connection is listed.
    let info = env.get("/api/transparent").await;
    assert_eq!(info["active"], true, "{info}");
    assert_eq!(info["listener"]["redirected"], 1);
    assert_eq!(
        info["listener"]["recent"][0]["originalDestination"],
        original.to_string()
    );
    assert!(info["listener"]["lastRedirectAt"].is_number());
    let interceptors = env.get("/api/interceptors").await;
    let card = interceptors
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == "transparent-linux")
        .unwrap();
    assert_eq!(card["active"], true);
    assert_eq!(card["metadata"]["listener"]["port"], port);

    // Commands for each mode.
    let (code, plan) = env
        .send(reqwest::Method::POST, "/api/transparent/plan", json!({}))
        .await;
    assert_eq!(code, 200, "{plan}");
    assert_eq!(plan["mode"], "app");
    assert_eq!(plan["port"], port);
    let rules = plan["rules"].as_str().unwrap();
    assert!(rules.contains("socket cgroupv2 level"), "{rules}");
    assert!(rules.contains(&format!("redirect to :{port}")), "{rules}");
    assert!(
        plan["launch"]["command"]
            .as_str()
            .unwrap()
            .contains("--slice=akev-capture.slice")
    );
    assert_eq!(
        plan["teardown"][0]["command"],
        "sudo nft delete table inet akev"
    );

    let (code, plan) = env
        .send(
            reqwest::Method::POST,
            "/api/transparent/plan",
            json!({"mode": "machine", "backend": "iptables", "ports": "all"}),
        )
        .await;
    assert_eq!(code, 200, "{plan}");
    assert!(
        plan["rules"]
            .as_str()
            .unwrap()
            .contains("-m owner --uid-owner"),
        "{plan}"
    );

    let (code, err) = env
        .send(
            reqwest::Method::POST,
            "/api/transparent/plan",
            json!({"mode": "user", "user": "no-such-user-akev"}),
        )
        .await;
    assert_eq!(code, 400, "{err}");
    assert!(err.to_string().contains("no user"), "{err}");
    let (code, _) = env
        .send(
            reqwest::Method::POST,
            "/api/transparent/plan",
            json!({"mode": "nope"}),
        )
        .await;
    assert_eq!(code, 400);

    // The interceptor hands out the same commands.
    let (code, activation) = env
        .send(
            reqwest::Method::POST,
            "/api/interceptors/transparent-linux/activate",
            json!({"options": {"mode": "machine"}}),
        )
        .await;
    assert_eq!(code, 200, "{activation}");
    assert_eq!(activation["success"], true);
    assert!(
        activation["metadata"]["rules"]
            .as_str()
            .unwrap()
            .contains("meta skuid")
    );

    // Turning the listener off closes the port.
    let (code, _) = env
        .set_transparent(json!({"enabled": false, "port": 0, "mark": 0}))
        .await;
    assert_eq!(code, 200);
    let info = env.get("/api/transparent").await;
    assert_eq!(info["listener"]["listening"], false);
    assert_eq!(info["active"], false);
    assert!(
        tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_err()
    );

    env.handle.shutdown().await;
}
