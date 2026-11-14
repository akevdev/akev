//! End to end: a WireGuard client (boringtun + smoltcp, like the WireGuard app on a phone)
//! sends real HTTP and HTTPS requests through Akev's WireGuard server; the proxy intercepts
//! them (TLS MITM with the client trusting the Akev CA), forwards them to local origin
//! servers, and records exchanges with the tunnelled device's original destination.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_ca::CertificateAuthority;
use akev_core::rules::RuleSet;
use akev_core::settings::{CaptureSettings, DnsOverride, ProxySettings};
use akev_core::{EventSink, NoBreakpoints, ProxyEvent, ProxyMode};
use akev_proxy::{ProxyHandle, ProxyOptions};
use akev_rules::RulesEngine;
use akev_wireguard::dns::SystemDns;
use akev_wireguard::testing::TestClient;
use akev_wireguard::{ProxyBridge, ServerOptions, WireguardKeys, WireguardServer};
use hickory_resolver::proto::op::{Message, MessageType, OpCode, Query};
use hickory_resolver::proto::rr::{Name, RData, RecordType};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

const T: Duration = Duration::from_secs(30);

struct ChannelSink(mpsc::UnboundedSender<ProxyEvent>);

impl EventSink for ChannelSink {
    fn emit(&self, event: ProxyEvent) {
        let _ = self.0.send(event);
    }
}

struct Setup {
    proxy: ProxyHandle,
    ca: Arc<CertificateAuthority>,
    events: mpsc::UnboundedReceiver<ProxyEvent>,
    server: WireguardServer,
    client: TestClient,
}

async fn setup() -> Setup {
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let (tx, events) = mpsc::unbounded_channel();
    let proxy = akev_proxy::start(ProxyOptions {
        settings: ProxySettings {
            port: 0,
            ignore_upstream_cert_errors: vec!["*".into()],
            dns_overrides: vec![DnsOverride {
                host: "origin.test".into(),
                address: "127.0.0.1".into(),
            }],
            ..Default::default()
        },
        capture: CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
        ca: ca.clone(),
        rules: Arc::new(RulesEngine::new(RuleSet::default())),
        sink: Arc::new(ChannelSink(tx)),
        breakpoints: Arc::new(NoBreakpoints),
    })
    .await
    .expect("proxy starts");

    let keys = WireguardKeys::generate();
    let bridge_proxy = proxy.clone();
    let server = WireguardServer::start(
        ServerOptions::new("127.0.0.1:0".parse().unwrap(), keys.clone()),
        Arc::new(ProxyBridge::new(move || Some(bridge_proxy.clone()))),
        SystemDns::new(),
    )
    .await
    .expect("WireGuard server starts");
    let client = TestClient::start(server.local_addr(), &keys).await.unwrap();
    client
        .handshake(Duration::from_secs(10))
        .await
        .expect("handshake");
    Setup {
        proxy,
        ca,
        events,
        server,
        client,
    }
}

/// Minimal HTTP/1.1 origin: answers every request with `origin says: <method> <path>`.
async fn serve_http<S: AsyncRead + AsyncWrite + Unpin>(mut s: S) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            buf.drain(..end + 4);
            let line = head.lines().next().unwrap_or_default().to_string();
            let mut parts = line.split(' ');
            let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
            let body = format!("origin says: {method} {path}");
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            );
            if s.write_all(resp.as_bytes()).await.is_err() {
                return;
            }
            continue;
        }
        match s.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn spawn_http_origin() -> SocketAddr {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            tokio::spawn(serve_http(s));
        }
    });
    addr
}

#[derive(Debug)]
struct FixedCert(Arc<rustls::sign::CertifiedKey>);

impl rustls::server::ResolvesServerCert for FixedCert {
    fn resolve(
        &self,
        _: rustls::server::ClientHello<'_>,
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// HTTPS origin for `origin.test` (certificate from an unrelated CA).
async fn spawn_tls_origin() -> SocketAddr {
    let ca = CertificateAuthority::generate().unwrap();
    let key = ca.leaf_for("origin.test").unwrap();
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(FixedCert(key)));
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = l.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(s).await {
                    serve_http(tls).await;
                }
            });
        }
    });
    addr
}

/// Read one HTTP/1.1 response with a Content-Length body; returns (head, body).
async fn read_response<S: AsyncRead + Unpin>(s: &mut S) -> (String, String) {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end]).to_string();
            let len: usize = head
                .lines()
                .find_map(|l| {
                    let (k, v) = l.split_once(':')?;
                    k.eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse().ok())?
                })
                .unwrap_or(0);
            while buf.len() < end + 4 + len {
                let n = s.read(&mut chunk).await.unwrap();
                assert!(n > 0, "connection closed mid-body");
                buf.extend_from_slice(&chunk[..n]);
            }
            let body = String::from_utf8_lossy(&buf[end + 4..end + 4 + len]).to_string();
            return (head, body);
        }
        let n = s.read(&mut chunk).await.unwrap();
        assert!(n > 0, "connection closed before the response head");
        buf.extend_from_slice(&chunk[..n]);
    }
}

/// Wait for the `RequestStarted` event of a request whose URL ends with `suffix`.
async fn request_event(
    events: &mut mpsc::UnboundedReceiver<ProxyEvent>,
    suffix: &str,
) -> (akev_core::RequestHead, akev_core::ClientInfo) {
    let deadline = tokio::time::Instant::now() + T;
    loop {
        let ev = tokio::time::timeout_at(deadline, events.recv())
            .await
            .expect("event in time")
            .expect("sink open");
        if let ProxyEvent::RequestStarted { head, client, .. } = ev
            && head.url.ends_with(suffix)
        {
            return (head, client);
        }
    }
}

#[tokio::test]
async fn http_through_the_tunnel_is_intercepted_and_recorded() {
    let mut s = setup().await;
    let origin = spawn_http_origin().await;
    let mut conn = s.client.connect(origin).await.unwrap();
    let req =
        format!("GET /hello?via=wg HTTP/1.1\r\nHost: {origin}\r\nUser-Agent: test-phone\r\n\r\n");
    conn.write_all(req.as_bytes()).await.unwrap();
    let (head, body) = tokio::time::timeout(T, read_response(&mut conn))
        .await
        .unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, "origin says: GET /hello?via=wg");

    // A second request on the same tunnelled connection (keep-alive).
    conn.write_all(
        format!("POST /again HTTP/1.1\r\nHost: {origin}\r\nContent-Length: 0\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let (_, body) = tokio::time::timeout(T, read_response(&mut conn))
        .await
        .unwrap();
    assert_eq!(body, "origin says: POST /again");

    let (head, client) = request_event(&mut s.events, "/hello?via=wg").await;
    assert_eq!(head.url, format!("http://{origin}/hello?via=wg"));
    assert_eq!(head.host, "127.0.0.1");
    assert_eq!(head.port, origin.port());
    assert_eq!(client.via, ProxyMode::Wireguard);
    assert_eq!(client.interceptor.as_deref(), Some("wireguard"));
    assert!(
        client.remote_addr.starts_with("10.0.0.2:"),
        "{}",
        client.remote_addr
    );
    let (_, again) = request_event(&mut s.events, "/again").await;
    assert_eq!(
        again.connection_id, client.connection_id,
        "same client connection"
    );
    assert!(s.proxy.stats().total_connections >= 1);
    s.server.stop().await;
    s.proxy.shutdown().await;
}

#[tokio::test]
async fn http_without_host_header_uses_the_original_destination() {
    let mut s = setup().await;
    let origin = spawn_http_origin().await;
    let mut conn = s.client.connect(origin).await.unwrap();
    // HTTP/1.0 without Host: only the tunnel knows where this was going.
    conn.write_all(b"GET /no-host HTTP/1.0\r\n\r\n")
        .await
        .unwrap();
    let (_, body) = tokio::time::timeout(T, read_response(&mut conn))
        .await
        .unwrap();
    assert_eq!(body, "origin says: GET /no-host");
    let (head, _) = request_event(&mut s.events, "/no-host").await;
    assert_eq!(
        head.url,
        format!("http://127.0.0.1:{}/no-host", origin.port())
    );
    s.server.stop().await;
    s.proxy.shutdown().await;
}

#[tokio::test]
async fn https_through_the_tunnel_is_decrypted_with_the_akev_ca() {
    let mut s = setup().await;
    let origin = spawn_tls_origin().await;
    // The device resolved origin.test to some address and connects there; the proxy reaches
    // the real server by name (here via a DNS override to 127.0.0.1) on the original port.
    let dst = SocketAddr::new(Ipv4Addr::new(192, 0, 2, 44).into(), origin.port());
    let tunnel = s.client.connect(dst).await.unwrap();

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(rustls_pki_types::CertificateDer::from(
            s.ca.cert_der().to_vec(),
        ))
        .unwrap();
    let mut cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_root_certificates(roots)
    .with_no_client_auth();
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let name = rustls_pki_types::ServerName::try_from("origin.test").unwrap();
    let mut tls = tokio::time::timeout(
        T,
        tokio_rustls::TlsConnector::from(Arc::new(cfg)).connect(name, tunnel),
    )
    .await
    .unwrap()
    // Only the proxy's CA is trusted here (the origin uses an unrelated CA): the handshake
    // succeeding proves the tunnelled TLS was intercepted.
    .expect("the device trusts the Akev CA");
    tls.write_all(b"GET /secure HTTP/1.1\r\nHost: origin.test\r\n\r\n")
        .await
        .unwrap();
    let (head, body) = tokio::time::timeout(T, read_response(&mut tls))
        .await
        .unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(body, "origin says: GET /secure");

    let (head, client) = request_event(&mut s.events, "/secure").await;
    assert_eq!(
        head.url,
        format!("https://origin.test:{}/secure", origin.port())
    );
    assert_eq!(head.port, origin.port(), "port of the original destination");
    assert_eq!(client.via, ProxyMode::Wireguard);
    assert_eq!(client.interceptor.as_deref(), Some("wireguard"));
    let tls_info = client.tls.expect("TLS details recorded");
    assert_eq!(tls_info.sni.as_deref(), Some("origin.test"));
    s.server.stop().await;
    s.proxy.shutdown().await;
}

#[tokio::test]
async fn non_http_protocols_are_tunnelled_to_the_original_destination() {
    let mut s = setup().await;
    // A server-speaks-first protocol (like SSH or SMTP).
    const BANNER: &[u8] = b"SSH-2.0-AkevTest\r\n";
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let banner = l.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut c, _)) = l.accept().await {
            tokio::spawn(async move {
                let _ = c.write_all(BANNER).await;
                let mut buf = [0u8; 64];
                if let Ok(n) = c.read(&mut buf).await {
                    let _ = c.write_all(&buf[..n]).await;
                }
            });
        }
    });
    let mut conn = s.client.connect(banner).await.unwrap();
    let mut got = vec![0u8; BANNER.len()];
    tokio::time::timeout(T, conn.read_exact(&mut got))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got, BANNER);
    conn.write_all(b"client-hello").await.unwrap();
    let mut echo = vec![0u8; 12];
    tokio::time::timeout(T, conn.read_exact(&mut echo))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(echo, b"client-hello");
    let deadline = tokio::time::Instant::now() + T;
    loop {
        let ev = tokio::time::timeout_at(deadline, s.events.recv())
            .await
            .unwrap()
            .unwrap();
        if let ProxyEvent::TunnelOpened {
            host, port, client, ..
        } = ev
        {
            assert_eq!((host.as_str(), port), ("127.0.0.1", banner.port()));
            assert_eq!(client.via, ProxyMode::Wireguard);
            break;
        }
    }
    s.server.stop().await;
    s.proxy.shutdown().await;
}

fn dns_query(name: &str, id: u16) -> Vec<u8> {
    let mut m = Message::new(id, MessageType::Query, OpCode::Query);
    m.metadata.recursion_desired = true;
    m.add_query(Query::query(Name::from_ascii(name).unwrap(), RecordType::A));
    m.to_vec().unwrap()
}

#[tokio::test]
async fn setup_page_and_dns_through_the_tunnel() {
    let mut s = setup().await;
    let relay: SocketAddr = "10.0.0.53:53".parse().unwrap();
    // The system resolver answers `localhost` (RFC 6761) without any network access.
    s.client.send_udp(7000, relay, &dns_query("localhost.", 9));
    let d = s.client.recv_udp(T).await.expect("DNS answer");
    let m = Message::from_vec(&d.payload).unwrap();
    assert_eq!(m.metadata.id, 9);
    assert!(
        m.answers
            .iter()
            .any(|r| matches!(&r.data, RData::A(a) if a.0 == Ipv4Addr::LOCALHOST)),
        "{m:?}"
    );
    // akev.cert → 10.0.0.1, where the proxy serves its setup page.
    s.client.send_udp(7001, relay, &dns_query("akev.cert.", 10));
    let m = Message::from_vec(&s.client.recv_udp(T).await.unwrap().payload).unwrap();
    let ip = m
        .answers
        .iter()
        .find_map(|r| match &r.data {
            RData::A(a) => Some(a.0),
            _ => None,
        })
        .expect("akev.cert has an address");
    assert_eq!(ip, Ipv4Addr::new(10, 0, 0, 1));
    let mut conn = s
        .client
        .connect(SocketAddr::new(ip.into(), 80))
        .await
        .unwrap();
    conn.write_all(b"GET /ca.pem HTTP/1.1\r\nHost: akev.cert\r\n\r\n")
        .await
        .unwrap();
    let (head, body) = tokio::time::timeout(T, read_response(&mut conn))
        .await
        .unwrap();
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    assert_eq!(
        body,
        s.ca.cert_pem(),
        "the CA certificate downloads through the tunnel"
    );
    s.server.stop().await;
    s.proxy.shutdown().await;
}
