//! In-process HTTP/3 test peers (quinn + h3 + rcgen certificates): an origin server that
//! echoes requests (headers, body, trailers) and can advertise / clear `Alt-Svc`, a plain TLS
//! HTTP/1.1 origin with a fixed `Alt-Svc` header, and an HTTP/3 client.
#![allow(dead_code)]

use bytes::{Buf, Bytes};
use parking_lot::Mutex;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A request seen by [`spawn_h3`].
#[derive(Clone, Debug)]
pub struct SeenRequest {
    pub method: String,
    /// `scheme://authority/path?query`.
    pub uri: String,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
    pub trailers: Option<http::HeaderMap>,
    pub peer: SocketAddr,
}

/// What the HTTP/3 origin saw.
#[derive(Default)]
pub struct H3Seen {
    pub connections: AtomicUsize,
    pub requests: Mutex<Vec<SeenRequest>>,
}

impl H3Seen {
    pub fn count(&self) -> usize {
        self.requests.lock().len()
    }

    pub fn last(&self) -> SeenRequest {
        self.requests.lock().last().cloned().expect("a request")
    }
}

/// A running HTTP/3 origin.
pub struct H3Origin {
    pub addr: SocketAddr,
    pub seen: Arc<H3Seen>,
    pub endpoint: quinn::Endpoint,
}

impl H3Origin {
    pub fn port(&self) -> u16 {
        self.addr.port()
    }
}

/// HTTP/3 origin on 127.0.0.1 (UDP). Every response: `200`, `x-proto: h3`, body
/// `h3 {METHOD} {path} body={body}`; `alt_svc` is added to every response (paths starting with
/// `/clear` answer `Alt-Svc: clear` instead). `x-want-trailers` → response trailer
/// `x-trailer: done`. `/big?n` → `n` bytes of `x`. `/slow` waits 300 ms.
pub async fn spawn_h3(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    alt_svc: Option<String>,
) -> H3Origin {
    spawn_h3_on("127.0.0.1:0".parse().unwrap(), chain, key, alt_svc).await
}

pub async fn spawn_h3_on(
    bind: SocketAddr,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    alt_svc: Option<String>,
) -> H3Origin {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    tls.alpn_protocols = vec![b"h3".to_vec()];
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).unwrap();
    let config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let endpoint = quinn::Endpoint::server(config, bind).expect("bind UDP");
    let addr = endpoint.local_addr().unwrap();
    let seen = Arc::new(H3Seen::default());
    let ep = endpoint.clone();
    let task_seen = seen.clone();
    tokio::spawn(async move {
        while let Some(incoming) = ep.accept().await {
            let seen = task_seen.clone();
            let alt_svc = alt_svc.clone();
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                seen.connections.fetch_add(1, Ordering::SeqCst);
                let peer = conn.remote_address();
                let Ok(mut h3conn) =
                    h3::server::Connection::<_, Bytes>::new(h3_quinn::Connection::new(conn)).await
                else {
                    return;
                };
                while let Ok(Some(resolver)) = h3conn.accept().await {
                    let seen = seen.clone();
                    let alt_svc = alt_svc.clone();
                    tokio::spawn(async move {
                        let _ = serve_request(resolver, seen, alt_svc, peer).await;
                    });
                }
            });
        }
    });
    H3Origin {
        addr,
        seen,
        endpoint,
    }
}

async fn serve_request(
    resolver: h3::server::RequestResolver<h3_quinn::Connection, Bytes>,
    seen: Arc<H3Seen>,
    alt_svc: Option<String>,
    peer: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (req, stream) = resolver.resolve_request().await?;
    let (mut send, mut recv) = stream.split();
    let mut body = Vec::new();
    while let Some(mut chunk) = recv.recv_data().await? {
        let b = chunk.copy_to_bytes(chunk.remaining());
        body.extend_from_slice(&b);
    }
    let trailers = recv.recv_trailers().await?;
    let path = req
        .uri()
        .path_and_query()
        .map(|p| p.as_str().to_string())
        .unwrap_or_default();
    seen.requests.lock().push(SeenRequest {
        method: req.method().to_string(),
        uri: req.uri().to_string(),
        headers: req.headers().clone(),
        body: body.clone(),
        trailers,
        peer,
    });
    if path.starts_with("/slow") {
        tokio::time::sleep(Duration::from_millis(300)).await;
    }
    let payload: Bytes = match path.strip_prefix("/big?") {
        Some(n) => Bytes::from(vec![b'x'; n.parse().unwrap_or(0)]),
        None => Bytes::from(format!(
            "h3 {} {} body={}",
            req.method(),
            path,
            String::from_utf8_lossy(&body)
        )),
    };
    let mut resp = http::Response::builder()
        .status(200)
        .header("content-type", "text/plain")
        .header("x-proto", "h3")
        .header("content-length", payload.len());
    if path.starts_with("/clear") {
        resp = resp.header("alt-svc", "clear");
    } else if let Some(alt) = &alt_svc {
        resp = resp.header("alt-svc", alt.as_str());
    }
    send.send_response(resp.body(()).unwrap()).await?;
    // Large bodies go out in pieces (exercises flow control).
    let mut rest = payload;
    while !rest.is_empty() {
        let n = rest.len().min(64 * 1024);
        send.send_data(rest.split_to(n)).await?;
    }
    if req.headers().contains_key("x-want-trailers") {
        let mut t = http::HeaderMap::new();
        t.insert("x-trailer", "done".parse().unwrap());
        send.send_trailers(t).await?;
    }
    send.finish().await?;
    Ok(())
}

/// TLS (TCP) HTTP/1.1 origin answering every request with `200`, body `tcp {path}` and the
/// extra `headers`. Returns its address and a request counter.
pub async fn spawn_tls_h1(
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    headers: Vec<(String, String)>,
) -> (SocketAddr, Arc<AtomicUsize>) {
    spawn_tls_h1_on("127.0.0.1:0", chain, key, headers).await
}

pub async fn spawn_tls_h1_on(
    bind: &str,
    chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
    headers: Vec<(String, String)>,
) -> (SocketAddr, Arc<AtomicUsize>) {
    let mut tls = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .unwrap();
    tls.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(tls));
    let listener = TcpListener::bind(bind).await.expect("bind TCP");
    let addr = listener.local_addr().unwrap();
    let count = Arc::new(AtomicUsize::new(0));
    let task_count = count.clone();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            let headers = headers.clone();
            let count = task_count.clone();
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(s).await else {
                    return;
                };
                let mut buf = Vec::new();
                loop {
                    let Some(end) = read_head(&mut tls, &mut buf).await else {
                        return;
                    };
                    let head = String::from_utf8_lossy(&buf[..end]).to_string();
                    buf.drain(..end);
                    count.fetch_add(1, Ordering::SeqCst);
                    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let body = format!("tcp {path}");
                    let mut resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n",
                        body.len()
                    );
                    for (k, v) in &headers {
                        resp.push_str(&format!("{k}: {v}\r\n"));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(&body);
                    if tls.write_all(resp.as_bytes()).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, count)
}

/// Read a request head (bodiless requests only); returns its length in `buf`.
async fn read_head<S: tokio::io::AsyncRead + Unpin>(s: &mut S, buf: &mut Vec<u8>) -> Option<usize> {
    loop {
        if let Some(end) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            return Some(end + 4);
        }
        let mut tmp = [0u8; 4096];
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
}

/// An HTTP/3 response as seen by [`H3Client`].
#[derive(Debug)]
pub struct H3Response {
    pub status: u16,
    pub headers: http::HeaderMap,
    pub body: Vec<u8>,
    pub trailers: Option<http::HeaderMap>,
}

impl H3Response {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// A minimal HTTP/3 client (one connection).
pub struct H3Client {
    send: h3::client::SendRequest<h3_quinn::OpenStreams, Bytes>,
    pub conn: quinn::Connection,
    _endpoint: quinn::Endpoint,
}

impl H3Client {
    /// Connect to `addr` with SNI `server_name`, trusting `roots`.
    pub async fn connect(
        addr: SocketAddr,
        server_name: &str,
        roots: &[CertificateDer<'static>],
    ) -> Result<H3Client, String> {
        let mut store = rustls::RootCertStore::empty();
        for r in roots {
            store.add(r.clone()).unwrap();
        }
        let mut tls = rustls::ClientConfig::builder_with_provider(provider())
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_root_certificates(store)
            .with_no_client_auth();
        tls.alpn_protocols = vec![b"h3".to_vec()];
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
        let config = quinn::ClientConfig::new(Arc::new(crypto));
        let bind: SocketAddr = if addr.is_ipv6() {
            "[::]:0".parse().unwrap()
        } else {
            "0.0.0.0:0".parse().unwrap()
        };
        let endpoint = quinn::Endpoint::client(bind).map_err(|e| e.to_string())?;
        let connecting = endpoint
            .connect_with(config, addr, server_name)
            .map_err(|e| e.to_string())?;
        let conn = tokio::time::timeout(Duration::from_secs(5), connecting)
            .await
            .map_err(|_| "handshake timed out".to_string())?
            .map_err(|e| e.to_string())?;
        let (mut driver, send) = h3::client::new(h3_quinn::Connection::new(conn.clone()))
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(async move {
            let _ = driver.wait_idle().await;
        });
        Ok(H3Client {
            send,
            conn,
            _endpoint: endpoint,
        })
    }

    /// Send a request (body streamed in `chunks`, then optional trailers).
    pub async fn request(
        &mut self,
        req: http::Request<()>,
        chunks: Vec<Bytes>,
        trailers: Option<http::HeaderMap>,
    ) -> Result<H3Response, String> {
        let stream = self
            .send
            .send_request(req)
            .await
            .map_err(|e| e.to_string())?;
        let (mut tx, mut rx) = stream.split();
        let sender = tokio::spawn(async move {
            for c in chunks {
                tx.send_data(c).await?;
            }
            if let Some(t) = trailers {
                tx.send_trailers(t).await?;
            }
            tx.finish().await
        });
        let resp = rx.recv_response().await.map_err(|e| e.to_string())?;
        let mut body = Vec::new();
        while let Some(mut chunk) = rx.recv_data().await.map_err(|e| e.to_string())? {
            let b = chunk.copy_to_bytes(chunk.remaining());
            body.extend_from_slice(&b);
        }
        let trailers = rx.recv_trailers().await.map_err(|e| e.to_string())?;
        let _ = sender.await;
        Ok(H3Response {
            status: resp.status().as_u16(),
            headers: resp.headers().clone(),
            body,
            trailers,
        })
    }

    /// GET `uri` with extra headers.
    pub async fn get(&mut self, uri: &str, headers: &[(&str, &str)]) -> Result<H3Response, String> {
        let mut b = http::Request::builder().method("GET").uri(uri);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        self.request(b.body(()).unwrap(), Vec::new(), None).await
    }
}
