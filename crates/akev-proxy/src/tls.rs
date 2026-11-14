//! TLS interception: ClientHello analysis (SNI, ALPN, JA3/JA4) from the peeked bytes, the
//! passthrough decision, the rustls MITM handshake with on-the-fly leaf certificates, and
//! classification of client-side handshake failures (including clients that silently hang up
//! after the handshake because they don't trust our CA).

use crate::client_hello::{self, ClientHello, HelloParse};
use crate::conn::ConnCtx;
use crate::errors::exchange_error;
use crate::io::{ClientIo, Rewind, read_more_timeout};
use crate::shared::Shared;
use crate::tunnel::TunnelReason;
use crate::upstream::{BoxIo, UpstreamError};
use akev_ca::{CertificateAuthority, CertifiedKey};
use akev_core::*;
use bytes::BytesMut;
use rustls::crypto::CryptoProvider;
use rustls::server::{
    ClientHello as RustlsHello, ResolvesServerCert, ServerSessionMemoryCache, StoresServerSessions,
};
use rustls::{AlertDescription, ProtocolVersion, ServerConfig};
use rustls_pki_types::ServerName;
use std::fmt;
use std::sync::Arc;
use std::time::{Duration, Instant};

const HELLO_TIMEOUT: Duration = Duration::from_secs(15);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// Resolves leaf certificates by SNI through the CA's cache.
struct SniResolver {
    ca: Arc<CertificateAuthority>,
}

impl fmt::Debug for SniResolver {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SniResolver")
    }
}

impl ResolvesServerCert for SniResolver {
    fn resolve(&self, hello: RustlsHello<'_>) -> Option<Arc<CertifiedKey>> {
        let sni = hello.server_name()?;
        match self.ca.leaf_for(sni) {
            Ok(key) => Some(key),
            Err(e) => {
                tracing::warn!(host = sni, error = %e, "leaf certificate generation failed");
                None
            }
        }
    }
}

/// Always presents one certificate (connections without SNI).
#[derive(Debug)]
struct FixedResolver(Arc<CertifiedKey>);

impl ResolvesServerCert for FixedResolver {
    fn resolve(&self, _hello: RustlsHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.clone())
    }
}

/// Shared rustls configurations (shared so TLS session resumption works across connections).
pub(crate) struct TlsConfigs {
    provider: Arc<CryptoProvider>,
    sessions: Arc<dyn StoresServerSessions>,
    with_h2: Arc<ServerConfig>,
    http1_only: Arc<ServerConfig>,
}

fn alpn(h2: bool) -> Vec<Vec<u8>> {
    if h2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    }
}

impl TlsConfigs {
    pub(crate) fn new(ca: Arc<CertificateAuthority>) -> Result<Self, rustls::Error> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let sessions: Arc<dyn StoresServerSessions> = ServerSessionMemoryCache::new(4096);
        let resolver: Arc<dyn ResolvesServerCert> = Arc::new(SniResolver { ca });
        let build = |h2: bool| -> Result<Arc<ServerConfig>, rustls::Error> {
            let mut cfg = ServerConfig::builder_with_provider(provider.clone())
                .with_safe_default_protocol_versions()?
                .with_no_client_auth()
                .with_cert_resolver(resolver.clone());
            cfg.alpn_protocols = alpn(h2);
            cfg.session_storage = sessions.clone();
            Ok(Arc::new(cfg))
        };
        Ok(TlsConfigs {
            with_h2: build(true)?,
            http1_only: build(false)?,
            provider,
            sessions,
        })
    }

    /// Config presenting a fixed certificate (no SNI).
    fn fixed(&self, key: Arc<CertifiedKey>, h2: bool) -> Result<Arc<ServerConfig>, rustls::Error> {
        let mut cfg = ServerConfig::builder_with_provider(self.provider.clone())
            .with_safe_default_protocol_versions()?
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(FixedResolver(key)));
        cfg.alpn_protocols = alpn(h2);
        cfg.session_storage = self.sessions.clone();
        Ok(Arc::new(cfg))
    }
}

/// Re-encrypt towards the server (non-HTTP protocol inside an intercepted TLS connection).
///
/// Uses the same upstream TLS settings as HTTP requests (trusted roots and extra CA files,
/// ignored certificate errors, client certificates, error messages), with the client's own
/// ALPN.
pub(crate) async fn connect_upstream(
    shared: &Shared,
    io: BoxIo,
    host: &str,
    port: u16,
    sni: &str,
    alpn: Option<&str>,
) -> Result<BoxIo, UpstreamError> {
    use crate::upstream::tls::{Alpn, map_handshake_error};
    let ctx = shared.upstream.tls_context();
    let insecure = ctx.ignores_errors_for(host, port);
    let base = ctx.client_config(insecure, Alpn::Http1, ctx.client_cert_for(host, port))?;
    let mut cfg = (*base).clone();
    cfg.alpn_protocols = alpn
        .map(|a| vec![a.as_bytes().to_vec()])
        .unwrap_or_default();
    let name = ServerName::try_from(sni.to_string()).map_err(|e| UpstreamError {
        kind: ErrorKind::UpstreamTlsFailed,
        message: format!("invalid server name {sni:?}: {e}"),
        code: None,
    })?;
    let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
    match tokio::time::timeout(HANDSHAKE_TIMEOUT, connector.connect(name, io)).await {
        Ok(Ok(s)) => Ok(Box::new(s)),
        Ok(Err(e)) => Err(map_handshake_error(&e, &format!("{host}:{port}"))),
        Err(_) => Err(UpstreamError {
            kind: ErrorKind::Timeout,
            message: "upstream TLS handshake timed out".into(),
            code: None,
        }),
    }
}

/// `UnknownCA` → `unknown_ca`.
fn alert_name(alert: AlertDescription) -> String {
    let debug = format!("{alert:?}");
    let mut out = String::with_capacity(debug.len() + 4);
    let chars: Vec<char> = debug.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        if c.is_ascii_uppercase() {
            let prev_lower = i > 0 && chars[i - 1].is_ascii_lowercase();
            let next_lower = chars.get(i + 1).is_some_and(|n| n.is_ascii_lowercase());
            let prev_upper = i > 0 && chars[i - 1].is_ascii_uppercase();
            if i > 0 && (prev_lower || (prev_upper && next_lower)) {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(*c);
        }
    }
    out
}

fn rejection_hint(host: &str) -> String {
    format!(
        "The client rejected Akev's certificate for {host}. Make sure the Akev CA is installed and trusted by this client \
         (apps that pin certificates can't be intercepted: add the host to TLS passthrough)."
    )
}

/// Classify a client-side handshake failure.
fn classify(e: &std::io::Error, host: &str) -> ExchangeError {
    if let Some(re) = e.get_ref().and_then(|i| i.downcast_ref::<rustls::Error>()) {
        return match re {
            rustls::Error::AlertReceived(alert) => {
                let name = alert_name(*alert);
                let cert_problem = matches!(
                    alert,
                    AlertDescription::BadCertificate
                        | AlertDescription::UnsupportedCertificate
                        | AlertDescription::CertificateRevoked
                        | AlertDescription::CertificateExpired
                        | AlertDescription::CertificateUnknown
                        | AlertDescription::UnknownCA
                        | AlertDescription::AccessDenied
                        | AlertDescription::DecryptError
                );
                if cert_problem {
                    exchange_error(
                        ErrorKind::ClientTlsRejected,
                        format!("{} (alert: {name})", rejection_hint(host)),
                        Some(&name),
                    )
                } else {
                    exchange_error(
                        ErrorKind::ClientProtocolError,
                        format!("The client aborted the TLS handshake (alert: {name})"),
                        Some(&name),
                    )
                }
            }
            rustls::Error::PeerIncompatible(p) => exchange_error(
                ErrorKind::ClientProtocolError,
                format!("The client's TLS configuration is incompatible ({p:?})"),
                Some("no_shared_cipher"),
            ),
            rustls::Error::NoApplicationProtocol => exchange_error(
                ErrorKind::ClientProtocolError,
                "No common ALPN protocol (HTTP/2 is disabled and the client only offered h2?)",
                Some("no_application_protocol"),
            ),
            rustls::Error::General(m) => {
                exchange_error(ErrorKind::Internal, format!("TLS setup failed: {m}"), None)
            }
            other => exchange_error(
                ErrorKind::ClientProtocolError,
                format!("TLS handshake failed: {other}"),
                None,
            ),
        };
    }
    match e.kind() {
        std::io::ErrorKind::UnexpectedEof => exchange_error(
            ErrorKind::ClientTlsRejected,
            format!(
                "The client closed the connection during the TLS handshake. {}",
                rejection_hint(host)
            ),
            Some("closed"),
        ),
        std::io::ErrorKind::ConnectionReset => exchange_error(
            ErrorKind::ClientTlsRejected,
            format!(
                "The client reset the connection during the TLS handshake. {}",
                rejection_hint(host)
            ),
            Some("ECONNRESET"),
        ),
        _ => exchange_error(
            ErrorKind::ClientProtocolError,
            format!("TLS handshake failed: {e}"),
            None,
        ),
    }
}

/// Destination host/port to report for a TLS connection.
fn tls_destination(ctx: &ConnCtx, sni: Option<&str>) -> (String, u16) {
    match (&ctx.tunnel, sni) {
        // A tunnel to a bare IP reports the SNI name instead (keeping the tunnel's port).
        (Some(t), Some(s)) if t.is_ip() => (s.to_string(), t.port),
        (Some(t), _) => (t.host.clone(), t.port),
        (None, Some(s)) => (s.to_string(), 443),
        (None, None) => (ctx.local.ip().to_string(), ctx.local.port()),
    }
}

/// Emit `TlsFailed` (if recording TLS failures and the host isn't ignored).
async fn report_failure(
    shared: &Shared,
    ctx: &ConnCtx,
    info: TlsInfo,
    host: String,
    port: u16,
    error: ExchangeError,
) {
    let settings = shared.settings();
    if !settings.capture.record_tls_failures || settings.ignore.matches(&host, Some(port)) {
        return;
    }
    let (mut client, _) = ctx.client_info(shared, ctx.mode()).await;
    client.tls = Some(info);
    shared.sink.emit(ProxyEvent::TlsFailed {
        id: ExchangeId::next(),
        client,
        host,
        port,
        error,
        at: UnixMicros::now(),
    });
}

/// The client completed the handshake but closed without sending anything.
pub(crate) async fn silent_rejection(shared: &Shared, ctx: &ConnCtx) {
    let Some(info) = ctx.tls.clone() else { return };
    let (host, port) = tls_destination(ctx, info.sni.as_deref());
    let err = exchange_error(
        ErrorKind::ClientTlsRejected,
        format!(
            "The client closed the connection right after the TLS handshake without sending a request. {}",
            rejection_hint(&host)
        ),
        Some("closed"),
    );
    report_failure(shared, ctx, info, host, port, err).await;
}

fn tls_version_name(v: ProtocolVersion) -> String {
    match v {
        ProtocolVersion::TLSv1_3 => "TLSv1.3".into(),
        ProtocolVersion::TLSv1_2 => "TLSv1.2".into(),
        other => format!("{other:?}"),
    }
}

/// Whether rustls will see this SNI value (a valid DNS name, not an IP literal).
fn usable_sni(sni: &str) -> bool {
    sni.parse::<std::net::IpAddr>().is_err() && rustls_pki_types::DnsName::try_from(sni).is_ok()
}

/// Why a ClientHello can't be intercepted (passthrough is the only option).
fn not_interceptable(hello: &ClientHello) -> Option<String> {
    if hello.max_version() < 0x0303 {
        return Some("client only supports TLS versions below 1.2".into());
    }
    if !hello.offers_http() {
        return Some(format!(
            "client offered non-HTTP ALPN protocols ({})",
            hello.alpn.join(", ")
        ));
    }
    None
}

/// Silent-rejection observation window, mockttp's heuristic: 10× the handshake duration,
/// at least 100ms.
fn rejection_window(handshake: Duration) -> Duration {
    (handshake * 10)
        .max(Duration::from_millis(100))
        .min(Duration::from_secs(5))
}

/// Handle a TLS connection. Returns the decrypted stream when intercepted; `None` when the
/// connection was tunnelled, failed, or closed.
pub(crate) async fn handle(
    shared: &Arc<Shared>,
    mut io: ClientIo,
    mut buf: BytesMut,
    ctx: &mut ConnCtx,
) -> Option<ClientIo> {
    let started = Instant::now();
    let hello = loop {
        match client_hello::parse(&buf) {
            HelloParse::Complete(h) => break Ok(h),
            HelloParse::Invalid(why) => break Err(why),
            HelloParse::NeedMore => match read_more_timeout(&mut io, &mut buf, HELLO_TIMEOUT).await
            {
                Ok(n) if n > 0 => {}
                _ => return None,
            },
        }
    };
    let hello = match hello {
        Ok(h) => h,
        Err(why) => {
            if let Some(t) = ctx.tunnel.clone() {
                let reason =
                    TunnelReason::NotInterceptable(format!("unparseable ClientHello: {why}"));
                crate::tunnel::run(
                    shared.clone(),
                    io,
                    buf.freeze(),
                    t.host,
                    t.port,
                    ctx,
                    reason,
                    None,
                )
                .await;
            } else {
                let err = exchange_error(
                    ErrorKind::ClientProtocolError,
                    format!("invalid TLS ClientHello: {why}"),
                    Some("INVALID_CLIENT_HELLO"),
                );
                crate::conn::client_error(shared, ctx, err, &buf).await;
            }
            return None;
        }
    };
    let settings = shared.settings();
    let sni = hello.sni.clone();
    let mut info = TlsInfo {
        sni: sni.clone(),
        alpn_offered: hello.alpn.clone(),
        ja3: Some(hello.ja3()),
        ja4: Some(hello.ja4()),
        ..Default::default()
    };
    let (host, port) = tls_destination(ctx, sni.as_deref());

    // Passthrough: keep the tunnel's port (never assume 443 when we know better).
    let pass_match = sni
        .as_deref()
        .is_some_and(|s| settings.passthrough.matches(s, Some(port)))
        || ctx
            .tunnel
            .as_ref()
            .is_some_and(|t| settings.passthrough.matches(&t.host, Some(t.port)));
    let reason = if pass_match {
        Some(TunnelReason::TlsPassthrough)
    } else {
        not_interceptable(&hello).map(TunnelReason::NotInterceptable)
    };
    if let Some(reason) = reason {
        let (dest_host, dest_port) = match &ctx.tunnel {
            Some(t) => (t.host.clone(), t.port),
            None => match &sni {
                Some(s) => (s.clone(), 443),
                None => {
                    let err = exchange_error(
                        ErrorKind::ClientProtocolError,
                        "TLS passthrough needs a destination (no SNI, no tunnel)",
                        None,
                    );
                    crate::conn::client_error(shared, ctx, err, &buf).await;
                    return None;
                }
            },
        };
        crate::tunnel::run(
            shared.clone(),
            io,
            buf.freeze(),
            dest_host,
            dest_port,
            ctx,
            reason,
            None,
        )
        .await;
        return None;
    }

    // Intercept.
    let use_h2 = settings.proxy.http2 && hello.alpn.iter().any(|p| p == "h2");
    // rustls ignores SNI values that aren't valid DNS names (IP literals, garbage), so those
    // connections get a fixed certificate like clients without SNI.
    let sni_dns = sni.as_deref().filter(|s| usable_sni(s));
    let config = match sni_dns {
        Some(_) => Ok(if use_h2 {
            shared.tls.with_h2.clone()
        } else {
            shared.tls.http1_only.clone()
        }),
        None => {
            let sni_ip = sni
                .as_deref()
                .filter(|s| s.parse::<std::net::IpAddr>().is_ok());
            let cert_host = sni_ip
                .map(str::to_string)
                .or_else(|| ctx.tunnel.as_ref().map(|t| t.host.clone()))
                .unwrap_or_else(|| ctx.local.ip().to_string());
            match shared.ca.leaf_for(&cert_host) {
                Ok(key) => shared.tls.fixed(key, use_h2).map_err(|e| e.to_string()),
                Err(e) => Err(e.to_string()),
            }
        }
    };
    let config = match config {
        Ok(c) => c,
        Err(e) => {
            let err = exchange_error(
                ErrorKind::Internal,
                format!("can't create a certificate for {host}: {e}"),
                None,
            );
            report_failure(shared, ctx, info, host, port, err).await;
            return None;
        }
    };
    let acceptor = tokio_rustls::TlsAcceptor::from(config);
    let accept = acceptor.accept(Rewind::new(buf.freeze(), io));
    let stream = tokio::select! {
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, accept) => r,
        _ = shared.hard_stop.cancelled() => return None,
    };
    match stream {
        Ok(Ok(stream)) => {
            let handshake = started.elapsed();
            {
                let (_, conn) = stream.get_ref();
                info.version = conn.protocol_version().map(tls_version_name);
                info.cipher = conn
                    .negotiated_cipher_suite()
                    .map(|s| format!("{:?}", s.suite()));
                info.alpn = conn
                    .alpn_protocol()
                    .map(|p| String::from_utf8_lossy(p).into_owned());
            }
            ctx.tls = Some(info);
            ctx.tls_check = Some((Instant::now(), rejection_window(handshake)));
            Some(Box::new(stream))
        }
        Ok(Err(e)) => {
            let err = classify(&e, &host);
            report_failure(shared, ctx, info, host, port, err).await;
            None
        }
        Err(_) => {
            let err = exchange_error(
                ErrorKind::Timeout,
                "The client didn't complete the TLS handshake in time",
                Some("handshake-timeout"),
            );
            report_failure(shared, ctx, info, host, port, err).await;
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_names_are_snake_case() {
        assert_eq!(alert_name(AlertDescription::UnknownCA), "unknown_ca");
        assert_eq!(
            alert_name(AlertDescription::BadCertificate),
            "bad_certificate"
        );
        assert_eq!(
            alert_name(AlertDescription::CertificateUnknown),
            "certificate_unknown"
        );
        assert_eq!(
            alert_name(AlertDescription::UnknownPSKIdentity),
            "unknown_psk_identity"
        );
    }

    #[test]
    fn classifies_handshake_errors() {
        let alert = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(AlertDescription::UnknownCA),
        );
        let e = classify(&alert, "example.com");
        assert_eq!(e.kind, ErrorKind::ClientTlsRejected);
        assert_eq!(e.code.as_deref(), Some("unknown_ca"));
        assert!(e.message.contains("example.com"));
        let alert = std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            rustls::Error::AlertReceived(AlertDescription::ProtocolVersion),
        );
        assert_eq!(classify(&alert, "h").kind, ErrorKind::ClientProtocolError);
        let eof = std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof");
        assert_eq!(classify(&eof, "h").code.as_deref(), Some("closed"));
        let reset = std::io::Error::new(std::io::ErrorKind::ConnectionReset, "rst");
        assert_eq!(classify(&reset, "h").kind, ErrorKind::ClientTlsRejected);
    }

    #[test]
    fn sni_usability() {
        assert!(usable_sni("example.com"));
        assert!(
            usable_sni("my_host.example.com")
                == rustls_pki_types::DnsName::try_from("my_host.example.com").is_ok()
        );
        assert!(!usable_sni("127.0.0.1"));
        assert!(!usable_sni("::1"));
        assert!(!usable_sni("bad name"));
    }

    #[test]
    fn rejection_window_bounds() {
        assert_eq!(
            rejection_window(Duration::from_millis(1)),
            Duration::from_millis(100)
        );
        assert_eq!(
            rejection_window(Duration::from_millis(50)),
            Duration::from_millis(500)
        );
        assert_eq!(
            rejection_window(Duration::from_secs(10)),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn interceptability() {
        use crate::client_hello::tests::{alpn_ext, build_hello, sni_ext};
        let parse = |raw: Vec<u8>| match client_hello::parse(&raw) {
            HelloParse::Complete(h) => h,
            other => panic!("{other:?}"),
        };
        let h = parse(build_hello(
            0x0303,
            &[0x1301],
            &[sni_ext("a.test"), alpn_ext(&["imap"])],
            None,
        ));
        assert!(not_interceptable(&h).is_some());
        let h = parse(build_hello(0x0301, &[0x002f], &[sni_ext("a.test")], None));
        assert!(not_interceptable(&h).is_some());
        let h = parse(build_hello(
            0x0303,
            &[0x1301],
            &[sni_ext("a.test"), alpn_ext(&["http/1.1"])],
            None,
        ));
        assert!(not_interceptable(&h).is_none());
    }
}
