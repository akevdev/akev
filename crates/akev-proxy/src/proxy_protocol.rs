//! HAProxy PROXY protocol (v1 text and v2 binary) parsing, so Akev can sit behind a load
//! balancer / k8s ingress and still see real client addresses.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

/// A parsed PROXY header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProxyHeader {
    /// Total header length in bytes (to strip from the stream).
    pub len: usize,
    /// Original client and destination addresses (None for `UNKNOWN` / `LOCAL`).
    pub addrs: Option<(SocketAddr, SocketAddr)>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseResult {
    Complete(ProxyHeader),
    NeedMore,
    Invalid(&'static str),
}

const V1_MAX: usize = 107;

/// Parse a PROXY v1 header (`PROXY TCP4 src dst sport dport\r\n`).
pub fn parse_v1(buf: &[u8]) -> ParseResult {
    let end = match buf.windows(2).position(|w| w == b"\r\n") {
        Some(i) => i,
        None if buf.len() >= V1_MAX => return ParseResult::Invalid("PROXY v1 header too long"),
        None => return ParseResult::NeedMore,
    };
    let Ok(line) = std::str::from_utf8(&buf[..end]) else {
        return ParseResult::Invalid("PROXY v1 header is not ASCII");
    };
    let mut parts = line.split(' ');
    if parts.next() != Some("PROXY") {
        return ParseResult::Invalid("missing PROXY signature");
    }
    let len = end + 2;
    match parts.next() {
        Some("UNKNOWN") => ParseResult::Complete(ProxyHeader { len, addrs: None }),
        Some(fam @ ("TCP4" | "TCP6")) => {
            let fields: Vec<&str> = parts.collect();
            if fields.len() != 4 {
                return ParseResult::Invalid("malformed PROXY v1 address fields");
            }
            let parse_ip = |s: &str| -> Option<IpAddr> {
                let ip: IpAddr = s.parse().ok()?;
                match (fam, ip) {
                    ("TCP4", IpAddr::V4(_)) | ("TCP6", IpAddr::V6(_)) => Some(ip),
                    _ => None,
                }
            };
            match (
                parse_ip(fields[0]),
                parse_ip(fields[1]),
                fields[2].parse::<u16>().ok(),
                fields[3].parse::<u16>().ok(),
            ) {
                (Some(src), Some(dst), Some(sp), Some(dp)) => ParseResult::Complete(ProxyHeader {
                    len,
                    addrs: Some((SocketAddr::new(src, sp), SocketAddr::new(dst, dp))),
                }),
                _ => ParseResult::Invalid("malformed PROXY v1 addresses"),
            }
        }
        _ => ParseResult::Invalid("unsupported PROXY v1 protocol family"),
    }
}

/// Parse a PROXY v2 header (12-byte signature already detected).
pub fn parse_v2(buf: &[u8]) -> ParseResult {
    if buf.len() < 16 {
        return ParseResult::NeedMore;
    }
    if buf[..12] != crate::sniff::PROXY_V2_SIG[..] {
        return ParseResult::Invalid("missing PROXY v2 signature");
    }
    let ver_cmd = buf[12];
    if ver_cmd >> 4 != 2 {
        return ParseResult::Invalid("unsupported PROXY protocol version");
    }
    let fam = buf[13];
    let addr_len = u16::from_be_bytes([buf[14], buf[15]]) as usize;
    let len = 16 + addr_len;
    if buf.len() < len {
        return ParseResult::NeedMore;
    }
    let body = &buf[16..len];
    // LOCAL command: health checks etc. — keep the socket addresses.
    if ver_cmd & 0x0F == 0 {
        return ParseResult::Complete(ProxyHeader { len, addrs: None });
    }
    if ver_cmd & 0x0F != 1 {
        return ParseResult::Invalid("unsupported PROXY v2 command");
    }
    let addrs = match fam >> 4 {
        0x1 if body.len() >= 12 => {
            let src = Ipv4Addr::new(body[0], body[1], body[2], body[3]);
            let dst = Ipv4Addr::new(body[4], body[5], body[6], body[7]);
            let sp = u16::from_be_bytes([body[8], body[9]]);
            let dp = u16::from_be_bytes([body[10], body[11]]);
            Some((
                SocketAddr::new(src.into(), sp),
                SocketAddr::new(dst.into(), dp),
            ))
        }
        0x2 if body.len() >= 36 => {
            let mut s = [0u8; 16];
            let mut d = [0u8; 16];
            s.copy_from_slice(&body[..16]);
            d.copy_from_slice(&body[16..32]);
            let sp = u16::from_be_bytes([body[32], body[33]]);
            let dp = u16::from_be_bytes([body[34], body[35]]);
            Some((
                SocketAddr::new(Ipv6Addr::from(s).into(), sp),
                SocketAddr::new(Ipv6Addr::from(d).into(), dp),
            ))
        }
        // AF_UNSPEC / AF_UNIX: no usable TCP addresses.
        _ => None,
    };
    ParseResult::Complete(ProxyHeader { len, addrs })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sniff::PROXY_V2_SIG;

    #[test]
    fn v1_tcp4_and_tcp6() {
        let h = b"PROXY TCP4 192.168.0.1 192.168.0.11 56324 443\r\nGET /";
        match parse_v1(h) {
            ParseResult::Complete(p) => {
                assert_eq!(p.len, 47);
                let (src, dst) = p.addrs.unwrap();
                assert_eq!(src, "192.168.0.1:56324".parse().unwrap());
                assert_eq!(dst, "192.168.0.11:443".parse().unwrap());
            }
            other => panic!("{other:?}"),
        }
        let h = b"PROXY TCP6 ::1 ::2 1 2\r\n";
        assert!(matches!(
            parse_v1(h),
            ParseResult::Complete(ProxyHeader { addrs: Some(_), .. })
        ));
        assert_eq!(
            parse_v1(b"PROXY UNKNOWN\r\n"),
            ParseResult::Complete(ProxyHeader {
                len: 15,
                addrs: None
            })
        );
        assert_eq!(parse_v1(b"PROXY TCP4 1.2.3.4"), ParseResult::NeedMore);
        assert!(matches!(
            parse_v1(b"PROXY TCP4 ::1 ::2 1 2\r\n"),
            ParseResult::Invalid(_)
        ));
        assert!(matches!(parse_v1(&[b'P'; 200]), ParseResult::Invalid(_)));
    }

    #[test]
    fn v2_ipv4_and_local() {
        let mut h = PROXY_V2_SIG.to_vec();
        h.extend_from_slice(&[0x21, 0x11, 0x00, 0x0C]);
        h.extend_from_slice(&[10, 0, 0, 1, 10, 0, 0, 2, 0x1F, 0x90, 0x01, 0xBB]);
        h.extend_from_slice(b"rest");
        match parse_v2(&h) {
            ParseResult::Complete(p) => {
                assert_eq!(p.len, 28);
                let (src, dst) = p.addrs.unwrap();
                assert_eq!(src, "10.0.0.1:8080".parse().unwrap());
                assert_eq!(dst, "10.0.0.2:443".parse().unwrap());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(parse_v2(&h[..20]), ParseResult::NeedMore);
        let mut local = PROXY_V2_SIG.to_vec();
        local.extend_from_slice(&[0x20, 0x00, 0x00, 0x00]);
        assert_eq!(
            parse_v2(&local),
            ParseResult::Complete(ProxyHeader {
                len: 16,
                addrs: None
            })
        );
        let mut bad = PROXY_V2_SIG.to_vec();
        bad.extend_from_slice(&[0x11, 0x00, 0x00, 0x00]);
        assert!(matches!(parse_v2(&bad), ParseResult::Invalid(_)));
    }
}
