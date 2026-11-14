//! SOCKS4 / SOCKS4a / SOCKS5 server handshakes (CONNECT only).
//!
//! Supported SOCKS5 methods: no-auth (0x00) and username/password (0x02, RFC 1929). Unless the
//! client must authenticate (proxy credentials set in `proxy.access`, client on another
//! machine), credentials are not verified. SOCKS4 has no password, so it is refused when
//! credentials are required.

use crate::io::fill_to;
use crate::target::HostPort;
use akev_core::access::ClientAccess;
use bytes::{Buf, BytesMut};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);

/// A successful SOCKS handshake.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SocksRequest {
    pub version: u8,
    pub target: HostPort,
}

#[derive(Debug, thiserror::Error)]
pub enum SocksError {
    #[error("io error during SOCKS handshake: {0}")]
    Io(#[from] std::io::Error),
    #[error("SOCKS protocol error: {0}")]
    Protocol(&'static str),
    /// Proxy credentials were required and missing or wrong.
    #[error("SOCKS authentication failed")]
    AuthFailed,
}

/// Run the server side of a SOCKS handshake. `buf` holds bytes already read (starting with the
/// version byte); on success it holds whatever the client sent after the handshake. With
/// `auth`, the client must present the proxy credentials (SOCKS5 username/password).
pub async fn handshake<IO>(
    io: &mut IO,
    buf: &mut BytesMut,
    auth: Option<&ClientAccess>,
) -> Result<SocksRequest, SocksError>
where
    IO: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    fill_to(io, buf, 1, HANDSHAKE_TIMEOUT).await?;
    match buf[0] {
        4 if auth.is_some() => {
            // SOCKS4 can't carry a password: "request rejected".
            io.write_all(&[0, 0x5B, 0, 0, 0, 0, 0, 0]).await?;
            Err(SocksError::AuthFailed)
        }
        4 => socks4(io, buf).await,
        5 => socks5(io, buf, auth).await,
        _ => Err(SocksError::Protocol("unknown SOCKS version")),
    }
}

/// Find a NUL-terminated string starting at `from`, reading more as needed (max 255 bytes).
async fn read_cstr<IO>(
    io: &mut IO,
    buf: &mut BytesMut,
    from: usize,
) -> Result<(String, usize), SocksError>
where
    IO: AsyncRead + Unpin + ?Sized,
{
    loop {
        if let Some(i) = buf[from..].iter().position(|&b| b == 0) {
            let s = String::from_utf8_lossy(&buf[from..from + i]).into_owned();
            return Ok((s, from + i + 1));
        }
        if buf.len() - from > 255 {
            return Err(SocksError::Protocol("SOCKS4 string too long"));
        }
        let want = buf.len() + 1;
        fill_to(io, buf, want, HANDSHAKE_TIMEOUT).await?;
    }
}

async fn socks4<IO>(io: &mut IO, buf: &mut BytesMut) -> Result<SocksRequest, SocksError>
where
    IO: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    // VER CMD PORT(2) IP(4) USERID\0 [HOST\0]
    fill_to(io, buf, 8, HANDSHAKE_TIMEOUT).await?;
    let cmd = buf[1];
    let port = u16::from_be_bytes([buf[2], buf[3]]);
    let ip = Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]);
    let (_userid, mut end) = read_cstr(io, buf, 8).await?;
    let octets = ip.octets();
    let host = if octets[0] == 0 && octets[1] == 0 && octets[2] == 0 && octets[3] != 0 {
        // SOCKS4a: hostname follows.
        let (host, e) = read_cstr(io, buf, end).await?;
        end = e;
        host
    } else {
        ip.to_string()
    };
    if cmd != 1 {
        io.write_all(&[0, 0x5B, 0, 0, 0, 0, 0, 0]).await?;
        return Err(SocksError::Protocol(
            "only the SOCKS CONNECT command is supported",
        ));
    }
    let Some(host) = socks_host(&host) else {
        io.write_all(&[0, 0x5B, 0, 0, 0, 0, 0, 0]).await?;
        return Err(SocksError::Protocol("invalid SOCKS4a hostname"));
    };
    let target = HostPort::new(host, port);
    buf.advance(end);
    io.write_all(&[0, 0x5A, 0, 0, 0, 0, 0, 0]).await?;
    io.flush().await?;
    Ok(SocksRequest { version: 4, target })
}

async fn socks5<IO>(
    io: &mut IO,
    buf: &mut BytesMut,
    auth: Option<&ClientAccess>,
) -> Result<SocksRequest, SocksError>
where
    IO: AsyncRead + AsyncWrite + Unpin + ?Sized,
{
    // Greeting: VER NMETHODS METHODS...
    fill_to(io, buf, 2, HANDSHAKE_TIMEOUT).await?;
    let nmethods = buf[1] as usize;
    fill_to(io, buf, 2 + nmethods, HANDSHAKE_TIMEOUT).await?;
    let methods = buf[2..2 + nmethods].to_vec();
    buf.advance(2 + nmethods);
    // Prefer user/pass when offered: clients only offer it when they have credentials.
    let method = if methods.contains(&0x02) {
        0x02
    } else if methods.contains(&0x00) && auth.is_none() {
        0x00
    } else {
        io.write_all(&[5, 0xFF]).await?;
        return Err(if auth.is_some() {
            SocksError::AuthFailed
        } else {
            SocksError::Protocol("no acceptable SOCKS5 authentication method")
        });
    };
    io.write_all(&[5, method]).await?;
    io.flush().await?;

    if method == 0x02 {
        // VER(1)=1 ULEN UNAME PLEN PASSWD
        fill_to(io, buf, 2, HANDSHAKE_TIMEOUT).await?;
        if buf[0] != 1 {
            io.write_all(&[1, 1]).await?;
            return Err(SocksError::Protocol("invalid SOCKS5 user/pass version"));
        }
        let ulen = buf[1] as usize;
        fill_to(io, buf, 2 + ulen + 1, HANDSHAKE_TIMEOUT).await?;
        let plen = buf[2 + ulen] as usize;
        fill_to(io, buf, 3 + ulen + plen, HANDSHAKE_TIMEOUT).await?;
        let accepted =
            auth.is_none_or(|a| a.check(&buf[2..2 + ulen], &buf[3 + ulen..3 + ulen + plen]));
        buf.advance(3 + ulen + plen);
        if !accepted {
            io.write_all(&[1, 1]).await?;
            io.flush().await?;
            return Err(SocksError::AuthFailed);
        }
        io.write_all(&[1, 0]).await?;
        io.flush().await?;
    }

    // Request: VER CMD RSV ATYP ADDR PORT
    fill_to(io, buf, 4, HANDSHAKE_TIMEOUT).await?;
    let (ver, cmd, atyp) = (buf[0], buf[1], buf[3]);
    if ver != 5 {
        io.write_all(&reply(0x01)).await?;
        return Err(SocksError::Protocol("invalid SOCKS5 request version"));
    }
    let (host, consumed) = match atyp {
        0x01 => {
            fill_to(io, buf, 4 + 4 + 2, HANDSHAKE_TIMEOUT).await?;
            (Ipv4Addr::new(buf[4], buf[5], buf[6], buf[7]).to_string(), 8)
        }
        0x03 => {
            fill_to(io, buf, 5, HANDSHAKE_TIMEOUT).await?;
            let len = buf[4] as usize;
            fill_to(io, buf, 5 + len + 2, HANDSHAKE_TIMEOUT).await?;
            (
                String::from_utf8_lossy(&buf[5..5 + len]).into_owned(),
                5 + len,
            )
        }
        0x04 => {
            fill_to(io, buf, 4 + 16 + 2, HANDSHAKE_TIMEOUT).await?;
            let mut o = [0u8; 16];
            o.copy_from_slice(&buf[4..20]);
            (Ipv6Addr::from(o).to_string(), 20)
        }
        _ => {
            io.write_all(&reply(0x08)).await?;
            return Err(SocksError::Protocol("unsupported SOCKS5 address type"));
        }
    };
    let port = u16::from_be_bytes([buf[consumed], buf[consumed + 1]]);
    buf.advance(consumed + 2);
    if cmd != 1 {
        io.write_all(&reply(0x07)).await?;
        return Err(SocksError::Protocol(
            "only the SOCKS CONNECT command is supported",
        ));
    }
    let Some(host) = socks_host(&host) else {
        io.write_all(&reply(0x04)).await?;
        return Err(SocksError::Protocol("invalid SOCKS5 destination host"));
    };
    let target = HostPort::new(host, port);
    io.write_all(&reply(0x00)).await?;
    io.flush().await?;
    Ok(SocksRequest { version: 5, target })
}

/// Validate & normalise a SOCKS destination host (IP literal or hostname, no port).
fn socks_host(host: &str) -> Option<String> {
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return Some(ip.to_string());
    }
    crate::target::valid_host(host).then(|| host.trim_end_matches('.').to_ascii_lowercase())
}

/// SOCKS5 reply with a zero IPv4 bind address.
fn reply(code: u8) -> [u8; 10] {
    [5, code, 0, 1, 0, 0, 0, 0, 0, 0]
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn run(client_bytes: Vec<u8>) -> (Result<SocksRequest, SocksError>, Vec<u8>, BytesMut) {
        let (mut client, mut server) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut buf = BytesMut::new();
            let res = handshake(&mut server, &mut buf, None).await;
            (res, buf)
        });
        client.write_all(&client_bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let (res, buf) = task.await.unwrap();
        let mut out = Vec::new();
        let _ =
            tokio::time::timeout(Duration::from_millis(500), client.read_to_end(&mut out)).await;
        (res, out, buf)
    }

    #[tokio::test]
    async fn socks5_no_auth_domain() {
        let mut bytes = vec![5, 1, 0, 5, 1, 0, 3, 11];
        bytes.extend_from_slice(b"example.com");
        bytes.extend_from_slice(&443u16.to_be_bytes());
        bytes.extend_from_slice(b"\x16\x03\x01"); // optimistic data
        let (res, out, rest) = run(bytes).await;
        let req = res.unwrap();
        assert_eq!(req.target, HostPort::new("example.com", 443));
        assert_eq!(out, vec![5, 0, 5, 0, 0, 1, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&rest[..], b"\x16\x03\x01");
    }

    #[tokio::test]
    async fn socks5_user_pass_without_required_credentials() {
        let mut bytes = vec![5, 2, 0, 2, 1, 6];
        bytes.extend_from_slice(b"chrome");
        bytes.push(3);
        bytes.extend_from_slice(b"pwd");
        bytes.extend_from_slice(&[5, 1, 0, 1, 127, 0, 0, 1, 0x1F, 0x90]);
        let (res, out, _) = run(bytes).await;
        let req = res.unwrap();
        assert_eq!(req.target, HostPort::new("127.0.0.1", 8080));
        assert_eq!(&out[..4], &[5, 2, 1, 0]);
    }

    #[tokio::test]
    async fn socks5_ipv6_is_canonicalised() {
        let mut bytes = vec![5, 1, 0, 5, 1, 0, 4];
        bytes.extend_from_slice(&"2001:db8::1".parse::<Ipv6Addr>().unwrap().octets());
        bytes.extend_from_slice(&80u16.to_be_bytes());
        let (res, _, _) = run(bytes).await;
        let t = res.unwrap().target;
        assert_eq!(t.host, "2001:db8::1");
        assert_eq!(t.to_string(), "[2001:db8::1]:80");
    }

    #[tokio::test]
    async fn socks5_rejects_bind_and_bad_methods() {
        let (res, out, _) = run(vec![5, 1, 0, 5, 2, 0, 1, 1, 2, 3, 4, 0, 80]).await;
        assert!(res.is_err());
        assert_eq!(&out[2..4], &[5, 7]);
        let (res, out, _) = run(vec![5, 1, 0x80]).await;
        assert!(res.is_err());
        assert_eq!(out, vec![5, 0xFF]);
        let (res, out, _) = run(vec![5, 1, 0, 5, 1, 0, 9]).await;
        assert!(res.is_err());
        assert_eq!(&out[2..4], &[5, 8]);
    }

    #[tokio::test]
    async fn socks4_and_4a() {
        let mut bytes = vec![4, 1, 0, 80, 10, 0, 0, 1];
        bytes.extend_from_slice(b"tag\0");
        let (res, out, _) = run(bytes).await;
        let req = res.unwrap();
        assert_eq!(req.version, 4);
        assert_eq!(req.target, HostPort::new("10.0.0.1", 80));
        assert_eq!(out, vec![0, 0x5A, 0, 0, 0, 0, 0, 0]);

        let mut bytes = vec![4, 1, 1, 0xBB, 0, 0, 0, 1, 0];
        bytes.extend_from_slice(b"Example.ORG\0GET");
        let (res, _, rest) = run(bytes).await;
        let req = res.unwrap();
        assert_eq!(req.target, HostPort::new("example.org", 443));
        assert_eq!(&rest[..], b"GET");

        let (res, out, _) = run(vec![4, 2, 0, 80, 1, 2, 3, 4, 0]).await;
        assert!(res.is_err());
        assert_eq!(out, vec![0, 0x5B, 0, 0, 0, 0, 0, 0]);
    }

    async fn run_with_auth(client_bytes: Vec<u8>) -> (Result<SocksRequest, SocksError>, Vec<u8>) {
        let access = ClientAccess::new(&akev_core::settings::ProxyAccessSettings {
            username: "phone".into(),
            password: "s3cret".into(),
            ..Default::default()
        });
        let (mut client, mut server) = tokio::io::duplex(4096);
        let task = tokio::spawn(async move {
            let mut buf = BytesMut::new();
            handshake(&mut server, &mut buf, Some(&access)).await
        });
        client.write_all(&client_bytes).await.unwrap();
        client.shutdown().await.unwrap();
        let res = task.await.unwrap();
        let mut out = Vec::new();
        let _ =
            tokio::time::timeout(Duration::from_millis(500), client.read_to_end(&mut out)).await;
        (res, out)
    }

    fn user_pass(user: &[u8], pass: &[u8]) -> Vec<u8> {
        let mut b = vec![5, 1, 2, 1, user.len() as u8];
        b.extend_from_slice(user);
        b.push(pass.len() as u8);
        b.extend_from_slice(pass);
        b.extend_from_slice(&[5, 1, 0, 1, 10, 0, 0, 1, 0, 80]);
        b
    }

    #[tokio::test]
    async fn socks5_credentials_are_verified_when_required() {
        // Right credentials: accepted.
        let (res, out) = run_with_auth(user_pass(b"phone", b"s3cret")).await;
        assert!(res.is_ok());
        assert_eq!(&out[..4], &[5, 2, 1, 0]);
        // Wrong password: RFC 1929 failure status, nothing proxied.
        let (res, out) = run_with_auth(user_pass(b"phone", b"guess")).await;
        assert!(matches!(res, Err(SocksError::AuthFailed)));
        assert_eq!(out, vec![5, 2, 1, 1]);
        // No-auth only: no acceptable method.
        let (res, out) = run_with_auth(vec![5, 1, 0]).await;
        assert!(matches!(res, Err(SocksError::AuthFailed)));
        assert_eq!(out, vec![5, 0xFF]);
        // SOCKS4 can't authenticate.
        let (res, out) = run_with_auth(vec![4, 1, 0, 80, 10, 0, 0, 1, 0]).await;
        assert!(matches!(res, Err(SocksError::AuthFailed)));
        assert_eq!(out[1], 0x5B);
    }

    #[tokio::test]
    async fn truncated_handshake_fails() {
        let (res, _, _) = run(vec![5, 3, 0]).await;
        assert!(matches!(res, Err(SocksError::Io(_))));
    }
}
