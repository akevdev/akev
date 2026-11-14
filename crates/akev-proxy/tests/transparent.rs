//! End-to-end: the transparent capture listener. Kernel redirects (nftables `REDIRECT`) need
//! root, so the original destination is injected through
//! `ProxyHandle::set_original_destination_lookup`: clients connect to the transparent port and
//! the lookup answers "this connection was really going to <upstream>", exactly as conntrack
//! would. Everything after the lookup — sniffing, TLS interception by SNI, passthrough, raw
//! tunnels, loop protection, status — is the production code path.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::{CaptureSettings, DnsOverride, ProxySettings};
use akev_core::*;
use common::*;
use parking_lot::Mutex;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Where the next transparent connections were "really" going (`None` = not redirected).
type Redirect = Arc<Mutex<Option<SocketAddr>>>;

fn settings(overrides: &[&str]) -> ProxySettings {
    let mut s = proxy_settings();
    s.transparent.enabled = true;
    s.transparent.port = 0;
    s.dns_overrides = overrides
        .iter()
        .map(|h| DnsOverride {
            host: (*h).to_string(),
            address: "127.0.0.1".into(),
        })
        .collect();
    s
}

async fn start(settings: ProxySettings) -> (TestProxy, Redirect, SocketAddr) {
    let proxy = start_proxy_with(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
        settings,
        CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
    )
    .await;
    let redirect: Redirect = Arc::default();
    let r = redirect.clone();
    proxy.handle.set_original_destination_lookup(Arc::new(
        move |_peer: SocketAddr, _local: SocketAddr| Ok(*r.lock()),
    ));
    let status = proxy.handle.transparent_status();
    assert!(status.listening, "{status:?}");
    let port = status.port.expect("port");
    assert!(
        status
            .addresses
            .iter()
            .any(|a| a == &format!("127.0.0.1:{port}")),
        "{status:?}"
    );
    (proxy, redirect, SocketAddr::from(([127, 0, 0, 1], port)))
}

fn started(events: &[ProxyEvent]) -> (&RequestHead, &ClientInfo) {
    events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestStarted { head, client, .. } => Some((head, client)),
            _ => None,
        })
        .expect("RequestStarted")
}

/// reqwest client that sends `host` to the transparent listener (like a redirected app: it
/// resolved `host` itself, then the kernel moved the connection) and trusts Akev's CA.
fn transparent_client(
    proxy: &TestProxy,
    host: &str,
    listener: SocketAddr,
) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .resolve(host, listener)
        .add_root_certificate(
            reqwest::Certificate::from_pem(proxy.ca.cert_pem().as_bytes()).unwrap(),
        )
        .tls_built_in_root_certs(false)
        .timeout(Duration::from_secs(10))
}

#[tokio::test]
async fn redirected_http_uses_host_header_and_original_port() {
    let upstream = spawn_http_upstream().await;
    let (mut proxy, redirect, listener) = start(settings(&["plain.test"])).await;
    // The app connected to plain.test:<upstream port>; conntrack says so.
    *redirect.lock() = Some(SocketAddr::from(([127, 0, 0, 2], upstream.port())));
    let mut tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
    tcp.write_all(b"GET /hello?x=1 HTTP/1.1\r\nHost: plain.test\r\nUser-Agent: app\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");

    let (_, events) = wait_exchange(&mut proxy, "/hello?x=1", is_completed).await;
    let (head, client) = started(&events);
    // Host from the Host header, port from the original destination.
    assert_eq!(
        head.url,
        format!("http://plain.test:{}/hello?x=1", upstream.port())
    );
    assert_eq!(client.via, ProxyMode::Transparent);
    assert!(events.iter().any(|e| matches!(
        e,
        ProxyEvent::ResponseStarted { head, .. } if head.status == 200
    )));

    let status = proxy.handle.transparent_status();
    assert_eq!((status.redirected, status.direct, status.loops), (1, 0, 0));
    assert!(status.last_redirect_at.is_some());
    assert!(status.redirect_active(akev_core::transparent::now_ms()));
    assert_eq!(
        status.recent[0].original_destination.as_deref(),
        Some(format!("127.0.0.2:{}", upstream.port()).as_str())
    );
    assert!(status.recent[0].client.starts_with("127.0.0.1:"));
}

#[tokio::test]
async fn redirected_https_is_intercepted_with_the_sni_host() {
    let upstream = spawn_tls_upstream().await;
    let (mut proxy, redirect, listener) = start(settings(&["secure.test"])).await;
    *redirect.lock() = Some(SocketAddr::from(([127, 0, 0, 2], upstream.port())));
    // Validates Akev's leaf for secure.test against Akev's CA: the MITM used the SNI name.
    let http = transparent_client(&proxy, "secure.test", listener)
        .http1_only()
        .build()
        .unwrap();
    let res = http
        .post(format!("https://secure.test:{}/echo", listener.port()))
        .header("X-Token", "abc")
        .body("payload")
        .send()
        .await
        .expect("intercepted HTTPS request");
    assert_eq!(res.status(), 200);
    let echoed = res.text().await.unwrap();
    assert!(
        echoed.contains("x-token: abc") && echoed.ends_with("payload"),
        "{echoed}"
    );

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let (head, client) = started(&events);
    assert_eq!(
        head.url,
        format!("https://secure.test:{}/echo", upstream.port())
    );
    assert_eq!(head.http_version, HttpVersion::Http11);
    assert_eq!(client.via, ProxyMode::Transparent);
    let tls = client.tls.as_ref().expect("TLS intercepted");
    assert_eq!(tls.sni.as_deref(), Some("secure.test"));
    assert_eq!(proxy.handle.transparent_status().redirected, 1);
}

#[tokio::test]
async fn redirected_http2_over_tls_is_intercepted() {
    let upstream = spawn_tls_upstream().await;
    let (mut proxy, redirect, listener) = start(settings(&["h2.test"])).await;
    *redirect.lock() = Some(SocketAddr::from(([127, 0, 0, 2], upstream.port())));
    let http = transparent_client(&proxy, "h2.test", listener)
        .build()
        .unwrap();
    let res = http
        .get(format!("https://h2.test:{}/hello", listener.port()))
        .send()
        .await
        .expect("h2 request");
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
    assert_eq!(res.text().await.unwrap(), "hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let (head, client) = started(&events);
    assert_eq!(head.http_version, HttpVersion::H2);
    assert_eq!(
        head.url,
        format!("https://h2.test:{}/hello", upstream.port())
    );
    assert_eq!(client.via, ProxyMode::Transparent);
}

#[tokio::test]
async fn tls_without_sni_uses_the_original_ip() {
    let upstream = spawn_tls_upstream().await;
    let (mut proxy, redirect, listener) = start(settings(&[])).await;
    *redirect.lock() = Some(upstream);
    let tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls_pki_types::CertificateDer::from(
            proxy.ca.cert_der().to_vec(),
        ))
        .unwrap();
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    // IP server name: no SNI; Akev's certificate must carry the original IP.
    let name = rustls_pki_types::ServerName::IpAddress(upstream.ip().into());
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tcp)
        .await
        .expect("handshake without SNI");
    tls.write_all(b"GET /hello HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tls, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let (head, client) = started(&events);
    assert_eq!(
        head.url,
        format!("https://127.0.0.1:{}/hello", upstream.port())
    );
    assert!(client.tls.as_ref().unwrap().sni.is_none());
    assert_eq!(client.via, ProxyMode::Transparent);
}

#[tokio::test]
async fn tls_passthrough_tunnels_to_the_original_destination() {
    let upstream = spawn_tls_upstream().await;
    let mut s = settings(&[]);
    s.tls_passthrough = vec!["pinned.test".into()];
    let (mut proxy, redirect, listener) = start(s).await;
    *redirect.lock() = Some(upstream);
    // The upstream's own certificate reaches the client: nothing was intercepted.
    let http = reqwest::Client::builder()
        .no_proxy()
        .resolve("pinned.test", listener)
        .danger_accept_invalid_certs(true)
        .http1_only()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let res = http
        .get(format!("https://pinned.test:{}/hello", listener.port()))
        .send()
        .await
        .expect("tunnelled request");
    assert_eq!(res.text().await.unwrap(), "hello");
    drop(http);
    let seen = proxy
        .wait_for(Duration::from_secs(10), |e| {
            matches!(e, ProxyEvent::TunnelClosed { .. })
        })
        .await;
    let (host, port, reason, via) = seen
        .iter()
        .find_map(|e| match e {
            ProxyEvent::TunnelOpened {
                host,
                port,
                reason,
                client,
                ..
            } => Some((host.clone(), *port, reason.clone(), client.via)),
            _ => None,
        })
        .expect("TunnelOpened");
    // Piped to the original destination itself (no DNS lookup of the SNI name).
    assert_eq!((host.as_str(), port), ("127.0.0.1", upstream.port()));
    assert!(reason.contains("tls-passthrough"), "{reason}");
    assert_eq!(via, ProxyMode::Transparent);
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, ProxyEvent::RequestStarted { .. }))
    );
}

#[tokio::test]
async fn other_protocols_are_piped_to_the_original_destination() {
    // A binary, client-speaks-first protocol: echoed back by the "real" server.
    let echo = spawn_raw(|mut s| async move {
        let mut buf = [0u8; 1024];
        while let Ok(n) = s.read(&mut buf).await {
            if n == 0 || s.write_all(&buf[..n]).await.is_err() {
                break;
            }
        }
    })
    .await;
    let (mut proxy, redirect, listener) = start(settings(&[])).await;
    *redirect.lock() = Some(echo);
    let mut tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
    tcp.write_all(&[0x00, 0x01, 0xfe, b'b', b'i', b'n'])
        .await
        .unwrap();
    let mut got = [0u8; 6];
    tokio::time::timeout(Duration::from_secs(10), tcp.read_exact(&mut got))
        .await
        .expect("echo in time")
        .unwrap();
    assert_eq!(got, [0x00, 0x01, 0xfe, b'b', b'i', b'n']);
    drop(tcp);
    let seen = proxy
        .wait_for(Duration::from_secs(10), |e| {
            matches!(e, ProxyEvent::TunnelClosed { .. })
        })
        .await;
    let opened = seen
        .iter()
        .find_map(|e| match e {
            ProxyEvent::TunnelOpened {
                host, port, client, ..
            } => Some((host.clone(), *port, client.via)),
            _ => None,
        })
        .expect("TunnelOpened");
    assert_eq!(
        opened,
        ("127.0.0.1".to_string(), echo.port(), ProxyMode::Transparent)
    );
}

#[tokio::test]
async fn unredirected_connections_are_handled_like_direct_requests() {
    let upstream = spawn_http_upstream().await;
    let (mut proxy, _redirect, listener) = start(settings(&["direct.test"])).await;
    // No original destination: Host decides, as on the main port.
    let req = format!(
        "GET /hello HTTP/1.1\r\nHost: direct.test:{}\r\n\r\n",
        upstream.port()
    );
    let mut tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
    tcp.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let (head, client) = started(&events);
    assert_eq!(
        head.url,
        format!("http://direct.test:{}/hello", upstream.port())
    );
    assert_eq!(client.via, ProxyMode::Direct);
    let status = proxy.handle.transparent_status();
    assert_eq!((status.redirected, status.direct), (0, 1));
    assert!(status.last_redirect_at.is_none());
    assert!(status.recent[0].original_destination.is_none());
}

#[tokio::test]
async fn own_upstream_connections_coming_back_are_refused() {
    // Redirect rules that forget to exclude Akev: its upstream connection to the original
    // destination is redirected straight back to the transparent port.
    let (proxy, redirect, listener) = start(settings(&["loop.test"])).await;
    *redirect.lock() = Some(SocketAddr::from(([127, 0, 0, 2], listener.port())));
    let mut tcp = tokio::net::TcpStream::connect(listener).await.unwrap();
    tcp.write_all(b"GET /loop HTTP/1.1\r\nHost: loop.test\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let (head, _) = read_response(&mut tcp, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 502"), "{head}");
    let status = proxy.handle.transparent_status();
    // (The upstream client may retry once on a fresh connection.)
    assert!((1..=2).contains(&status.loops), "{status:?}");
    assert_eq!(
        status.redirected, 1,
        "Akev's own connections are never recorded"
    );
}

#[tokio::test]
async fn listener_follows_settings_live() {
    let (proxy, _redirect, listener) = start(settings(&[])).await;
    let handle = proxy.handle.clone();
    let capture = CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };

    // Disable: the port closes.
    let mut s = settings(&[]);
    s.transparent.enabled = false;
    handle.update_settings(s.clone(), capture.clone());
    let status = handle.sync_transparent().await;
    assert!(
        !status.listening && !status.enabled && status.port.is_none(),
        "{status:?}"
    );
    assert!(tokio::net::TcpStream::connect(listener).await.is_err());

    // Re-enable on a specific free port.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let wanted = free.local_addr().unwrap().port();
    drop(free);
    s.transparent.enabled = true;
    s.transparent.port = wanted;
    handle.update_settings(s.clone(), capture.clone());
    let status = handle.sync_transparent().await;
    assert!(status.listening, "{status:?}");
    assert_eq!(status.requested_port, wanted);
    let port = status.port.unwrap();
    assert!(
        port >= wanted && port <= wanted.saturating_add(100),
        "{port}"
    );
    tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("listening on the new port");

    // Same settings again: the listener is kept as is.
    handle.update_settings(s.clone(), capture.clone());
    assert_eq!(handle.sync_transparent().await.port, Some(port));

    // A port that is taken by something else: the next free one is used and reported.
    let squatter = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let taken = squatter.local_addr().unwrap().port();
    if taken < u16::MAX - 100 {
        s.transparent.port = taken;
        handle.update_settings(s.clone(), capture.clone());
        let status = handle.sync_transparent().await;
        assert_eq!(status.requested_port, taken);
        assert!(status.port.is_some_and(|p| p != taken), "{status:?}");
    }

    // The mark is reported (and explained when Akev can't set it).
    s.transparent.mark = 0x2a;
    handle.update_settings(s.clone(), capture.clone());
    let status = handle.sync_transparent().await;
    assert_eq!(status.mark, 0x2a);
    #[cfg(not(target_os = "linux"))]
    assert!(status.mark_error.is_some());
    s.transparent.mark = 0;
    handle.update_settings(s, capture);
    assert_eq!(handle.sync_transparent().await.mark_error, None);

    // Shutdown closes it.
    handle.shutdown().await;
    let status = handle.transparent_status();
    assert!(!status.listening, "{status:?}");
}

#[tokio::test]
async fn disabled_by_default() {
    let proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let status = proxy.handle.transparent_status();
    assert!(!status.enabled && !status.listening, "{status:?}");
    assert_eq!(status.requested_port, 8001);
    assert_eq!(status.supported, cfg!(target_os = "linux"));
}
