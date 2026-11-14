//! HTTP/3 server side: QUIC connections accepted from clients (Akev's per-SNI leaf
//! certificates, ALPN `h3`), each request stream turned into an exchange through the shared
//! pipeline (rules, breakpoints, recording, upstream over HTTP/1.1, HTTP/2 or HTTP/3) and
//! answered over HTTP/3.

use crate::body::ClientBody;
use crate::conn::ConnCtx;
use crate::errors::exchange_error;
use crate::local::{MAGIC_HOST, is_local_target};
use crate::pipeline::{self, ExchangeEnv, ExchangeRequest};
use crate::responder::{AbortKind, BodyKind, Responder};
use crate::shared::{CounterGuard, Shared};
use crate::target::{HostPort, RequestTarget, build_url, resolve_target};
use crate::upstream::{BodyChunk, BodyStream, UpstreamError};
use akev_ca::{CertificateAuthority, CertifiedKey};
use akev_core::*;
use bytes::{Buf, Bytes};
use h3::error::Code;
use parking_lot::Mutex;
use rustls::server::{ClientHello, ResolvesServerCert, StoresServerSessions};
use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Limit for the QUIC + TLS handshake of a client.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// Limit for the HTTP/3 SETTINGS exchange after the handshake.
const SETUP_TIMEOUT: Duration = Duration::from_secs(10);
/// Largest request header section accepted.
const MAX_FIELD_SECTION: u64 = 256 * 1024;
/// Concurrent request streams per connection.
const MAX_STREAMS: u32 = 256;
/// How long a request whose HEADERS didn't say whether a body follows may take to show it,
/// for methods without request-body semantics (as for HTTP/2).
const BODY_PEEK_GRACE: Duration = Duration::from_millis(500);
/// Server configs cached per local address (connections without SNI get a certificate for
/// the address they connected to).
const MAX_CONFIGS: usize = 64;

type SendHalf = h3::server::RequestStream<h3_quinn::SendStream<Bytes>, Bytes>;
type RecvHalf = h3::server::RequestStream<h3_quinn::RecvStream, Bytes>;
type Resolver = h3::server::RequestResolver<h3_quinn::Connection, Bytes>;

/// Leaf certificates by SNI; connections without SNI get one for `fallback` (the address the
/// client connected to).
struct QuicCertResolver {
    ca: Arc<CertificateAuthority>,
    fallback: String,
}

impl std::fmt::Debug for QuicCertResolver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicCertResolver")
            .field("fallback", &self.fallback)
            .finish()
    }
}

impl ResolvesServerCert for QuicCertResolver {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let name = hello.server_name().unwrap_or(&self.fallback);
        match self.ca.leaf_for(name) {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(host = name, error = %e, "leaf certificate generation failed");
                None
            }
        }
    }
}

/// Builds (and caches, per local address) the QUIC server configuration.
pub(crate) struct ServerConfigs {
    ca: Arc<CertificateAuthority>,
    sessions: Arc<dyn StoresServerSessions>,
    by_local: Mutex<HashMap<String, quinn::ServerConfig>>,
}

impl ServerConfigs {
    pub(crate) fn new(ca: Arc<CertificateAuthority>) -> Self {
        ServerConfigs {
            ca,
            sessions: rustls::server::ServerSessionMemoryCache::new(1024),
            by_local: Mutex::new(HashMap::new()),
        }
    }

    /// The configuration for connections to `local` (certificate fallback without SNI).
    pub(crate) fn get(&self, local: Option<IpAddr>) -> Result<quinn::ServerConfig, String> {
        let fallback = match local {
            Some(ip) if !ip.is_unspecified() => super::canonical_ip(ip).to_string(),
            _ => "localhost".to_string(),
        };
        if let Some(c) = self.by_local.lock().get(&fallback) {
            return Ok(c.clone());
        }
        let config = build_server_config(self.ca.clone(), self.sessions.clone(), &fallback)?;
        let mut map = self.by_local.lock();
        if map.len() >= MAX_CONFIGS {
            map.clear();
        }
        map.insert(fallback, config.clone());
        Ok(config)
    }
}

fn build_server_config(
    ca: Arc<CertificateAuthority>,
    sessions: Arc<dyn StoresServerSessions>,
    fallback: &str,
) -> Result<quinn::ServerConfig, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut tls = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| format!("QUIC TLS configuration error: {e}"))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(QuicCertResolver {
            ca,
            fallback: fallback.to_string(),
        }));
    tls.alpn_protocols = vec![b"h3".to_vec()];
    tls.session_storage = sessions;
    // No 0-RTT: replayable early requests must not reach rules / upstream twice.
    tls.max_early_data_size = 0;
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| format!("QUIC TLS configuration error: {e}"))?;
    let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    let mut transport = quinn::TransportConfig::default();
    transport.max_concurrent_bidi_streams(MAX_STREAMS.into());
    transport.max_idle_timeout(quinn::IdleTimeout::try_from(Duration::from_secs(60)).ok());
    config.transport_config(Arc::new(transport));
    Ok(config)
}

/// Accept QUIC connections from `endpoint` until it is closed or the proxy shuts down.
/// `original_dst`: where the client's packets were really going (transparent setups), used
/// like a tunnel target to resolve request URLs.
pub(crate) async fn serve_endpoint(
    shared: Arc<Shared>,
    configs: Arc<ServerConfigs>,
    endpoint: quinn::Endpoint,
    original_dst: Option<SocketAddr>,
) {
    let listen_addr = endpoint
        .local_addr()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
    loop {
        let incoming = tokio::select! {
            i = endpoint.accept() => match i {
                Some(i) => i,
                None => break,
            },
            _ = shared.shutdown.cancelled() => break,
        };
        let peer = incoming.remote_address();
        let local_ip = incoming.local_ip();
        if crate::upstream::is_own_quic_endpoint(peer, local_ip) {
            tracing::warn!(%peer, "proxy loop detected (QUIC connection from Akev's own upstream socket); refusing");
            incoming.refuse();
            continue;
        }
        // The client access policy. QUIC clients can't send proxy credentials, so when those
        // are required only this machine (or a tunnel with a known destination) may use it.
        if original_dst.is_none() {
            if !shared.admit(peer.ip()) {
                incoming.refuse();
                continue;
            }
            if shared.settings().access.needs_auth(peer.ip()) {
                shared.auth_failed(peer, "HTTP/3 without proxy credentials");
                incoming.refuse();
                continue;
            }
        }
        let config = match configs.get(local_ip.or(original_dst.map(|d| d.ip()))) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "cannot accept QUIC connection");
                incoming.refuse();
                continue;
            }
        };
        let connecting = match incoming.accept_with(Arc::new(config)) {
            Ok(c) => c,
            Err(e) => {
                tracing::debug!(%peer, error = %e, "QUIC connection not accepted");
                continue;
            }
        };
        let local = SocketAddr::new(local_ip.unwrap_or(listen_addr.ip()), listen_addr.port());
        let task_shared = shared.clone();
        shared.tracker.spawn(async move {
            serve_connection(task_shared, connecting, peer, local, original_dst).await;
        });
    }
}

/// Classify a failed QUIC handshake with a client.
fn handshake_failure(e: &quinn::ConnectionError, host: &str) -> Option<ExchangeError> {
    let rejected = |alert: u64| {
        // 0x100 + TLS alert: bad_certificate (42) .. certificate_unknown (46), unknown_ca
        // (48), access_denied (49), decrypt_error (51).
        matches!(alert, 42..=46 | 48 | 49 | 51)
    };
    let hint = format!(
        "The client rejected Akev's certificate for {host} over HTTP/3. Make sure the Akev CA is installed and trusted by this client."
    );
    match e {
        quinn::ConnectionError::ConnectionClosed(close) => {
            let code = u64::from(close.error_code);
            if (0x100..0x200).contains(&code) && rejected(code - 0x100) {
                Some(exchange_error(
                    ErrorKind::ClientTlsRejected,
                    format!("{hint} ({close})"),
                    Some("QUIC_TLS_ALERT"),
                ))
            } else {
                Some(exchange_error(
                    ErrorKind::ClientProtocolError,
                    format!("The client aborted the QUIC handshake: {close}"),
                    Some("QUIC_CLOSED"),
                ))
            }
        }
        quinn::ConnectionError::TransportError(t) => Some(exchange_error(
            ErrorKind::ClientProtocolError,
            format!("QUIC handshake failed: {t}"),
            Some("QUIC_TRANSPORT_ERROR"),
        )),
        _ => None,
    }
}

async fn serve_connection(
    shared: Arc<Shared>,
    mut connecting: quinn::Connecting,
    peer: SocketAddr,
    local: SocketAddr,
    original_dst: Option<SocketAddr>,
) {
    shared
        .stats
        .total_connections
        .fetch_add(1, Ordering::Relaxed);
    let _active = CounterGuard::new(&shared.stats.active_connections);
    let mut ctx = ConnCtx::new(peer, local, Arc::new(AtomicBool::new(false)));
    ctx.remote = !akev_core::access::is_loopback(peer.ip());
    if let Some(dst) = original_dst {
        ctx.tunnel = Some(HostPort::new(
            super::canonical_ip(dst.ip()).to_string(),
            dst.port(),
        ));
        ctx.tunnel_via = ProxyMode::Direct;
    }
    // SNI and ALPN are known once the ClientHello is processed (also for failure reports).
    let hello = tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting.handshake_data())
        .await
        .ok()
        .and_then(Result::ok)
        .and_then(|d| d.downcast::<quinn::crypto::rustls::HandshakeData>().ok());
    let sni = hello.as_ref().and_then(|d| d.server_name.clone());
    ctx.tls = Some(TlsInfo {
        sni: sni.clone(),
        alpn: hello
            .as_ref()
            .and_then(|d| d.protocol.as_deref())
            .map(|p| String::from_utf8_lossy(p).into_owned()),
        version: Some("TLSv1.3".into()),
        ..Default::default()
    });
    let conn = match tokio::time::timeout(HANDSHAKE_TIMEOUT, connecting).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            let host = sni.clone().unwrap_or_else(|| local.ip().to_string());
            if let Some(err) = handshake_failure(&e, &host) {
                report_tls_failure(
                    &shared,
                    &ctx,
                    host,
                    ctx.tunnel.as_ref().map_or(local.port(), |t| t.port),
                    err,
                )
                .await;
            } else {
                tracing::debug!(%peer, error = %e, "QUIC handshake failed");
            }
            return;
        }
        Err(_) => {
            tracing::debug!(%peer, "QUIC handshake timed out");
            return;
        }
    };
    let mut builder = h3::server::builder();
    builder.max_field_section_size(MAX_FIELD_SECTION);
    let setup = builder.build::<_, Bytes>(h3_quinn::Connection::new(conn.clone()));
    let mut h3conn = match tokio::time::timeout(SETUP_TIMEOUT, setup).await {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            tracing::debug!(%peer, error = %e, "HTTP/3 connection setup failed");
            return;
        }
        Err(_) => return,
    };
    let ctx = Arc::new(ctx);
    let streams = tokio_util::task::TaskTracker::new();
    let mut graceful = false;
    loop {
        tokio::select! {
            accepted = h3conn.accept() => match accepted {
                Ok(Some(resolver)) => {
                    let task_shared = shared.clone();
                    let ctx = ctx.clone();
                    let quic = conn.clone();
                    streams.spawn(async move { stream_task(task_shared, ctx, resolver, quic).await });
                }
                Ok(None) => break,
                Err(e) => {
                    if !e.is_h3_no_error() {
                        tracing::debug!(conn = %ctx.conn.id, error = %e, "HTTP/3 connection error");
                    }
                    break;
                }
            },
            _ = shared.shutdown.cancelled(), if !graceful => {
                graceful = true;
                // GOAWAY: in-flight requests finish, new ones go elsewhere; the connection
                // closes once they are done.
                let _ = h3conn.shutdown(0).await;
                streams.close();
            }
            _ = streams.wait(), if graceful => break,
            _ = shared.hard_stop.cancelled() => break,
        }
    }
    // Requests already accepted finish before the connection is dropped (which closes it).
    streams.close();
    tokio::select! {
        _ = streams.wait() => {}
        _ = shared.hard_stop.cancelled() => {}
    }
    drop(h3conn);
    conn.close(quinn::VarInt::from_u32(0x100), b"");
    let stats = conn.stats();
    shared
        .stats
        .bytes
        .bytes_in
        .fetch_add(stats.udp_rx.bytes, Ordering::Relaxed);
    shared
        .stats
        .bytes
        .bytes_out
        .fetch_add(stats.udp_tx.bytes, Ordering::Relaxed);
}

/// Emit `TlsFailed` (if recording TLS failures and the host isn't ignored).
async fn report_tls_failure(
    shared: &Shared,
    ctx: &ConnCtx,
    host: String,
    port: u16,
    error: ExchangeError,
) {
    let settings = shared.settings();
    if !settings.capture.record_tls_failures || settings.ignore.matches(&host, Some(port)) {
        return;
    }
    let (client, _) = ctx.client_info(shared, ctx.mode()).await;
    shared.sink.emit(ProxyEvent::TlsFailed {
        id: ExchangeId::next(),
        client,
        host,
        port,
        error,
        at: UnixMicros::now(),
    });
}

fn body_error(e: h3::error::StreamError) -> UpstreamError {
    UpstreamError {
        kind: ErrorKind::ClientAborted,
        message: format!("HTTP/3 request body error: {e}"),
        code: Some("H3_REQUEST_BODY".into()),
    }
}

enum Item {
    Data(Bytes),
    Trailers(Headers),
    End,
    Error(UpstreamError),
}

/// The next request body item (empty DATA frames skipped).
async fn next_item(recv: &mut RecvHalf, trailers: &mut bool) -> Item {
    loop {
        if !*trailers {
            let data = match recv.recv_data().await {
                Ok(Some(mut b)) => Ok(Some(b.copy_to_bytes(b.remaining()))),
                Ok(None) => Ok(None),
                Err(e) => Err(e),
            };
            match data {
                Ok(Some(d)) if d.is_empty() => continue,
                Ok(Some(d)) => return Item::Data(d),
                Ok(None) => *trailers = true,
                Err(e) => return Item::Error(body_error(e)),
            }
        } else {
            return match recv.recv_trailers().await {
                Ok(Some(map)) => Item::Trailers(Headers::from_header_map(&map)),
                Ok(None) => Item::End,
                Err(e) => Item::Error(body_error(e)),
            };
        }
    }
}

/// Request body stream (QUIC flow control propagates backpressure as it is polled).
fn h3_body(first: Option<Item>, recv: RecvHalf) -> BodyStream {
    struct State {
        first: Option<Item>,
        recv: RecvHalf,
        trailers: bool,
    }
    let state = State {
        first,
        recv,
        trailers: false,
    };
    Box::pin(futures::stream::unfold(Some(state), |state| async move {
        let mut st = state?;
        let item = match st.first.take() {
            Some(item) => item,
            None => next_item(&mut st.recv, &mut st.trailers).await,
        };
        match item {
            Item::Data(d) => Some((Ok(BodyChunk::Data(d)), Some(st))),
            Item::Trailers(t) => Some((Ok(BodyChunk::Trailers(t)), None)),
            Item::End => None,
            Item::Error(e) => Some((Err(e), None)),
        }
    }))
}

/// Methods whose requests normally carry a body.
fn method_has_request_body(method: &str) -> bool {
    ["POST", "PUT", "PATCH"]
        .iter()
        .any(|m| method.eq_ignore_ascii_case(m))
}

/// Connection-specific headers that must not appear in HTTP/3 messages.
fn is_connection_specific(name: &str) -> bool {
    [
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "upgrade",
        "host",
    ]
    .iter()
    .any(|h| name.eq_ignore_ascii_case(h))
}

/// Merge `cookie` headers into one (HTTP/1 servers expect a single Cookie header).
fn merge_cookies(headers: &mut Headers) {
    let cookies: Vec<String> = headers.get_all("cookie").map(str::to_string).collect();
    if cookies.len() > 1 {
        headers.set("cookie", cookies.join("; "));
    }
}

fn h3_io_error(e: h3::error::StreamError) -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, e.to_string())
}

/// Writes responses on an HTTP/3 request stream.
pub(crate) struct H3Responder {
    send: SendHalf,
    quic: quinn::Connection,
    head_request: bool,
    expect_continue: bool,
    continue_sent: bool,
    head_sent: bool,
    finished: bool,
}

impl H3Responder {
    pub(crate) fn head_sent(&self) -> bool {
        self.head_sent
    }

    pub(crate) async fn send_continue(&mut self) {
        if self.expect_continue && !self.continue_sent && !self.head_sent {
            self.continue_sent = true;
            if let Ok(resp) = ::http::Response::builder().status(100).body(()) {
                let _ = self.send.send_response(resp).await;
            }
        }
    }

    pub(crate) async fn send_head(
        &mut self,
        head: &ResponseHead,
        body: BodyKind,
    ) -> io::Result<()> {
        let mut builder = ::http::Response::builder().status(head.status);
        // Transfer-Encoding overrides Content-Length (RFC 9112 §6.3).
        let drop_length = head.headers.contains("transfer-encoding");
        for (name, value) in head.headers.iter() {
            if name.starts_with(':') || is_connection_specific(name) {
                continue;
            }
            if drop_length && name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            if name.eq_ignore_ascii_case("te") && !value.eq_ignore_ascii_case("trailers") {
                continue;
            }
            match (
                ::http::HeaderName::from_bytes(name.as_bytes()),
                ::http::HeaderValue::from_bytes(&string_to_header_bytes(value)),
            ) {
                (Ok(n), Ok(v)) => builder = builder.header(n, v),
                _ => tracing::debug!(header = name, "dropping invalid HTTP/3 response header"),
            }
        }
        let resp = builder.body(()).map_err(io::Error::other)?;
        let no_body = self.head_request
            || (100..200).contains(&head.status)
            || head.status == 204
            || head.status == 304;
        self.head_sent = true;
        self.send.send_response(resp).await.map_err(h3_io_error)?;
        if no_body || body == BodyKind::None {
            self.finished = true;
            self.send.finish().await.map_err(h3_io_error)?;
        }
        Ok(())
    }

    pub(crate) async fn send_data(&mut self, data: Bytes) -> io::Result<()> {
        if self.finished || data.is_empty() {
            return Ok(());
        }
        self.send.send_data(data).await.map_err(h3_io_error)
    }

    pub(crate) async fn finish(&mut self, trailers: Option<&Headers>) -> io::Result<()> {
        if std::mem::replace(&mut self.finished, true) {
            return Ok(());
        }
        if let Some(t) = trailers.filter(|t| !t.is_empty()) {
            self.send
                .send_trailers(t.to_header_map())
                .await
                .map_err(h3_io_error)?;
        }
        self.send.finish().await.map_err(h3_io_error)
    }

    pub(crate) fn abort(&mut self, kind: AbortKind) {
        self.finished = true;
        let code = match kind {
            AbortKind::Close => Code::H3_REQUEST_CANCELLED,
            AbortKind::Reset | AbortKind::Incomplete => Code::H3_INTERNAL_ERROR,
        };
        self.send.stop_stream(code);
    }

    /// Resolves when the client connection is gone (a reset of just this stream is noticed
    /// when writing to it).
    pub(crate) async fn client_gone(&mut self) {
        self.quic.closed().await;
    }
}

async fn reject(send: &mut SendHalf, status: u16) {
    if let Ok(resp) = ::http::Response::builder().status(status).body(())
        && send.send_response(resp).await.is_ok()
    {
        let _ = send.finish().await;
    }
}

async fn stream_task(
    shared: Arc<Shared>,
    ctx: Arc<ConnCtx>,
    resolver: Resolver,
    quic: quinn::Connection,
) {
    let sw = Stopwatch::start();
    let (req, stream) = match resolver.resolve_request().await {
        Ok(r) => r,
        Err(e) => {
            tracing::debug!(conn = %ctx.conn.id, error = %e, "HTTP/3 request rejected");
            return;
        }
    };
    let (mut send, mut recv) = stream.split();
    let (parts, ()) = req.into_parts();
    if parts.method == ::http::Method::CONNECT {
        // CONNECT tunnels over HTTP/3 aren't supported (clients use HTTP/1.1 or HTTP/2
        // proxies); extended CONNECT isn't negotiated.
        let err = exchange_error(
            ErrorKind::ClientProtocolError,
            "CONNECT over HTTP/3 is not supported (use the proxy port over TCP)",
            Some("H3_CONNECT_UNSUPPORTED"),
        );
        crate::conn::client_error(&shared, &ctx, err, b"CONNECT").await;
        reject(&mut send, 501).await;
        return;
    }
    let headers = Headers::from_header_map(&parts.headers);
    let authority = parts
        .uri
        .authority()
        .map(|a| a.as_str().to_string())
        .or_else(|| headers.get("host").map(str::to_string));
    let raw_path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let target = if raw_path == "*" {
        RequestTarget::Asterisk
    } else {
        RequestTarget::Origin(raw_path.to_string())
    };
    let resolved = match resolve_target(
        &ctx.target_context(),
        &target,
        authority.as_deref(),
        parts.uri.scheme_str(),
        false,
    ) {
        Ok(r) => r,
        Err(e) => {
            let err = exchange_error(
                ErrorKind::ClientProtocolError,
                e.to_string(),
                Some("ERR_INVALID_URL"),
            );
            crate::conn::client_error(&shared, &ctx, err, raw_path.as_bytes()).await;
            reject(&mut send, 400).await;
            return;
        }
    };
    let head = RequestHead {
        method: parts.method.as_str().to_string(),
        url: build_url(
            &resolved.scheme,
            &resolved.host,
            resolved.port,
            &resolved.path,
        ),
        scheme: resolved.scheme.clone(),
        host: resolved.host.clone(),
        port: resolved.port,
        path: resolved.path.clone(),
        http_version: HttpVersion::H3,
        headers,
    };
    // HTTP/3 → HTTP/1 translation for the forwarded request (Host from :authority, one
    // Cookie header). Harmless for HTTP/2 and HTTP/3 upstreams.
    let mut forward = head.clone();
    if !forward.headers.contains("host") {
        let authority = forward.authority();
        forward.headers.0.insert(0, ("host".to_string(), authority));
    }
    merge_cookies(&mut forward.headers);
    let expect_continue = crate::http1::parse::expects_continue(&head.headers);
    let request_headers_ms = sw.elapsed_ms();
    let declared = head.headers.content_length();
    let mut first = None;
    let mut no_body = declared == Some(0);
    let unknown_length = declared.is_none() && !forward.headers.contains("transfer-encoding");
    if unknown_length && !method_has_request_body(&head.method) && !expect_continue {
        // HTTP/3 HEADERS don't say whether a body follows: find out (briefly) before choosing
        // the HTTP/1 framing, so a bodiless GET isn't forwarded as chunked.
        let mut trailers = false;
        match tokio::time::timeout(BODY_PEEK_GRACE, next_item(&mut recv, &mut trailers)).await {
            Ok(Item::End) => no_body = true,
            Ok(item) => first = Some(item),
            Err(_) => {}
        }
    }
    if unknown_length && !no_body {
        forward.headers.append("transfer-encoding", "chunked");
    }
    let local = is_local_target(&head.host, head.port, resolved.mode, ctx.local).then(|| {
        if head.host.eq_ignore_ascii_case(MAGIC_HOST) {
            crate::local::display_addr(ctx.local)
        } else {
            authority
                .clone()
                .unwrap_or_else(|| crate::local::display_addr(ctx.local))
        }
    });
    let body = if no_body {
        ClientBody::Empty
    } else {
        ClientBody::Stream {
            length: declared,
            stream: h3_body(first, recv),
        }
    };
    let head_request = head.method.eq_ignore_ascii_case("HEAD");
    let (client, source) = ctx.client_info(&shared, resolved.mode).await;
    let env = ExchangeEnv {
        client,
        source,
        request_headers_ms,
        sw,
        apply_rules: true,
        options: ctx.request_options(&shared),
        local,
    };
    let req = ExchangeRequest {
        head,
        forward,
        body,
        upgrade: None,
    };
    let mut responder = H3Responder {
        send,
        quic,
        head_request,
        expect_continue,
        continue_sent: false,
        head_sent: false,
        finished: false,
    };
    let mut r = Responder::H3(&mut responder);
    let _ = pipeline::run(&shared, env, req, &mut r).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_specific_headers() {
        assert!(is_connection_specific("Transfer-Encoding"));
        assert!(is_connection_specific("host"));
        assert!(!is_connection_specific("content-length"));
    }

    #[test]
    fn cookies_merge() {
        let mut h: Headers = vec![
            ("cookie".to_string(), "a=1".to_string()),
            ("cookie".to_string(), "b=2".to_string()),
        ]
        .into_iter()
        .collect();
        merge_cookies(&mut h);
        assert_eq!(h.get_all("cookie").collect::<Vec<_>>(), vec!["a=1; b=2"]);
    }

    #[test]
    fn handshake_failures_are_classified() {
        use quinn::ConnectionError as E;
        assert!(handshake_failure(&E::TimedOut, "a.test").is_none());
        assert!(handshake_failure(&E::LocallyClosed, "a.test").is_none());
    }

    #[tokio::test]
    async fn server_configs_are_cached_per_local_address() {
        let ca = Arc::new(CertificateAuthority::generate().unwrap());
        let configs = ServerConfigs::new(ca.clone());
        configs.get(Some([127, 0, 0, 1].into())).unwrap();
        configs.get(Some([127, 0, 0, 1].into())).unwrap();
        configs.get(None).unwrap();
        configs.get(Some([0, 0, 0, 0].into())).unwrap();
        assert_eq!(configs.by_local.lock().len(), 2);
    }
}
