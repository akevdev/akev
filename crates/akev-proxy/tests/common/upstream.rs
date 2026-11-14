//! In-process test servers for the upstream client integration tests: raw HTTP/1 servers,
//! an rcgen PKI, TLS servers (HTTP/1.1 and HTTP/2 via ALPN), a minimal HTTP proxy
//! (CONNECT + absolute-form) and a minimal SOCKS5 server.
#![allow(dead_code)]

use akev_core::settings::ProxySettings;
use akev_core::{Headers, HttpVersion, RequestHead};
use akev_proxy::upstream::{BodyChunk, BodyStream, RequestOptions, UpstreamClient};
use bytes::{Buf, Bytes, BytesMut};
use futures::StreamExt;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub fn client(settings: &ProxySettings) -> UpstreamClient {
    UpstreamClient::new(settings).expect("client")
}

pub fn opts() -> RequestOptions {
    RequestOptions::default()
}

/// Build a request head from a URL and header pairs.
pub fn head(method: &str, url: &str, headers: &[(&str, &str)]) -> RequestHead {
    let parsed = url::Url::parse(url).expect("url");
    let mut path = parsed.path().to_owned();
    if let Some(q) = parsed.query() {
        path.push('?');
        path.push_str(q);
    }
    let host = parsed
        .host_str()
        .unwrap_or("")
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    RequestHead {
        method: method.into(),
        url: url.into(),
        scheme: parsed.scheme().into(),
        host,
        port: parsed.port_or_known_default().unwrap_or(80),
        path,
        http_version: HttpVersion::Http11,
        headers: Headers(
            headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        ),
    }
}

/// Drain a body stream into bytes + trailers.
pub async fn collect(mut body: BodyStream) -> (Vec<u8>, Option<Headers>) {
    let mut data = Vec::new();
    let mut trailers = None;
    while let Some(chunk) = body.next().await {
        match chunk.expect("body chunk") {
            BodyChunk::Data(d) => data.extend_from_slice(&d),
            BodyChunk::Trailers(t) => trailers = Some(t),
        }
    }
    (data, trailers)
}

/// A request as received by a test server.
#[derive(Clone, Debug, Default)]
pub struct RawRequest {
    pub method: String,
    pub target: String,
    pub raw_head: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub trailers: Vec<(String, String)>,
    pub chunked: bool,
}

impl RawRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

async fn fill<S: AsyncRead + Unpin>(s: &mut S, buf: &mut BytesMut) -> bool {
    buf.reserve(8192);
    matches!(s.read_buf(buf).await, Ok(n) if n > 0)
}

async fn read_line<S: AsyncRead + Unpin>(s: &mut S, buf: &mut BytesMut) -> Option<String> {
    loop {
        if let Some(pos) = buf.windows(2).position(|w| w == b"\r\n") {
            let line = String::from_utf8_lossy(&buf[..pos]).into_owned();
            buf.advance(pos + 2);
            return Some(line);
        }
        if !fill(s, buf).await {
            return None;
        }
    }
}

/// Read only the request head (no body).
pub async fn read_request_head<S: AsyncRead + Unpin>(
    s: &mut S,
    buf: &mut BytesMut,
) -> Option<RawRequest> {
    loop {
        let mut headers = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers);
        match req.parse(buf) {
            Ok(httparse::Status::Complete(len)) => {
                let raw = RawRequest {
                    method: req.method.unwrap_or("").to_owned(),
                    target: req.path.unwrap_or("").to_owned(),
                    raw_head: String::from_utf8_lossy(&buf[..len]).into_owned(),
                    headers: req
                        .headers
                        .iter()
                        .map(|h| {
                            (
                                h.name.to_owned(),
                                String::from_utf8_lossy(h.value).into_owned(),
                            )
                        })
                        .collect(),
                    ..Default::default()
                };
                buf.advance(len);
                return Some(raw);
            }
            Ok(httparse::Status::Partial) => {
                if !fill(s, buf).await {
                    return None;
                }
            }
            Err(e) => panic!("bad request from client: {e}"),
        }
    }
}

/// Read a body framed per the request headers.
pub async fn read_request_body<S: AsyncRead + Unpin>(
    s: &mut S,
    buf: &mut BytesMut,
    req: &mut RawRequest,
) {
    let chunked = req
        .header("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    if chunked {
        req.chunked = true;
        loop {
            let line = read_line(s, buf).await.expect("chunk size");
            let size = usize::from_str_radix(line.split(';').next().unwrap().trim(), 16)
                .expect("hex size");
            if size == 0 {
                loop {
                    let line = read_line(s, buf).await.expect("trailer line");
                    if line.is_empty() {
                        break;
                    }
                    let (k, v) = line.split_once(':').expect("trailer");
                    req.trailers
                        .push((k.trim().to_owned(), v.trim().to_owned()));
                }
                break;
            }
            while buf.len() < size + 2 {
                assert!(fill(s, buf).await, "eof in chunk");
            }
            req.body.extend_from_slice(&buf[..size]);
            buf.advance(size + 2);
        }
    } else if let Some(len) = req
        .header("content-length")
        .and_then(|v| v.trim().parse::<usize>().ok())
    {
        while buf.len() < len {
            assert!(fill(s, buf).await, "eof in body");
        }
        req.body.extend_from_slice(&buf[..len]);
        buf.advance(len);
    }
}

/// Read a full request (head + body).
pub async fn read_request<S: AsyncRead + Unpin>(
    s: &mut S,
    buf: &mut BytesMut,
) -> Option<RawRequest> {
    let mut req = read_request_head(s, buf).await?;
    read_request_body(s, buf, &mut req).await;
    Some(req)
}

/// Spawn a TCP server; `handler` gets each accepted connection and its index.
pub async fn spawn_server<F, Fut>(handler: F) -> (SocketAddr, Arc<AtomicUsize>)
where
    F: Fn(TcpStream, usize) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let accepted = Arc::new(AtomicUsize::new(0));
    let counter = accepted.clone();
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let n = counter.fetch_add(1, Ordering::SeqCst);
            let h = handler.clone();
            tokio::spawn(async move { h(stream, n).await });
        }
    });
    (addr, accepted)
}

/// A keep-alive HTTP/1.1 server that answers every request with `respond(request, conn_index)`.
pub async fn spawn_http1<F>(respond: F) -> (SocketAddr, Arc<AtomicUsize>)
where
    F: Fn(&RawRequest, usize) -> Vec<u8> + Send + Sync + 'static,
{
    let respond = Arc::new(respond);
    spawn_server(move |mut stream, n| {
        let respond = respond.clone();
        async move {
            let mut buf = BytesMut::new();
            while let Some(req) = read_request(&mut stream, &mut buf).await {
                let response = respond(&req, n);
                if stream.write_all(&response).await.is_err() {
                    break;
                }
            }
        }
    })
    .await
}

/// A response whose body echoes the raw request head (+ body after a blank line).
pub fn echo_response(req: &RawRequest) -> Vec<u8> {
    let mut body = req.raw_head.clone().into_bytes();
    body.extend_from_slice(&req.body);
    let mut out = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
    out.extend_from_slice(&body);
    out
}

// ---------------------------------------------------------------------------------------
// PKI
// ---------------------------------------------------------------------------------------

/// A test certificate authority.
pub struct TestCa {
    pub params: rcgen::CertificateParams,
    pub key: rcgen::KeyPair,
    pub cert: rcgen::Certificate,
}

impl TestCa {
    pub fn new(name: &str) -> Self {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.key_usages = vec![
            rcgen::KeyUsagePurpose::KeyCertSign,
            rcgen::KeyUsagePurpose::CrlSign,
            rcgen::KeyUsagePurpose::DigitalSignature,
        ];
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        TestCa { params, key, cert }
    }

    pub fn pem(&self) -> String {
        self.cert.pem()
    }

    pub fn der(&self) -> CertificateDer<'static> {
        self.cert.der().clone()
    }

    /// Issue a leaf certificate for `names` (expired when `expired`).
    pub fn leaf(
        &self,
        names: &[&str],
        expired: bool,
    ) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
        let mut params =
            rcgen::CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
                .unwrap();
        params.distinguished_name.push(
            rcgen::DnType::CommonName,
            names.first().copied().unwrap_or("leaf"),
        );
        if expired {
            params.not_before = rcgen::date_time_ymd(2001, 1, 1);
            params.not_after = rcgen::date_time_ymd(2002, 1, 1);
        }
        let key = rcgen::KeyPair::generate().unwrap();
        let issuer = rcgen::Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).unwrap();
        (
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
    }

    /// Issue a client certificate; returns (PEM cert+key, DER cert).
    pub fn client_identity(&self, name: &str) -> (String, Vec<u8>) {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params
            .distinguished_name
            .push(rcgen::DnType::CommonName, name);
        params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ClientAuth];
        let key = rcgen::KeyPair::generate().unwrap();
        let issuer = rcgen::Issuer::from_params(&self.params, &self.key);
        let cert = params.signed_by(&key, &issuer).unwrap();
        (
            format!("{}{}", cert.pem(), key.serialize_pem()),
            cert.der().to_vec(),
        )
    }
}

/// A self-signed leaf (no CA).
pub fn self_signed(names: &[&str]) -> (Vec<CertificateDer<'static>>, PrivateKeyDer<'static>) {
    let mut params =
        rcgen::CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>())
            .unwrap();
    params.distinguished_name.push(
        rcgen::DnType::CommonName,
        names.first().copied().unwrap_or("self"),
    );
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
}

pub fn write_temp(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> String {
    let path = dir.path().join(name);
    std::fs::write(&path, content).unwrap();
    path.to_string_lossy().into_owned()
}

pub fn server_config(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    alpn: &[&[u8]],
    client_ca: Option<CertificateDer<'static>>,
) -> Arc<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .unwrap();
    let builder = match client_ca {
        Some(ca) => {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(ca).unwrap();
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            .unwrap();
            builder.with_client_cert_verifier(verifier)
        }
        None => builder.with_no_client_auth(),
    };
    let mut cfg = builder.with_single_cert(chain, key).unwrap();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Arc::new(cfg)
}

/// What a TLS server saw on a connection.
#[derive(Clone, Debug, Default)]
pub struct TlsSeen {
    pub alpn: Option<String>,
    pub sni: Option<String>,
    pub client_cert: Option<Vec<u8>>,
}

/// A TLS server: HTTP/2 when ALPN negotiates h2 (responds `h2 <path>`, with a trailer
/// `x-trailer: yes` for `/trailers`, and `<n>` bytes for `/big/<n>`), otherwise HTTP/1.1
/// echo. Records each connection.
pub async fn spawn_tls(
    config: Arc<rustls::ServerConfig>,
) -> (
    SocketAddr,
    Arc<AtomicUsize>,
    Arc<parking_lot::Mutex<Vec<TlsSeen>>>,
) {
    spawn_tls_with(config, None).await
}

/// [`spawn_tls`] whose HTTP/2 connections advertise `SETTINGS_MAX_CONCURRENT_STREAMS`.
pub async fn spawn_tls_with(
    config: Arc<rustls::ServerConfig>,
    max_concurrent_streams: Option<u32>,
) -> (
    SocketAddr,
    Arc<AtomicUsize>,
    Arc<parking_lot::Mutex<Vec<TlsSeen>>>,
) {
    let seen = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let (addr, count) = spawn_server(move |stream, _| {
        let acceptor = acceptor.clone();
        let seen = seen2.clone();
        async move {
            let Ok(mut tls) = acceptor.accept(stream).await else {
                return;
            };
            let (_, conn) = tls.get_ref();
            let info = TlsSeen {
                alpn: conn
                    .alpn_protocol()
                    .map(|p| String::from_utf8_lossy(p).into_owned()),
                sni: conn.server_name().map(str::to_owned),
                client_cert: conn
                    .peer_certificates()
                    .and_then(|c| c.first())
                    .map(|c| c.to_vec()),
            };
            let is_h2 = info.alpn.as_deref() == Some("h2");
            seen.lock().push(info);
            if is_h2 {
                serve_h2(tls, max_concurrent_streams).await;
            } else {
                let mut buf = BytesMut::new();
                while let Some(req) = read_request(&mut tls, &mut buf).await {
                    if tls.write_all(&echo_response(&req)).await.is_err() {
                        break;
                    }
                }
            }
        }
    })
    .await;
    (addr, count, seen)
}

async fn serve_h2<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    io: S,
    max_concurrent_streams: Option<u32>,
) {
    let mut builder = h2::server::Builder::new();
    if let Some(n) = max_concurrent_streams {
        builder.max_concurrent_streams(n);
    }
    let Ok(mut conn) = builder.handshake(io).await else {
        return;
    };
    while let Some(Ok((request, mut respond))) = conn.accept().await {
        tokio::spawn(async move {
            let (parts, mut body) = request.into_parts();
            let mut received = Vec::new();
            while let Some(Ok(chunk)) = body.data().await {
                let _ = body.flow_control().release_capacity(chunk.len());
                received.extend_from_slice(&chunk);
            }
            let req_trailers = body.trailers().await.ok().flatten();
            let path = parts.uri.path().to_owned();
            if path == "/slow" {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            let mut text = format!("h2 {} {}", parts.method, path);
            if let Some(ua) = parts.headers.get("x-test") {
                text.push_str(&format!(" x-test={}", ua.to_str().unwrap_or("")));
            }
            if parts.headers.contains_key("connection") || parts.headers.contains_key("host") {
                text.push_str(" BAD-HEADERS");
            }
            if !received.is_empty() {
                text.push_str(&format!(" body={}", String::from_utf8_lossy(&received)));
            }
            if let Some(t) = req_trailers
                && let Some(v) = t.get("x-req-trailer")
            {
                text.push_str(&format!(" req-trailer={}", v.to_str().unwrap_or("")));
            }
            let response = http::Response::builder()
                .status(200)
                .header("X-Mixed-Case", "kept")
                .body(())
                .unwrap();
            let with_trailers = path == "/trailers";
            let Ok(mut send) = respond.send_response(response, false) else {
                return;
            };
            if let Some(size) = path
                .strip_prefix("/big/")
                .and_then(|n| n.parse::<usize>().ok())
            {
                // A large body, sent as fast as the client's flow-control windows allow.
                let chunk = Bytes::from(vec![b'x'; 64 * 1024]);
                let mut left = size;
                while left > 0 {
                    send.reserve_capacity(left.min(chunk.len()));
                    match std::future::poll_fn(|cx| send.poll_capacity(cx)).await {
                        Some(Ok(0)) => continue,
                        Some(Ok(n)) => {
                            let n = n.min(left).min(chunk.len());
                            if send.send_data(chunk.slice(..n), false).is_err() {
                                return;
                            }
                            left -= n;
                        }
                        _ => return,
                    }
                }
                let _ = send.send_data(Bytes::new(), true);
                return;
            }
            let _ = send.send_data(Bytes::from(text), !with_trailers);
            if with_trailers {
                let mut trailers = http::HeaderMap::new();
                trailers.insert("x-trailer", http::HeaderValue::from_static("yes"));
                let _ = send.send_trailers(trailers);
            }
        });
    }
}

// ---------------------------------------------------------------------------------------
// Proxies
// ---------------------------------------------------------------------------------------

/// What a test proxy observed.
#[derive(Clone, Debug, Default)]
pub struct ProxySeen {
    pub requests: Vec<RawRequest>,
}

async fn pipe<A, B>(a: A, b: B)
where
    A: AsyncRead + AsyncWrite + Unpin,
    B: AsyncRead + AsyncWrite + Unpin,
{
    let (mut a, mut b) = (a, b);
    let _ = tokio::io::copy_bidirectional(&mut a, &mut b).await;
}

/// Serve one client connection of the minimal HTTP proxy.
async fn handle_proxy_conn<S>(
    mut stream: S,
    auth: Option<&'static str>,
    seen: Arc<parking_lot::Mutex<ProxySeen>>,
) where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let mut buf = BytesMut::new();
    loop {
        let Some(mut req) = read_request_head(&mut stream, &mut buf).await else {
            return;
        };
        seen.lock().requests.push(req.clone());
        if let Some(expected) = auth
            && req.header("proxy-authorization") != Some(expected)
        {
            let _ = stream
                .write_all(
                    b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n",
                )
                .await;
            continue;
        }
        if req.method == "CONNECT" {
            let target = req.target.clone();
            let connect_to = match target.rsplit_once(':') {
                // Test hostnames resolve to localhost.
                Some((host, port))
                    if host.parse::<std::net::IpAddr>().is_err() && !host.starts_with('[') =>
                {
                    format!("127.0.0.1:{port}")
                }
                _ => target.clone(),
            };
            let Ok(upstream) = TcpStream::connect(&connect_to).await else {
                let _ = stream
                    .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
                    .await;
                return;
            };
            let _ = stream
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                .await;
            let _ = stream.write_all(&buf).await;
            pipe(stream, upstream).await;
            return;
        }
        // Absolute-form: forward in origin-form to the target and relay one response.
        read_request_body(&mut stream, &mut buf, &mut req).await;
        let url = url::Url::parse(&req.target).expect("absolute-form target");
        let host = url.host_str().unwrap();
        let host = if host.parse::<std::net::IpAddr>().is_ok() {
            host.to_owned()
        } else {
            "127.0.0.1".to_owned()
        };
        let target = format!("{host}:{}", url.port_or_known_default().unwrap());
        let Ok(mut upstream) = TcpStream::connect(&target).await else {
            return;
        };
        let mut out = format!("{} {} HTTP/1.1\r\n", req.method, url.path()).into_bytes();
        for (k, v) in &req.headers {
            if !k.eq_ignore_ascii_case("proxy-authorization") {
                out.extend_from_slice(format!("{k}: {v}\r\n").as_bytes());
            }
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&req.body);
        let _ = upstream.write_all(&out).await;
        // Relay a Content-Length framed response.
        let mut ubuf = BytesMut::new();
        let mut resp_head = Vec::new();
        loop {
            if let Some(pos) = ubuf.windows(4).position(|w| w == b"\r\n\r\n") {
                resp_head.extend_from_slice(&ubuf[..pos + 4]);
                ubuf.advance(pos + 4);
                break;
            }
            if !fill(&mut upstream, &mut ubuf).await {
                return;
            }
        }
        let head_text = String::from_utf8_lossy(&resp_head).to_ascii_lowercase();
        let len: usize = head_text
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .map(|v| v.trim().parse().unwrap_or(0))
            })
            .unwrap_or(0);
        while ubuf.len() < len {
            if !fill(&mut upstream, &mut ubuf).await {
                return;
            }
        }
        let _ = stream.write_all(&resp_head).await;
        let _ = stream.write_all(&ubuf[..len]).await;
    }
}

/// A minimal HTTP proxy: CONNECT tunnels and absolute-form forwarding. When `auth` is set,
/// requires `Proxy-Authorization: <auth>`.
pub async fn spawn_http_proxy(
    auth: Option<&'static str>,
) -> (SocketAddr, Arc<parking_lot::Mutex<ProxySeen>>) {
    let seen = Arc::new(parking_lot::Mutex::new(ProxySeen::default()));
    let seen2 = seen.clone();
    let (addr, _) =
        spawn_server(move |stream, _| handle_proxy_conn(stream, auth, seen2.clone())).await;
    (addr, seen)
}

/// The same proxy behind TLS (an `https://` upstream proxy).
pub async fn spawn_https_proxy(
    config: Arc<rustls::ServerConfig>,
) -> (SocketAddr, Arc<parking_lot::Mutex<ProxySeen>>) {
    let seen = Arc::new(parking_lot::Mutex::new(ProxySeen::default()));
    let seen2 = seen.clone();
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let (addr, _) = spawn_server(move |stream, _| {
        let acceptor = acceptor.clone();
        let seen = seen2.clone();
        async move {
            if let Ok(tls) = acceptor.accept(stream).await {
                handle_proxy_conn(tls, None, seen).await;
            }
        }
    })
    .await;
    (addr, seen)
}

/// What the SOCKS5 server observed: requested destination (as text) and username.
#[derive(Clone, Debug, Default)]
pub struct SocksSeen {
    pub destinations: Vec<String>,
    pub users: Vec<String>,
}

/// A minimal SOCKS5 server (no-auth, or user/pass when `creds` is set).
pub async fn spawn_socks5(
    creds: Option<(&'static str, &'static str)>,
) -> (SocketAddr, Arc<parking_lot::Mutex<SocksSeen>>) {
    let seen = Arc::new(parking_lot::Mutex::new(SocksSeen::default()));
    let seen2 = seen.clone();
    let (addr, _) = spawn_server(move |mut s, _| {
        let seen = seen2.clone();
        async move {
            let mut hdr = [0u8; 2];
            if s.read_exact(&mut hdr).await.is_err() {
                return;
            }
            let mut methods = vec![0u8; hdr[1] as usize];
            s.read_exact(&mut methods).await.unwrap();
            match creds {
                Some((user, pass)) => {
                    if !methods.contains(&2) {
                        let _ = s.write_all(&[5, 0xFF]).await;
                        return;
                    }
                    s.write_all(&[5, 2]).await.unwrap();
                    let mut v = [0u8; 2];
                    s.read_exact(&mut v).await.unwrap();
                    let mut u = vec![0u8; v[1] as usize];
                    s.read_exact(&mut u).await.unwrap();
                    let mut pl = [0u8; 1];
                    s.read_exact(&mut pl).await.unwrap();
                    let mut p = vec![0u8; pl[0] as usize];
                    s.read_exact(&mut p).await.unwrap();
                    let ok = u == user.as_bytes() && p == pass.as_bytes();
                    seen.lock()
                        .users
                        .push(String::from_utf8_lossy(&u).into_owned());
                    s.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                    if !ok {
                        return;
                    }
                }
                None => s.write_all(&[5, 0]).await.unwrap(),
            }
            let mut req = [0u8; 4];
            s.read_exact(&mut req).await.unwrap();
            let host = match req[3] {
                1 => {
                    let mut ip = [0u8; 4];
                    s.read_exact(&mut ip).await.unwrap();
                    std::net::Ipv4Addr::from(ip).to_string()
                }
                3 => {
                    let mut len = [0u8; 1];
                    s.read_exact(&mut len).await.unwrap();
                    let mut name = vec![0u8; len[0] as usize];
                    s.read_exact(&mut name).await.unwrap();
                    String::from_utf8_lossy(&name).into_owned()
                }
                4 => {
                    let mut ip = [0u8; 16];
                    s.read_exact(&mut ip).await.unwrap();
                    std::net::Ipv6Addr::from(ip).to_string()
                }
                _ => return,
            };
            let mut port = [0u8; 2];
            s.read_exact(&mut port).await.unwrap();
            let port = u16::from_be_bytes(port);
            seen.lock().destinations.push(format!("{host}:{port}"));
            // Domain names resolve to localhost in tests.
            let connect_host = if host.parse::<std::net::IpAddr>().is_ok() {
                host.clone()
            } else {
                "127.0.0.1".into()
            };
            match TcpStream::connect((connect_host.as_str(), port)).await {
                Ok(upstream) => {
                    s.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                        .await
                        .unwrap();
                    pipe(s, upstream).await;
                }
                Err(_) => {
                    let _ = s.write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                }
            }
        }
    })
    .await;
    (addr, seen)
}
