//! Tunnel handshakes through upstream proxies: HTTP `CONNECT` and SOCKS4/4a/5 (RFC 1928,
//! username/password per RFC 1929).

use super::{ProxyKind, ProxyServer, host_port};
use crate::upstream::UpstreamError;
use crate::upstream::error::{error, from_io_as};
use crate::upstream::h1::{HeadError, read_head};
use akev_core::ErrorKind;
use bytes::{Bytes, BytesMut};
use std::net::IpAddr;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

fn proxy_error(proxy: &ProxyServer, message: impl std::fmt::Display, code: &str) -> UpstreamError {
    error(
        ErrorKind::UpstreamProxyFailed,
        format!("upstream proxy {}: {message}", proxy.display),
        code,
    )
}

fn proxy_io_error(proxy: &ProxyServer, e: &std::io::Error) -> UpstreamError {
    from_io_as(
        ErrorKind::UpstreamProxyFailed,
        e,
        format_args!("upstream proxy {}", proxy.display),
    )
}

/// Open a `CONNECT` tunnel to `host:port`. Returns bytes the proxy sent after its response
/// head (to be replayed before reading from the tunnel).
pub(crate) async fn http_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    io: &mut S,
    proxy: &ProxyServer,
    host: &str,
    port: u16,
) -> Result<Bytes, UpstreamError> {
    let authority = host_port(host, port);
    let mut req = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = proxy.basic_auth() {
        req.push_str("Proxy-Authorization: ");
        req.push_str(&auth);
        req.push_str("\r\n");
    }
    req.push_str("\r\n");
    io.write_all(req.as_bytes())
        .await
        .map_err(|e| proxy_io_error(proxy, &e))?;
    io.flush().await.map_err(|e| proxy_io_error(proxy, &e))?;

    let mut buf = BytesMut::with_capacity(1024);
    let head = loop {
        let head = read_head(io, &mut buf).await.map_err(|e| match e {
            HeadError::ClosedBeforeResponse => proxy_error(
                proxy,
                format_args!("closed the connection while opening a tunnel to {authority}"),
                "PROXY_CONNECT_FAILED",
            ),
            HeadError::Io(e) => proxy_io_error(proxy, &e),
            HeadError::Protocol(msg) => proxy_error(
                proxy,
                format_args!("invalid CONNECT response: {msg}"),
                "PROXY_PROTOCOL_ERROR",
            ),
        })?;
        if !(100..200).contains(&head.status) {
            break head;
        }
    };
    match head.status {
        200..=299 => Ok(buf.freeze()),
        407 => Err(proxy_error(
            proxy,
            if proxy.username.is_some() {
                "rejected the proxy credentials (407 Proxy Authentication Required)".to_owned()
            } else {
                "requires authentication (407 Proxy Authentication Required)".to_owned()
            },
            if proxy.username.is_some() {
                "PROXY_AUTH_FAILED"
            } else {
                "PROXY_AUTH_REQUIRED"
            },
        )),
        status => Err(proxy_error(
            proxy,
            format_args!(
                "refused to open a tunnel to {authority}: {status} {}",
                head.reason
            ),
            "PROXY_CONNECT_FAILED",
        )),
    }
}

/// Destination address for SOCKS.
#[derive(Clone, Copy, Debug)]
pub(crate) enum SocksAddr<'a> {
    Ip(IpAddr),
    Domain(&'a str),
}

/// SOCKS handshake (version chosen from the proxy kind).
pub(crate) async fn socks_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    io: &mut S,
    proxy: &ProxyServer,
    addr: SocksAddr<'_>,
    port: u16,
) -> Result<(), UpstreamError> {
    match proxy.kind {
        ProxyKind::Socks4 | ProxyKind::Socks4a => socks4_connect(io, proxy, addr, port).await,
        _ => socks5_connect(io, proxy, addr, port).await,
    }
}

async fn socks5_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    io: &mut S,
    proxy: &ProxyServer,
    addr: SocksAddr<'_>,
    port: u16,
) -> Result<(), UpstreamError> {
    let io_err = |e: std::io::Error| proxy_io_error(proxy, &e);
    let has_auth = proxy.username.is_some();
    let greeting: &[u8] = if has_auth {
        &[5, 2, 0x00, 0x02]
    } else {
        &[5, 1, 0x00]
    };
    io.write_all(greeting).await.map_err(io_err)?;
    let mut reply = [0u8; 2];
    io.read_exact(&mut reply).await.map_err(io_err)?;
    if reply[0] != 5 {
        return Err(proxy_error(
            proxy,
            "is not a SOCKS5 server",
            "SOCKS_PROTOCOL_ERROR",
        ));
    }
    match reply[1] {
        0x00 => {}
        0x02 if has_auth => {
            let user = proxy.username.as_deref().unwrap_or("").as_bytes();
            let pass = proxy.password.as_deref().unwrap_or("").as_bytes();
            if user.len() > 255 || pass.len() > 255 {
                return Err(proxy_error(
                    proxy,
                    "SOCKS5 credentials are too long",
                    "SOCKS_AUTH_FAILED",
                ));
            }
            let mut msg = Vec::with_capacity(3 + user.len() + pass.len());
            msg.push(1);
            msg.push(user.len() as u8);
            msg.extend_from_slice(user);
            msg.push(pass.len() as u8);
            msg.extend_from_slice(pass);
            io.write_all(&msg).await.map_err(io_err)?;
            let mut status = [0u8; 2];
            io.read_exact(&mut status).await.map_err(io_err)?;
            if status[1] != 0 {
                return Err(proxy_error(
                    proxy,
                    "rejected the SOCKS5 credentials",
                    "SOCKS_AUTH_FAILED",
                ));
            }
        }
        0x02 | 0xFF => {
            return Err(proxy_error(
                proxy,
                "requires authentication (no acceptable SOCKS5 auth method)",
                "SOCKS_AUTH_REQUIRED",
            ));
        }
        other => {
            return Err(proxy_error(
                proxy,
                format_args!("selected unsupported SOCKS5 auth method {other:#x}"),
                "SOCKS_PROTOCOL_ERROR",
            ));
        }
    }

    let mut req = Vec::with_capacity(22);
    req.extend_from_slice(&[5, 1, 0]);
    match addr {
        SocksAddr::Ip(IpAddr::V4(v4)) => {
            req.push(1);
            req.extend_from_slice(&v4.octets());
        }
        SocksAddr::Ip(IpAddr::V6(v6)) => {
            req.push(4);
            req.extend_from_slice(&v6.octets());
        }
        SocksAddr::Domain(d) => {
            if d.len() > 255 {
                return Err(proxy_error(
                    proxy,
                    "hostname too long for SOCKS5",
                    "SOCKS_PROTOCOL_ERROR",
                ));
            }
            req.push(3);
            req.push(d.len() as u8);
            req.extend_from_slice(d.as_bytes());
        }
    }
    req.extend_from_slice(&port.to_be_bytes());
    io.write_all(&req).await.map_err(io_err)?;

    let mut head = [0u8; 4];
    io.read_exact(&mut head).await.map_err(io_err)?;
    if head[0] != 5 {
        return Err(proxy_error(
            proxy,
            "sent an invalid SOCKS5 reply",
            "SOCKS_PROTOCOL_ERROR",
        ));
    }
    let target = match addr {
        SocksAddr::Ip(ip) => host_port(&ip.to_string(), port),
        SocksAddr::Domain(d) => host_port(d, port),
    };
    if head[1] != 0 {
        let (kind, msg, code) = match head[1] {
            1 => (
                ErrorKind::UpstreamProxyFailed,
                "general SOCKS server failure",
                "SOCKS_GENERAL_FAILURE",
            ),
            2 => (
                ErrorKind::UpstreamProxyFailed,
                "connection not allowed by ruleset",
                "SOCKS_NOT_ALLOWED",
            ),
            3 => (
                ErrorKind::HostUnreachable,
                "network unreachable",
                "ENETUNREACH",
            ),
            4 => (
                ErrorKind::HostUnreachable,
                "host unreachable",
                "EHOSTUNREACH",
            ),
            5 => (
                ErrorKind::ConnectionRefused,
                "connection refused",
                "ECONNREFUSED",
            ),
            6 => (ErrorKind::Timeout, "TTL expired", "ETIMEDOUT"),
            7 => (
                ErrorKind::UpstreamProxyFailed,
                "command not supported",
                "SOCKS_COMMAND_NOT_SUPPORTED",
            ),
            8 => (
                ErrorKind::UpstreamProxyFailed,
                "address type not supported",
                "SOCKS_ADDRESS_NOT_SUPPORTED",
            ),
            _ => (
                ErrorKind::UpstreamProxyFailed,
                "unknown SOCKS error",
                "SOCKS_ERROR",
            ),
        };
        return Err(error(
            kind,
            format!("connecting to {target} via {}: {msg}", proxy.display),
            code,
        ));
    }
    // Skip the bound address.
    let skip = match head[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            io.read_exact(&mut len).await.map_err(io_err)?;
            usize::from(len[0]) + 2
        }
        _ => {
            return Err(proxy_error(
                proxy,
                "sent an invalid SOCKS5 address type",
                "SOCKS_PROTOCOL_ERROR",
            ));
        }
    };
    let mut rest = [0u8; 258];
    io.read_exact(&mut rest[..skip]).await.map_err(io_err)?;
    Ok(())
}

async fn socks4_connect<S: AsyncRead + AsyncWrite + Unpin + ?Sized>(
    io: &mut S,
    proxy: &ProxyServer,
    addr: SocksAddr<'_>,
    port: u16,
) -> Result<(), UpstreamError> {
    let io_err = |e: std::io::Error| proxy_io_error(proxy, &e);
    let mut req = Vec::with_capacity(32);
    req.extend_from_slice(&[4, 1]);
    req.extend_from_slice(&port.to_be_bytes());
    let domain = match addr {
        SocksAddr::Ip(IpAddr::V4(v4)) => {
            req.extend_from_slice(&v4.octets());
            None
        }
        SocksAddr::Ip(IpAddr::V6(_)) => {
            return Err(proxy_error(
                proxy,
                "SOCKS4 cannot connect to IPv6 addresses",
                "SOCKS_ADDRESS_NOT_SUPPORTED",
            ));
        }
        SocksAddr::Domain(d) => {
            // SOCKS4a: invalid IP 0.0.0.x + hostname after the user id.
            req.extend_from_slice(&[0, 0, 0, 1]);
            Some(d)
        }
    };
    req.extend_from_slice(proxy.username.as_deref().unwrap_or("").as_bytes());
    req.push(0);
    if let Some(d) = domain {
        req.extend_from_slice(d.as_bytes());
        req.push(0);
    }
    io.write_all(&req).await.map_err(io_err)?;
    let mut reply = [0u8; 8];
    io.read_exact(&mut reply).await.map_err(io_err)?;
    match reply[1] {
        0x5A => Ok(()),
        0x5B => Err(proxy_error(
            proxy,
            "SOCKS4 request rejected or failed",
            "SOCKS_REJECTED",
        )),
        0x5C | 0x5D => Err(proxy_error(
            proxy,
            "SOCKS4 identd authentication failed",
            "SOCKS_AUTH_FAILED",
        )),
        _ => Err(proxy_error(
            proxy,
            "sent an invalid SOCKS4 reply",
            "SOCKS_PROTOCOL_ERROR",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    #[tokio::test]
    async fn connect_success_keeps_leftover() {
        let proxy = ProxyServer::parse("http://u:p@proxy.test:3128").unwrap();
        let (mut client, mut server) = duplex(4096);
        let srv = tokio::spawn(async move {
            let mut buf = vec![0u8; 1024];
            let n = server.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            server
                .write_all(b"HTTP/1.1 200 Connection established\r\n\r\nEARLY")
                .await
                .unwrap();
            req
        });
        let leftover = http_connect(&mut client, &proxy, "::1", 443).await.unwrap();
        assert_eq!(&leftover[..], b"EARLY");
        let req = srv.await.unwrap();
        assert!(req.starts_with("CONNECT [::1]:443 HTTP/1.1\r\nHost: [::1]:443\r\n"));
        assert!(req.contains("Proxy-Authorization: Basic dTpw\r\n"));
    }

    #[tokio::test]
    async fn connect_failures_are_proxy_errors() {
        let proxy = ProxyServer::parse("http://proxy.test:3128").unwrap();
        for (response, code) in [
            (
                &b"HTTP/1.1 407 Proxy Authentication Required\r\n\r\n"[..],
                "PROXY_AUTH_REQUIRED",
            ),
            (
                &b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n"[..],
                "PROXY_CONNECT_FAILED",
            ),
            (&b""[..], "PROXY_CONNECT_FAILED"),
        ] {
            let (mut client, mut server) = duplex(4096);
            let response = response.to_vec();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 1024];
                let _ = server.read(&mut buf).await;
                let _ = server.write_all(&response).await;
            });
            let err = http_connect(&mut client, &proxy, "example.com", 443)
                .await
                .unwrap_err();
            assert_eq!(err.kind, ErrorKind::UpstreamProxyFailed);
            assert_eq!(err.code.as_deref(), Some(code));
        }
    }

    #[tokio::test]
    async fn socks5_with_auth_and_domain() {
        let proxy = ProxyServer::parse("socks5h://alice:secret@socks.test:1080").unwrap();
        let (mut client, mut server) = duplex(4096);
        let srv = tokio::spawn(async move {
            let mut g = [0u8; 4];
            server.read_exact(&mut g).await.unwrap();
            assert_eq!(g, [5, 2, 0, 2]);
            server.write_all(&[5, 2]).await.unwrap();
            let mut auth = [0u8; 1 + 1 + 5 + 1 + 6];
            server.read_exact(&mut auth).await.unwrap();
            assert_eq!(&auth[2..7], b"alice");
            assert_eq!(&auth[8..], b"secret");
            server.write_all(&[1, 0]).await.unwrap();
            let mut req = [0u8; 5];
            server.read_exact(&mut req).await.unwrap();
            assert_eq!(&req[..4], &[5, 1, 0, 3]);
            let mut name = vec![0u8; req[4] as usize + 2];
            server.read_exact(&mut name).await.unwrap();
            assert_eq!(&name[..name.len() - 2], b"example.com");
            assert_eq!(&name[name.len() - 2..], &443u16.to_be_bytes());
            server
                .write_all(&[5, 0, 0, 1, 10, 0, 0, 1, 0, 80])
                .await
                .unwrap();
        });
        socks_connect(&mut client, &proxy, SocksAddr::Domain("example.com"), 443)
            .await
            .unwrap();
        srv.await.unwrap();
    }

    #[tokio::test]
    async fn socks5_refusal_maps_to_connection_refused() {
        let proxy = ProxyServer::parse("socks5://socks.test").unwrap();
        let (mut client, mut server) = duplex(4096);
        tokio::spawn(async move {
            let mut g = [0u8; 3];
            server.read_exact(&mut g).await.unwrap();
            server.write_all(&[5, 0]).await.unwrap();
            let mut req = [0u8; 10];
            server.read_exact(&mut req).await.unwrap();
            server
                .write_all(&[5, 5, 0, 1, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
        });
        let err = socks_connect(
            &mut client,
            &proxy,
            SocksAddr::Ip("10.0.0.1".parse().unwrap()),
            80,
        )
        .await
        .unwrap_err();
        assert_eq!(err.kind, ErrorKind::ConnectionRefused);
    }

    #[tokio::test]
    async fn socks5_requires_auth() {
        let proxy = ProxyServer::parse("socks5://socks.test").unwrap();
        let (mut client, mut server) = duplex(4096);
        tokio::spawn(async move {
            let mut g = [0u8; 3];
            server.read_exact(&mut g).await.unwrap();
            server.write_all(&[5, 0xFF]).await.unwrap();
        });
        let err = socks_connect(&mut client, &proxy, SocksAddr::Domain("x.test"), 80)
            .await
            .unwrap_err();
        assert_eq!(err.code.as_deref(), Some("SOCKS_AUTH_REQUIRED"));
    }

    #[tokio::test]
    async fn socks4a_domain() {
        let proxy = ProxyServer::parse("socks4a://socks.test").unwrap();
        let (mut client, mut server) = duplex(4096);
        let srv = tokio::spawn(async move {
            let mut buf = vec![0u8; 64];
            let n = server.read(&mut buf).await.unwrap();
            server
                .write_all(&[0, 0x5A, 0, 0, 0, 0, 0, 0])
                .await
                .unwrap();
            buf.truncate(n);
            buf
        });
        socks_connect(&mut client, &proxy, SocksAddr::Domain("example.com"), 8080)
            .await
            .unwrap();
        let req = srv.await.unwrap();
        assert_eq!(&req[..8], &[4, 1, 0x1F, 0x90, 0, 0, 0, 1]);
        assert_eq!(&req[8..], b"\0example.com\0");
    }
}
