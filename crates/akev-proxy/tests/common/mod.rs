//! Test harness: starts the proxy with a real CA, a channel event sink and local upstream
//! servers (plain HTTP, TLS, WebSocket).

#![allow(dead_code)]

use akev_ca::CertificateAuthority;
use akev_core::rules::RuleSet;
use akev_core::settings::{CaptureSettings, ProxySettings};
use akev_core::*;
use akev_proxy::{ProxyHandle, ProxyOptions};
use akev_rules::RulesEngine;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

pub struct TestProxy {
    pub handle: ProxyHandle,
    pub events: mpsc::UnboundedReceiver<ProxyEvent>,
    pub ca: Arc<CertificateAuthority>,
}

impl TestProxy {
    pub fn addr(&self) -> SocketAddr {
        self.handle.local_addr()
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr())
    }

    /// Wait (up to `timeout`) for an event matching `pred`, returning all events seen so far
    /// including the match.
    pub async fn wait_for(
        &mut self,
        timeout: Duration,
        mut pred: impl FnMut(&ProxyEvent) -> bool,
    ) -> Vec<ProxyEvent> {
        let mut seen = Vec::new();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(ev)) => {
                    let hit = pred(&ev);
                    seen.push(ev);
                    if hit {
                        return seen;
                    }
                }
                _ => panic!(
                    "timed out waiting for event; saw: {:#?}",
                    seen.iter().map(event_name).collect::<Vec<_>>()
                ),
            }
        }
    }

    /// Collect events until `quiet` passes without a new one.
    pub async fn drain(&mut self, quiet: Duration) -> Vec<ProxyEvent> {
        let mut out = Vec::new();
        while let Ok(Some(ev)) = tokio::time::timeout(quiet, self.events.recv()).await {
            out.push(ev);
        }
        out
    }
}

pub fn event_name(ev: &ProxyEvent) -> &'static str {
    match ev {
        ProxyEvent::RequestStarted { .. } => "RequestStarted",
        ProxyEvent::RequestCompleted { .. } => "RequestCompleted",
        ProxyEvent::RequestModified { .. } => "RequestModified",
        ProxyEvent::RuleMatched { .. } => "RuleMatched",
        ProxyEvent::Upstream { .. } => "Upstream",
        ProxyEvent::ResponseStarted { .. } => "ResponseStarted",
        ProxyEvent::ResponseCompleted { .. } => "ResponseCompleted",
        ProxyEvent::ResponseModified { .. } => "ResponseModified",
        ProxyEvent::Paused { .. } => "Paused",
        ProxyEvent::Resumed { .. } => "Resumed",
        ProxyEvent::Aborted { .. } => "Aborted",
        ProxyEvent::StreamOpened { .. } => "StreamOpened",
        ProxyEvent::StreamMessage { .. } => "StreamMessage",
        ProxyEvent::StreamClosed { .. } => "StreamClosed",
        ProxyEvent::TlsFailed { .. } => "TlsFailed",
        ProxyEvent::TunnelOpened { .. } => "TunnelOpened",
        ProxyEvent::TunnelClosed { .. } => "TunnelClosed",
        ProxyEvent::ClientError { .. } => "ClientError",
        ProxyEvent::Highlighted { .. } => "Highlighted",
        ProxyEvent::Notice { .. } => "Notice",
    }
}

pub fn names_for(events: &[ProxyEvent], id: ExchangeId) -> Vec<&'static str> {
    events
        .iter()
        .filter(|e| e.exchange_id() == id)
        .map(event_name)
        .collect()
}

pub fn rules(json: &str) -> RuleSet {
    serde_json::from_str(json).expect("valid rule set JSON")
}

pub fn proxy_settings() -> ProxySettings {
    ProxySettings {
        port: 0,
        ignore_upstream_cert_errors: vec!["*".into()],
        ..Default::default()
    }
}

pub async fn start_proxy(rule_set: RuleSet, breakpoints: Arc<dyn BreakpointHandler>) -> TestProxy {
    start_proxy_with(
        rule_set,
        breakpoints,
        proxy_settings(),
        CaptureSettings {
            detect_processes: false,
            ..Default::default()
        },
    )
    .await
}

pub async fn start_proxy_with(
    rule_set: RuleSet,
    breakpoints: Arc<dyn BreakpointHandler>,
    settings: ProxySettings,
    capture: CaptureSettings,
) -> TestProxy {
    start_proxy_engine(
        Arc::new(RulesEngine::new(rule_set)),
        breakpoints,
        settings,
        capture,
    )
    .await
}

/// Start a proxy on a caller-owned rules engine (to change rules while it runs).
pub async fn start_proxy_engine(
    rules: Arc<RulesEngine>,
    breakpoints: Arc<dyn BreakpointHandler>,
    settings: ProxySettings,
    capture: CaptureSettings,
) -> TestProxy {
    let ca = Arc::new(CertificateAuthority::generate().expect("CA"));
    let (tx, rx) = mpsc::unbounded_channel();
    let handle = akev_proxy::start(ProxyOptions {
        settings,
        capture,
        ca: ca.clone(),
        rules,
        sink: Arc::new(ChannelSink(tx)),
        breakpoints,
    })
    .await
    .expect("proxy starts");
    TestProxy {
        handle,
        events: rx,
        ca,
    }
}

/// Read one HTTP/1 request (head + Content-Length/chunked body). Returns (raw head, body).
pub async fn read_request<S: AsyncRead + Unpin>(
    s: &mut S,
    buf: &mut Vec<u8>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = buf[..end + 4].to_vec();
            let mut hs = [httparse::EMPTY_HEADER; 64];
            let mut req = httparse::Request::new(&mut hs);
            req.parse(&head).ok()?;
            let cl = req
                .headers
                .iter()
                .find(|h| h.name.eq_ignore_ascii_case("content-length"))
                .and_then(|h| {
                    std::str::from_utf8(h.value)
                        .ok()?
                        .trim()
                        .parse::<usize>()
                        .ok()
                });
            let chunked = req.headers.iter().any(|h| {
                h.name.eq_ignore_ascii_case("transfer-encoding")
                    && h.value.eq_ignore_ascii_case(b"chunked")
            });
            buf.drain(..end + 4);
            let body = if let Some(n) = cl {
                while buf.len() < n {
                    let mut tmp = [0u8; 8192];
                    let k = s.read(&mut tmp).await.ok()?;
                    if k == 0 {
                        return None;
                    }
                    buf.extend_from_slice(&tmp[..k]);
                }
                buf.drain(..n).collect()
            } else if chunked {
                let mut body = Vec::new();
                loop {
                    while !buf.windows(2).any(|w| w == b"\r\n") {
                        let mut tmp = [0u8; 8192];
                        let k = s.read(&mut tmp).await.ok()?;
                        if k == 0 {
                            return None;
                        }
                        buf.extend_from_slice(&tmp[..k]);
                    }
                    let line_end = buf.windows(2).position(|w| w == b"\r\n")?;
                    let size = usize::from_str_radix(
                        std::str::from_utf8(&buf[..line_end])
                            .ok()?
                            .split(';')
                            .next()?
                            .trim(),
                        16,
                    )
                    .ok()?;
                    buf.drain(..line_end + 2);
                    if size == 0 {
                        // skip trailers up to the blank line
                        while !buf.windows(2).any(|w| w == b"\r\n") {
                            let mut tmp = [0u8; 8192];
                            let k = s.read(&mut tmp).await.ok()?;
                            if k == 0 {
                                return None;
                            }
                            buf.extend_from_slice(&tmp[..k]);
                        }
                        loop {
                            let e = buf.windows(2).position(|w| w == b"\r\n")?;
                            let empty = e == 0;
                            buf.drain(..e + 2);
                            if empty {
                                break;
                            }
                        }
                        break;
                    }
                    while buf.len() < size + 2 {
                        let mut tmp = [0u8; 8192];
                        let k = s.read(&mut tmp).await.ok()?;
                        if k == 0 {
                            return None;
                        }
                        buf.extend_from_slice(&tmp[..k]);
                    }
                    body.extend(buf.drain(..size));
                    buf.drain(..2);
                }
                body
            } else {
                Vec::new()
            };
            return Some((head, body));
        }
        let mut tmp = [0u8; 8192];
        let k = s.read(&mut tmp).await.ok()?;
        if k == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..k]);
    }
}

/// Upstream behaviour:
/// * `/hello` → `hello`
/// * `/echo` → the raw request head + `\n---\n` + body
/// * `/sse` → two server-sent events, then close
/// * `/slow` → waits 300ms before answering
/// * `/redirect` → 302 to `/hello`
/// * `/chunked` → chunked `hello world` with an `X-Checksum` trailer
/// * `/big` → 100 000 bytes of `x`
/// * `/gzip` → `hello gzip world`, gzip-encoded
/// * anything else → 404
pub async fn serve_http<S: AsyncRead + AsyncWrite + Unpin>(mut s: S) {
    let mut buf = Vec::new();
    while let Some((head, body)) = read_request(&mut s, &mut buf).await {
        let line = String::from_utf8_lossy(&head);
        let target = line.split_whitespace().nth(1).unwrap_or("/");
        let path = target.split('?').next().unwrap_or("/").to_string();
        if path == "/sse" {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").await;
            let _ = s.write_all(b"data: one\n\n").await;
            let _ = s.flush().await;
            tokio::time::sleep(Duration::from_millis(50)).await;
            let _ = s.write_all(b"event: tick\nid: 2\ndata: two\n\n").await;
            let _ = s.flush().await;
            return;
        }
        if path == "/slow" {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if path == "/redirect" {
            if s.write_all(b"HTTP/1.1 302 Found\r\nLocation: /hello\r\nContent-Length: 0\r\n\r\n")
                .await
                .is_err()
            {
                return;
            }
            continue;
        }
        if path == "/gzip" {
            let body =
                akev_content::decode::encode_body(b"hello gzip world", &["gzip".to_string()])
                    .expect("gzip");
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
                body.len()
            );
            if s.write_all(head.as_bytes()).await.is_err() || s.write_all(&body).await.is_err() {
                return;
            }
            continue;
        }
        if path == "/chunked" {
            let resp = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\nTrailer: X-Checksum\r\n\r\n6\r\nhello \r\n5\r\nworld\r\n0\r\nX-Checksum: abc\r\n\r\n";
            if s.write_all(resp).await.is_err() {
                return;
            }
            continue;
        }
        let (status, payload): (&str, Vec<u8>) = match path.as_str() {
            "/hello" | "/slow" => ("200 OK", b"hello".to_vec()),
            "/big" => ("200 OK", vec![b'x'; 100_000]),
            "/echo" => {
                let mut p = head.clone();
                p.extend_from_slice(b"\n---\n");
                p.extend_from_slice(&body);
                ("200 OK", p)
            }
            _ => ("404 Not Found", b"nope".to_vec()),
        };
        let resp = format!(
            "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nX-Upstream: test\r\nContent-Length: {}\r\n\r\n",
            payload.len()
        );
        if s.write_all(resp.as_bytes()).await.is_err() || s.write_all(&payload).await.is_err() {
            return;
        }
        let _ = s.flush().await;
    }
}

/// A raw upstream: every accepted connection is handed to `handler`.
pub async fn spawn_raw<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(tokio::net::TcpStream) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let h = handler.clone();
            tokio::spawn(async move { h(s).await });
        }
    });
    addr
}

/// Plain HTTP/1.1 upstream.
pub async fn spawn_http_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
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

/// HTTPS upstream (HTTP/1.1) with a certificate from a separate, untrusted CA.
pub async fn spawn_tls_upstream() -> SocketAddr {
    let ca = CertificateAuthority::generate().expect("upstream CA");
    let key = ca.leaf_for("localhost").expect("leaf");
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("versions")
    .with_no_client_auth()
    .with_cert_resolver(Arc::new(FixedCert(key)));
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
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

/// WebSocket echo upstream.
pub async fn spawn_ws_upstream() -> SocketAddr {
    use futures::{SinkExt, StreamExt};
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(mut ws) = tokio_tungstenite::accept_async(s).await else {
                    return;
                };
                while let Some(Ok(msg)) = ws.next().await {
                    if msg.is_close() {
                        let _ = ws.close(None).await;
                        break;
                    }
                    if (msg.is_text() || msg.is_binary()) && ws.send(msg).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

/// Read an HTTP/1 response (head + Content-Length / close-delimited body) from a raw stream.
pub async fn read_response<S: AsyncRead + Unpin>(
    s: &mut S,
    buf: &mut Vec<u8>,
) -> (String, Vec<u8>) {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..end + 4]).to_string();
            let cl = head.lines().find_map(|l| {
                l.split_once(':')
                    .filter(|(k, _)| k.eq_ignore_ascii_case("content-length"))
                    .map(|(_, v)| v.trim().parse::<usize>().unwrap_or(0))
            });
            buf.drain(..end + 4);
            let n = cl.unwrap_or(0);
            while buf.len() < n {
                let mut tmp = [0u8; 8192];
                let k = s.read(&mut tmp).await.expect("read");
                assert!(k > 0, "connection closed mid-body");
                buf.extend_from_slice(&tmp[..k]);
            }
            let body = buf.drain(..n).collect();
            return (head, body);
        }
        let mut tmp = [0u8; 8192];
        let k = s.read(&mut tmp).await.expect("read");
        assert!(
            k > 0,
            "connection closed before response; got {:?}",
            String::from_utf8_lossy(buf)
        );
        buf.extend_from_slice(&tmp[..k]);
    }
}

/// reqwest client using the proxy and trusting its CA.
pub fn client(proxy: &TestProxy) -> reqwest::Client {
    reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(proxy.url()).expect("proxy url"))
        .add_root_certificate(
            reqwest::Certificate::from_pem(proxy.ca.cert_pem().as_bytes()).expect("ca pem"),
        )
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client")
}

/// Open a CONNECT tunnel through the proxy to `target` (`host:port`).
pub async fn connect_tunnel(proxy: &TestProxy, target: &str) -> tokio::net::TcpStream {
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr())
        .await
        .expect("connect proxy");
    tcp.write_all(format!("CONNECT {target} HTTP/1.1\r\nHost: {target}\r\n\r\n").as_bytes())
        .await
        .expect("write CONNECT");
    let mut buf = Vec::new();
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        let mut tmp = [0u8; 1024];
        let n = tcp.read(&mut tmp).await.expect("read CONNECT reply");
        assert!(n > 0, "proxy closed the CONNECT tunnel");
        buf.extend_from_slice(&tmp[..n]);
    }
    let reply = String::from_utf8_lossy(&buf);
    assert!(reply.starts_with("HTTP/1.1 200"), "CONNECT failed: {reply}");
    tcp
}

/// Send raw bytes to the proxy and read one response.
pub async fn raw_exchange(proxy: &TestProxy, request: &[u8]) -> (String, Vec<u8>) {
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr())
        .await
        .expect("connect proxy");
    tcp.write_all(request).await.expect("write request");
    let mut buf = Vec::new();
    read_response(&mut tcp, &mut buf).await
}

/// Events of the first exchange whose `RequestStarted` head path equals `path`.
pub fn exchange_events(events: &[ProxyEvent], path: &str) -> (ExchangeId, Vec<ProxyEvent>) {
    let id = events
        .iter()
        .find_map(|e| match e {
            ProxyEvent::RequestStarted { id, head, .. } if head.path == path => Some(*id),
            _ => None,
        })
        .unwrap_or_else(|| {
            panic!(
                "no exchange for {path}; saw {:?}",
                events.iter().map(event_name).collect::<Vec<_>>()
            )
        });
    (
        id,
        events
            .iter()
            .filter(|e| e.exchange_id() == id)
            .cloned()
            .collect(),
    )
}

/// Wait until the exchange with RequestStarted path `path` emits an event matching `done`.
pub async fn wait_exchange(
    proxy: &mut TestProxy,
    path: &str,
    done: impl Fn(&ProxyEvent) -> bool,
) -> (ExchangeId, Vec<ProxyEvent>) {
    let mut id: Option<ExchangeId> = None;
    let seen = proxy
        .wait_for(Duration::from_secs(10), |e| {
            if let ProxyEvent::RequestStarted { id: i, head, .. } = e
                && head.path == path
                && id.is_none()
            {
                id = Some(*i);
            }
            id == Some(e.exchange_id()) && done(e)
        })
        .await;
    exchange_events(&seen, path)
}

pub fn is_completed(e: &ProxyEvent) -> bool {
    matches!(
        e,
        ProxyEvent::ResponseCompleted { .. } | ProxyEvent::Aborted { .. }
    )
}
