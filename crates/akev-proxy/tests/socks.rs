//! End-to-end: SOCKS4a / SOCKS5 tunnels (plain HTTP and TLS inside), username tagging.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::CaptureSettings;
use akev_core::*;
use common::*;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

fn no_rules() -> akev_core::rules::RuleSet {
    akev_core::rules::RuleSet::default()
}

/// SOCKS5 handshake for a domain target, optionally with username/password auth.
async fn socks5(proxy: &TestProxy, host: &str, port: u16, user: Option<&str>) -> TcpStream {
    let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
    match user {
        Some(_) => s.write_all(&[5, 2, 0, 2]).await.unwrap(),
        None => s.write_all(&[5, 1, 0]).await.unwrap(),
    }
    let mut choice = [0u8; 2];
    s.read_exact(&mut choice).await.unwrap();
    if let Some(u) = user {
        assert_eq!(choice, [5, 2]);
        let mut auth = vec![1, u.len() as u8];
        auth.extend_from_slice(u.as_bytes());
        auth.extend_from_slice(&[6]);
        auth.extend_from_slice(b"secret");
        s.write_all(&auth).await.unwrap();
        let mut ok = [0u8; 2];
        s.read_exact(&mut ok).await.unwrap();
        assert_eq!(ok, [1, 0]);
    } else {
        assert_eq!(choice, [5, 0]);
    }
    let mut req = vec![5, 1, 0, 3, host.len() as u8];
    req.extend_from_slice(host.as_bytes());
    req.extend_from_slice(&port.to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(&reply[..2], &[5, 0], "SOCKS5 CONNECT refused");
    s
}

#[tokio::test]
async fn socks5_tunnel_is_intercepted() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut s = socks5(&proxy, "localhost", upstream.port(), None).await;
    s.write_all(
        format!(
            "GET /hello HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
            upstream.port()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut s, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(client.via, ProxyMode::Socks);
    assert_eq!(
        head.url,
        format!("http://localhost:{}/hello", upstream.port())
    );
}

#[tokio::test]
async fn socks5_tls_inside_is_intercepted() {
    let upstream = spawn_tls_upstream().await;
    let capture = CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(
        no_rules(),
        Arc::new(NoBreakpoints),
        proxy_settings(),
        capture,
    )
    .await;
    let tcp = socks5(&proxy, "localhost", upstream.port(), Some("chrome-7")).await;
    // TLS through the SOCKS tunnel, trusting the Akev CA.
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
    let name = rustls_pki_types::ServerName::try_from("localhost").unwrap();
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tcp)
        .await
        .expect("MITM handshake");
    tls.write_all(
        format!(
            "GET /hello HTTP/1.1\r\nHost: localhost:{}\r\n\r\n",
            upstream.port()
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut tls, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, b"hello");
    let (_, events) = wait_exchange(&mut proxy, "/hello", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.scheme, "https");
    assert_eq!(client.via, ProxyMode::Socks);
    assert_eq!(client.interceptor, None);
    assert_eq!(
        client.tls.as_ref().and_then(|t| t.sni.as_deref()),
        Some("localhost")
    );
}

#[tokio::test]
async fn socks4a_and_socks5_ipv4_targets() {
    let upstream = spawn_http_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;

    // SOCKS4a: 0.0.0.1 + hostname.
    let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
    let mut req = vec![4, 1];
    req.extend_from_slice(&upstream.port().to_be_bytes());
    req.extend_from_slice(&[0, 0, 0, 1]);
    req.extend_from_slice(b"user4\0localhost\0");
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 8];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0x5A);
    s.write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let (_, body) = read_response(&mut s, &mut buf).await;
    assert_eq!(body, b"hello");

    // SOCKS5 with an IPv4 address: the URL still uses the Host header's name, with the
    // tunnel's port.
    let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut choice = [0u8; 2];
    s.read_exact(&mut choice).await.unwrap();
    let mut req = vec![5, 1, 0, 1, 127, 0, 0, 1];
    req.extend_from_slice(&upstream.port().to_be_bytes());
    s.write_all(&req).await.unwrap();
    let mut reply = [0u8; 10];
    s.read_exact(&mut reply).await.unwrap();
    assert_eq!(reply[1], 0);
    s.write_all(b"GET /echo HTTP/1.1\r\nHost: localhost:1234\r\n\r\n")
        .await
        .unwrap();
    let mut buf = Vec::new();
    let (head, body) = read_response(&mut s, &mut buf).await;
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    // The upstream still receives the client's Host header untouched.
    assert!(String::from_utf8_lossy(&body).contains("Host: localhost:1234\r\n"));
    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let ProxyEvent::RequestStarted { head, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(
        head.url,
        format!("http://localhost:{}/echo", upstream.port())
    );
}

#[tokio::test]
async fn socks_can_be_disabled() {
    let mut settings = proxy_settings();
    settings.socks = false;
    let capture = CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(no_rules(), Arc::new(NoBreakpoints), settings, capture).await;
    let mut s = TcpStream::connect(proxy.addr()).await.unwrap();
    s.write_all(&[5, 1, 0]).await.unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(std::time::Duration::from_secs(5), s.read_to_end(&mut out)).await;
    assert!(
        !out.starts_with(&[5, 0]),
        "SOCKS must not be accepted when disabled"
    );
    proxy
        .wait_for(std::time::Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::ClientError { .. })
        })
        .await;
}
