//! HTTPS for an API reachable from the network (`api_bind` not loopback).
//!
//! Other devices reach the UI and API over TLS with a certificate issued by Akev's own CA
//! (which they trust to be intercepted anyway) for the bind address, `localhost`, the
//! machine's hostname and its LAN addresses. Plain HTTP keeps working for clients on this
//! machine (loopback, or a connection to one of its own addresses), so the CLI, the MCP
//! bridge, the terminal one-liners and the desktop shell are unaffected; a network peer that
//! speaks plain HTTP gets a short answer telling it to use `https://`.
//!
//! The listener sniffs the first byte (a TLS record starts with `0x16`), runs handshakes off
//! the accept loop (bounded, with a timeout) and hands finished connections to `axum::serve`
//! through [`ApiListener`]. The certificate is re-issued when the CA is regenerated.

use akev_ca::{CertificateAuthority, CertifiedKey};
use arc_swap::ArcSwap;
use rustls::server::{ClientHello, ResolvesServerCert};
use std::io;
use std::net::{IpAddr, SocketAddr};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};

/// Limit for sniffing the first byte and for the TLS handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
/// Handshakes in progress at once (slow clients can't exhaust the process).
const MAX_HANDSHAKES: usize = 256;

/// The API's server certificate (swapped when the CA is regenerated).
#[derive(Debug)]
pub(crate) struct ApiTls {
    names: Vec<String>,
    cert: Arc<ArcSwap<CertifiedKey>>,
    config: Arc<rustls::ServerConfig>,
}

/// Hands out the current certificate whatever the client asked for.
#[derive(Debug)]
struct Resolver(Arc<ArcSwap<CertifiedKey>>);

impl ResolvesServerCert for Resolver {
    fn resolve(&self, _hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.0.load_full())
    }
}

impl ApiTls {
    /// A certificate from `ca` for `names` (see [`certificate_names`]).
    pub(crate) fn new(ca: &CertificateAuthority, names: Vec<String>) -> anyhow::Result<Arc<Self>> {
        let cert = Arc::new(ArcSwap::new(ca.server_certificate(&names)?));
        let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(Resolver(cert.clone())));
        // axum serves HTTP/1.1 (WebSockets included).
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Arc::new(ApiTls {
            names,
            cert,
            config: Arc::new(config),
        }))
    }

    /// Re-issue the certificate from a new CA (after `POST /api/ca/regenerate`).
    pub(crate) fn reissue(&self, ca: &CertificateAuthority) -> anyhow::Result<()> {
        self.cert.store(ca.server_certificate(&self.names)?);
        Ok(())
    }
}

/// Names for the API certificate of a server bound to `bind`: the bind address (unless it is
/// the unspecified address), `localhost`, the loopback addresses, the hostname (and
/// `<hostname>.local`) and the LAN addresses.
pub(crate) fn certificate_names(bind: IpAddr, lan: &[std::net::Ipv4Addr]) -> Vec<String> {
    let mut names = Vec::new();
    let mut add = |n: String| {
        if !n.is_empty() && !names.contains(&n) {
            names.push(n);
        }
    };
    if !bind.is_unspecified() {
        add(bind.to_string());
    }
    for ip in lan {
        add(ip.to_string());
    }
    if let Ok(host) = sysinfo::System::host_name() {
        let host = host.trim().trim_end_matches('.').to_ascii_lowercase();
        if !host.is_empty() {
            if !host.contains('.') {
                add(format!("{host}.local"));
            }
            add(host);
        }
    }
    add("localhost".into());
    add("127.0.0.1".into());
    add("::1".into());
    names
}

/// A connection accepted by [`ApiListener`].
pub(crate) enum ApiIo {
    Plain(TcpStream),
    Tls(Box<tokio_rustls::server::TlsStream<TcpStream>>),
}

impl AsyncRead for ApiIo {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ApiIo::Plain(s) => Pin::new(s).poll_read(cx, buf),
            ApiIo::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for ApiIo {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match self.get_mut() {
            ApiIo::Plain(s) => Pin::new(s).poll_write(cx, buf),
            ApiIo::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ApiIo::Plain(s) => Pin::new(s).poll_flush(cx),
            ApiIo::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match self.get_mut() {
            ApiIo::Plain(s) => Pin::new(s).poll_shutdown(cx),
            ApiIo::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

/// Accepted connections, TLS handshakes already done, for `axum::serve`.
pub(crate) struct ApiListener {
    rx: mpsc::Receiver<(ApiIo, SocketAddr)>,
    local: SocketAddr,
}

impl axum::serve::Listener for ApiListener {
    type Io = ApiIo;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(conn) => conn,
            // The accept task ended (it never does while the listener lives).
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// Whether `ip` is this machine (loopback or one of its LAN addresses).
pub(crate) fn is_this_host(ip: IpAddr) -> bool {
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    ip.is_loopback()
        || matches!(ip, IpAddr::V4(v4) if crate::net::lan_ipv4_addresses().contains(&v4))
}

/// Whether a peer of a connection to `local` is this machine itself: plain HTTP is fine for
/// it (nothing crosses the network).
fn same_host(peer: IpAddr, local: IpAddr) -> bool {
    let canonical = |ip: IpAddr| match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    let (peer, local) = (canonical(peer), canonical(local));
    peer.is_loopback() || peer == local
}

/// Accept on `listener`: TLS for everyone, plain HTTP for this machine only.
pub(crate) fn listen(listener: TcpListener, tls: Arc<ApiTls>) -> io::Result<ApiListener> {
    let local = listener.local_addr()?;
    let (tx, rx) = mpsc::channel(64);
    let acceptor = tokio_rustls::TlsAcceptor::from(tls.config.clone());
    let permits = Arc::new(Semaphore::new(MAX_HANDSHAKES));
    tokio::spawn(async move {
        loop {
            let accepted = tokio::select! {
                r = listener.accept() => r,
                _ = tx.closed() => break,
            };
            let (stream, peer) = match accepted {
                Ok(conn) => conn,
                Err(e) => {
                    tracing::debug!("API accept failed: {e}");
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    continue;
                }
            };
            let _ = stream.set_nodelay(true);
            let Ok(permit) = permits.clone().try_acquire_owned() else {
                // Too many handshakes in progress: shed load.
                continue;
            };
            let (tx, acceptor) = (tx.clone(), acceptor.clone());
            tokio::spawn(async move {
                let _permit = permit;
                let local_ip = stream.local_addr().map(|a| a.ip()).unwrap_or(local.ip());
                let mut first = [0u8; 1];
                let peeked = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.peek(&mut first)).await;
                let io = match peeked {
                    Ok(Ok(1)) if first[0] == 0x16 => {
                        match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await
                        {
                            Ok(Ok(tls)) => ApiIo::Tls(Box::new(tls)),
                            Ok(Err(e)) => {
                                tracing::debug!(%peer, "API TLS handshake failed: {e}");
                                return;
                            }
                            Err(_) => return,
                        }
                    }
                    Ok(Ok(1)) if same_host(peer.ip(), local_ip) => ApiIo::Plain(stream),
                    Ok(Ok(1)) => {
                        refuse_plain_http(stream, local).await;
                        return;
                    }
                    _ => return,
                };
                let _ = tx.send((io, peer)).await;
            });
        }
    });
    Ok(ApiListener { rx, local })
}

/// Answer a plain-HTTP request from another machine with a pointer to `https://`.
async fn refuse_plain_http(mut stream: TcpStream, local: SocketAddr) {
    let body = format!(
        "This Akev instance is reachable from the network only over HTTPS. Open https://<this computer's address>:{}/ instead (the certificate is issued by the Akev CA).\n",
        local.port()
    );
    let response = format!(
        "HTTP/1.1 400 Bad Request\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(HANDSHAKE_TIMEOUT, stream.write_all(response.as_bytes())).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn names_cover_bind_lan_hostname_and_loopback() {
        let lan = [Ipv4Addr::new(192, 168, 1, 23)];
        let names = certificate_names("10.0.0.5".parse().unwrap(), &lan);
        assert_eq!(names[0], "10.0.0.5");
        assert!(names.contains(&"192.168.1.23".to_string()));
        for n in ["localhost", "127.0.0.1", "::1"] {
            assert!(names.contains(&n.to_string()), "{n}");
        }
        let any = certificate_names("0.0.0.0".parse().unwrap(), &lan);
        assert!(!any.contains(&"0.0.0.0".to_string()));
        assert_eq!(any[0], "192.168.1.23");
    }

    #[test]
    fn same_host_peers() {
        let lan: IpAddr = "192.168.1.23".parse().unwrap();
        assert!(same_host("127.0.0.1".parse().unwrap(), lan));
        assert!(same_host("::1".parse().unwrap(), lan));
        assert!(same_host(lan, lan));
        assert!(same_host("::ffff:192.168.1.23".parse().unwrap(), lan));
        assert!(!same_host("192.168.1.50".parse().unwrap(), lan));
    }
}
