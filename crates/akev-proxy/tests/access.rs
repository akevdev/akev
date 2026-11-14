//! Who may use the proxy (`proxy.access`): client policy, blocked clients, proxy
//! credentials (HTTP 407, SOCKS5), loopback targets and PROXY protocol trust.
//!
//! Test clients connect over loopback, so clients on "other machines" are simulated with a
//! PROXY protocol header from a trusted load balancer: the proxy then applies the policy,
//! credentials and loopback-target rules to the address the header declares, exactly as it
//! does for the TCP peer address (`Shared::admit`). The test upstreams listen on loopback,
//! so tests about something else let devices reach it (`allow_loopback_targets`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::{
    CaptureSettings, ClientPolicy, DnsOverride, ProxyAccessSettings, ProxySettings,
};
use akev_core::*;
use base64::Engine;
use common::*;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

const LAN: &str = "192.168.1.23";

fn capture() -> CaptureSettings {
    CaptureSettings {
        detect_processes: false,
        ..Default::default()
    }
}

/// A proxy that trusts PROXY protocol headers (so tests can declare client addresses).
async fn proxy(access: ProxyAccessSettings) -> TestProxy {
    proxy_with(access, |_| {}).await
}

/// `access` letting devices reach the (loopback) test upstreams.
fn reachable(access: ProxyAccessSettings) -> ProxyAccessSettings {
    ProxyAccessSettings {
        allow_loopback_targets: true,
        ..access
    }
}

async fn proxy_with(access: ProxyAccessSettings, f: impl FnOnce(&mut ProxySettings)) -> TestProxy {
    let mut settings = proxy_settings();
    settings.access = ProxyAccessSettings {
        trust_proxy_protocol: true,
        ..access
    };
    f(&mut settings);
    start_proxy_with(
        rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
        settings,
        capture(),
    )
    .await
}

/// Connect "from" `client` (a PROXY v1 header), ready to speak HTTP / SOCKS.
async fn connect_as(proxy: &TestProxy, client: &str) -> TcpStream {
    let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
    let family = if client.contains(':') { "TCP6" } else { "TCP4" };
    let dst = if client.contains(':') {
        "::1"
    } else {
        "127.0.0.1"
    };
    s.write_all(
        format!(
            "PROXY {family} {client} {dst} 50123 {}\r\n",
            proxy.addr().port()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    s
}

/// Send `request`, read the whole answer until the proxy closes (or 5 s).
async fn exchange(mut s: TcpStream, request: &str) -> String {
    s.write_all(request.as_bytes()).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    String::from_utf8_lossy(&out).into_owned()
}

fn get(url: &str, extra: &str) -> String {
    format!("GET {url} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n{extra}\r\n")
}

fn basic(user: &str, password: &str) -> String {
    format!(
        "Proxy-Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"))
    )
}

fn credentials() -> ProxyAccessSettings {
    ProxyAccessSettings {
        username: "phone".into(),
        password: "s3cret".into(),
        ..Default::default()
    }
}

#[tokio::test]
async fn lan_clients_are_admitted_by_default_and_public_ones_refused() {
    let upstream = spawn_http_upstream().await;
    let p = proxy(reachable(ProxyAccessSettings::default())).await;
    let url = format!("http://{upstream}/hello");
    let seen: Arc<Mutex<Vec<IpAddr>>> = Arc::default();
    let observed = seen.clone();
    p.handle
        .set_client_observer(Arc::new(move |ip| observed.lock().unwrap().push(ip)));

    let ok = exchange(connect_as(&p, LAN).await, &get(&url, "")).await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    assert_eq!(*seen.lock().unwrap(), [LAN.parse::<IpAddr>().unwrap()]);

    // A public address is outside the default `private` policy: closed without a response.
    let refused = exchange(connect_as(&p, "203.0.113.9").await, &get(&url, "")).await;
    assert_eq!(refused, "", "no response at all");
    assert_eq!(p.handle.stats().refused_connections, 1);
    // Loopback is always admitted and never reported as a new device.
    let local = exchange(connect_as(&p, "127.0.0.1").await, &get(&url, "")).await;
    assert!(local.starts_with("HTTP/1.1 200"), "{local}");
    assert_eq!(seen.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn allowlist_and_blocklist() {
    let upstream = spawn_http_upstream().await;
    let p = proxy(reachable(ProxyAccessSettings {
        policy: ClientPolicy::Allowlist,
        allow: vec!["192.168.1.0/28".into(), "fd00::7".into()],
        block: vec!["192.168.1.5".into()],
        ..Default::default()
    }))
    .await;
    let url = format!("http://{upstream}/hello");
    for (client, admitted) in [
        (LAN, false),
        ("192.168.1.2", true),
        ("192.168.1.5", false),
        ("fd00::7", true),
        ("10.0.0.1", false),
    ] {
        let out = exchange(connect_as(&p, client).await, &get(&url, "")).await;
        assert_eq!(
            out.starts_with("HTTP/1.1 200"),
            admitted,
            "{client}: {out:?}"
        );
    }
    assert_eq!(p.handle.stats().refused_connections, 3);
}

#[tokio::test]
async fn blocking_a_client_cuts_its_open_connections() {
    let upstream = spawn_http_upstream().await;
    let mut settings = proxy_settings();
    settings.access.trust_proxy_protocol = true;
    settings.access.allow_loopback_targets = true;
    let p = start_proxy_with(
        rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
        settings.clone(),
        capture(),
    )
    .await;
    // A CONNECT tunnel from the device stays open...
    let mut s = connect_as(&p, LAN).await;
    s.write_all(format!("CONNECT {upstream} HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut reply = [0u8; 64];
    let n = s.read(&mut reply).await.unwrap();
    assert!(String::from_utf8_lossy(&reply[..n]).starts_with("HTTP/1.1 200"));
    // ...until the user blocks the device.
    settings.access.block = vec![LAN.into()];
    p.handle.update_settings(settings, capture());
    let mut rest = Vec::new();
    let closed = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut rest)).await;
    assert!(closed.is_ok(), "the tunnel was closed");
}

#[tokio::test]
async fn proxy_credentials_http_407_flow() {
    let upstream = spawn_http_upstream().await;
    let p = proxy(reachable(credentials())).await;
    let url = format!("http://{upstream}/hello");

    let challenge = exchange(connect_as(&p, LAN).await, &get(&url, "")).await;
    assert!(challenge.starts_with("HTTP/1.1 407"), "{challenge}");
    assert!(
        challenge.contains("Proxy-Authenticate: Basic realm=\"Akev\""),
        "{challenge}"
    );
    let wrong = exchange(
        connect_as(&p, LAN).await,
        &get(&url, &basic("phone", "guess")),
    )
    .await;
    assert!(wrong.starts_with("HTTP/1.1 407"), "{wrong}");
    let ok = exchange(
        connect_as(&p, LAN).await,
        &get(&url, &basic("phone", "s3cret")),
    )
    .await;
    assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
    // The upstream never sees the credentials.
    assert!(
        !ok.to_ascii_lowercase().contains("proxy-authorization"),
        "{ok}"
    );

    // CONNECT too.
    let tunnel = exchange(
        connect_as(&p, LAN).await,
        &format!("CONNECT {upstream} HTTP/1.1\r\nHost: {upstream}\r\n\r\n"),
    )
    .await;
    assert!(tunnel.starts_with("HTTP/1.1 407"), "{tunnel}");
    let mut s = connect_as(&p, LAN).await;
    s.write_all(
        format!(
            "CONNECT {upstream} HTTP/1.1\r\nHost: {upstream}\r\n{}\r\n",
            basic("phone", "s3cret")
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut reply = [0u8; 64];
    let n = s.read(&mut reply).await.unwrap();
    assert!(String::from_utf8_lossy(&reply[..n]).starts_with("HTTP/1.1 200"));

    // TLS straight to the port can't authenticate: closed.
    let mut s = connect_as(&p, LAN).await;
    s.write_all(&[0x16, 0x03, 0x01, 0x00, 0x05, 0x01, 0, 0, 1, 0])
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(5), s.read_to_end(&mut out)).await;
    assert!(out.is_empty());

    // The device setup page needs no credentials (fetched before the device is set up).
    let setup = exchange(connect_as(&p, LAN).await, &get("http://akev.cert/", "")).await;
    assert!(setup.starts_with("HTTP/1.1 200"), "{setup}");

    // This machine needs no credentials.
    let local = exchange(connect_as(&p, "127.0.0.1").await, &get(&url, "")).await;
    assert!(local.starts_with("HTTP/1.1 200"), "{local}");
    assert!(p.handle.stats().auth_failures >= 4);
}

#[tokio::test]
async fn proxy_credentials_socks5() {
    let upstream = spawn_http_upstream().await;
    let p = proxy(reachable(credentials())).await;
    let socks = |user: &'static str, password: &'static str| {
        let p = &p;
        async move {
            let mut s = connect_as(p, LAN).await;
            s.write_all(&[5, 2, 0, 2]).await.unwrap();
            let mut choice = [0u8; 2];
            s.read_exact(&mut choice).await.unwrap();
            assert_eq!(choice, [5, 2], "username/password is required");
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(password.len() as u8);
            auth.extend_from_slice(password.as_bytes());
            s.write_all(&auth).await.unwrap();
            let mut status = [0u8; 2];
            s.read_exact(&mut status).await.unwrap();
            (s, status[1])
        }
    };
    let (_, status) = socks("phone", "guess").await;
    assert_eq!(status, 1, "wrong password refused");
    let (mut s, status) = socks("phone", "s3cret").await;
    assert_eq!(status, 0);
    let std::net::SocketAddr::V4(v4) = upstream else {
        panic!("IPv4 upstream")
    };
    let mut req = vec![5, 1, 0, 1];
    req.extend_from_slice(&v4.ip().octets());
    req.extend_from_slice(&v4.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0, "connected");

    // No-auth only is refused when credentials are required.
    let mut s = connect_as(&p, LAN).await;
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut choice = [0u8; 2];
    s.read_exact(&mut choice).await.unwrap();
    assert_eq!(choice, [5, 0xFF]);
}

#[tokio::test]
async fn lan_clients_cant_reach_this_machines_loopback() {
    // The upstream listens on 127.0.0.1, like a local-only dev server or database.
    let upstream = spawn_http_upstream().await;
    let port = upstream.port();
    let p = proxy_with(ProxyAccessSettings::default(), |s| {
        s.dns_overrides = vec![DnsOverride {
            host: "rebind.test".into(),
            address: "127.0.0.1".into(),
        }];
    })
    .await;
    for url in [
        format!("http://127.0.0.1:{port}/"),
        format!("http://localhost:{port}/"),
        format!("http://0.0.0.0:{port}/"),
        format!("http://[::1]:{port}/"),
        // A name that resolves to loopback (DNS rebinding).
        format!("http://rebind.test:{port}/"),
    ] {
        let out = exchange(connect_as(&p, LAN).await, &get(&url, "")).await;
        assert!(out.starts_with("HTTP/1.1 502"), "{url}: {out}");
        assert!(
            out.contains("LOOPBACK_TARGET_REFUSED") || out.contains("this computer's own address"),
            "{url}: {out}"
        );
    }
    // CONNECT tunnels too (the tunnel opens, the upstream connection is refused).
    let mut s = connect_as(&p, LAN).await;
    s.write_all(format!("CONNECT 127.0.0.1:{port} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut reply = [0u8; 64];
    let n = s.read(&mut reply).await.unwrap();
    assert!(String::from_utf8_lossy(&reply[..n]).starts_with("HTTP/1.1 200"));
    let tunnelled = exchange(s, &get("/", "")).await;
    assert!(!tunnelled.contains("HTTP/1.1 200"), "{tunnelled}");

    // This machine may.
    let local = exchange(
        connect_as(&p, "127.0.0.1").await,
        &get(&format!("http://127.0.0.1:{port}/hello"), ""),
    )
    .await;
    assert!(local.starts_with("HTTP/1.1 200"), "{local}");

    // And so may devices once the user allows it.
    let open = proxy(ProxyAccessSettings {
        allow_loopback_targets: true,
        ..Default::default()
    })
    .await;
    let out = exchange(
        connect_as(&open, LAN).await,
        &get(&format!("http://127.0.0.1:{port}/hello"), ""),
    )
    .await;
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
}

#[tokio::test]
async fn a_forward_rule_to_localhost_still_works_for_devices() {
    let upstream = spawn_http_upstream().await;
    let rules = rules(&format!(
        r#"{{"version":1,"items":[{{"type":"rule","id":"fwd","enabled":true,"matchers":[],
            "action":{{"type":"forward","target":"http://127.0.0.1:{}"}}}}]}}"#,
        upstream.port()
    ));
    let mut settings = proxy_settings();
    settings.access.trust_proxy_protocol = true;
    let p = start_proxy_with(rules, Arc::new(NoBreakpoints), settings, capture()).await;
    let out = exchange(
        connect_as(&p, LAN).await,
        &get("http://api.example.test/hello", ""),
    )
    .await;
    assert!(
        out.starts_with("HTTP/1.1 200"),
        "the user chose that target: {out}"
    );
}

#[tokio::test]
async fn proxy_protocol_is_ignored_unless_trusted() {
    let upstream = spawn_http_upstream().await;
    let url = format!("http://{upstream}/hello");
    // Default settings: the header is garbage, the request behind it is never proxied.
    let mut p = start_proxy(rules::RuleSet::default(), Arc::new(NoBreakpoints)).await;
    let out = exchange(connect_as(&p, LAN).await, &get(&url, "")).await;
    assert!(out.starts_with("HTTP/1.1 400"), "{out}");
    let events = p.drain(Duration::from_millis(300)).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ClientError { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RequestStarted { .. }))
    );

    // Trusted: the declared address is the client.
    let mut trusted = proxy(reachable(ProxyAccessSettings::default())).await;
    let out = exchange(connect_as(&trusted, LAN).await, &get(&url, "")).await;
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    let events = trusted.drain(Duration::from_millis(300)).await;
    let client = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestStarted { client, .. } => Some(client.remote_addr.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(client, format!("{LAN}:50123"));
}
