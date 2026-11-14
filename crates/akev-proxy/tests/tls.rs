//! End-to-end: HTTPS interception through CONNECT (HTTP/1.1 and HTTP/2 to the client), h2c
//! prior knowledge, TLS fingerprints, client-side certificate rejection and TLS passthrough.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use common::*;
use std::sync::Arc;
use std::time::Duration;

fn no_rules() -> akev_core::rules::RuleSet {
    akev_core::rules::RuleSet::default()
}

fn trusting_client(proxy: &TestProxy) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .add_root_certificate(
            reqwest::Certificate::from_pem(proxy.ca.cert_pem().as_bytes()).unwrap(),
        )
        .tls_built_in_root_certs(false)
        .timeout(Duration::from_secs(10))
}

#[tokio::test]
async fn https_via_connect_is_intercepted() {
    let upstream = spawn_tls_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let http = trusting_client(&proxy).http1_only().build().unwrap();
    let url = format!("https://localhost:{}/echo", upstream.port());
    let res = http
        .post(&url)
        .header("X-Secret", "s3cr3t")
        .body("tls body")
        .send()
        .await
        .expect("intercepted request");
    assert_eq!(res.status(), 200);
    let echoed = res.text().await.unwrap();
    // (hyper lowercases header names on the wire)
    assert!(
        echoed.contains("x-secret: s3cr3t") && echoed.ends_with("\n---\ntls body"),
        "{echoed}"
    );

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.url, url);
    assert_eq!(
        (head.scheme.as_str(), head.host.as_str(), head.port),
        ("https", "localhost", upstream.port())
    );
    assert_eq!(head.http_version, HttpVersion::Http11);
    assert_eq!(client.via, ProxyMode::Connect);
    let tls = client.tls.as_ref().expect("TLS info recorded");
    assert_eq!(tls.sni.as_deref(), Some("localhost"));
    assert_eq!(tls.alpn.as_deref(), Some("http/1.1"));
    assert_eq!(tls.alpn_offered, vec!["http/1.1".to_string()]);
    assert!(
        tls.version
            .as_deref()
            .is_some_and(|v| v.starts_with("TLSv1.")),
        "{tls:?}"
    );
    assert!(tls.cipher.is_some());
    assert_eq!(tls.ja3.as_ref().map(String::len), Some(32));
    let ja4 = tls.ja4.as_deref().unwrap_or_default();
    assert_eq!(ja4.len(), 36, "{ja4}");
    assert!(ja4.starts_with("t13d") && ja4[8..10] == *"h1", "{ja4}");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ResponseCompleted { .. }))
    );
}

#[tokio::test]
async fn http2_is_negotiated_with_the_client() {
    let upstream = spawn_tls_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let http = trusting_client(&proxy).build().unwrap();
    let res = http
        .get(format!("https://localhost:{}/hello", upstream.port()))
        .send()
        .await
        .expect("h2 request");
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "hello");
    // Several streams multiplexed over the same connection.
    let (a, b) = tokio::join!(
        http.get(format!("https://localhost:{}/hello", upstream.port()))
            .send(),
        http.post(format!("https://localhost:{}/echo", upstream.port()))
            .body("over h2")
            .send()
    );
    assert_eq!(a.unwrap().text().await.unwrap(), "hello");
    let echoed = b.unwrap().text().await.unwrap();
    // The HTTP/1 upstream gets a Host header derived from :authority and a framed body.
    assert!(
        echoed.contains(&format!("host: localhost:{}", upstream.port())),
        "{echoed}"
    );
    assert!(echoed.ends_with("\n---\nover h2"), "{echoed}");

    let (_, events) = wait_exchange(&mut proxy, "/echo", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.http_version, HttpVersion::H2);
    assert_eq!(head.scheme, "https");
    assert_eq!(
        client.tls.as_ref().and_then(|t| t.alpn.as_deref()),
        Some("h2")
    );
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ResponseCompleted { .. }))
    );
}

#[tokio::test]
async fn http2_can_be_disabled() {
    let upstream = spawn_tls_upstream().await;
    let mut settings = proxy_settings();
    settings.http2 = false;
    let capture = akev_core::settings::CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };
    let proxy = start_proxy_with(no_rules(), Arc::new(NoBreakpoints), settings, capture).await;
    let http = trusting_client(&proxy).build().unwrap();
    let res = http
        .get(format!("https://localhost:{}/hello", upstream.port()))
        .send()
        .await
        .unwrap();
    assert_eq!(res.version(), reqwest::Version::HTTP_11);
    assert_eq!(res.text().await.unwrap(), "hello");
}

#[tokio::test]
async fn h2c_prior_knowledge_works() {
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let http = reqwest::Client::builder()
        .no_proxy()
        .http2_prior_knowledge()
        .build()
        .unwrap();
    let res = http
        .get(format!("http://{}/ca.pem", proxy.addr()))
        .send()
        .await
        .expect("h2c request");
    assert_eq!(res.version(), reqwest::Version::HTTP_2);
    assert_eq!(res.text().await.unwrap(), proxy.ca.cert_pem());
    let (_, events) = wait_exchange(&mut proxy, "/ca.pem", is_completed).await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.http_version, HttpVersion::H2);
    assert_eq!(client.via, ProxyMode::Direct);
}

#[tokio::test]
async fn client_rejecting_our_ca_is_reported() {
    let upstream = spawn_tls_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    // A client that trusts some other CA, but not Akev's.
    let other = akev_ca::CertificateAuthority::generate().unwrap();
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .tls_built_in_root_certs(false)
        .add_root_certificate(reqwest::Certificate::from_pem(other.cert_pem().as_bytes()).unwrap())
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    assert!(
        http.get(format!("https://localhost:{}/hello", upstream.port()))
            .send()
            .await
            .is_err()
    );
    let seen = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::TlsFailed { .. })
        })
        .await;
    let Some(ProxyEvent::TlsFailed {
        host,
        port,
        error,
        client,
        ..
    }) = seen.last()
    else {
        panic!()
    };
    assert_eq!((host.as_str(), *port), ("localhost", upstream.port()));
    assert_eq!(error.kind, ErrorKind::ClientTlsRejected, "{error:?}");
    assert!(error.message.contains("localhost"));
    let tls = client.tls.as_ref().expect("hello details on failures too");
    assert_eq!(tls.sni.as_deref(), Some("localhost"));
    assert!(tls.ja3.is_some() && tls.ja4.is_some());
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, ProxyEvent::RequestStarted { .. }))
    );
}

#[tokio::test]
async fn tls_passthrough_tunnels_without_interception() {
    let upstream = spawn_tls_upstream().await;
    let mut settings = proxy_settings();
    settings.tls_passthrough = vec!["localhost".into()];
    let capture = akev_core::settings::CaptureSettings {
        detect_processes: false,
        ..Default::default()
    };
    let mut proxy = start_proxy_with(no_rules(), Arc::new(NoBreakpoints), settings, capture).await;
    // The upstream's own (untrusted) certificate reaches the client untouched.
    let http = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).unwrap())
        .danger_accept_invalid_certs(true)
        .http1_only()
        .build()
        .unwrap();
    let res = http
        .get(format!("https://localhost:{}/hello", upstream.port()))
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
    let opened = seen.iter().find_map(|e| match e {
        ProxyEvent::TunnelOpened {
            host,
            port,
            reason,
            client,
            ..
        } => Some((host.clone(), *port, reason.clone(), client.via)),
        _ => None,
    });
    let (host, port, reason, via) = opened.expect("TunnelOpened");
    // The tunnel keeps the CONNECT target's port (never assumes 443).
    assert_eq!((host.as_str(), port), ("localhost", upstream.port()));
    assert!(reason.contains("tls-passthrough"), "{reason}");
    assert_eq!(via, ProxyMode::Connect);
    let Some(ProxyEvent::TunnelClosed { stats, .. }) = seen.last() else {
        panic!()
    };
    assert!(
        stats.bytes_sent > 0 && stats.bytes_received > 0,
        "{stats:?}"
    );
    assert!(
        !seen
            .iter()
            .any(|e| matches!(e, ProxyEvent::RequestStarted { .. }))
    );
}

#[tokio::test]
async fn tls_without_sni_uses_the_tunnel_ip() {
    use tokio::io::AsyncWriteExt;
    let upstream = spawn_tls_upstream().await;
    let mut proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let tunnel = connect_tunnel(&proxy, &upstream.to_string()).await;
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
    // An IP server name: rustls sends no SNI and verifies the certificate's IP SAN.
    let name = rustls_pki_types::ServerName::IpAddress(upstream.ip().into());
    let mut tls = tokio_rustls::TlsConnector::from(Arc::new(cfg))
        .connect(name, tunnel)
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
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(
        head.url,
        format!("https://127.0.0.1:{}/hello", upstream.port())
    );
    let tls = client.tls.as_ref().unwrap();
    assert!(tls.sni.is_none());
    assert!(
        tls.ja4.as_deref().is_some_and(|j| j.starts_with("t13i")),
        "{tls:?}"
    );
}

/// An HTTP/2 (h2c prior knowledge) client connection to the proxy.
async fn h2c_client(proxy: &TestProxy) -> h2::client::SendRequest<bytes::Bytes> {
    let tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let (send, conn) = h2::client::handshake(tcp).await.expect("h2c handshake");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    send
}

/// Read an HTTP/2 response body and its trailers.
async fn h2_body(mut body: h2::RecvStream) -> (Vec<u8>, Option<::http::HeaderMap>) {
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.expect("body data");
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    (data, body.trailers().await.expect("trailers"))
}

#[tokio::test]
async fn h2_get_without_end_stream_on_headers_is_forwarded_without_framing() {
    let upstream = spawn_http_upstream().await;
    let proxy = start_proxy(no_rules(), Arc::new(NoBreakpoints)).await;
    let mut h2 = h2c_client(&proxy).await;

    // HEADERS without END_STREAM, then an empty END_STREAM DATA frame: no body.
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("http://{upstream}/echo"))
        .body(())
        .unwrap();
    let (response, mut stream) = h2.send_request(request, false).unwrap();
    stream.send_data(bytes::Bytes::new(), true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 200);
    let (body, _) = h2_body(response.into_body()).await;
    let echoed = String::from_utf8_lossy(&body).to_ascii_lowercase();
    assert!(echoed.starts_with("get /echo http/1.1"), "{echoed}");
    assert!(!echoed.contains("transfer-encoding"), "{echoed}");
    assert!(!echoed.contains("content-length"), "{echoed}");

    // A body that does follow is still streamed, chunked.
    let request = ::http::Request::builder()
        .method("GET")
        .uri(format!("http://{upstream}/echo"))
        .body(())
        .unwrap();
    let (response, mut stream) = h2.send_request(request, false).unwrap();
    stream
        .send_data(bytes::Bytes::from_static(b"get-body"), true)
        .unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    let (body, _) = h2_body(response.into_body()).await;
    let echoed = String::from_utf8_lossy(&body);
    assert!(
        echoed
            .to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "{echoed}"
    );
    assert!(echoed.ends_with("\n---\nget-body"), "{echoed}");
}

#[tokio::test]
async fn h2_trailers_survive_a_buffered_empty_response_body() {
    // gRPC-style error: no DATA, the status travels in trailers.
    let upstream = spawn_raw(|mut s| async move {
        let mut buf = Vec::new();
        while read_request(&mut s, &mut buf).await.is_some() {
            let response = b"HTTP/1.1 200 OK\r\nContent-Type: application/grpc\r\nTransfer-Encoding: chunked\r\nTrailer: grpc-status, grpc-message\r\n\r\n0\r\ngrpc-status: 5\r\ngrpc-message: not found\r\n\r\n";
            if tokio::io::AsyncWriteExt::write_all(&mut s, response)
                .await
                .is_err()
            {
                return;
            }
        }
    })
    .await;
    // A response body transform makes the proxy buffer the (empty) body.
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"buffer","matchers":[{"type":"path","pattern":{"op":"equals","value":"/grpc-empty"}}],
            "action":{"type":"passthrough","response":{"body":{"type":"match-replace","replacements":[{"find":"zzz","replace":"yyy"}]}}}}]}"#,
    );
    let proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let mut h2 = h2c_client(&proxy).await;
    let request = ::http::Request::builder()
        .method("POST")
        .uri(format!("http://{upstream}/grpc-empty"))
        .header("content-type", "application/grpc")
        .body(())
        .unwrap();
    let (response, _) = h2.send_request(request, true).unwrap();
    let response = tokio::time::timeout(Duration::from_secs(5), response)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(response.status(), 200);
    let (body, trailers) = h2_body(response.into_body()).await;
    assert!(body.is_empty());
    let trailers = trailers.expect("trailers reach the HTTP/2 client");
    assert_eq!(trailers["grpc-status"], "5");
    assert_eq!(trailers["grpc-message"], "not found");
}
