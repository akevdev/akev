//! Upstream TLS fingerprints: the ClientHello / HTTP/2 preface Akev sends, captured by local
//! rustls / h2 servers and fingerprinted with Akev's own ClientHello parser.
//!
//! * With `--features impersonate`: the Chrome, Firefox, Safari and Edge profiles must match
//!   the browsers' published JA4, cipher / extension / group / signature-algorithm lists and
//!   orders, and HTTP/2 SETTINGS / WINDOW_UPDATE / pseudo-header order; certificate
//!   verification and client certificates behave like the rustls path; rules and
//!   `mirror-client` pick the profile end to end through the proxy.
//! * Without it: browser profiles fall back to the native stack and say so.
//! * Always: native connections report the JA3/JA4 they actually sent.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
// Helpers shared with the feature-gated tests.
#![cfg_attr(not(feature = "impersonate"), allow(dead_code, unused_imports))]

mod common;
#[path = "common/upstream.rs"]
mod upstream_support;

use akev_core::settings::{
    CaptureSettings, ProxySettings, TlsProfile, TlsProfileOverride, TlsProfileSettings,
};
use akev_core::{NoBreakpoints, ProxyEvent, UpstreamFingerprint};
use akev_proxy::client_hello::{self, ClientHello, HelloParse, is_grease};
use akev_proxy::upstream::fingerprint::{self, Browser};
use akev_proxy::upstream::{RequestBody, RequestOptions, UpstreamResponse};
use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use upstream_support::{TestCa, head, server_config, write_temp};

// ---------------------------------------------------------------------------------------
// Capturing upstream server
// ---------------------------------------------------------------------------------------

/// What the capturing server saw on one connection.
#[derive(Debug, Clone)]
struct Captured {
    hello: ClientHello,
    /// Client bytes after the TLS handshake on HTTP/2 connections (preface, SETTINGS...).
    h2_bytes: Option<Vec<u8>>,
    client_cert: bool,
}

/// Replays already-read bytes before reading from the socket.
struct Rewind<S> {
    prefix: Bytes,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Rewind<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        if !self.prefix.is_empty() {
            let n = self.prefix.len().min(buf.remaining());
            buf.put_slice(&self.prefix.split_to(n));
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Rewind<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// Records everything read through it.
struct ReadRecorder<S> {
    inner: S,
    seen: Arc<Mutex<Vec<u8>>>,
}

impl<S: AsyncRead + Unpin> AsyncRead for ReadRecorder<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if poll.is_ready() {
            let new = buf.filled()[before..].to_vec();
            self.seen.lock().extend_from_slice(&new);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for ReadRecorder<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

/// A TLS server that parses each raw ClientHello before handing the connection to rustls,
/// then answers `200 ok` over HTTP/1.1 (`Connection: close`) or HTTP/2 (recording the client
/// preface).
async fn spawn_capture(
    config: Arc<rustls::ServerConfig>,
) -> (SocketAddr, mpsc::UnboundedReceiver<Captured>) {
    let (tx, rx) = mpsc::unbounded_channel();
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut tcp, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let tx = tx.clone();
            tokio::spawn(async move {
                let mut raw = Vec::new();
                let hello = loop {
                    let mut tmp = [0u8; 4096];
                    let n = match tcp.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    raw.extend_from_slice(&tmp[..n]);
                    match client_hello::parse(&raw) {
                        HelloParse::Complete(h) => break h,
                        HelloParse::NeedMore => continue,
                        HelloParse::Invalid(why) => panic!("not a ClientHello: {why}"),
                    }
                };
                let rewind = Rewind {
                    prefix: Bytes::from(raw),
                    inner: tcp,
                };
                let Ok(tls) = acceptor.accept(rewind).await else {
                    // Still report the hello (e.g. certificate tests where the client aborts).
                    let _ = tx.send(Captured {
                        hello,
                        h2_bytes: None,
                        client_cert: false,
                    });
                    return;
                };
                let (_, conn) = tls.get_ref();
                let client_cert = conn.peer_certificates().is_some_and(|c| !c.is_empty());
                if conn.alpn_protocol() == Some(b"h2") {
                    let seen = Arc::new(Mutex::new(Vec::new()));
                    let io = ReadRecorder {
                        inner: tls,
                        seen: seen.clone(),
                    };
                    let Ok(mut h2) = h2::server::handshake(io).await else {
                        return;
                    };
                    if let Some(Ok((_req, mut respond))) = h2.accept().await {
                        let _ = tx.send(Captured {
                            hello,
                            h2_bytes: Some(seen.lock().clone()),
                            client_cert,
                        });
                        let resp = http::Response::builder().status(200).body(()).unwrap();
                        if let Ok(mut body) = respond.send_response(resp, false) {
                            let _ = body.send_data(Bytes::from_static(b"ok"), true);
                        }
                        // Keep driving the connection until the client is done with it.
                        let _ = tokio::time::timeout(Duration::from_secs(5), async {
                            while let Some(Ok(_)) = h2.accept().await {}
                        })
                        .await;
                    }
                } else {
                    let mut tls = tls;
                    let _ = tx.send(Captured {
                        hello,
                        h2_bytes: None,
                        client_cert,
                    });
                    let mut buf = BytesMut::new();
                    if upstream_support::read_request(&mut tls, &mut buf)
                        .await
                        .is_some()
                    {
                        let _ = tls
                            .write_all(
                                b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
                            )
                            .await;
                        let _ = tls.shutdown().await;
                    }
                }
            });
        }
    });
    (addr, rx)
}

struct Pki {
    ca: TestCa,
    dir: tempfile::TempDir,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
}

fn pki() -> Pki {
    let ca = TestCa::new("Akev Impersonation Test CA");
    let (chain, key) = ca.leaf(&["localhost"], false);
    Pki {
        ca,
        dir: tempfile::tempdir().unwrap(),
        chain,
        key,
    }
}

impl Pki {
    fn server(&self, alpn: &[&[u8]]) -> Arc<rustls::ServerConfig> {
        server_config(self.chain.clone(), self.key.clone_key(), alpn, None)
    }

    /// Settings trusting the test CA (strict verification), with a TLS profile.
    fn settings(&self, profile: TlsProfile) -> ProxySettings {
        ProxySettings {
            trusted_ca_paths: vec![write_temp(&self.dir, "ca.pem", self.ca.pem().as_bytes())],
            tls_profile: TlsProfileSettings {
                profile,
                overrides: Vec::new(),
            },
            ..Default::default()
        }
    }
}

const H1: &[&[u8]] = &[b"http/1.1"];
const H2: &[&[u8]] = &[b"h2", b"http/1.1"];

async fn get(
    settings: &ProxySettings,
    port: u16,
    path: &str,
    options: &RequestOptions,
) -> UpstreamResponse {
    let client = upstream_support::client(settings);
    let url = format!("https://localhost:{port}{path}");
    let resp = client
        .send(&head("GET", &url, &[]), RequestBody::Empty, options)
        .await
        .unwrap_or_else(|e| panic!("request to {url} failed: {e:?}"));
    assert_eq!(resp.head.status, 200);
    let (body, _) = upstream_support::collect(resp.body).await;
    assert_eq!(body, b"ok");
    UpstreamResponse {
        body: Box::pin(futures::stream::empty()),
        ..resp
    }
}

async fn next(rx: &mut mpsc::UnboundedReceiver<Captured>) -> Captured {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("a captured connection")
        .expect("capture server alive")
}

fn non_grease(v: &[u16]) -> Vec<u16> {
    v.iter().copied().filter(|x| !is_grease(*x)).collect()
}

fn fingerprint_of(resp: &UpstreamResponse) -> UpstreamFingerprint {
    resp.info
        .fingerprint
        .as_deref()
        .cloned()
        .expect("TLS connections report their fingerprint")
}

// ---------------------------------------------------------------------------------------
// HTTP/2 preface parsing (Akamai fingerprint)
// ---------------------------------------------------------------------------------------

/// Akamai fingerprint (`settings|window|priority frames|pseudo order`) and the first HEADERS
/// frame's priority `(exclusive, dependency, weight)`, from raw client bytes.
fn h2_fingerprint(bytes: &[u8]) -> (String, Option<(bool, u32, u8)>) {
    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
    assert!(bytes.starts_with(PREFACE), "no HTTP/2 client preface");
    let mut pos = PREFACE.len();
    let mut settings = Vec::new();
    let mut window = None;
    let mut priorities = Vec::new();
    loop {
        assert!(bytes.len() >= pos + 9, "truncated frame header");
        let len = u32::from_be_bytes([0, bytes[pos], bytes[pos + 1], bytes[pos + 2]]) as usize;
        let ty = bytes[pos + 3];
        let flags = bytes[pos + 4];
        let stream = u32::from_be_bytes(bytes[pos + 5..pos + 9].try_into().unwrap()) & 0x7fff_ffff;
        let payload = &bytes[pos + 9..pos + 9 + len];
        pos += 9 + len;
        match ty {
            // SETTINGS (not the ACK)
            4 if flags & 1 == 0 => {
                for pair in payload.as_chunks::<6>().0 {
                    let id = u16::from_be_bytes([pair[0], pair[1]]);
                    let value = u32::from_be_bytes(pair[2..6].try_into().unwrap());
                    settings.push(format!("{id}:{value}"));
                }
            }
            // WINDOW_UPDATE on the connection
            8 if stream == 0 && window.is_none() => {
                window = Some(u32::from_be_bytes(payload.try_into().unwrap()) & 0x7fff_ffff);
            }
            // PRIORITY
            2 => priorities.push(stream),
            // HEADERS
            1 => {
                let mut block = payload;
                let mut priority = None;
                if flags & 0x8 != 0 {
                    let pad = block[0] as usize;
                    block = &block[1..block.len() - pad];
                }
                if flags & 0x20 != 0 {
                    let dep = u32::from_be_bytes(block[..4].try_into().unwrap());
                    priority = Some((dep & 0x8000_0000 != 0, dep & 0x7fff_ffff, block[4]));
                    block = &block[5..];
                }
                let pseudo = pseudo_order(block);
                let fp = format!(
                    "{}|{}|{}|{}",
                    settings.join(";"),
                    window.map_or("00".to_string(), |w| w.to_string()),
                    if priorities.is_empty() {
                        "0".to_string()
                    } else {
                        format!("{} PRIORITY frames", priorities.len())
                    },
                    pseudo.join(",")
                );
                return (fp, priority);
            }
            _ => {}
        }
    }
}

/// HPACK integer with an `n`-bit prefix.
fn hpack_int(block: &[u8], pos: &mut usize, n: u8) -> usize {
    let mask = (1u16 << n) as usize - 1;
    let mut value = block[*pos] as usize & mask;
    *pos += 1;
    if value < mask {
        return value;
    }
    let mut shift = 0;
    loop {
        let b = block[*pos] as usize;
        *pos += 1;
        value += (b & 0x7f) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return value;
        }
    }
}

/// Order of the pseudo-header fields at the start of an HPACK block (names come from the
/// static table, as every encoder does for them).
fn pseudo_order(block: &[u8]) -> Vec<&'static str> {
    let static_name = |i: usize| match i {
        1 => Some("a"),
        2 | 3 => Some("m"),
        4 | 5 => Some("p"),
        6 | 7 => Some("s"),
        _ => None,
    };
    let mut pos = 0;
    let mut out = Vec::new();
    while pos < block.len() && out.len() < 4 {
        let b = block[pos];
        let index = if b & 0x80 != 0 {
            hpack_int(block, &mut pos, 7)
        } else if b & 0xc0 == 0x40 {
            let i = hpack_int(block, &mut pos, 6);
            skip_literal_value(block, &mut pos, i);
            i
        } else if b & 0xe0 == 0x20 {
            hpack_int(block, &mut pos, 5); // table size update
            continue;
        } else {
            let i = hpack_int(block, &mut pos, 4);
            skip_literal_value(block, &mut pos, i);
            i
        };
        match static_name(index) {
            Some(name) => out.push(name),
            None => break,
        }
    }
    out
}

fn skip_literal_value(block: &[u8], pos: &mut usize, name_index: usize) {
    if name_index == 0 {
        let len = hpack_int(block, pos, 7);
        *pos += len;
    }
    let len = hpack_int(block, pos, 7);
    *pos += len;
}

// ---------------------------------------------------------------------------------------
// Always: the native stack reports what it sent
// ---------------------------------------------------------------------------------------

#[tokio::test]
async fn native_connections_report_the_ja3_and_ja4_they_sent() {
    let pki = pki();
    let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
    let resp = get(
        &pki.settings(TlsProfile::Native),
        addr.port(),
        "/",
        &RequestOptions::default(),
    )
    .await;
    let seen = next(&mut rx).await;
    let fp = fingerprint_of(&resp);
    assert_eq!(fp.profile, "native");
    assert_eq!(fp.requested, None);
    assert_eq!(fp.note, None);
    assert_eq!(fp.ja4.as_deref(), Some(seen.hello.ja4().as_str()));
    assert_eq!(fp.ja3.as_deref(), Some(seen.hello.ja3().as_str()));
    assert_eq!(
        fp.ja3_text.as_deref(),
        Some(seen.hello.ja3_string().as_str())
    );
    // rustls: TLS 1.3, SNI, h2 + http/1.1 offered, no GREASE.
    let ja4 = fp.ja4.unwrap();
    assert!(ja4.starts_with("t13d") && &ja4[8..10] == "h2", "{ja4}");
    assert!(!seen.hello.cipher_suites.iter().any(|c| is_grease(*c)));
    for browser in Browser::ALL {
        assert_ne!(ja4, browser.ja4(), "native must not look like {browser:?}");
    }
}

#[tokio::test]
async fn native_http2_preface_is_reported() {
    let pki = pki();
    let (addr, mut rx) = spawn_capture(pki.server(H2)).await;
    let resp = get(
        &pki.settings(TlsProfile::Native),
        addr.port(),
        "/",
        &RequestOptions::default(),
    )
    .await;
    assert_eq!(resp.info.alpn.as_deref(), Some("h2"));
    let seen = next(&mut rx).await;
    let (akamai, priority) = h2_fingerprint(&seen.h2_bytes.expect("h2 bytes"));
    assert_eq!(akamai, "2:0;4:2097152|1073676289|0|m,s,a,p");
    assert_eq!(priority, None);
    assert_eq!(
        fingerprint_of(&resp).http2.as_deref(),
        Some(akamai.as_str())
    );
}

#[tokio::test]
async fn the_catalog_reflects_the_build() {
    let catalog = fingerprint::catalog();
    assert_eq!(catalog.len(), TlsProfile::ALL.len());
    assert_eq!(fingerprint::available(), cfg!(feature = "impersonate"));
    for p in &catalog {
        assert_eq!(
            p.available,
            p.id == "native" || cfg!(feature = "impersonate"),
            "{}",
            p.id
        );
    }
}

// ---------------------------------------------------------------------------------------
// Without the backend: graceful fallback
// ---------------------------------------------------------------------------------------

#[cfg(not(feature = "impersonate"))]
mod fallback {
    use super::*;

    #[tokio::test]
    async fn browser_profiles_fall_back_to_native_and_say_why() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        for profile in [
            TlsProfile::Chrome,
            TlsProfile::Firefox,
            TlsProfile::Safari,
            TlsProfile::Edge,
        ] {
            let resp = get(
                &pki.settings(profile),
                addr.port(),
                "/",
                &RequestOptions::default(),
            )
            .await;
            let seen = next(&mut rx).await;
            let fp = fingerprint_of(&resp);
            assert_eq!(fp.profile, "native");
            assert_eq!(fp.requested.as_deref(), Some(profile.as_str()));
            assert!(
                fp.note.as_deref().unwrap().contains("impersonation build"),
                "{fp:?}"
            );
            // What went out is rustls' hello, not the browser's.
            assert_eq!(fp.ja4.as_deref(), Some(seen.hello.ja4().as_str()));
            assert_ne!(
                seen.hello.ja4(),
                Browser::from_profile(profile).unwrap().ja4()
            );
            assert!(!seen.hello.cipher_suites.iter().any(|c| is_grease(*c)));
        }
    }

    #[tokio::test]
    async fn rule_overrides_fall_back_too() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let options = RequestOptions {
            fingerprint: fingerprint::FingerprintOptions {
                rule: Some(TlsProfile::Firefox),
                client: Some(Browser::Chrome),
            },
            ..Default::default()
        };
        let resp = get(
            &pki.settings(TlsProfile::Native),
            addr.port(),
            "/",
            &options,
        )
        .await;
        let _ = next(&mut rx).await;
        let fp = fingerprint_of(&resp);
        assert_eq!(
            (fp.profile.as_str(), fp.requested.as_deref()),
            ("native", Some("firefox"))
        );
    }
}

// ---------------------------------------------------------------------------------------
// With the BoringSSL backend
// ---------------------------------------------------------------------------------------

#[cfg(feature = "impersonate")]
mod impersonated {
    use super::*;

    /// `application_settings` (ALPS, new codepoint).
    const ALPS: u16 = 17613;

    /// Check a captured hello against a browser profile's expected ClientHello.
    fn assert_matches(browser: Browser, hello: &ClientHello, alpn_h2: bool) {
        let expected = browser.expected_hello();
        assert_eq!(
            non_grease(&hello.cipher_suites),
            expected.ciphers,
            "{browser:?} ciphers"
        );
        assert_eq!(
            non_grease(&hello.groups),
            expected.groups,
            "{browser:?} groups"
        );
        assert_eq!(
            hello.signature_algorithms, expected.signature_algorithms,
            "{browser:?} signature algorithms"
        );
        // ALPS (application_settings) is only sent along with an `h2` offer.
        let want_exts: Vec<u16> = expected
            .extensions
            .iter()
            .copied()
            .filter(|&e| alpn_h2 || e != ALPS)
            .collect();
        let mut exts = non_grease(&hello.extensions);
        if expected.extension_order_randomized {
            let mut want = want_exts.clone();
            exts.sort_unstable();
            want.sort_unstable();
            assert_eq!(exts, want, "{browser:?} extension set");
        } else {
            assert_eq!(exts, want_exts, "{browser:?} extension order");
        }
        assert_eq!(non_grease(&hello.supported_versions), [0x0304u16, 0x0303]);
        assert_eq!(hello.ec_point_formats, [0u8]);
        assert_eq!(hello.legacy_version, 0x0303);
        assert_eq!(hello.sni.as_deref(), Some("localhost"));
        let grease = |v: &[u16]| v.iter().any(|x| is_grease(*x));
        assert_eq!(
            grease(&hello.cipher_suites),
            expected.grease,
            "{browser:?} GREASE"
        );
        assert_eq!(grease(&hello.extensions), expected.grease);
        assert_eq!(grease(&hello.groups), expected.grease);
        assert_eq!(grease(&hello.supported_versions), expected.grease);
        if expected.grease {
            // Browser GREASE placement: first cipher, first group, first version, and the
            // first and last-but-padding extensions.
            assert!(is_grease(hello.cipher_suites[0]));
            assert!(is_grease(hello.groups[0]));
            assert!(is_grease(hello.supported_versions[0]));
            assert!(is_grease(hello.extensions[0]));
        }
        if alpn_h2 {
            assert_eq!(hello.alpn, ["h2", "http/1.1"]);
            assert_eq!(hello.ja4(), expected.ja4, "{browser:?} JA4");
        } else {
            assert_eq!(hello.alpn, ["http/1.1"]);
            // The JA4 of the browser's own http/1.1-only hello (e.g. Chrome's WebSockets).
            let reference = ClientHello {
                legacy_version: 0x0303,
                cipher_suites: expected.ciphers.to_vec(),
                extensions: want_exts,
                sni: Some("localhost".into()),
                alpn: vec!["http/1.1".into()],
                supported_versions: vec![0x0304, 0x0303],
                groups: expected.groups.to_vec(),
                ec_point_formats: vec![0],
                signature_algorithms: expected.signature_algorithms.to_vec(),
            };
            assert_eq!(
                hello.ja4(),
                reference.ja4(),
                "{browser:?} JA4 (http/1.1 only)"
            );
            assert_eq!(&hello.ja4()[8..10], "h1");
        }
    }

    #[tokio::test]
    async fn chrome_hello_matches_chrome() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let resp = get(
            &pki.settings(TlsProfile::Chrome),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        let seen = next(&mut rx).await;
        assert_matches(Browser::Chrome, &seen.hello, true);
        assert_eq!(seen.hello.ja4(), "t13d1516h2_8daaf6152771_d8a2da3f94cd");
        // GREASE last extension (ECH GREASE and ALPS present, no padding: the hello with the
        // X25519MLKEM768 share is > 512 bytes).
        assert!(is_grease(*seen.hello.extensions.last().unwrap()));
        assert!(!seen.hello.extensions.contains(&21));

        let fp = fingerprint_of(&resp);
        assert_eq!(fp.profile, "chrome");
        assert_eq!(fp.requested, None);
        assert_eq!(
            fp.ja4.as_deref(),
            Some("t13d1516h2_8daaf6152771_d8a2da3f94cd")
        );
        assert_eq!(fp.ja3.as_deref(), Some(seen.hello.ja3().as_str()));
        // Negotiated via BoringSSL; verified with the test CA (strict mode).
        assert_eq!(resp.info.tls_version.as_deref(), Some("TLSv1.3"));
        assert_eq!(resp.info.cipher.as_deref(), Some("TLS_AES_128_GCM_SHA256"));
        assert_eq!(resp.info.alpn.as_deref(), Some("http/1.1"));
        assert_eq!(
            resp.info.certificate.as_ref().map(|c| c.subject.as_str()),
            Some("CN=localhost")
        );
    }

    #[tokio::test]
    async fn chrome_permutes_extensions_per_connection_and_reports_each_ja3() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let settings = pki.settings(TlsProfile::Chrome);
        let mut orders = std::collections::HashSet::new();
        let mut ja3s = std::collections::HashSet::new();
        for _ in 0..6 {
            // `Connection: close` responses: every request opens a new connection.
            let resp = get(&settings, addr.port(), "/", &RequestOptions::default()).await;
            let seen = next(&mut rx).await;
            assert_matches(Browser::Chrome, &seen.hello, true);
            let fp = fingerprint_of(&resp);
            assert_eq!(fp.ja3.as_deref(), Some(seen.hello.ja3().as_str()));
            assert_eq!(fp.ja4.as_deref(), Some(Browser::Chrome.ja4()));
            orders.insert(non_grease(&seen.hello.extensions));
            ja3s.insert(fp.ja3.unwrap());
        }
        assert!(orders.len() > 1, "extension order must be randomised");
        assert_eq!(
            ja3s.len(),
            orders.len(),
            "JA3 follows the order (as with real Chrome)"
        );
    }

    #[tokio::test]
    async fn firefox_hello_matches_firefox_exactly() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let resp = get(
            &pki.settings(TlsProfile::Firefox),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        let seen = next(&mut rx).await;
        assert_matches(Browser::Firefox, &seen.hello, true);
        assert_eq!(seen.hello.ja4(), "t13d1717h2_5b57614c22b0_3cbfd9057e0d");
        // Fixed order and no GREASE: the JA3 is deterministic.
        let ja3 = "771,4865-4867-4866-49195-49199-52393-52392-49196-49200-49162-49161-49171-49172-156-157-47-53,0-23-65281-10-11-35-16-5-34-18-51-43-13-45-28-27-65037,4588-29-23-24-25-256-257,0";
        assert_eq!(seen.hello.ja3_string(), ja3);
        // The JA3 published for real Firefox 135+.
        assert_eq!(seen.hello.ja3(), "6f7889b9fb1a62a9577e685c1fcfa919");
        let fp = fingerprint_of(&resp);
        assert_eq!(fp.profile, "firefox");
        assert_eq!(fp.ja3_text.as_deref(), Some(ja3));
        assert_eq!(fp.ja3.as_deref(), Some(seen.hello.ja3().as_str()));

        // A second connection sends the same hello.
        let _ = get(
            &pki.settings(TlsProfile::Firefox),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        assert_eq!(next(&mut rx).await.hello.ja3_string(), ja3);
    }

    #[tokio::test]
    async fn safari_hello_matches_safari() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let resp = get(
            &pki.settings(TlsProfile::Safari),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        let seen = next(&mut rx).await;
        assert_matches(Browser::Safari, &seen.hello, true);
        assert_eq!(seen.hello.ja4(), "t13d2014h2_a09f3c656075_14788d8d241b");
        // JA3 ignores GREASE, so Safari's is stable: the one published for real Safari 17/18.
        assert_eq!(
            seen.hello.ja3_string(),
            "771,4865-4866-4867-49196-49195-52393-49200-49199-52392-49162-49161-49172-49171-157-156-53-47-49160-49170-10,0-23-65281-10-11-16-5-13-18-51-45-43-27-21,29-23-24-25,0"
        );
        assert_eq!(seen.hello.ja3(), "773906b0efdefa24a7f2b8eb6985bf37");
        // Safari: GREASE first and before the trailing padding extension.
        let n = seen.hello.extensions.len();
        assert_eq!(seen.hello.extensions[n - 1], 21);
        assert!(is_grease(seen.hello.extensions[n - 2]));
        assert_eq!(fingerprint_of(&resp).profile, "safari");
    }

    #[tokio::test]
    async fn edge_presents_chromiums_fingerprint() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let resp = get(
            &pki.settings(TlsProfile::Edge),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        let seen = next(&mut rx).await;
        assert_matches(Browser::Edge, &seen.hello, true);
        assert_eq!(seen.hello.ja4(), Browser::Chrome.ja4());
        assert_eq!(fingerprint_of(&resp).profile, "edge");
    }

    #[tokio::test]
    async fn http1_only_requests_offer_only_http1_like_browsers_do() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        for browser in Browser::ALL {
            let options = RequestOptions {
                http_version: akev_proxy::upstream::UpstreamHttpVersion::Http1,
                ..Default::default()
            };
            get(&pki.settings(browser.profile()), addr.port(), "/", &options).await;
            let seen = next(&mut rx).await;
            assert_matches(browser, &seen.hello, false);
        }
    }

    #[tokio::test]
    async fn websocket_upgrades_are_impersonated_too() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let client = upstream_support::client(&pki.settings(TlsProfile::Chrome));
        let url = format!("wss://localhost:{}/socket", addr.port());
        let upgrade = head(
            "GET",
            &url,
            &[
                ("Connection", "Upgrade"),
                ("Upgrade", "websocket"),
                ("Sec-WebSocket-Version", "13"),
                ("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ=="),
            ],
        );
        // The capture server answers 200 instead of 101: enough to see the handshake.
        let (response_head, upgraded, _, info) = client
            .send_upgrade(&upgrade, &RequestOptions::default())
            .await
            .unwrap();
        assert_eq!(response_head.status, 200);
        assert!(upgraded.is_none());
        let seen = next(&mut rx).await;
        assert_matches(Browser::Chrome, &seen.hello, false);
        let fp = info.fingerprint.expect("fingerprint");
        assert_eq!(fp.profile, "chrome");
        assert_eq!(fp.ja4.as_deref(), Some(seen.hello.ja4().as_str()));
    }

    #[tokio::test]
    async fn http2_prefaces_match_the_browsers() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H2)).await;
        let cases = [
            (
                Browser::Chrome,
                "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p",
                Some((true, 0, 255)),
            ),
            (
                Browser::Firefox,
                "1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s",
                Some((false, 0, 41)),
            ),
            (
                Browser::Safari,
                "2:0;3:100;4:2097152;9:1|10420225|0|m,s,a,p",
                None,
            ),
            (
                Browser::Edge,
                "1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p",
                Some((true, 0, 255)),
            ),
        ];
        for (browser, akamai, priority) in cases {
            let resp = get(
                &pki.settings(browser.profile()),
                addr.port(),
                "/",
                &RequestOptions::default(),
            )
            .await;
            assert_eq!(resp.info.alpn.as_deref(), Some("h2"), "{browser:?}");
            let seen = next(&mut rx).await;
            assert_matches(browser, &seen.hello, true);
            let (captured, captured_priority) = h2_fingerprint(&seen.h2_bytes.unwrap());
            assert_eq!(captured, akamai, "{browser:?} HTTP/2 preface");
            assert_eq!(captured_priority, priority, "{browser:?} HEADERS priority");
            assert_eq!(browser.http2_fingerprint(), akamai);
            let fp = fingerprint_of(&resp);
            assert_eq!(fp.http2.as_deref(), Some(akamai), "{browser:?} reported");
            assert_eq!(fp.profile, browser.as_str());
        }
    }

    #[tokio::test]
    async fn impersonated_http2_connections_multiplex_many_streams() {
        // Chrome's connection window (15 MiB) only covers two 6 MiB stream windows; it grows
        // as streams open so that concurrent requests still share one connection.
        let pki = pki();
        let (chain, key) = (pki.chain.clone(), pki.key.clone_key());
        let (addr, accepted, _) =
            upstream_support::spawn_tls(server_config(chain, key, H2, None)).await;
        let client = upstream_support::client(&pki.settings(TlsProfile::Chrome));
        let mut tasks = Vec::new();
        for i in 0..12usize {
            let client = client.clone();
            let url = format!("https://localhost:{}/big/{}", addr.port(), 300_000 + i);
            tasks.push(tokio::spawn(async move {
                let resp = client
                    .send(
                        &head("GET", &url, &[]),
                        RequestBody::Empty,
                        &RequestOptions::default(),
                    )
                    .await
                    .unwrap();
                assert_eq!(resp.head.status, 200);
                let (body, _) = upstream_support::collect(resp.body).await;
                assert_eq!(body.len(), 300_000 + i);
                resp.info.fingerprint.unwrap().profile.clone()
            }));
        }
        for t in tasks {
            assert_eq!(t.await.unwrap(), "chrome");
        }
        assert_eq!(
            accepted.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "all streams on one connection"
        );
    }

    #[tokio::test]
    async fn stalled_streams_do_not_starve_an_impersonated_connection() {
        // Three responses larger than Chrome's 6 MiB stream window that nobody reads would
        // take 18 MiB of connection window — more than Chrome's initial 15 MiB. The window
        // grows with the streams, so a fourth request on the same connection still completes.
        let pki = pki();
        let (chain, key) = (pki.chain.clone(), pki.key.clone_key());
        let (addr, accepted, _) =
            upstream_support::spawn_tls(server_config(chain, key, H2, None)).await;
        let client = upstream_support::client(&pki.settings(TlsProfile::Chrome));
        let mut stalled = Vec::new();
        for _ in 0..3 {
            let url = format!("https://localhost:{}/big/7000000", addr.port());
            let resp = client
                .send(
                    &head("GET", &url, &[]),
                    RequestBody::Empty,
                    &RequestOptions::default(),
                )
                .await
                .unwrap();
            assert_eq!(resp.head.status, 200);
            stalled.push(resp); // body never polled
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let url = format!("https://localhost:{}/big/100000", addr.port());
        let resp = tokio::time::timeout(Duration::from_secs(10), async {
            let resp = client
                .send(
                    &head("GET", &url, &[]),
                    RequestBody::Empty,
                    &RequestOptions::default(),
                )
                .await
                .unwrap();
            upstream_support::collect(resp.body).await
        })
        .await
        .expect("the connection must not be starved by stalled streams");
        assert_eq!(resp.0.len(), 100_000);
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
        drop(stalled);
    }

    #[tokio::test]
    async fn certificate_errors_are_reported_like_the_native_stack() {
        let pki = pki();
        let (addr, _rx) = spawn_capture(pki.server(H1)).await;
        // Not trusting the test CA.
        let untrusting = ProxySettings {
            tls_profile: TlsProfileSettings {
                profile: TlsProfile::Chrome,
                overrides: Vec::new(),
            },
            ..Default::default()
        };
        let client = upstream_support::client(&untrusting);
        let url = format!("https://localhost:{}/", addr.port());
        let err = client
            .send(
                &head("GET", &url, &[]),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(
            err.kind,
            akev_core::ErrorKind::UpstreamCertInvalid,
            "{err:?}"
        );
        assert_eq!(err.code.as_deref(), Some("UNKNOWN_ISSUER"), "{err:?}");
        assert!(
            err.message.contains("Akev Impersonation Test CA"),
            "{err:?}"
        );

        // Hostname mismatch (certificate for localhost, connecting by IP).
        let ip_url = format!("https://127.0.0.1:{}/", addr.port());
        let err = upstream_support::client(&pki.settings(TlsProfile::Firefox))
            .send(
                &head("GET", &ip_url, &[]),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("HOSTNAME_MISMATCH"), "{err:?}");

        // Ignored for this host by settings → works.
        let lax = ProxySettings {
            ignore_upstream_cert_errors: vec!["localhost".into()],
            ..untrusting.clone()
        };
        let resp = get(&lax, addr.port(), "/", &RequestOptions::default()).await;
        assert_eq!(fingerprint_of(&resp).profile, "chrome");
        // ...or by the request (Compose's "ignore certificate errors").
        let insecure = RequestOptions {
            insecure: true,
            ..Default::default()
        };
        get(&untrusting, addr.port(), "/", &insecure).await;
    }

    #[tokio::test]
    async fn expired_certificates_fail_with_the_native_code() {
        let pki = pki();
        let (chain, key) = pki.ca.leaf(&["localhost"], true);
        let (addr, _rx) = spawn_capture(server_config(chain, key, H1, None)).await;
        let err = upstream_support::client(&pki.settings(TlsProfile::Safari))
            .send(
                &head("GET", &format!("https://localhost:{}/", addr.port()), &[]),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("CERT_HAS_EXPIRED"), "{err:?}");
    }

    #[tokio::test]
    async fn client_certificates_are_presented() {
        let pki = pki();
        let client_ca = TestCa::new("Client CA");
        let (identity_pem, _) = client_ca.client_identity("akev-client");
        let config = server_config(
            pki.chain.clone(),
            pki.key.clone_key(),
            H1,
            Some(client_ca.der()),
        );
        let (addr, mut rx) = spawn_capture(config).await;
        let mut settings = pki.settings(TlsProfile::Chrome);
        settings.client_certificates = vec![akev_core::settings::ClientCertificate {
            host: "localhost".into(),
            path: write_temp(&pki.dir, "client.pem", identity_pem.as_bytes()),
            passphrase: None,
        }];
        get(&settings, addr.port(), "/", &RequestOptions::default()).await;
        let seen = next(&mut rx).await;
        assert!(
            seen.client_cert,
            "the server received the client certificate"
        );
        assert_matches(Browser::Chrome, &seen.hello, true);
    }

    #[tokio::test]
    async fn tls_alerts_map_to_the_native_codes() {
        // A server that only speaks TLS 1.2 with a cipher none of the profiles offer.
        let pki = pki();
        let provider = rustls::crypto::CryptoProvider {
            cipher_suites: vec![rustls::crypto::ring::cipher_suite::TLS13_CHACHA20_POLY1305_SHA256],
            ..rustls::crypto::ring::default_provider()
        };
        let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(provider))
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(pki.chain.clone(), pki.key.clone_key())
            .unwrap();
        cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
        let (addr, _rx) = spawn_capture(Arc::new(cfg)).await;
        // Chrome offers ChaCha20 for TLS 1.3, so this works...
        get(
            &pki.settings(TlsProfile::Chrome),
            addr.port(),
            "/",
            &RequestOptions::default(),
        )
        .await;
        // ...and a server refusing the connection mid-handshake yields TLS_ALERT_*.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
                // Fatal handshake_failure alert.
                let _ = s
                    .write_all(&[0x15, 0x03, 0x03, 0x00, 0x02, 0x02, 0x28])
                    .await;
            }
        });
        let err = upstream_support::client(&pki.settings(TlsProfile::Firefox))
            .send(
                &head("GET", &format!("https://localhost:{port}/"), &[]),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, akev_core::ErrorKind::UpstreamTlsFailed, "{err:?}");
        assert_eq!(
            err.code.as_deref(),
            Some("TLS_ALERT_HANDSHAKE_FAILURE"),
            "{err:?}"
        );
        // A server closing the connection.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let mut buf = vec![0u8; 4096];
                let _ = s.read(&mut buf).await;
            }
        });
        let err = upstream_support::client(&pki.settings(TlsProfile::Safari))
            .send(
                &head("GET", &format!("https://localhost:{port}/"), &[]),
                RequestBody::Empty,
                &RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert_eq!(err.kind, akev_core::ErrorKind::UpstreamTlsFailed, "{err:?}");
        assert_eq!(err.code.as_deref(), Some("TLS_HANDSHAKE_EOF"), "{err:?}");
    }

    #[tokio::test]
    async fn per_host_overrides_and_rules_pick_the_profile() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let mut settings = pki.settings(TlsProfile::Native);
        settings.tls_profile.overrides = vec![TlsProfileOverride {
            host: "localhost".into(),
            profile: TlsProfile::Firefox,
        }];
        get(&settings, addr.port(), "/", &RequestOptions::default()).await;
        assert_eq!(next(&mut rx).await.hello.ja4(), Browser::Firefox.ja4());
        let rule = RequestOptions {
            fingerprint: fingerprint::FingerprintOptions {
                rule: Some(TlsProfile::Safari),
                client: None,
            },
            ..Default::default()
        };
        let resp = get(&settings, addr.port(), "/", &rule).await;
        assert_eq!(next(&mut rx).await.hello.ja4(), Browser::Safari.ja4());
        assert_eq!(fingerprint_of(&resp).profile, "safari");
        // A rule can force the native stack for a host with an override.
        let native = RequestOptions {
            fingerprint: fingerprint::FingerprintOptions {
                rule: Some(TlsProfile::Native),
                client: None,
            },
            ..Default::default()
        };
        let resp = get(&settings, addr.port(), "/", &native).await;
        let seen = next(&mut rx).await;
        assert!(!seen.hello.cipher_suites.iter().any(|c| is_grease(*c)));
        assert_eq!(fingerprint_of(&resp).profile, "native");
    }

    // ---- end to end through the proxy ----------------------------------------------------

    const FIREFOX_UA: &str =
        "Mozilla/5.0 (X11; Linux x86_64; rv:144.0) Gecko/20100101 Firefox/144.0";

    async fn proxy_with(settings: ProxySettings, rules_json: &str) -> common::TestProxy {
        common::start_proxy_with(
            common::rules(rules_json),
            Arc::new(NoBreakpoints),
            settings,
            CaptureSettings {
                detect_processes: false,
                ..Default::default()
            },
        )
        .await
    }

    fn upstream_fingerprint(events: &[ProxyEvent]) -> UpstreamFingerprint {
        events
            .iter()
            .find_map(|e| match e {
                ProxyEvent::Upstream { info, .. } => info.fingerprint.as_deref().cloned(),
                _ => None,
            })
            .expect("an Upstream event with a fingerprint")
    }

    #[tokio::test]
    async fn mirror_client_follows_the_intercepted_browser() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let mut settings = pki.settings(TlsProfile::MirrorClient);
        settings.port = 0;
        let mut proxy = proxy_with(settings, r#"{"items":[]}"#).await;
        let client = common::client(&proxy);

        let url = format!("https://localhost:{}/firefox", addr.port());
        let resp = client
            .get(&url)
            .header("user-agent", FIREFOX_UA)
            .send()
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(next(&mut rx).await.hello.ja4(), Browser::Firefox.ja4());
        let (_, events) = common::wait_exchange(&mut proxy, "/firefox", common::is_completed).await;
        let fp = upstream_fingerprint(&events);
        assert_eq!(fp.profile, "firefox");
        assert_eq!(fp.requested.as_deref(), Some("mirror-client"));

        // A non-browser client (reqwest's own User-Agent and TLS stack) gets the native stack.
        let url = format!("https://localhost:{}/script", addr.port());
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
        let seen = next(&mut rx).await;
        assert!(!seen.hello.cipher_suites.iter().any(|c| is_grease(*c)));
        let (_, events) = common::wait_exchange(&mut proxy, "/script", common::is_completed).await;
        let fp = upstream_fingerprint(&events);
        assert_eq!(fp.profile, "native");
        assert!(fp.note.unwrap().contains("recognised browser"));
    }

    #[tokio::test]
    async fn tls_profile_rules_apply_through_the_proxy() {
        let pki = pki();
        let (addr, mut rx) = spawn_capture(pki.server(H1)).await;
        let mut settings = pki.settings(TlsProfile::Native);
        settings.port = 0;
        let mut proxy = proxy_with(
            settings,
            r#"{"items":[{"type":"rule","id":"safari","matchers":[{"type":"path","pattern":{"op":"equals","value":"/safari"}}],
                "action":{"type":"tls-profile","profile":"safari"}}]}"#,
        )
        .await;
        let client = common::client(&proxy);
        let url = format!("https://localhost:{}/safari", addr.port());
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
        assert_eq!(next(&mut rx).await.hello.ja4(), Browser::Safari.ja4());
        let (_, events) = common::wait_exchange(&mut proxy, "/safari", common::is_completed).await;
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ProxyEvent::RuleMatched { rule, .. } if rule == "safari"))
        );
        assert_eq!(upstream_fingerprint(&events).profile, "safari");

        let url = format!("https://localhost:{}/plain", addr.port());
        assert_eq!(client.get(&url).send().await.unwrap().status(), 200);
        assert!(
            !next(&mut rx)
                .await
                .hello
                .cipher_suites
                .iter()
                .any(|c| is_grease(*c))
        );
    }
}
