//! Protocol detection from the first bytes of a stream. Runs on every layer: the raw TCP
//! connection, and again after each unwrap (CONNECT tunnel, SOCKS handshake, TLS decryption,
//! PROXY protocol header).

/// What the peeked bytes look like.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// TLS handshake record (ClientHello).
    Tls,
    Socks4,
    Socks5,
    /// HTTP/2 connection preface (h2c prior knowledge, or h2 after ALPN).
    H2Preface,
    /// An HTTP/1.x request line.
    Http1,
    /// HAProxy PROXY protocol v1 (text).
    ProxyV1,
    /// HAProxy PROXY protocol v2 (binary).
    ProxyV2,
    /// Anything else.
    Unknown,
}

/// Result of a sniffing attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sniff {
    Detected(Protocol),
    /// More bytes are needed to decide.
    NeedMore,
}

/// The HTTP/2 client connection preface.
pub const H2_PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";
/// PROXY protocol v2 signature.
pub const PROXY_V2_SIG: &[u8] = b"\r\n\r\n\0\r\nQUIT\n";

/// Longest method token we accept as "looks like HTTP/1".
const MAX_METHOD_LEN: usize = 24;

/// Methods we recognise even when they are not plain uppercase words (e.g. `M-SEARCH`).
const KNOWN_METHODS: &[&[u8]] = &[
    b"GET",
    b"POST",
    b"PUT",
    b"DELETE",
    b"HEAD",
    b"OPTIONS",
    b"PATCH",
    b"CONNECT",
    b"TRACE",
    b"PROPFIND",
    b"PROPPATCH",
    b"MKCOL",
    b"COPY",
    b"MOVE",
    b"LOCK",
    b"UNLOCK",
    b"REPORT",
    b"MKACTIVITY",
    b"CHECKOUT",
    b"MERGE",
    b"M-SEARCH",
    b"NOTIFY",
    b"SUBSCRIBE",
    b"UNSUBSCRIBE",
    b"SEARCH",
    b"PURGE",
    b"LINK",
    b"UNLINK",
    b"BIND",
    b"REBIND",
    b"UNBIND",
    b"ACL",
    b"MKCALENDAR",
    b"QUERY",
    b"SOURCE",
    b"PRI",
];

/// Classify the start of a stream. `allow_proxy_protocol` enables PROXY v1/v2 detection (only
/// meaningful on the outermost TCP layer).
pub fn sniff(buf: &[u8], allow_proxy_protocol: bool) -> Sniff {
    let Some(&first) = buf.first() else {
        return Sniff::NeedMore;
    };
    match first {
        0x16 => {
            // TLS record: type 0x16, version 0x03 0x00..=0x04.
            match (buf.get(1), buf.get(2)) {
                (None, _) | (Some(0x03), None) => Sniff::NeedMore,
                (Some(0x03), Some(minor)) if *minor <= 0x04 => Sniff::Detected(Protocol::Tls),
                _ => Sniff::Detected(Protocol::Unknown),
            }
        }
        0x04 => match buf.get(1) {
            None => Sniff::NeedMore,
            Some(0x01) | Some(0x02) => Sniff::Detected(Protocol::Socks4),
            Some(_) => Sniff::Detected(Protocol::Unknown),
        },
        0x05 => match buf.get(1) {
            None => Sniff::NeedMore,
            Some(n) if *n >= 1 => Sniff::Detected(Protocol::Socks5),
            Some(_) => Sniff::Detected(Protocol::Unknown),
        },
        b'\r' | b'\n' => sniff_leading_newlines(buf, allow_proxy_protocol),
        _ => sniff_text(buf, allow_proxy_protocol),
    }
}

/// Leading CR/LF: either a PROXY v2 signature or blank lines before an HTTP/1 request line
/// (RFC 9112 §2.2 says servers should skip them).
fn sniff_leading_newlines(buf: &[u8], allow_proxy_protocol: bool) -> Sniff {
    if allow_proxy_protocol {
        let n = buf.len().min(PROXY_V2_SIG.len());
        if buf[..n] == PROXY_V2_SIG[..n] {
            return if n == PROXY_V2_SIG.len() {
                Sniff::Detected(Protocol::ProxyV2)
            } else {
                Sniff::NeedMore
            };
        }
    }
    let start = buf.iter().position(|&b| b != b'\r' && b != b'\n');
    match start {
        None if buf.len() > 16 => Sniff::Detected(Protocol::Unknown),
        None => Sniff::NeedMore,
        Some(i) => match sniff_text(&buf[i..], false) {
            Sniff::Detected(Protocol::Http1) => Sniff::Detected(Protocol::Http1),
            Sniff::NeedMore => Sniff::NeedMore,
            Sniff::Detected(_) => Sniff::Detected(Protocol::Unknown),
        },
    }
}

fn sniff_text(buf: &[u8], allow_proxy_protocol: bool) -> Sniff {
    // HTTP/2 preface ("PRI * HTTP/2.0...") — must be checked before generic methods.
    let n = buf.len().min(H2_PREFACE.len());
    if buf[..n] == H2_PREFACE[..n] {
        return if n == H2_PREFACE.len() {
            Sniff::Detected(Protocol::H2Preface)
        } else {
            Sniff::NeedMore
        };
    }
    if allow_proxy_protocol {
        const V1: &[u8] = b"PROXY ";
        let n = buf.len().min(V1.len());
        if buf[..n] == V1[..n] {
            return if n == V1.len() {
                Sniff::Detected(Protocol::ProxyV1)
            } else {
                Sniff::NeedMore
            };
        }
    }
    match buf.iter().position(|&b| b == b' ') {
        Some(0) => Sniff::Detected(Protocol::Unknown),
        Some(i) => {
            let token = &buf[..i];
            if i <= MAX_METHOD_LEN && (is_upper_word(token) || KNOWN_METHODS.contains(&token)) {
                Sniff::Detected(Protocol::Http1)
            } else {
                Sniff::Detected(Protocol::Unknown)
            }
        }
        None => {
            // Still reading the method: it must stay a plausible token.
            if buf.len() <= MAX_METHOD_LEN
                && buf
                    .iter()
                    .all(|&b| b.is_ascii_uppercase() || b == b'-' || b == b'_')
            {
                Sniff::NeedMore
            } else {
                Sniff::Detected(Protocol::Unknown)
            }
        }
    }
}

fn is_upper_word(token: &[u8]) -> bool {
    !token.is_empty()
        && token
            .iter()
            .all(|&b| b.is_ascii_uppercase() || b == b'-' || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(buf: &[u8]) -> Sniff {
        sniff(buf, true)
    }

    #[test]
    fn detects_tls() {
        assert_eq!(d(&[0x16]), Sniff::NeedMore);
        assert_eq!(d(&[0x16, 0x03]), Sniff::NeedMore);
        assert_eq!(
            d(&[0x16, 0x03, 0x01, 0x02, 0x00]),
            Sniff::Detected(Protocol::Tls)
        );
        assert_eq!(d(&[0x16, 0x03, 0x03]), Sniff::Detected(Protocol::Tls));
        assert_eq!(d(&[0x16, 0x03, 0x09]), Sniff::Detected(Protocol::Unknown));
        assert_eq!(d(&[0x16, 0x01, 0x00]), Sniff::Detected(Protocol::Unknown));
    }

    #[test]
    fn detects_socks() {
        assert_eq!(d(&[0x05]), Sniff::NeedMore);
        assert_eq!(d(&[0x05, 0x01, 0x00]), Sniff::Detected(Protocol::Socks5));
        assert_eq!(d(&[0x05, 0x00]), Sniff::Detected(Protocol::Unknown));
        assert_eq!(
            d(&[0x04, 0x01, 0x00, 0x50]),
            Sniff::Detected(Protocol::Socks4)
        );
        assert_eq!(d(&[0x04, 0x09]), Sniff::Detected(Protocol::Unknown));
    }

    #[test]
    fn detects_h2_preface_incrementally() {
        for i in 1..H2_PREFACE.len() {
            assert_eq!(d(&H2_PREFACE[..i]), Sniff::NeedMore, "prefix len {i}");
        }
        assert_eq!(d(H2_PREFACE), Sniff::Detected(Protocol::H2Preface));
        // "PRI" followed by something else is just an (odd) HTTP/1 method.
        assert_eq!(d(b"PRI /x HTTP/1.1\r\n"), Sniff::Detected(Protocol::Http1));
    }

    #[test]
    fn detects_http1() {
        assert_eq!(d(b"GET / HTTP/1.1\r\n"), Sniff::Detected(Protocol::Http1));
        assert_eq!(d(b"G"), Sniff::NeedMore);
        assert_eq!(
            d(b"CONNECT example.com:443 HTTP/1.1"),
            Sniff::Detected(Protocol::Http1)
        );
        assert_eq!(d(b"M-SEARCH * HTTP/1.1"), Sniff::Detected(Protocol::Http1));
        assert_eq!(d(b"FOOBAR / HTTP/1.1"), Sniff::Detected(Protocol::Http1));
        assert_eq!(d(b"\r\nGET / HTTP/1.1"), Sniff::Detected(Protocol::Http1));
        assert_eq!(d(b"get / HTTP/1.1"), Sniff::Detected(Protocol::Unknown));
        assert_eq!(
            d(b"SSH-2.0-OpenSSH_9.6"),
            Sniff::Detected(Protocol::Unknown)
        );
        assert_eq!(d(b" GET"), Sniff::Detected(Protocol::Unknown));
        assert_eq!(d(&[0x00, 0x01]), Sniff::Detected(Protocol::Unknown));
        assert_eq!(
            d(b"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
            Sniff::Detected(Protocol::Unknown)
        );
    }

    #[test]
    fn detects_proxy_protocol_only_when_allowed() {
        assert_eq!(d(b"PROX"), Sniff::NeedMore);
        assert_eq!(
            d(b"PROXY TCP4 1.2.3.4 5.6.7.8 1 2\r\n"),
            Sniff::Detected(Protocol::ProxyV1)
        );
        assert_eq!(d(&PROXY_V2_SIG[..5]), Sniff::NeedMore);
        assert_eq!(d(PROXY_V2_SIG), Sniff::Detected(Protocol::ProxyV2));
        assert_eq!(
            sniff(b"PROXY TCP4 ", false),
            Sniff::Detected(Protocol::Http1)
        );
        assert_eq!(
            sniff(PROXY_V2_SIG, false),
            Sniff::Detected(Protocol::Unknown)
        );
    }
}
