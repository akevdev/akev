//! HTTP/1.x request head parsing (httparse) preserving header casing, order and duplicates,
//! plus the framing / connection semantics derived from the headers.

use akev_core::{Headers, HttpVersion, header_value_to_string};

/// Maximum request head size (HTTP Toolkit's desktop app raises Node's 16KB to 100KB).
pub const MAX_HEAD_SIZE: usize = 128 * 1024;
const MAX_HEADERS: usize = 4096;

/// A parsed request head, byte-faithful.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRequest {
    pub method: String,
    /// Request target exactly as sent.
    pub target: String,
    pub version: HttpVersion,
    pub headers: Headers,
    /// Number of bytes the head occupied (including the blank line).
    pub head_len: usize,
}

/// Parse failures, with llhttp-style codes (familiar from Node / HTTP Toolkit).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    TooLarge,
    Invalid { message: String, code: &'static str },
}

impl ParseError {
    pub fn code(&self) -> &'static str {
        match self {
            ParseError::TooLarge => "HPE_HEADER_OVERFLOW",
            ParseError::Invalid { code, .. } => code,
        }
    }

    pub fn message(&self) -> String {
        match self {
            ParseError::TooLarge => format!("request head larger than {} KB", MAX_HEAD_SIZE / 1024),
            ParseError::Invalid { message, .. } => message.clone(),
        }
    }

    fn invalid(message: impl Into<String>, code: &'static str) -> Self {
        ParseError::Invalid {
            message: message.into(),
            code,
        }
    }
}

fn map_httparse_error(e: httparse::Error) -> ParseError {
    let code = match e {
        httparse::Error::Token => "HPE_INVALID_METHOD",
        httparse::Error::Version => "HPE_INVALID_VERSION",
        httparse::Error::HeaderName => "HPE_INVALID_HEADER_TOKEN",
        httparse::Error::HeaderValue => "HPE_INVALID_HEADER_VALUE",
        httparse::Error::NewLine => "HPE_LF_EXPECTED",
        httparse::Error::Status => "HPE_INVALID_STATUS",
        httparse::Error::TooManyHeaders => "HPE_HEADER_OVERFLOW",
    };
    ParseError::invalid(e.to_string(), code)
}

/// Whether `buf` (from offset `from`) contains the end of a head (`\n\r\n` or `\n\n`).
/// Lets the reader avoid re-running the parser on every partial read.
pub fn has_head_end(buf: &[u8], from: usize) -> bool {
    let start = from.saturating_sub(3);
    let tail = &buf[start.min(buf.len())..];
    tail.windows(2).any(|w| w == b"\n\n") || tail.windows(3).any(|w| w == b"\n\r\n")
}

/// Parse a request head from the start of `buf`. `Ok(None)` = incomplete.
pub fn parse_request(buf: &[u8]) -> Result<Option<RawRequest>, ParseError> {
    let mut config = httparse::ParserConfig::default();
    config
        .allow_multiple_spaces_in_request_line_delimiters(true)
        .ignore_invalid_headers_in_requests(true);

    let mut stack = [httparse::EMPTY_HEADER; 64];
    let mut heap: Vec<httparse::Header<'_>>;
    let mut capacity = stack.len();
    loop {
        let headers: &mut [httparse::Header<'_>] = if capacity == stack.len() {
            &mut stack
        } else {
            heap = vec![httparse::EMPTY_HEADER; capacity];
            &mut heap
        };
        let mut req = httparse::Request::new(headers);
        match config.parse_request(&mut req, buf) {
            Ok(httparse::Status::Complete(head_len)) => return Ok(Some(build(&req, head_len))),
            Ok(httparse::Status::Partial) => {
                return if buf.len() > MAX_HEAD_SIZE {
                    Err(ParseError::TooLarge)
                } else {
                    Ok(None)
                };
            }
            Err(httparse::Error::TooManyHeaders) if capacity < MAX_HEADERS => capacity *= 4,
            Err(httparse::Error::TooManyHeaders) => return Err(ParseError::TooLarge),
            Err(e) => return Err(map_httparse_error(e)),
        }
    }
}

fn build(req: &httparse::Request<'_, '_>, head_len: usize) -> RawRequest {
    let mut headers = Headers::with_capacity(req.headers.len());
    for h in req.headers.iter() {
        headers.append(h.name.to_string(), header_value_to_string(h.value));
    }
    RawRequest {
        method: req.method.unwrap_or_default().to_string(),
        target: req.path.unwrap_or_default().to_string(),
        version: if req.version == Some(0) {
            HttpVersion::Http10
        } else {
            HttpVersion::Http11
        },
        headers,
        head_len,
    }
}

/// How a message body is delimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    Empty,
    Length(u64),
    Chunked,
}

/// Comma-separated tokens of all values of a header, trimmed and lowercased.
pub fn header_tokens(headers: &Headers, name: &str) -> Vec<String> {
    headers
        .get_all(name)
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect()
}

/// Request body framing per RFC 9112 §6.3 (Transfer-Encoding wins over Content-Length;
/// conflicting Content-Length values and non-chunked final codings are rejected).
pub fn request_framing(headers: &Headers) -> Result<Framing, ParseError> {
    let te = header_tokens(headers, "transfer-encoding");
    if !te.is_empty() {
        return if te.last().map(String::as_str) == Some("chunked") {
            Ok(Framing::Chunked)
        } else {
            Err(ParseError::invalid(
                format!("unsupported request transfer-encoding: {}", te.join(", ")),
                "HPE_INVALID_TRANSFER_ENCODING",
            ))
        };
    }
    let mut length: Option<u64> = None;
    for v in headers.get_all("content-length").flat_map(|v| v.split(',')) {
        let v = v.trim();
        let n: u64 = v.parse().map_err(|_| {
            ParseError::invalid(
                format!("invalid content-length: {v:?}"),
                "HPE_INVALID_CONTENT_LENGTH",
            )
        })?;
        match length {
            Some(prev) if prev != n => {
                return Err(ParseError::invalid(
                    "conflicting content-length values",
                    "HPE_UNEXPECTED_CONTENT_LENGTH",
                ));
            }
            _ => length = Some(n),
        }
    }
    Ok(match length {
        Some(0) | None => Framing::Empty,
        Some(n) => Framing::Length(n),
    })
}

/// What an `Upgrade` request asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpgradeRequest {
    WebSocket,
    /// HTTP/2 cleartext upgrade — handled as a normal request (the upgrade headers are dropped
    /// from the forwarded request).
    H2c,
    Other(String),
}

/// Detect a connection upgrade request (`Connection: upgrade` + `Upgrade: ...`).
pub fn upgrade_request(headers: &Headers) -> Option<UpgradeRequest> {
    if !header_tokens(headers, "connection")
        .iter()
        .any(|t| t == "upgrade")
    {
        return None;
    }
    let upgrade = headers.get("upgrade")?.trim();
    let tokens: Vec<String> = upgrade
        .split(',')
        .map(|t| t.trim().to_ascii_lowercase())
        .collect();
    if tokens.iter().any(|t| t == "websocket") {
        Some(UpgradeRequest::WebSocket)
    } else if tokens.iter().any(|t| t == "h2c") {
        Some(UpgradeRequest::H2c)
    } else if upgrade.is_empty() {
        None
    } else {
        Some(UpgradeRequest::Other(upgrade.to_string()))
    }
}

/// Whether the client wants the connection closed after this exchange.
pub fn client_wants_close(headers: &Headers, version: HttpVersion) -> bool {
    let tokens = header_tokens(headers, "connection");
    if tokens.iter().any(|t| t == "close") {
        return true;
    }
    version == HttpVersion::Http10 && !tokens.iter().any(|t| t == "keep-alive")
}

/// `Expect: 100-continue` present.
pub fn expects_continue(headers: &Headers) -> bool {
    headers
        .get_all("expect")
        .any(|v| v.trim().eq_ignore_ascii_case("100-continue"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_casing_order_and_duplicates() {
        let raw = b"POST /a?b=c HTTP/1.1\r\nHost: example.com\r\nX-Custom-CASE: One\r\nx-custom-case: two\r\nCookie: a=1\r\nCookie: b=2\r\nContent-Length: 0\r\n\r\nNEXT";
        let req = parse_request(raw).unwrap().unwrap();
        assert_eq!(req.method, "POST");
        assert_eq!(req.target, "/a?b=c");
        assert_eq!(req.version, HttpVersion::Http11);
        assert_eq!(req.head_len, raw.len() - 4);
        let names: Vec<&str> = req.headers.iter().map(|(k, _)| k).collect();
        assert_eq!(
            names,
            vec![
                "Host",
                "X-Custom-CASE",
                "x-custom-case",
                "Cookie",
                "Cookie",
                "Content-Length"
            ]
        );
        assert_eq!(
            req.headers.get_all("x-custom-case").collect::<Vec<_>>(),
            vec!["One", "two"]
        );
    }

    #[test]
    fn incomplete_and_oversized() {
        assert_eq!(parse_request(b"GET / HTTP/1.1\r\nHost: a\r\n"), Ok(None));
        assert_eq!(parse_request(b"GE"), Ok(None));
        let mut big = b"GET / HTTP/1.1\r\nX-Big: ".to_vec();
        big.extend(std::iter::repeat_n(b'a', MAX_HEAD_SIZE + 10));
        assert_eq!(parse_request(&big), Err(ParseError::TooLarge));
    }

    #[test]
    fn many_headers_grow_the_table() {
        let mut raw = b"GET / HTTP/1.1\r\n".to_vec();
        for i in 0..300 {
            raw.extend_from_slice(format!("X-H{i}: {i}\r\n").as_bytes());
        }
        raw.extend_from_slice(b"\r\n");
        let req = parse_request(&raw).unwrap().unwrap();
        assert_eq!(req.headers.len(), 300);
        assert_eq!(req.headers.get("x-h299"), Some("299"));
    }

    #[test]
    fn malformed_input() {
        let err = parse_request(b"GET / HTTP/9.9\r\n\r\n").unwrap_err();
        assert_eq!(err.code(), "HPE_INVALID_VERSION");
        let err = parse_request(b"G\x01T / HTTP/1.1\r\n\r\n").unwrap_err();
        assert_eq!(err.code(), "HPE_INVALID_METHOD");
        // Leading blank lines are tolerated.
        assert!(
            parse_request(b"\r\nGET / HTTP/1.1\r\n\r\n")
                .unwrap()
                .is_some()
        );
        // HTTP/1.0 and obs-text (latin-1) header values round-trip.
        let req = parse_request(b"GET / HTTP/1.0\r\nX: caf\xe9\r\n\r\n")
            .unwrap()
            .unwrap();
        assert_eq!(req.version, HttpVersion::Http10);
        assert_eq!(
            akev_core::string_to_header_bytes(req.headers.get("x").unwrap()),
            b"caf\xe9".to_vec()
        );
    }

    #[test]
    fn head_end_detection() {
        assert!(has_head_end(b"GET / HTTP/1.1\r\n\r\n", 0));
        assert!(has_head_end(b"GET / HTTP/1.1\n\n", 0));
        assert!(!has_head_end(b"GET / HTTP/1.1\r\nHost: x\r\n", 0));
        // The terminator split across reads is still found from the previous offset.
        assert!(has_head_end(b"GET / HTTP/1.1\r\n\r\n", 16));
    }

    fn hdrs(pairs: &[(&str, &str)]) -> Headers {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn framing_rules() {
        assert_eq!(request_framing(&hdrs(&[])), Ok(Framing::Empty));
        assert_eq!(
            request_framing(&hdrs(&[("Content-Length", "10")])),
            Ok(Framing::Length(10))
        );
        assert_eq!(
            request_framing(&hdrs(&[("Content-Length", "0")])),
            Ok(Framing::Empty)
        );
        assert_eq!(
            request_framing(&hdrs(&[
                ("Content-Length", "5, 5"),
                ("content-length", "5")
            ])),
            Ok(Framing::Length(5))
        );
        assert!(
            request_framing(&hdrs(&[("Content-Length", "5"), ("Content-Length", "6")])).is_err()
        );
        assert!(request_framing(&hdrs(&[("Content-Length", "-1")])).is_err());
        assert_eq!(
            request_framing(&hdrs(&[
                ("Transfer-Encoding", "gzip, Chunked"),
                ("Content-Length", "3")
            ])),
            Ok(Framing::Chunked)
        );
        assert!(request_framing(&hdrs(&[("Transfer-Encoding", "chunked, gzip")])).is_err());
    }

    #[test]
    fn upgrade_and_connection_semantics() {
        let ws = hdrs(&[
            ("Connection", "keep-alive, Upgrade"),
            ("Upgrade", "WebSocket"),
        ]);
        assert_eq!(upgrade_request(&ws), Some(UpgradeRequest::WebSocket));
        assert_eq!(
            upgrade_request(&hdrs(&[
                ("Connection", "Upgrade, HTTP2-Settings"),
                ("Upgrade", "h2c")
            ])),
            Some(UpgradeRequest::H2c)
        );
        assert_eq!(
            upgrade_request(&hdrs(&[("Connection", "upgrade"), ("Upgrade", "foo/2")])),
            Some(UpgradeRequest::Other("foo/2".into()))
        );
        assert_eq!(upgrade_request(&hdrs(&[("Upgrade", "websocket")])), None);
        assert!(client_wants_close(
            &hdrs(&[("Connection", "close")]),
            HttpVersion::Http11
        ));
        assert!(!client_wants_close(&hdrs(&[]), HttpVersion::Http11));
        assert!(client_wants_close(&hdrs(&[]), HttpVersion::Http10));
        assert!(!client_wants_close(
            &hdrs(&[("Connection", "Keep-Alive")]),
            HttpVersion::Http10
        ));
        assert!(expects_continue(&hdrs(&[("Expect", "100-Continue")])));
    }
}
