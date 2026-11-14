//! TLS, certificate handling and HTTP/2 tests against in-process rustls / h2 servers.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/upstream.rs"]
mod upstream_support;

use akev_core::settings::{ClientCertificate, DnsOverride, ProxySettings};
use akev_core::{ChainTrustStatus, ErrorKind, Headers, HttpVersion, TrustAnchorSource};
use akev_proxy::upstream::{BodyChunk, RequestBody, RequestOptions, UpstreamError};
use bytes::Bytes;
use std::sync::atomic::Ordering;
use upstream_support::*;

const ALPN_BOTH: &[&[u8]] = &[b"h2", b"http/1.1"];

fn trusting(ca: &TestCa, dir: &tempfile::TempDir) -> ProxySettings {
    ProxySettings {
        trusted_ca_paths: vec![write_temp(dir, "ca.pem", ca.pem().as_bytes())],
        ..Default::default()
    }
}

#[tokio::test]
async fn h2_alpn_multiplexing_and_certificate_capture() {
    let ca = TestCa::new("Akev Test CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost", "127.0.0.1"], false);
    let (addr, accepted, seen) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let url = format!("https://localhost:{}/hello", addr.port());

    let resp = c
        .send(
            &head("GET", &url, &[("Host", "localhost"), ("X-Test", "one")]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    assert_eq!(resp.head.status_message, "OK");
    assert_eq!(resp.head.http_version, HttpVersion::H2);
    assert_eq!(resp.head.headers.get("x-mixed-case"), Some("kept"));
    assert_eq!(resp.info.alpn.as_deref(), Some("h2"));
    assert_eq!(resp.info.tls_version.as_deref(), Some("TLSv1.3"));
    assert!(
        resp.info
            .cipher
            .as_deref()
            .is_some_and(|c| c.starts_with("TLS13_"))
    );
    assert!(!resp.info.reused);
    assert!(
        resp.timings.tls_ms.is_some()
            && resp.timings.connect_ms.is_some()
            && resp.timings.dns_ms.is_some()
    );
    let cert = resp.info.certificate.clone().expect("certificate captured");
    assert_eq!(cert.subject, "CN=localhost");
    assert_eq!(cert.issuer, "CN=Akev Test CA");
    assert_eq!(
        cert.san,
        vec!["localhost".to_string(), "127.0.0.1".to_string()]
    );
    assert!(!cert.not_before.is_empty() && !cert.not_after.is_empty() && !cert.serial.is_empty());
    assert_eq!(cert.sha256_fingerprint.len(), 95);
    assert_eq!(collect(resp.body).await.0, b"h2 GET /hello x-test=one");

    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(resp.info.reused);
    assert!(resp.timings.tls_ms.is_none());
    assert!(resp.info.certificate.is_some());
    collect(resp.body).await;

    // Concurrent requests multiplex over the single connection.
    let mut tasks = Vec::new();
    for i in 0..16 {
        let c = c.clone();
        let url = format!("https://localhost:{}/slow?{i}", addr.port());
        tasks.push(tokio::spawn(async move {
            let resp = c
                .send(
                    &head("GET", &url, &[]),
                    RequestBody::Empty,
                    &RequestOptions::default(),
                )
                .await
                .unwrap();
            collect(resp.body).await.0
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), b"h2 GET /slow");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    assert_eq!(seen.lock()[0].sni.as_deref(), Some("localhost"));
}

/// Root → intermediate → leaf, the server sending all three plus an unrelated certificate.
#[tokio::test]
async fn the_presented_chain_and_its_trusted_root_are_reported() {
    let root = TestCa::new("Akev Chain Root");
    let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "Akev Chain Intermediate");
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::DigitalSignature,
    ];
    let int_key = rcgen::KeyPair::generate().unwrap();
    let int_cert = params
        .signed_by(
            &int_key,
            &rcgen::Issuer::from_params(&root.params, &root.key),
        )
        .unwrap();
    let mut leaf_params = rcgen::CertificateParams::new(vec!["localhost".to_string()]).unwrap();
    leaf_params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "localhost");
    let leaf_key = rcgen::KeyPair::generate().unwrap();
    let leaf = leaf_params
        .signed_by(&leaf_key, &rcgen::Issuer::from_params(&params, &int_key))
        .unwrap();
    let stray = TestCa::new("Unrelated");
    let presented = vec![
        leaf.der().clone(),
        stray.der(),
        int_cert.der().clone(),
        root.der(),
    ];
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(rustls::pki_types::PrivatePkcs8KeyDer::from(
        leaf_key.serialize_der(),
    ));
    let (addr, _, _) = spawn_tls(server_config(presented, key, ALPN_BOTH, None)).await;
    let dir = tempfile::tempdir().unwrap();
    let c = client(&trusting(&root, &dir));
    let url = format!("https://localhost:{}/chain", addr.port());
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    let chain = resp.info.chain.clone().expect("chain captured");
    let subjects: Vec<&str> = chain
        .certificates
        .iter()
        .map(|c| c.subject.as_str())
        .collect();
    assert_eq!(
        subjects,
        [
            "CN=localhost",
            "CN=Unrelated",
            "CN=Akev Chain Intermediate",
            "CN=Akev Chain Root"
        ]
    );
    assert_eq!(chain.certificates[0].issuer, "CN=Akev Chain Intermediate");
    assert_eq!(chain.certificates[0].san, ["localhost"]);
    assert_eq!(chain.trust.status, ChainTrustStatus::Verified);
    // The unrelated certificate isn't part of the verified path.
    assert_eq!(chain.trust.path, [0, 2]);
    let anchor = chain.trust.anchor.as_ref().expect("trusted root");
    assert_eq!(anchor.subject, "CN=Akev Chain Root");
    assert_eq!(anchor.index, Some(3));
    assert_eq!(anchor.source, TrustAnchorSource::Custom);
    collect(resp.body).await;

    // Exchanges on the pooled connection share the chain.
    let again = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(again.info.reused);
    assert!(std::sync::Arc::ptr_eq(
        again.info.chain.as_ref().unwrap(),
        &chain
    ));
    collect(again.body).await;
}

#[tokio::test]
async fn concurrent_first_requests_share_one_h2_connection() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, accepted, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let c = c.clone();
        let url = format!("https://localhost:{}/x", addr.port());
        tasks.push(tokio::spawn(async move {
            let resp = c
                .send(
                    &head("GET", &url, &[]),
                    RequestBody::Empty,
                    &RequestOptions::default(),
                )
                .await
                .unwrap();
            collect(resp.body).await.0
        }));
    }
    for t in tasks {
        assert_eq!(t.await.unwrap(), b"h2 GET /x");
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn h2_request_body_trailers_and_header_conversion() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let mut trailers = Headers::new();
    trailers.append("X-Req-Trailer", "t1");
    let body: akev_proxy::upstream::BodyStream = Box::pin(futures::stream::iter(
        vec![
            BodyChunk::Data(Bytes::from_static(b"abc")),
            BodyChunk::Data(Bytes::from(vec![b'd'; 100_000])),
            BodyChunk::Trailers(trailers),
        ]
        .into_iter()
        .map(Ok::<_, UpstreamError>),
    ));
    let h = head(
        "POST",
        &format!("https://localhost:{}/trailers", addr.port()),
        &[
            ("Host", "localhost"),
            ("Connection", "keep-alive"),
            ("Keep-Alive", "timeout=5"),
            ("Transfer-Encoding", "chunked"),
            ("TE", "trailers"),
        ],
    );
    let resp = c
        .send(
            &h,
            RequestBody::Stream {
                stream: body,
                length: None,
            },
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.http_version, HttpVersion::H2);
    let (data, trailers) = collect(resp.body).await;
    let text = String::from_utf8(data).unwrap();
    assert!(
        text.starts_with("h2 POST /trailers body=abcddd"),
        "{}",
        &text[..40]
    );
    assert!(text.ends_with("req-trailer=t1"));
    assert!(!text.contains("BAD-HEADERS"));
    assert_eq!(trailers.unwrap().get("x-trailer"), Some("yes"));
}

#[tokio::test]
async fn http1_only_negotiates_http11_over_tls() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, _, seen) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let options = RequestOptions {
        http_version: akev_proxy::upstream::UpstreamHttpVersion::Http1,
        ..Default::default()
    };
    let h = head(
        "GET",
        &format!("https://localhost:{}/h1", addr.port()),
        &[("Host", "localhost"), ("X-Case", "Kept")],
    );
    let resp = c.send(&h, RequestBody::Empty, &options).await.unwrap();
    assert_eq!(resp.head.http_version, HttpVersion::Http11);
    assert_eq!(resp.info.alpn.as_deref(), Some("http/1.1"));
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert_eq!(
        text,
        "GET /h1 HTTP/1.1\r\nHost: localhost\r\nX-Case: Kept\r\n\r\n"
    );
    assert_eq!(seen.lock()[0].alpn.as_deref(), Some("http/1.1"));

    // Pooled HTTP/1.1-over-TLS reuse.
    let resp = c.send(&h, RequestBody::Empty, &options).await.unwrap();
    assert!(resp.info.reused);
    collect(resp.body).await;
}

#[tokio::test]
async fn h1_only_server_is_remembered_and_pooled() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, accepted, _) = spawn_tls(server_config(chain, key, &[b"http/1.1"], None)).await;
    let c = client(&trusting(&ca, &dir));
    let url = format!("https://localhost:{}/", addr.port());
    for i in 0..3 {
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        assert_eq!(resp.head.http_version, HttpVersion::Http11);
        assert_eq!(resp.info.reused, i > 0);
        collect(resp.body).await;
    }
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn sni_is_omitted_for_ip_literals() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["127.0.0.1"], false);
    let (addr, _, seen) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let resp = c
        .send(
            &head("GET", &format!("https://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(seen.lock()[0].sni, None);
}

async fn expect_cert_error(settings: &ProxySettings, port: u16, host: &str) -> UpstreamError {
    let c = client(settings);
    let err = c
        .send(
            &head("GET", &format!("https://{host}:{port}/"), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .expect_err("certificate error expected");
    assert_eq!(err.kind, ErrorKind::UpstreamCertInvalid, "{err:?}");
    err
}

#[tokio::test]
async fn certificate_errors_are_classified() {
    let ca = TestCa::new("Trusted CA");
    let dir = tempfile::tempdir().unwrap();
    let settings = trusting(&ca, &dir);

    let (chain, key) = ca.leaf(&["localhost"], true);
    let (addr, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let err = expect_cert_error(&settings, addr.port(), "localhost").await;
    assert_eq!(err.code.as_deref(), Some("CERT_HAS_EXPIRED"));
    assert!(
        err.message.starts_with("certificate has expired"),
        "{}",
        err.message
    );
    assert!(err.message.contains("2002-01-01"), "{}", err.message);

    let (chain, key) = self_signed(&["localhost"]);
    let (addr, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let err = expect_cert_error(&settings, addr.port(), "localhost").await;
    assert_eq!(err.code.as_deref(), Some("SELF_SIGNED"));
    assert_eq!(err.message, "self-signed certificate");

    let (chain, key) = ca.leaf(&["other.test", "*.other.test"], false);
    let (addr, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let err = expect_cert_error(&settings, addr.port(), "localhost").await;
    assert_eq!(err.code.as_deref(), Some("HOSTNAME_MISMATCH"));
    assert_eq!(
        err.message,
        "hostname mismatch: cert valid for other.test, *.other.test"
    );

    let rogue = TestCa::new("Rogue CA");
    let (chain, key) = rogue.leaf(&["localhost"], false);
    let (addr, _, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let err = expect_cert_error(&settings, addr.port(), "localhost").await;
    assert_eq!(err.code.as_deref(), Some("UNKNOWN_ISSUER"));
    assert!(err.message.starts_with("unknown issuer"), "{}", err.message);
    assert!(err.message.contains("Rogue CA"), "{}", err.message);
}

#[tokio::test]
async fn ignore_cert_errors_by_glob_or_request_option() {
    let (chain, key) = self_signed(&["localhost"]);
    let (addr, accepted, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let url = format!("https://localhost:{}/", addr.port());

    for patterns in [
        vec!["local*".to_string()],
        vec!["*".to_string()],
        vec![format!("localhost:{}", addr.port())],
    ] {
        let c = client(&ProxySettings {
            ignore_upstream_cert_errors: patterns,
            ..Default::default()
        });
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        assert_eq!(resp.head.status, 200);
        // The certificate is still captured when verification is skipped.
        assert_eq!(resp.info.certificate.unwrap().subject, "CN=localhost");
        let chain = resp.info.chain.clone().expect("chain captured");
        assert_eq!(chain.trust.status, ChainTrustStatus::NotVerified);
        assert!(chain.trust.anchor.is_none());
        assert_eq!(chain.certificates[0].subject, "CN=localhost");
        collect(resp.body).await;
    }

    let c = client(&ProxySettings {
        ignore_upstream_cert_errors: vec!["other.test".into()],
        ..Default::default()
    });
    let err = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamCertInvalid);
    let insecure = RequestOptions {
        insecure: true,
        ..Default::default()
    };
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &insecure)
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    // A strict request must not reuse the insecure connection.
    let err = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamCertInvalid);
    assert!(accepted.load(Ordering::SeqCst) >= 6);
}

#[tokio::test]
async fn client_certificates_pem_and_pkcs12() {
    let server_ca = TestCa::new("Server CA");
    let client_ca = TestCa::new("Client CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = server_ca.leaf(&["localhost"], false);
    let (addr, _, seen) =
        spawn_tls(server_config(chain, key, ALPN_BOTH, Some(client_ca.der()))).await;
    let (pem, der) = client_ca.client_identity("akev-client");
    let pem_path = write_temp(&dir, "client.pem", pem.as_bytes());
    let url = format!("https://localhost:{}/mtls", addr.port());

    let mut settings = trusting(&server_ca, &dir);
    settings.client_certificates = vec![
        ClientCertificate {
            host: "*.nomatch.test".into(),
            path: "/nonexistent".into(),
            passphrase: None,
        },
        ClientCertificate {
            host: "local*".into(),
            path: pem_path,
            passphrase: None,
        },
    ];
    let c = client(&settings);
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
    assert_eq!(
        seen.lock().last().unwrap().client_cert.as_deref(),
        Some(der.as_slice())
    );

    // PKCS#12 with a passphrase.
    let (pem2, der2) = client_ca.client_identity("p12-client");
    let p12 = to_pkcs12(&pem2, "hunter2");
    let p12_path = write_temp(&dir, "client.p12", &p12);
    let mut settings = trusting(&server_ca, &dir);
    settings.client_certificates = vec![ClientCertificate {
        host: "localhost".into(),
        path: p12_path,
        passphrase: Some("hunter2".into()),
    }];
    let c = client(&settings);
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    collect(resp.body).await;
    assert_eq!(
        seen.lock().last().unwrap().client_cert.as_deref(),
        Some(der2.as_slice())
    );

    // No certificate: the server rejects the handshake.
    let c = client(&trusting(&server_ca, &dir));
    let err = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .err()
        .unwrap();
    assert!(
        matches!(
            err.kind,
            ErrorKind::UpstreamTlsFailed | ErrorKind::ConnectionReset
        ),
        "{err:?}"
    );

    // A configured but unloadable certificate fails loudly instead of connecting without it.
    let mut settings = trusting(&server_ca, &dir);
    settings.client_certificates = vec![ClientCertificate {
        host: "localhost".into(),
        path: "/nonexistent.p12".into(),
        passphrase: None,
    }];
    let err = client(&settings)
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .err()
        .unwrap();
    assert_eq!(err.code.as_deref(), Some("CLIENT_CERT_INVALID"));
}

fn to_pkcs12(pem: &str, pass: &str) -> Vec<u8> {
    use rustls_pki_types::pem::PemObject;
    let certs: Vec<rustls_pki_types::CertificateDer<'static>> =
        rustls_pki_types::CertificateDer::pem_slice_iter(pem.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap();
    let key = rustls_pki_types::PrivatePkcs8KeyDer::from_pem_slice(pem.as_bytes()).unwrap();
    let key = p12_keystore::PrivateKey::from_der(key.secret_pkcs8_der()).unwrap();
    let chain = p12_keystore::PrivateKeyChain::new(
        vec![7u8],
        key,
        certs
            .iter()
            .map(|c| p12_keystore::Certificate::from_der(c).unwrap()),
    );
    let mut ks = p12_keystore::KeyStore::new();
    ks.add_entry(
        "client",
        p12_keystore::KeyStoreEntry::PrivateKeyChain(chain),
    );
    ks.writer(pass).write().unwrap()
}

#[tokio::test]
async fn tls_to_a_plaintext_server_fails_cleanly() {
    let (addr, _) = spawn_http1(|req, _| echo_response(req)).await;
    let c = client(&ProxySettings::default());
    let err = c
        .send(
            &head("GET", &format!("https://127.0.0.1:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(err.kind, ErrorKind::UpstreamTlsFailed, "{err:?}");
}

#[tokio::test]
async fn settings_change_drops_tls_pools() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost", "alias.test"], false);
    let (addr, accepted, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let mut settings = trusting(&ca, &dir);
    let c = client(&settings);
    let url = format!("https://localhost:{}/", addr.port());
    collect(
        c.send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap()
            .body,
    )
    .await;
    settings.ignore_upstream_cert_errors = vec!["unrelated.test".into()];
    c.update_settings(&settings);
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(!resp.info.reused);
    collect(resp.body).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 2);

    // DNS overrides apply to TLS targets too (SNI/verification use the requested name).
    settings.dns_overrides = vec![DnsOverride {
        host: "alias.test".into(),
        address: "127.0.0.1".into(),
    }];
    c.update_settings(&settings);
    let resp = c
        .send(
            &head("GET", &format!("https://alias.test:{}/", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.status, 200);
    collect(resp.body).await;
}

#[tokio::test]
async fn wss_upgrade_uses_http11_over_tls() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    // Echo server: the "101" isn't produced by the echo handler, so expect a 200 response
    // body echoing the request, proving HTTP/1.1 (not h2) was used for the upgrade.
    let (addr, _, seen) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let h = head(
        "GET",
        &format!("wss://localhost:{}/ws", addr.port()),
        &[
            ("Host", "localhost"),
            ("Upgrade", "websocket"),
            ("Connection", "Upgrade"),
        ],
    );
    let (resp_head, upgraded, response, info) = c.send_upgrade(&h, &opts()).await.unwrap();
    assert_eq!(resp_head.status, 200);
    assert!(upgraded.is_none());
    assert_eq!(info.alpn.as_deref(), Some("http/1.1"));
    let text = String::from_utf8(collect(response.unwrap().body).await.0).unwrap();
    assert!(text.starts_with("GET /ws HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"));
    assert_eq!(seen.lock()[0].alpn.as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn close_delimited_tls_body_without_close_notify_completes() {
    let (chain, key) = self_signed(&["localhost"]);
    let acceptor = tokio_rustls::TlsAcceptor::from(server_config(chain, key, &[b"http/1.1"], None));
    let (addr, _) = spawn_server(move |stream, n| {
        let acceptor = acceptor.clone();
        async move {
            let mut tls = acceptor.accept(stream).await.unwrap();
            let mut buf = bytes::BytesMut::new();
            read_request(&mut tls, &mut buf).await;
            use tokio::io::AsyncWriteExt;
            let response: &[u8] = if n == 0 {
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nuntil the socket closes"
            } else {
                // A length-delimited body cut short is still a truncation.
                b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort"
            };
            tls.write_all(response).await.unwrap();
            tls.flush().await.unwrap();
            // Drop without `shutdown()`: TCP FIN, no TLS close_notify.
        }
    })
    .await;
    let c = client(&ProxySettings {
        ignore_upstream_cert_errors: vec!["*".into()],
        ..Default::default()
    });
    let url = format!("https://localhost:{}/", addr.port());
    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    let mut body = resp.body;
    let mut data = Vec::new();
    while let Some(item) = futures::StreamExt::next(&mut body).await {
        match item.expect("close-delimited body must end cleanly") {
            BodyChunk::Data(d) => data.extend_from_slice(&d),
            BodyChunk::Trailers(_) => panic!("unexpected trailers"),
        }
    }
    assert_eq!(data, b"until the socket closes");

    let resp = c
        .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
        .await
        .unwrap();
    assert!(!resp.info.reused);
    let mut body = resp.body;
    let mut last = None;
    while let Some(item) = futures::StreamExt::next(&mut body).await {
        last = Some(item);
    }
    let err = last.unwrap().unwrap_err();
    assert_eq!(err.kind, ErrorKind::ConnectionReset, "{err:?}");
}

#[tokio::test]
async fn upgrade_requests_sent_normally_do_not_offer_h2() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, _, seen) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let h = head(
        "GET",
        &format!("https://localhost:{}/up", addr.port()),
        &[
            ("Host", "localhost"),
            ("Connection", "Upgrade"),
            ("Upgrade", "foo/2"),
        ],
    );
    // The server ignores the upgrade and answers normally, over HTTP/1.1 with the
    // Upgrade headers intact.
    let resp = c.send(&h, RequestBody::Empty, &opts()).await.unwrap();
    assert_eq!(resp.info.alpn.as_deref(), Some("http/1.1"));
    let text = String::from_utf8(collect(resp.body).await.0).unwrap();
    assert!(text.contains("\r\nUpgrade: foo/2\r\n"), "{text}");
    assert_eq!(seen.lock()[0].alpn.as_deref(), Some("http/1.1"));

    // Ordinary requests to the same server still negotiate h2.
    let resp = c
        .send(
            &head("GET", &format!("https://localhost:{}/x", addr.port()), &[]),
            RequestBody::Empty,
            &opts(),
        )
        .await
        .unwrap();
    assert_eq!(resp.head.http_version, HttpVersion::H2);
    collect(resp.body).await;
}

/// Open `count` downloads of a large body whose consumers never read (slow clients, throttled
/// downloads): each pins a full stream window of unreleased data on the shared connection.
async fn stalled_downloads(
    c: &akev_proxy::upstream::UpstreamClient,
    port: u16,
    count: usize,
) -> Vec<akev_proxy::upstream::UpstreamResponse> {
    let mut stalled = Vec::new();
    for _ in 0..count {
        let url = format!("https://localhost:{port}/big/{}", 64 << 20);
        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            c.send(&head("GET", &url, &[]), RequestBody::Empty, &opts()),
        )
        .await
        .expect("download started")
        .unwrap();
        stalled.push(resp);
    }
    // Let the server fill the windows.
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    stalled
}

/// A small request to the same origin must complete promptly.
async fn small_request_completes(c: &akev_proxy::upstream::UpstreamClient, port: u16) {
    let url = format!("https://localhost:{port}/small");
    let done = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let resp = c
            .send(&head("GET", &url, &[]), RequestBody::Empty, &opts())
            .await
            .unwrap();
        collect(resp.body).await.0
    })
    .await
    .expect("a request to the origin was blocked by stalled downloads");
    assert_eq!(done, b"h2 GET /small");
}

#[tokio::test]
async fn stalled_h2_downloads_do_not_block_other_requests() {
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, accepted, _) = spawn_tls(server_config(chain, key, ALPN_BOTH, None)).await;
    let c = client(&trusting(&ca, &dir));
    let stalled = stalled_downloads(&c, addr.port(), 3).await;
    small_request_completes(&c, addr.port()).await;
    // Still multiplexed over the one connection.
    assert_eq!(accepted.load(Ordering::SeqCst), 1);
    drop(stalled);
}

#[tokio::test]
async fn saturated_h2_connection_gets_a_sibling() {
    // The server allows only two concurrent streams per connection and both are stalled.
    let ca = TestCa::new("CA");
    let dir = tempfile::tempdir().unwrap();
    let (chain, key) = ca.leaf(&["localhost"], false);
    let (addr, accepted, _) =
        spawn_tls_with(server_config(chain, key, ALPN_BOTH, None), Some(2)).await;
    let c = client(&trusting(&ca, &dir));
    // A first request so the server's SETTINGS are known.
    small_request_completes(&c, addr.port()).await;
    let stalled = stalled_downloads(&c, addr.port(), 2).await;
    small_request_completes(&c, addr.port()).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
    drop(stalled);
    // Once streams are free again, requests use the existing connections.
    small_request_completes(&c, addr.port()).await;
    assert_eq!(accepted.load(Ordering::SeqCst), 2);
}
