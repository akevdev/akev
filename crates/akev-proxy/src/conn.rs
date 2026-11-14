//! Per-connection context and the protocol dispatch loop. Each accepted socket is sniffed; the
//! loop unwraps layers (PROXY protocol header, SOCKS handshake, CONNECT tunnel, TLS) and
//! re-sniffs the inner stream until it reaches HTTP/1, HTTP/2, or a raw tunnel.

use crate::client_hello;
use crate::errors::exchange_error;
use crate::io::{ClientIo, read_more_timeout};
use crate::proxy_protocol::{self, ParseResult};
use crate::shared::Shared;
use crate::sniff::{Protocol, Sniff, sniff};
use crate::target::{HostPort, TargetContext};
use crate::tunnel::TunnelReason;
use crate::upstream::{ConnectionOwner, RequestOptions};
use akev_core::*;
use bytes::{Buf, Bytes, BytesMut};
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};
use tokio::io::AsyncWriteExt;

/// Nesting limit for tunnels-in-tunnels.
const MAX_DEPTH: u8 = 8;
/// How long an idle, freshly accepted connection may stay silent.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Silence after which a tunnel is assumed to carry a server-speaks-first protocol.
const SERVER_FIRST_TIMEOUT: Duration = Duration::from_secs(2);
/// Max bytes of garbage included in `ClientError` events.
pub(crate) const RAW_PREFIX_LIMIT: usize = 4096;

/// Facts about the accepted TCP connection, shared by all layers.
pub(crate) struct ConnShared {
    pub id: ConnectionId,
    /// Socket addresses as seen by the OS (process resolution needs these).
    pub tcp_peer: SocketAddr,
    pub tcp_local: SocketAddr,
    /// Set to make the client socket close with a TCP RST.
    pub reset: Arc<AtomicBool>,
    /// Identity of this client connection for the upstream pool: upstream connections
    /// authenticated with NTLM / Negotiate for one of its requests stay pinned to it.
    pub owner: ConnectionOwner,
    process: tokio::sync::OnceCell<Option<ProcessInfo>>,
    /// Interceptor that delivered this connection (e.g. `wireguard`).
    interceptor: Option<String>,
    /// Its entry in `Shared::clients` (cut when the client gets blocked).
    pub client_slot: std::sync::OnceLock<u64>,
}

/// State of one protocol layer of a connection.
#[derive(Clone)]
pub(crate) struct ConnCtx {
    pub conn: Arc<ConnShared>,
    /// Client address (from PROXY protocol when present).
    pub peer: SocketAddr,
    pub local: SocketAddr,
    /// Innermost tunnel target (CONNECT / SOCKS).
    pub tunnel: Option<HostPort>,
    pub tunnel_via: ProxyMode,
    /// TLS details of the innermost TLS layer (if we decrypted one since the last tunnel).
    pub tls: Option<TlsInfo>,
    /// Pending "did the client silently reject our certificate?" check after a handshake:
    /// (handshake completion time, observation window).
    pub tls_check: Option<(Instant, Duration)>,
    pub depth: u8,
    /// PROXY protocol is only honoured on the raw TCP layer.
    pub outermost: bool,
    /// The client is on another machine: it can't reach this machine's loopback services
    /// through the proxy (unless `proxy.access.allowLoopbackTargets`).
    pub remote: bool,
    /// The client may use the proxy: no credentials needed, or it presented them (inner
    /// layers of an authenticated tunnel inherit this).
    pub authenticated: bool,
}

impl ConnCtx {
    pub(crate) fn new(peer: SocketAddr, local: SocketAddr, reset: Arc<AtomicBool>) -> Self {
        ConnCtx {
            conn: Arc::new(ConnShared {
                id: ConnectionId::next(),
                tcp_peer: peer,
                tcp_local: local,
                reset,
                owner: ConnectionOwner::new(),
                process: tokio::sync::OnceCell::new(),
                interceptor: None,
                client_slot: std::sync::OnceLock::new(),
            }),
            peer,
            local,
            tunnel: None,
            tunnel_via: ProxyMode::Direct,
            tls: None,
            tls_check: None,
            depth: 0,
            outermost: true,
            remote: false,
            authenticated: true,
        }
    }

    /// A connection whose destination is already known (handed over by e.g. the WireGuard
    /// server): it starts inside a tunnel to `target`, is attributed to `interceptor`, and has
    /// no local process to look up.
    pub(crate) fn tunnelled(
        peer: SocketAddr,
        local: SocketAddr,
        reset: Arc<AtomicBool>,
        target: HostPort,
        via: ProxyMode,
        interceptor: Option<String>,
    ) -> Self {
        let mut ctx = ConnCtx {
            conn: Arc::new(ConnShared {
                id: ConnectionId::next(),
                tcp_peer: peer,
                tcp_local: local,
                reset,
                owner: ConnectionOwner::new(),
                process: tokio::sync::OnceCell::new_with(Some(None)),
                interceptor,
                client_slot: std::sync::OnceLock::new(),
            }),
            peer,
            local,
            tunnel: None,
            tunnel_via: ProxyMode::Direct,
            tls: None,
            tls_check: None,
            depth: 0,
            outermost: true,
            // Authenticated by whoever handed it over (the WireGuard server: the device holds
            // the key the user gave it), so trusted like this machine's own clients: no
            // credentials, and it may reach this machine's loopback as before.
            remote: false,
            authenticated: true,
        };
        ctx.enter_tunnel(target, via);
        ctx
    }

    /// Upstream options for a request received on this connection.
    pub(crate) fn request_options(&self, shared: &Shared) -> RequestOptions {
        RequestOptions {
            owner: Some(self.conn.owner.clone()),
            restrict_loopback: self.restricts_loopback(shared),
            ..Default::default()
        }
    }

    /// Whether this client may not reach this machine's loopback services.
    pub(crate) fn restricts_loopback(&self, shared: &Shared) -> bool {
        self.remote && !shared.settings().access.allow_loopback_targets
    }

    /// How requests on this layer reached the proxy (before looking at the request target).
    pub(crate) fn mode(&self) -> ProxyMode {
        if self.tunnel.is_some() {
            self.tunnel_via
        } else {
            ProxyMode::Direct
        }
    }

    pub(crate) fn target_context(&self) -> TargetContext<'_> {
        TargetContext {
            tunnel: self.tunnel.as_ref(),
            tunnel_via: self.tunnel_via,
            tls: self.tls.is_some(),
            sni: self.tls.as_ref().and_then(|t| t.sni.as_deref()),
            local_port: self.local.port(),
        }
    }

    /// Enter a new tunnel (CONNECT / SOCKS): inner layers start without TLS state.
    pub(crate) fn enter_tunnel(&mut self, target: HostPort, via: ProxyMode) {
        self.tunnel = Some(target);
        self.tunnel_via = via;
        self.tls = None;
        self.tls_check = None;
        self.depth += 1;
        self.outermost = false;
    }

    /// The local process owning the client socket (resolved once per connection, off-thread).
    pub(crate) async fn process(&self, shared: &Shared) -> Option<ProcessInfo> {
        if !shared.settings().capture.detect_processes {
            return None;
        }
        let resolver = shared.process_resolver.clone();
        let (peer, local) = (self.conn.tcp_peer, self.conn.tcp_local);
        self.conn
            .process
            .get_or_init(|| async move {
                tokio::task::spawn_blocking(move || resolver.resolve(peer, local))
                    .await
                    .ok()
                    .flatten()
            })
            .await
            .clone()
    }

    /// Build the `ClientInfo` for an exchange plus the `source` used by rule matchers.
    pub(crate) async fn client_info(
        &self,
        shared: &Shared,
        via: ProxyMode,
    ) -> (ClientInfo, Option<String>) {
        let process = self.process(shared).await;
        let interceptor = self.conn.interceptor.clone();
        let source = interceptor
            .clone()
            .or_else(|| process.as_ref().map(|p| p.name.clone()));
        let info = ClientInfo {
            remote_addr: self.peer.to_string(),
            connection_id: self.conn.id,
            process,
            interceptor,
            via,
            tls: self.tls.clone(),
        };
        (info, source)
    }
}

/// Emit a `ClientError` event for garbage received on a connection.
pub(crate) async fn client_error(shared: &Shared, ctx: &ConnCtx, error: ExchangeError, raw: &[u8]) {
    let (client, _) = ctx.client_info(shared, ctx.mode()).await;
    let raw_prefix = Bytes::copy_from_slice(&raw[..raw.len().min(RAW_PREFIX_LIMIT)]);
    shared.sink.emit(ProxyEvent::ClientError {
        id: ExchangeId::next(),
        client,
        error,
        raw_prefix,
        at: UnixMicros::now(),
    });
}

enum Detected {
    Proto(Protocol),
    /// Tunnel stayed silent: probably a server-speaks-first protocol.
    ServerFirst,
}

fn client_speaks_first_port(port: u16) -> bool {
    matches!(port, 80 | 443 | 8080 | 8443)
}

/// Read until the protocol can be identified. `None` = connection closed / idle / failed.
async fn detect(
    shared: &Shared,
    io: &mut ClientIo,
    buf: &mut BytesMut,
    ctx: &mut ConnCtx,
) -> Option<Detected> {
    loop {
        if !buf.is_empty() {
            match sniff(buf, ctx.outermost) {
                Sniff::Detected(p) => {
                    ctx.tls_check = None;
                    return Some(Detected::Proto(p));
                }
                Sniff::NeedMore => {}
            }
        }
        let server_first = buf.is_empty()
            && ctx.tls.is_none()
            && ctx
                .tunnel
                .as_ref()
                .is_some_and(|t| !client_speaks_first_port(t.port));
        let (timeout, rejection_window) = match ctx.tls_check {
            Some((at, window)) if buf.is_empty() => (
                window
                    .saturating_sub(at.elapsed())
                    .max(Duration::from_millis(1)),
                true,
            ),
            _ if server_first => (SERVER_FIRST_TIMEOUT, false),
            _ => (IDLE_TIMEOUT, false),
        };
        let res = read_more_timeout(io, buf, timeout).await;
        let timed_out = matches!(&res, Err(e) if e.kind() == std::io::ErrorKind::TimedOut);
        if rejection_window {
            ctx.tls_check = None;
            match res {
                // Window passed quietly (or data arrived): the client accepted our certificate.
                _ if timed_out => continue,
                Ok(n) if n > 0 => continue,
                // The handshake completed but the client hung up without sending anything:
                // the classic sign of a client that doesn't trust our certificate.
                _ => {
                    crate::tls::silent_rejection(shared, ctx).await;
                    return None;
                }
            }
        }
        match res {
            Err(_) if timed_out => return server_first.then_some(Detected::ServerFirst),
            Err(_) => return None,
            Ok(0) => {
                return if buf.is_empty() {
                    None
                } else {
                    Some(Detected::Proto(Protocol::Unknown))
                };
            }
            Ok(_) => {}
        }
    }
}

/// Handle a connection layer by layer until it ends.
///
/// Boxed with an explicit type: connections recurse through HTTP/2 CONNECT streams, and the
/// concrete future type would otherwise be self-referential.
pub(crate) fn handle(
    shared: Arc<Shared>,
    io: ClientIo,
    prefix: BytesMut,
    ctx: ConnCtx,
) -> Pin<Box<dyn Future<Output = ()> + Send>> {
    Box::pin(handle_layers(shared, io, prefix, ctx))
}

async fn handle_layers(shared: Arc<Shared>, io: ClientIo, prefix: BytesMut, ctx: ConnCtx) {
    let mut io = io;
    let mut buf = prefix;
    let mut ctx = ctx;
    loop {
        if ctx.depth > MAX_DEPTH {
            tracing::debug!(conn = %ctx.conn.id, "tunnel nesting limit reached");
            return;
        }
        let detected = tokio::select! {
            d = detect(&shared, &mut io, &mut buf, &mut ctx) => d,
            _ = shared.shutdown.cancelled(), if buf.is_empty() => None,
        };
        let Some(detected) = detected else { return };
        let protocol = match detected {
            Detected::ServerFirst => {
                if let Some(t) = ctx.tunnel.clone() {
                    crate::tunnel::run(
                        shared.clone(),
                        io,
                        Bytes::new(),
                        t.host,
                        t.port,
                        &ctx,
                        TunnelReason::ServerSpeaksFirst,
                        None,
                    )
                    .await;
                }
                return;
            }
            Detected::Proto(p) => p,
        };
        match protocol {
            // Without proxy credentials there is nothing to authenticate TLS or HTTP/2 prior
            // knowledge sent straight to the port: refuse them.
            Protocol::Tls | Protocol::H2Preface if !ctx.authenticated => {
                shared.auth_failed(ctx.peer, "no proxy credentials before TLS / HTTP/2");
                return;
            }
            Protocol::Tls => {
                match crate::tls::handle(&shared, io, std::mem::take(&mut buf), &mut ctx).await {
                    Some(inner) => {
                        io = inner;
                        ctx.depth += 1;
                        ctx.outermost = false;
                    }
                    None => return,
                }
            }
            Protocol::Socks4 | Protocol::Socks5 if shared.settings().proxy.socks => {
                let settings = shared.settings();
                let auth = (!ctx.authenticated).then_some(&settings.access);
                match crate::socks::handshake(&mut io, &mut buf, auth).await {
                    Ok(req) => {
                        ctx.authenticated = true;
                        ctx.enter_tunnel(req.target, ProxyMode::Socks)
                    }
                    Err(crate::socks::SocksError::AuthFailed) => {
                        shared.auth_failed(ctx.peer, "SOCKS");
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(conn = %ctx.conn.id, error = %e, "SOCKS handshake failed");
                        return;
                    }
                }
            }
            // PROXY protocol headers are only trusted when the settings say the peer is a load
            // balancer that vouches for the client address; otherwise they are garbage.
            Protocol::ProxyV1 | Protocol::ProxyV2
                if ctx.outermost && shared.settings().access.trust_proxy_protocol =>
            {
                let parsed = loop {
                    let r = if protocol == Protocol::ProxyV1 {
                        proxy_protocol::parse_v1(&buf)
                    } else {
                        proxy_protocol::parse_v2(&buf)
                    };
                    match r {
                        ParseResult::NeedMore => {
                            match read_more_timeout(&mut io, &mut buf, Duration::from_secs(10))
                                .await
                            {
                                Ok(n) if n > 0 => continue,
                                _ => return,
                            }
                        }
                        other => break other,
                    }
                };
                match parsed {
                    ParseResult::Complete(h) => {
                        buf.advance(h.len);
                        if let Some((src, dst)) = h.addrs {
                            // The access policy and credentials apply to the real client.
                            if !shared.admit(src.ip()) {
                                return;
                            }
                            if let Some(slot) = ctx.conn.client_slot.get() {
                                shared.clients.update(*slot, src.ip());
                            }
                            ctx.peer = src;
                            ctx.local = dst;
                            ctx.remote = !akev_core::access::is_loopback(src.ip());
                            ctx.authenticated = !shared.settings().access.needs_auth(src.ip());
                        }
                        ctx.outermost = false;
                    }
                    ParseResult::Invalid(why) => {
                        let err = exchange_error(
                            ErrorKind::ClientProtocolError,
                            format!("invalid PROXY protocol header: {why}"),
                            Some("PROXY_PROTOCOL"),
                        );
                        client_error(&shared, &ctx, err, &buf).await;
                        return;
                    }
                    ParseResult::NeedMore => return,
                }
            }
            Protocol::H2Preface => {
                crate::h2::serve(shared.clone(), io, buf, ctx).await;
                return;
            }
            Protocol::Http1 => {
                match crate::http1::server::serve(shared.clone(), io, buf, ctx).await {
                    Some((inner, rest, next_ctx)) => {
                        io = inner;
                        buf = rest;
                        ctx = next_ctx;
                    }
                    None => return,
                }
            }
            _ => {
                unknown_protocol(shared, io, buf, ctx).await;
                return;
            }
        }
        ctx.outermost = false;
    }
}

/// Bytes we can't interpret: tunnel them if we know where they were going, else report.
async fn unknown_protocol(shared: Arc<Shared>, mut io: ClientIo, buf: BytesMut, ctx: ConnCtx) {
    if let Some(t) = ctx.tunnel.clone() {
        // Decrypted TLS carrying a non-HTTP protocol: re-encrypt towards the server.
        let rewrap = ctx.tls.as_ref().map(|tls| {
            (
                tls.sni.clone().unwrap_or_else(|| t.host.clone()),
                tls.alpn.clone(),
            )
        });
        crate::tunnel::run(
            shared,
            io,
            buf.freeze(),
            t.host,
            t.port,
            &ctx,
            TunnelReason::UnknownProtocol,
            rewrap,
        )
        .await;
        return;
    }
    let looks_like_tls_garbage = client_hello::parse(&buf) != client_hello::HelloParse::NeedMore
        && buf.first() == Some(&0x16);
    let message = if looks_like_tls_garbage {
        "unparseable TLS data and no known destination".to_string()
    } else {
        "unrecognised protocol and no known destination to tunnel it to".to_string()
    };
    let err = exchange_error(
        ErrorKind::ClientProtocolError,
        message,
        Some("UNKNOWN_PROTOCOL_NO_DESTINATION"),
    );
    client_error(&shared, &ctx, err, &buf).await;
    let _ = io
        .write_all(b"HTTP/1.1 400 Bad Request\r\nConnection: close\r\nContent-Length: 0\r\n\r\n")
        .await;
    let _ = io.shutdown().await;
}
