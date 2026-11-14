//! HTTP/3: the upstream client over QUIC (forced, Alt-Svc discovery / caching / clearing /
//! expiry, `prefer` mode, fallback to TCP, certificate verification), the inbound QUIC
//! listener (h3 client → Akev → origin, recorded as HTTP/3, rules applied, h3 → h3), its
//! settings lifecycle, `serve_quic_socket` with an original destination, and Compose with
//! `httpVersion: http3`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;
#[path = "common/h3.rs"]
mod h3_support;
#[path = "common/upstream.rs"]
mod upstream_support;

use akev_core::rules::RuleSet;
use akev_core::settings::{CaptureSettings, Http3Settings, Http3UpstreamMode, ProxySettings};
use akev_core::*;
use akev_proxy::upstream::{BodyChunk, RequestBody, RequestOptions, UpstreamHttpVersion};
use akev_proxy::{SendOptions, UpstreamHttpVersion as ReexportedVersion};
use bytes::Bytes;
use common::{TestProxy, start_proxy_with};
use h3_support::*;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};
use upstream_support::{TestCa, collect, head, write_temp};

fn trusting(ca: &TestCa, dir: &tempfile::TempDir) -> ProxySettings {
    ProxySettings {
        trusted_ca_paths: vec![write_temp(dir, "ca.pem", ca.pem().as_bytes())],
        ..Default::default()
    }
}

fn with_mode(mut s: ProxySettings, mode: Http3UpstreamMode) -> ProxySettings {
    s.http3.upstream = mode;
    s
}

fn forced() -> RequestOptions {
    RequestOptions {
        http_version: UpstreamHttpVersion::Http3,
        ..Default::default()
    }
}

fn get(url: &str) -> RequestHead {
    head("GET", url, &[])
}

// -------------------------------------------------------------------------------------------
// Upstream client
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn forced_h3_request_with_headers_body_and_trailers() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let c = upstream_support::client(&trusting(&ca, &dir));
    let url = format!("https://localhost:{}/echo?x=1", origin.port());

    // Streamed body with request trailers.
    let chunks: Vec<Result<BodyChunk, akev_proxy::upstream::UpstreamError>> = vec![
        Ok(BodyChunk::Data(Bytes::from_static(b"hello "))),
        Ok(BodyChunk::Data(Bytes::from_static(b"world"))),
        Ok(BodyChunk::Trailers(Headers(vec![(
            "X-Req-Trailer".into(),
            "yes".into(),
        )]))),
    ];
    let body = RequestBody::Stream {
        stream: Box::pin(futures::stream::iter(chunks)),
        length: None,
    };
    let h = head(
        "POST",
        &url,
        &[
            ("Host", &format!("localhost:{}", origin.port())),
            ("X-Test", "One"),
            ("Connection", "keep-alive"),
            ("x-want-trailers", "1"),
        ],
    );
    let resp = c.send(&h, body, &forced()).await.unwrap();
    assert_eq!(resp.head.status, 200);
    assert_eq!(resp.head.status_message, "OK");
    assert_eq!(resp.head.http_version, HttpVersion::H3);
    assert_eq!(resp.head.headers.get("x-proto"), Some("h3"));
    assert_eq!(resp.info.alpn.as_deref(), Some("h3"));
    assert_eq!(resp.info.tls_version.as_deref(), Some("TLSv1.3"));
    assert!(!resp.info.reused);
    assert_eq!(
        resp.info.remote_addr.as_deref(),
        Some(origin.addr.to_string().as_str())
    );
    let cert = resp.info.certificate.clone().expect("certificate captured");
    assert_eq!(cert.subject, "CN=localhost");
    assert_eq!(cert.issuer, "CN=H3 Test CA");
    let chain = resp.info.chain.clone().expect("chain captured");
    assert_eq!(chain.certificates[0], cert);
    assert_eq!(chain.trust.status, ChainTrustStatus::Verified);
    let anchor = chain.trust.anchor.as_ref().expect("trusted root");
    assert_eq!(anchor.subject, "CN=H3 Test CA");
    assert_eq!(anchor.source, TrustAnchorSource::Custom);
    assert!(resp.timings.dns_ms.is_some() && resp.timings.connect_ms.is_some());
    assert!(
        resp.timings.tls_ms.is_none(),
        "QUIC has no separate TLS phase"
    );
    assert!(resp.request_sent_ms.is_some());
    let (data, trailers) = collect(resp.body).await;
    assert_eq!(
        String::from_utf8_lossy(&data),
        "h3 POST /echo?x=1 body=hello world"
    );
    assert_eq!(trailers.unwrap().get("x-trailer"), Some("done"));

    let seen = origin.seen.last();
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.uri, url);
    assert_eq!(seen.headers.get("x-test").unwrap(), "One");
    // Connection-specific headers and Host don't travel over HTTP/3.
    assert!(seen.headers.get("connection").is_none() && seen.headers.get("host").is_none());
    assert_eq!(seen.trailers.unwrap().get("x-req-trailer").unwrap(), "yes");

    // The connection is reused, and concurrent requests multiplex over it.
    let resp = c
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap();
    assert!(resp.info.reused);
    assert!(resp.timings.connect_ms.is_none());
    assert!(resp.info.certificate.is_some());
    collect(resp.body).await;
    let mut tasks = Vec::new();
    for i in 0..12 {
        let c = c.clone();
        let url = format!("https://localhost:{}/slow?{i}", origin.port());
        tasks.push(tokio::spawn(async move {
            let r = c
                .send(&get(&url), RequestBody::Empty, &forced())
                .await
                .unwrap();
            (r.head.http_version, collect(r.body).await.0)
        }));
    }
    for (i, t) in tasks.into_iter().enumerate() {
        let (v, body) = t.await.unwrap();
        assert_eq!(v, HttpVersion::H3);
        assert_eq!(
            String::from_utf8_lossy(&body),
            format!("h3 GET /slow?{i} body=")
        );
    }
    assert_eq!(origin.seen.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn busy_connections_get_company() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let c = upstream_support::client(&trusting(&ca, &dir));
    // More concurrent slow requests than one pooled connection takes (64): a second
    // connection carries the rest instead of queueing them behind the first ones.
    let mut tasks = Vec::new();
    for i in 0..80 {
        let c = c.clone();
        let url = format!("https://localhost:{}/slow?{i}", origin.port());
        tasks.push(tokio::spawn(async move {
            let r = c
                .send(&get(&url), RequestBody::Empty, &forced())
                .await
                .unwrap();
            collect(r.body).await.0.len()
        }));
    }
    for t in tasks {
        assert!(t.await.unwrap() > 0);
    }
    assert_eq!(origin.seen.connections.load(Ordering::SeqCst), 2);
    assert_eq!(origin.seen.count(), 80);
}

#[tokio::test]
async fn large_bodies_stream_both_ways() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let c = upstream_support::client(&trusting(&ca, &dir));
    let n = 3_000_000;
    let url = format!("https://localhost:{}/big?{n}", origin.port());
    let resp = c
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap();
    let (data, _) = collect(resp.body).await;
    assert_eq!(data.len(), n);
    assert!(data.iter().all(|b| *b == b'x'));

    let upload = Bytes::from(vec![b'u'; 2_000_000]);
    let url = format!("https://localhost:{}/up", origin.port());
    let resp = c
        .send(
            &head("PUT", &url, &[]),
            RequestBody::Full(upload),
            &forced(),
        )
        .await
        .unwrap();
    collect(resp.body).await;
    let seen = origin.seen.last();
    assert_eq!(seen.body.len(), 2_000_000);
    assert_eq!(
        seen.headers
            .get("content-length")
            .map(|v| v.to_str().unwrap()),
        None,
        "no Content-Length invented"
    );
}

#[tokio::test]
async fn alt_svc_discovery_caching_and_clear() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain.clone(), key.clone_key(), None).await;
    // The TCP origin advertises the HTTP/3 one (on another port: an alternative).
    let alt = format!(r#"h3=":{}"; ma=60, h2=":443""#, origin.port());
    let (tcp, tcp_count) = spawn_tls_h1(chain, key, vec![("Alt-Svc".into(), alt)]).await;
    let c = upstream_support::client(&with_mode(trusting(&ca, &dir), Http3UpstreamMode::AltSvc));
    let url = |p: &str| format!("https://localhost:{}{p}", tcp.port());

    let r1 = c
        .send(
            &get(&url("/one")),
            RequestBody::Empty,
            &RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r1.head.http_version, HttpVersion::Http11);
    assert_eq!(collect(r1.body).await.0, b"tcp /one");

    // Now known: HTTP/3 on the advertised alternative, certificate checked for the origin.
    let r2 = c
        .send(
            &get(&url("/two")),
            RequestBody::Empty,
            &RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r2.head.http_version, HttpVersion::H3);
    assert_eq!(r2.info.alpn.as_deref(), Some("h3"));
    assert_eq!(
        r2.info.remote_addr.as_deref(),
        Some(origin.addr.to_string().as_str())
    );
    assert_eq!(
        String::from_utf8_lossy(&collect(r2.body).await.0),
        "h3 GET /two body="
    );
    // The request keeps the origin's authority.
    assert_eq!(origin.seen.last().uri, url("/two"));
    assert_eq!(tcp_count.load(Ordering::SeqCst), 1);

    // Explicit HTTP/1.1 / HTTP/2 requests stay on TCP.
    for v in [UpstreamHttpVersion::Http1, UpstreamHttpVersion::Http2] {
        let o = RequestOptions {
            http_version: v,
            ..Default::default()
        };
        let r = c
            .send(&get(&url("/tcp")), RequestBody::Empty, &o)
            .await
            .unwrap();
        assert_eq!(r.head.http_version, HttpVersion::Http11, "{v:?}");
        collect(r.body).await;
    }
    // So do WebSocket-style upgrades.
    let upgrade = head(
        "GET",
        &url("/ws"),
        &[("Upgrade", "websocket"), ("Connection", "Upgrade")],
    );
    let r = c
        .send(&upgrade, RequestBody::Empty, &RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::Http11);
    collect(r.body).await;
    assert_eq!(tcp_count.load(Ordering::SeqCst), 4);

    // The HTTP/3 server withdraws the alternative: back to TCP.
    let r = c
        .send(
            &get(&url("/clear")),
            RequestBody::Empty,
            &RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::H3);
    assert_eq!(r.head.headers.get("alt-svc"), Some("clear"));
    collect(r.body).await;
    let r = c
        .send(
            &get(&url("/after")),
            RequestBody::Empty,
            &RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::Http11);
    collect(r.body).await;

    // With HTTP/3 off, advertisements are remembered but unused.
    let off = upstream_support::client(&trusting(&ca, &dir));
    for _ in 0..2 {
        let r = off
            .send(
                &get(&url("/off")),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap();
        assert_eq!(r.head.http_version, HttpVersion::Http11);
        collect(r.body).await;
    }
}

#[tokio::test]
async fn alt_svc_entries_expire() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain.clone(), key.clone_key(), None).await;
    let alt = format!(r#"h3=":{}"; ma=1"#, origin.port());
    let (tcp, _) = spawn_tls_h1(chain, key, vec![("alt-svc".into(), alt)]).await;
    let c = upstream_support::client(&with_mode(trusting(&ca, &dir), Http3UpstreamMode::AltSvc));
    let url = format!("https://localhost:{}/x", tcp.port());
    let version = |c: &akev_proxy::upstream::UpstreamClient| {
        let c = c.clone();
        let url = url.clone();
        async move {
            let r = c
                .send(&get(&url), RequestBody::Empty, &RequestOptions::default())
                .await
                .unwrap();
            let v = r.head.http_version;
            collect(r.body).await;
            v
        }
    };
    assert_eq!(version(&c).await, HttpVersion::Http11);
    assert_eq!(version(&c).await, HttpVersion::H3);
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert_eq!(version(&c).await, HttpVersion::Http11, "ma=1 expired");
    assert_eq!(version(&c).await, HttpVersion::H3, "re-advertised");
}

#[tokio::test]
async fn prefer_mode_falls_back_to_tcp_and_remembers() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (tcp, count) = spawn_tls_h1(chain, key, vec![]).await;
    let mut settings = with_mode(trusting(&ca, &dir), Http3UpstreamMode::Prefer);
    settings.connect_timeout_ms = 800;
    let c = upstream_support::client(&settings);
    let url = format!("https://localhost:{}/p", tcp.port());

    // Nothing answers QUIC: the attempt gives up and TCP serves the request.
    let r = c
        .send(
            &get(&url),
            RequestBody::Full(Bytes::new()),
            &RequestOptions::default(),
        )
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::Http11);
    assert_eq!(collect(r.body).await.0, b"tcp /p");
    // The origin is now known not to speak HTTP/3: straight to TCP.
    let started = Instant::now();
    let r = c
        .send(&get(&url), RequestBody::Empty, &RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::Http11);
    collect(r.body).await;
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn prefer_mode_uses_h3_without_advertisement() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let c = upstream_support::client(&with_mode(trusting(&ca, &dir), Http3UpstreamMode::Prefer));
    let url = format!("https://localhost:{}/pref", origin.port());
    let r = c
        .send(&get(&url), RequestBody::Empty, &RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::H3);
    collect(r.body).await;
    // Plain-HTTP requests never use QUIC.
    let plain = format!("http://127.0.0.1:{}/x", origin.port());
    let err = c
        .send(&get(&plain), RequestBody::Empty, &RequestOptions::default())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionRefused);
}

#[tokio::test]
async fn forced_h3_falls_back_to_tcp_when_quic_fails() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (tcp, _) = spawn_tls_h1(chain, key, vec![]).await;
    let mut settings = trusting(&ca, &dir);
    settings.connect_timeout_ms = 800;
    let c = upstream_support::client(&settings);
    let url = format!("https://localhost:{}/f", tcp.port());
    let r = c
        .send(
            &get(&url),
            RequestBody::Full(Bytes::from_static(b"kept")),
            &forced(),
        )
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::Http11);
    assert_eq!(r.info.alpn.as_deref(), Some("http/1.1"));
    assert_eq!(collect(r.body).await.0, b"tcp /f");
}

#[tokio::test]
async fn h3_certificate_verification_follows_tls_settings() {
    // A certificate from a CA nobody trusts, and no TCP server behind the same port.
    let untrusted = TestCa::new("Untrusted CA");
    let (chain, key) = untrusted.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let url = format!("https://localhost:{}/v", origin.port());

    // QUIC refuses the certificate; the TCP fallback finds nothing listening.
    let c = upstream_support::client(&ProxySettings::default());
    let err = c
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionRefused, "{err:?}");
    assert_eq!(origin.seen.count(), 0);

    // Insecure requests and ignore-errors hosts accept it, over HTTP/3.
    let insecure = RequestOptions {
        insecure: true,
        ..forced()
    };
    let r = c
        .send(&get(&url), RequestBody::Empty, &insecure)
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::H3);
    assert_eq!(r.info.certificate.unwrap().issuer, "CN=Untrusted CA");
    assert_eq!(
        r.info.chain.as_ref().map(|c| c.trust.status),
        Some(ChainTrustStatus::NotVerified)
    );
    collect(r.body).await;
    let ignoring = upstream_support::client(&ProxySettings {
        ignore_upstream_cert_errors: vec!["localhost".into()],
        ..Default::default()
    });
    let r = ignoring
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::H3);
    collect(r.body).await;
}

#[tokio::test]
async fn upstream_proxies_disable_h3() {
    let ca = TestCa::new("H3 Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["h3.test"], false);
    let origin = spawn_h3(chain, key, None).await;
    let mut settings = with_mode(trusting(&ca, &dir), Http3UpstreamMode::Prefer);
    settings.dns_overrides = vec![akev_core::settings::DnsOverride {
        host: "h3.test".into(),
        address: "127.0.0.1".into(),
    }];
    // Reachable directly over HTTP/3...
    let direct = upstream_support::client(&settings);
    let url = format!("https://h3.test:{}/proxied", origin.port());
    let r = direct
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap();
    assert_eq!(r.head.http_version, HttpVersion::H3);
    collect(r.body).await;
    // ...but with a (dead) upstream proxy configured, QUIC must not bypass it.
    let dead = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);
    settings.upstream_proxy = akev_core::settings::UpstreamProxy {
        mode: akev_core::settings::UpstreamProxyMode::Manual,
        url: format!("http://127.0.0.1:{dead_port}"),
        no_proxy: vec![],
    };
    let proxied = upstream_support::client(&settings);
    let err = proxied
        .send(&get(&url), RequestBody::Empty, &forced())
        .await
        .unwrap_err();
    assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed, "{err:?}");
    assert_eq!(origin.seen.count(), 1);
}

// -------------------------------------------------------------------------------------------
// Inbound QUIC listener
// -------------------------------------------------------------------------------------------

fn h3_settings(upstream: Http3UpstreamMode) -> ProxySettings {
    ProxySettings {
        http3: Http3Settings {
            upstream,
            listen: true,
            listen_port: 0,
        },
        ..common::proxy_settings()
    }
}

async fn h3_proxy(rules: RuleSet, settings: ProxySettings) -> TestProxy {
    start_proxy_with(
        rules,
        Arc::new(akev_core::NoBreakpoints),
        settings,
        CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
    )
    .await
}

async fn connect_proxy(proxy: &TestProxy) -> H3Client {
    let status = proxy.handle.http3_status();
    let port = status.port.expect("QUIC listener running");
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let ca = rustls::pki_types::CertificateDer::from(proxy.ca.cert_der().to_vec());
    H3Client::connect(addr, "localhost", &[ca])
        .await
        .expect("connect to Akev over QUIC")
}

#[tokio::test]
async fn quic_listener_records_h3_exchanges() {
    let origin = common::spawn_tls_upstream().await;
    let mut proxy = h3_proxy(RuleSet::default(), h3_settings(Http3UpstreamMode::Off)).await;
    let status = proxy.handle.http3_status();
    assert!(status.enabled);
    assert_eq!(
        status.port,
        Some(proxy.addr().port()),
        "same port number as the proxy"
    );
    assert!(status.error.is_none());

    let mut client = connect_proxy(&proxy).await;
    let authority = format!("localhost:{}", origin.port());
    let resp = client
        .get(&format!("https://{authority}/hello"), &[("x-client", "h3")])
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "hello");
    assert_eq!(resp.headers.get("x-upstream").unwrap(), "test");

    let (id, events) = common::wait_exchange(&mut proxy, "/hello", common::is_completed).await;
    let started = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestStarted { head, client, .. } => Some((head.clone(), client.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(started.0.http_version, HttpVersion::H3);
    assert_eq!(started.0.url, format!("https://{authority}/hello"));
    assert_eq!(started.0.headers.get("x-client"), Some("h3"));
    assert_eq!(started.1.via, ProxyMode::Direct);
    let tls = started.1.tls.expect("client TLS info");
    assert_eq!(tls.sni.as_deref(), Some("localhost"));
    assert_eq!(tls.alpn.as_deref(), Some("h3"));
    assert_eq!(tls.version.as_deref(), Some("TLSv1.3"));
    assert!(events.iter().any(|e| matches!(e, ProxyEvent::Upstream { info, .. } if info.alpn.as_deref() == Some("http/1.1"))));
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ResponseCompleted { id: i, .. } if *i == id))
    );

    // A body (streamed, unknown length) reaches the HTTP/1.1 origin chunked, and the
    // response comes back over HTTP/3.
    let req = ::http::Request::builder()
        .method("POST")
        .uri(format!("https://{authority}/echo"))
        .header("content-type", "text/plain")
        .body(())
        .unwrap();
    let resp = client
        .request(
            req,
            vec![
                Bytes::from_static(b"part one, "),
                Bytes::from_static(b"part two"),
            ],
            None,
        )
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    let text = resp.text();
    assert!(text.starts_with("POST /echo HTTP/1.1\r\n"), "{text}");
    assert!(
        text.to_ascii_lowercase()
            .contains("transfer-encoding: chunked"),
        "{text}"
    );
    assert!(text.ends_with("part one, part two"), "{text}");
    let (_, events) = common::wait_exchange(&mut proxy, "/echo", common::is_completed).await;
    let body = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestCompleted { body, .. } => Some(body.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(&body.bytes[..], b"part one, part two");

    // Trailers from the origin are relayed as HTTP/3 trailers.
    let resp = client
        .get(&format!("https://{authority}/chunked"), &[])
        .await
        .unwrap();
    assert_eq!(resp.text(), "hello world");
    assert_eq!(resp.trailers.unwrap().get("x-checksum").unwrap(), "abc");

    let stats = proxy.handle.stats();
    assert!(stats.total_connections >= 1);
    drop(client);
    proxy.handle.shutdown().await;
}

#[tokio::test]
async fn quic_listener_applies_rules() {
    let rules = common::rules(
        r#"{"items":[{"type":"rule","id":"m","matchers":[{"type":"path","pattern":{"op":"equals","value":"/mocked"}}],"action":{"type":"mock","status":418,"headers":[["x-mock","yes"]],"body":{"type":"text","text":"from a rule"}}}]}"#,
    );
    let mut proxy = h3_proxy(rules, h3_settings(Http3UpstreamMode::Off)).await;
    let mut client = connect_proxy(&proxy).await;
    let resp = client
        .get("https://unreachable.invalid/mocked", &[])
        .await
        .unwrap();
    assert_eq!(resp.status, 418);
    assert_eq!(resp.headers.get("x-mock").unwrap(), "yes");
    assert_eq!(resp.text(), "from a rule");
    let (_, events) = common::wait_exchange(&mut proxy, "/mocked", common::is_completed).await;
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::RuleMatched { .. }))
    );
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );
}

#[tokio::test]
async fn quic_listener_to_h3_upstream() {
    let origin_ca = TestCa::new("Origin CA");
    let (chain, key) = origin_ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    // The proxy ignores upstream certificate errors (test settings) and prefers HTTP/3.
    let mut proxy = h3_proxy(RuleSet::default(), h3_settings(Http3UpstreamMode::Prefer)).await;
    let mut client = connect_proxy(&proxy).await;
    let req = ::http::Request::builder()
        .method("PUT")
        .uri(format!("https://localhost:{}/both-ways", origin.port()))
        .header("x-hop", "1")
        .body(())
        .unwrap();
    let resp = client
        .request(req, vec![Bytes::from_static(b"payload")], None)
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "h3 PUT /both-ways body=payload");
    let seen = origin.seen.last();
    assert_eq!(seen.headers.get("x-hop").unwrap(), "1");
    assert!(seen.headers.get("transfer-encoding").is_none());
    let (_, events) = common::wait_exchange(&mut proxy, "/both-ways", common::is_completed).await;
    assert!(events.iter().any(
        |e| matches!(e, ProxyEvent::Upstream { info, .. } if info.alpn.as_deref() == Some("h3"))
    ));
    assert!(events.iter().any(|e| matches!(e, ProxyEvent::ResponseStarted { head, .. } if head.http_version == HttpVersion::H3)));
}

#[tokio::test]
async fn quic_listener_follows_settings() {
    let mut settings = common::proxy_settings();
    let mut proxy = h3_proxy(RuleSet::default(), settings.clone()).await;
    let status = proxy.handle.http3_status();
    assert!(!status.enabled && status.port.is_none() && status.error.is_none());

    // Enabling starts it on the proxy's port number.
    settings.http3.listen = true;
    let capture = CaptureSettings::default();
    settings.port = proxy.addr().port();
    proxy
        .handle
        .update_settings(settings.clone(), capture.clone());
    assert_eq!(proxy.handle.http3_status().port, Some(proxy.addr().port()));
    let mut client = connect_proxy(&proxy).await;
    let r = client.get("https://akev.cert/", &[]).await.unwrap();
    assert_eq!(
        r.status, 200,
        "the certificate page is served over HTTP/3 too"
    );
    let _ = proxy.drain(Duration::from_millis(100)).await;

    // An explicit port moves it.
    let free = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);
    settings.http3.listen_port = port;
    proxy
        .handle
        .update_settings(settings.clone(), capture.clone());
    assert_eq!(proxy.handle.http3_status().port, Some(port));

    // A busy port is reported.
    let busy = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    settings.http3.listen_port = busy.local_addr().unwrap().port();
    proxy
        .handle
        .update_settings(settings.clone(), capture.clone());
    let status = proxy.handle.http3_status();
    assert!(status.port.is_none());
    assert!(
        status
            .error
            .as_deref()
            .is_some_and(|e| e.contains("cannot listen")),
        "{status:?}"
    );
    let busy_port = settings.http3.listen_port;
    drop(busy);
    // Retried for a while: once the port is free, the listener starts on its own.
    let deadline = Instant::now() + Duration::from_secs(3);
    while proxy.handle.http3_status().port.is_none() {
        assert!(Instant::now() < deadline, "listener never started");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let status = proxy.handle.http3_status();
    assert_eq!(status.port, Some(busy_port));
    assert!(status.error.is_none());
    // A settings change with nothing new for the listener leaves it alone.
    proxy
        .handle
        .update_settings(settings.clone(), capture.clone());
    assert_eq!(proxy.handle.http3_status().port, Some(busy_port));

    // Rebinding the proxy moves a listener that follows the proxy's port.
    settings.http3.listen_port = 0;
    proxy
        .handle
        .update_settings(settings.clone(), capture.clone());
    let addr = proxy.handle.rebind(0, "127.0.0.1").await.unwrap();
    assert_eq!(proxy.handle.http3_status().port, Some(addr.port()));

    settings.http3.listen = false;
    proxy.handle.update_settings(settings, capture);
    let status = proxy.handle.http3_status();
    assert!(!status.enabled && status.port.is_none());
}

#[tokio::test]
async fn serve_quic_socket_with_original_destination() {
    let origin = common::spawn_tls_upstream().await;
    let mut proxy = h3_proxy(RuleSet::default(), common::proxy_settings()).await;
    let socket = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = socket.local_addr().unwrap();
    let handle = proxy.handle.clone();
    // The flow was going to 127.0.0.1:<origin port> (as a VPN / transparent capture sees it).
    let task = tokio::spawn(async move { handle.serve_quic_socket(socket, Some(origin)).await });
    let ca = rustls::pki_types::CertificateDer::from(proxy.ca.cert_der().to_vec());
    let mut client = H3Client::connect(addr, "localhost", &[ca]).await.unwrap();
    // The authority names the host only; the port comes from the original destination.
    let resp = client.get("https://localhost/hello", &[]).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), "hello");
    let (_, events) = common::wait_exchange(&mut proxy, "/hello", common::is_completed).await;
    let head = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestStarted { head, .. } => Some(head.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(head.http_version, HttpVersion::H3);
    assert_eq!(
        head.url,
        format!("https://localhost:{}/hello", origin.port())
    );
    drop(client);
    proxy.handle.shutdown().await;
    tokio::time::timeout(Duration::from_secs(5), task)
        .await
        .expect("serve_quic_socket returns on shutdown")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn untrusted_clients_are_reported_as_tls_failures() {
    let mut proxy = h3_proxy(RuleSet::default(), h3_settings(Http3UpstreamMode::Off)).await;
    let port = proxy.handle.http3_status().port.unwrap();
    // A client that doesn't trust Akev's CA.
    let other = TestCa::new("Someone else");
    let err = H3Client::connect(
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        "pinned.example",
        &[other.der()],
    )
    .await
    .err()
    .expect("handshake fails");
    assert!(
        err.to_lowercase().contains("certificate") || err.contains("UnknownIssuer"),
        "{err}"
    );
    let events = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::TlsFailed { .. })
        })
        .await;
    let (host, error) = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::TlsFailed { host, error, .. } => Some((host.clone(), error.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(host, "pinned.example");
    assert_eq!(error.kind, ErrorKind::ClientTlsRejected, "{error:?}");
}

// -------------------------------------------------------------------------------------------
// Compose
// -------------------------------------------------------------------------------------------

#[tokio::test]
async fn compose_with_http_version_http3() {
    let origin_ca = TestCa::new("Origin CA");
    let (chain, key) = origin_ca.leaf(&["localhost"], false);
    let origin = spawn_h3(chain, key, None).await;
    let mut settings = common::proxy_settings();
    settings.connect_timeout_ms = 1000;
    let mut proxy = h3_proxy(RuleSet::default(), settings).await;
    let request = EditableRequest {
        method: "POST".into(),
        url: format!("https://localhost:{}/compose", origin.port()),
        headers: Headers(vec![("Content-Type".into(), "text/plain".into())]),
        body: Bytes::from_static(b"composed"),
    };
    let opts: SendOptions =
        serde_json::from_value(serde_json::json!({"httpVersion": "http3"})).unwrap();
    assert_eq!(opts.http_version, ReexportedVersion::Http3);
    let result = proxy.handle.send(request.clone(), opts).await;
    assert!(result.error.is_none(), "{:?}", result.error);
    let response = result.response.unwrap();
    assert_eq!(response.http_version, HttpVersion::H3);
    assert_eq!(&result.body[..], b"h3 POST /compose body=composed");
    let events = proxy
        .wait_for(Duration::from_secs(5), |e| {
            matches!(e, ProxyEvent::Upstream { .. })
        })
        .await;
    assert!(events.iter().any(
        |e| matches!(e, ProxyEvent::Upstream { info, .. } if info.alpn.as_deref() == Some("h3"))
    ));
    assert_eq!(
        events.iter().find_map(|e| match e {
            ProxyEvent::RequestStarted { client, .. } => Some(client.via),
            _ => None,
        }),
        Some(ProxyMode::Composer)
    );

    // `auto` with HTTP/3 off uses TCP — where nothing listens.
    let auto = proxy.handle.send(request, SendOptions::default()).await;
    assert_eq!(auto.error.unwrap().kind, ErrorKind::ConnectionRefused);
}
