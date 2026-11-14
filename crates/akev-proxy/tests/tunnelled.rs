//! `ProxyHandle::serve_tunnelled`: connections handed over with a known original destination
//! (WireGuard mode) are served like the inside of a CONNECT tunnel.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_proxy::TunnelledConnection;
use common::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn hand_over(
    proxy: &TestProxy,
    destination: std::net::SocketAddr,
) -> (tokio::io::DuplexStream, Arc<AtomicBool>) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let reset = Arc::new(AtomicBool::new(false));
    assert!(proxy.handle.serve_tunnelled(TunnelledConnection {
        stream: Box::new(server),
        client: "10.0.0.2:40000".parse().unwrap(),
        destination,
        via: ProxyMode::Wireguard,
        interceptor: Some("wireguard".into()),
        reset: reset.clone(),
    }));
    (client, reset)
}

#[tokio::test]
async fn http_with_known_destination() {
    let mut proxy = start_proxy(Default::default(), Arc::new(NoBreakpoints)).await;
    let upstream = spawn_http_upstream().await;
    let (mut client, _) = hand_over(&proxy, upstream);
    // No Host header: only the original destination says where this goes.
    client
        .write_all(b"GET /hello HTTP/1.0\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut out))
        .await
        .unwrap()
        .unwrap();
    let text = String::from_utf8_lossy(&out);
    assert!(
        text.starts_with("HTTP/1.0 200") || text.starts_with("HTTP/1.1 200"),
        "{text}"
    );
    assert!(text.ends_with("hello"));
    let events = proxy
        .wait_for(Duration::from_secs(10), |e| {
            matches!(e, ProxyEvent::RequestStarted { .. })
        })
        .await;
    let Some(ProxyEvent::RequestStarted { head, client, .. }) = events.last() else {
        unreachable!()
    };
    assert_eq!(head.url, format!("http://{upstream}/hello"));
    assert_eq!(client.via, ProxyMode::Wireguard);
    assert_eq!(client.interceptor.as_deref(), Some("wireguard"));
    assert_eq!(client.remote_addr, "10.0.0.2:40000");
    assert!(client.process.is_none());
    assert_eq!(proxy.handle.stats().total_connections, 1);
}

#[tokio::test]
async fn tls_is_intercepted_by_sni_on_the_original_port() {
    let mut proxy = start_proxy(Default::default(), Arc::new(NoBreakpoints)).await;
    let upstream = spawn_tls_upstream().await;
    let (client, _) = hand_over(&proxy, upstream);
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
        .connect(name, client)
        .await
        .expect("trusts the Akev CA");
    tls.write_all(b"GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), tls.read_to_end(&mut out)).await;
    assert!(String::from_utf8_lossy(&out).ends_with("hello"));
    let events = proxy
        .wait_for(Duration::from_secs(10), |e| {
            matches!(e, ProxyEvent::RequestStarted { .. })
        })
        .await;
    let Some(ProxyEvent::RequestStarted { head, client, .. }) = events.last() else {
        unreachable!()
    };
    assert_eq!(
        head.url,
        format!("https://localhost:{}/hello", upstream.port())
    );
    assert_eq!(client.via, ProxyMode::Wireguard);
    assert_eq!(
        client.tls.as_ref().and_then(|t| t.sni.as_deref()),
        Some("localhost")
    );
}

#[tokio::test]
async fn reset_rules_set_the_reset_flag_and_shutdown_refuses() {
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"reset","matchers":[{"type":"path","pattern":{"op":"equals","value":"/reset"}}],"action":{"type":"reset"}}]}"#,
    );
    let proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let upstream = spawn_http_upstream().await;
    let (mut client, reset) = hand_over(&proxy, upstream);
    client
        .write_all(format!("GET /reset HTTP/1.1\r\nHost: {upstream}\r\n\r\n").as_bytes())
        .await
        .unwrap();
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(10), client.read_to_end(&mut out)).await;
    assert!(out.is_empty());
    assert!(
        reset.load(Ordering::Relaxed),
        "the stream owner is told to send a RST"
    );
    proxy.handle.shutdown().await;
    let (_c, server) = tokio::io::duplex(16);
    assert!(!proxy.handle.serve_tunnelled(TunnelledConnection {
        stream: Box::new(server),
        client: "10.0.0.2:1".parse().unwrap(),
        destination: upstream,
        via: ProxyMode::Wireguard,
        interceptor: None,
        reset: Arc::default(),
    }));
}
