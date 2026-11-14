//! HTTP/1.x response serialization: headers are written verbatim (casing, order, duplicates);
//! only framing headers are adjusted when the body's framing must change (e.g. an HTTP/2
//! upstream response without Content-Length going to an HTTP/1.1 client becomes chunked).

use super::parse::header_tokens;
use akev_core::{Headers, HttpVersion, ResponseHead, canonical_reason, string_to_header_bytes};

/// What body the responder will provide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BodyKind {
    /// No body will follow.
    None,
    /// A complete body of this length.
    Full(u64),
    /// A streamed body (length if known from upstream framing).
    Stream,
}

/// How the body is framed on the wire towards the client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutFraming {
    /// No body allowed (HEAD, 1xx, 204, 304) or none provided.
    None,
    Length(u64),
    Chunked,
    /// Delimited by closing the connection.
    CloseDelimited,
}

/// Facts about the request the response answers.
#[derive(Clone, Copy, Debug)]
pub struct ResponseContext {
    pub head_request: bool,
    pub client_version: HttpVersion,
    pub client_wants_close: bool,
}

/// The serialized head and framing decisions.
#[derive(Clone, Debug)]
pub struct ResponsePlan {
    pub head: Vec<u8>,
    pub framing: OutFraming,
    /// Close the connection after the response.
    pub close_after: bool,
}

/// Header name validity (RFC 9110 token).
pub fn valid_header_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| {
            b.is_ascii_alphanumeric()
                || matches!(
                    b,
                    b'!' | b'#'
                        | b'$'
                        | b'%'
                        | b'&'
                        | b'\''
                        | b'*'
                        | b'+'
                        | b'-'
                        | b'.'
                        | b'^'
                        | b'_'
                        | b'`'
                        | b'|'
                        | b'~'
                )
        })
}

/// Header value validity for writing (no CR/LF/NUL — prevents response splitting).
pub fn valid_header_value(value: &[u8]) -> bool {
    !value.iter().any(|&b| b == b'\r' || b == b'\n' || b == 0)
}

fn status_has_no_body(status: u16) -> bool {
    (100..200).contains(&status) || status == 204 || status == 304
}

/// Decide framing and serialize the response head.
pub fn plan_response(head: &ResponseHead, ctx: &ResponseContext, body: BodyKind) -> ResponsePlan {
    let te = header_tokens(&head.headers, "transfer-encoding");
    let has_te = head.headers.contains("transfer-encoding");
    let chunked_header = te.last().map(String::as_str) == Some("chunked");
    // Transfer-Encoding overrides Content-Length (RFC 9112 §6.3), and an intermediary must
    // remove a received Content-Length before forwarding such a message: with TE present, a
    // length is only ever written for a body whose size is known (`set_len`).
    let drop_len = has_te;
    let declared_len = if has_te {
        None
    } else {
        head.headers.content_length()
    };
    let client_11 = ctx.client_version != HttpVersion::Http10;
    let response_close = header_tokens(&head.headers, "connection")
        .iter()
        .any(|t| t == "close");

    let mut remove_te = false;
    let mut set_len: Option<u64> = None;
    let mut add_chunked = false;

    let framing = if ctx.head_request || status_has_no_body(head.status) || body == BodyKind::None {
        OutFraming::None
    } else if chunked_header {
        if client_11 {
            OutFraming::Chunked
        } else {
            // HTTP/1.0 clients don't understand chunked: strip it and send a length when known,
            // else close-delimit.
            remove_te = true;
            match body {
                BodyKind::Full(n) => {
                    set_len = Some(n);
                    OutFraming::Length(n)
                }
                _ => OutFraming::CloseDelimited,
            }
        }
    } else if has_te {
        // A transfer coding other than chunked comes last: the body runs until the connection
        // closes (RFC 9112 §6.3), so it stays close-delimited. Its bytes are still
        // transfer-coded (relayed as received, with the header), so no length describes them.
        OutFraming::CloseDelimited
    } else if let BodyKind::Full(n) = body {
        if declared_len != Some(n) {
            set_len = Some(n);
        }
        OutFraming::Length(n)
    } else if let Some(n) = declared_len {
        OutFraming::Length(n)
    } else if head.http_version == HttpVersion::Http10 || head.http_version == HttpVersion::Http11 {
        // Upstream used close-delimited framing: preserve it.
        OutFraming::CloseDelimited
    } else if client_11 {
        add_chunked = true;
        OutFraming::Chunked
    } else {
        OutFraming::CloseDelimited
    };

    let close_after =
        ctx.client_wants_close || response_close || framing == OutFraming::CloseDelimited;

    let mut out = Vec::with_capacity(64 + head.headers.wire_size());
    let version = if head.http_version == HttpVersion::Http10 {
        "HTTP/1.0"
    } else {
        "HTTP/1.1"
    };
    let reason = if head.status_message.is_empty() {
        canonical_reason(head.status)
    } else {
        head.status_message.as_str()
    };
    out.extend_from_slice(version.as_bytes());
    out.push(b' ');
    out.extend_from_slice(head.status.to_string().as_bytes());
    out.push(b' ');
    let reason_bytes = string_to_header_bytes(reason);
    if valid_header_value(&reason_bytes) {
        out.extend_from_slice(&reason_bytes);
    }
    out.extend_from_slice(b"\r\n");
    let mut wrote_len = false;
    for (name, value) in head.headers.iter() {
        if remove_te && name.eq_ignore_ascii_case("transfer-encoding") {
            continue;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if let Some(n) = set_len {
                if !wrote_len {
                    write_header(&mut out, name, n.to_string().as_bytes());
                    wrote_len = true;
                }
                continue;
            }
            if drop_len {
                continue;
            }
        }
        let value = string_to_header_bytes(value);
        if !valid_header_name(name) || !valid_header_value(&value) {
            tracing::debug!(header = name, "dropping invalid response header");
            continue;
        }
        write_header(&mut out, name, &value);
    }
    if let (Some(n), false) = (set_len, wrote_len) {
        write_header(&mut out, "Content-Length", n.to_string().as_bytes());
    }
    if add_chunked {
        write_header(&mut out, "Transfer-Encoding", b"chunked");
    }
    out.extend_from_slice(b"\r\n");
    ResponsePlan {
        head: out,
        framing,
        close_after,
    }
}

fn write_header(out: &mut Vec<u8>, name: &str, value: &[u8]) {
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b": ");
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
}

/// Serialize an interim (1xx) response.
pub fn informational(status: u16, headers: &Headers) -> Vec<u8> {
    let mut out = format!("HTTP/1.1 {status} {}\r\n", canonical_reason(status)).into_bytes();
    for (name, value) in headers.iter() {
        let value = string_to_header_bytes(value);
        if valid_header_name(name) && valid_header_value(&value) {
            write_header(&mut out, name, &value);
        }
    }
    out.extend_from_slice(b"\r\n");
    out
}

/// Chunk-size line for a chunk of `len` bytes.
pub fn chunk_header(len: usize) -> Vec<u8> {
    format!("{len:X}\r\n").into_bytes()
}

/// Final zero-length chunk, optional trailers and the terminating blank line.
pub fn last_chunk(trailers: Option<&Headers>) -> Vec<u8> {
    let mut out = b"0\r\n".to_vec();
    if let Some(t) = trailers {
        for (name, value) in t.iter() {
            let value = string_to_header_bytes(value);
            if valid_header_name(name) && valid_header_value(&value) {
                write_header(&mut out, name, &value);
            }
        }
    }
    out.extend_from_slice(b"\r\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(version: HttpVersion, status: u16, headers: &[(&str, &str)]) -> ResponseHead {
        ResponseHead {
            status,
            status_message: String::new(),
            http_version: version,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    fn ctx(version: HttpVersion) -> ResponseContext {
        ResponseContext {
            head_request: false,
            client_version: version,
            client_wants_close: false,
        }
    }

    fn text(plan: &ResponsePlan) -> String {
        String::from_utf8(plan.head.clone()).unwrap()
    }

    #[test]
    fn writes_headers_verbatim() {
        let mut h = head(
            HttpVersion::Http11,
            200,
            &[
                ("X-B", "1"),
                ("content-length", "3"),
                ("x-b", "2"),
                ("Set-Cookie", "a=1"),
                ("Set-Cookie", "b=2"),
            ],
        );
        h.status_message = "Totally OK".into();
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream);
        assert_eq!(
            text(&plan),
            "HTTP/1.1 200 Totally OK\r\nX-B: 1\r\ncontent-length: 3\r\nx-b: 2\r\nSet-Cookie: a=1\r\nSet-Cookie: b=2\r\n\r\n"
        );
        assert_eq!(plan.framing, OutFraming::Length(3));
        assert!(!plan.close_after);
    }

    #[test]
    fn fixes_content_length_for_full_bodies() {
        let h = head(
            HttpVersion::Http11,
            200,
            &[("Content-Length", "99"), ("A", "b")],
        );
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Full(5));
        assert_eq!(
            text(&plan),
            "HTTP/1.1 200 OK\r\nContent-Length: 5\r\nA: b\r\n\r\n"
        );
        let h = head(HttpVersion::H2, 201, &[("a", "b")]);
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Full(2));
        assert_eq!(
            text(&plan),
            "HTTP/1.1 201 Created\r\na: b\r\nContent-Length: 2\r\n\r\n"
        );
    }

    #[test]
    fn h2_upstream_stream_becomes_chunked_or_close_delimited() {
        let h = head(HttpVersion::H2, 200, &[("content-type", "text/plain")]);
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::Chunked);
        assert!(text(&plan).ends_with("Transfer-Encoding: chunked\r\n\r\n"));
        let plan = plan_response(&h, &ctx(HttpVersion::Http10), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::CloseDelimited);
        assert!(plan.close_after);
    }

    #[test]
    fn preserves_close_delimited_and_chunked_upstream_framing() {
        let h = head(HttpVersion::Http11, 200, &[("Server", "x")]);
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::CloseDelimited);
        assert!(plan.close_after);
        let h = head(
            HttpVersion::Http11,
            200,
            &[("Transfer-Encoding", "chunked")],
        );
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::Chunked);
        // HTTP/1.0 client: TE stripped.
        let plan = plan_response(&h, &ctx(HttpVersion::Http10), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::CloseDelimited);
        assert!(!text(&plan).contains("Transfer-Encoding"));
    }

    #[test]
    fn content_length_is_dropped_when_transfer_encoding_is_present() {
        // RFC 9112 §6.3: TE overrides CL; an intermediary must remove the received CL.
        let h = head(
            HttpVersion::Http11,
            200,
            &[("Content-Length", "3"), ("Transfer-Encoding", "chunked")],
        );
        // HTTP/1.1 client: chunked, no Content-Length.
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::Chunked);
        assert_eq!(
            text(&plan),
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n"
        );
        // HTTP/1.0 client, streamed body: purely close-delimited (no stale length).
        let plan = plan_response(&h, &ctx(HttpVersion::Http10), BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::CloseDelimited);
        assert_eq!(text(&plan), "HTTP/1.1 200 OK\r\n\r\n");
        assert!(plan.close_after);
        // HTTP/1.0 client, known body: the real length, at the original position.
        let plan = plan_response(&h, &ctx(HttpVersion::Http10), BodyKind::Full(5));
        assert_eq!(plan.framing, OutFraming::Length(5));
        assert_eq!(text(&plan), "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n");
        // HTTP/1.1 client, known body: still chunked as declared, without the length.
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Full(5));
        assert_eq!(plan.framing, OutFraming::Chunked);
        assert!(!text(&plan).contains("Content-Length"));
    }

    #[test]
    fn non_chunked_transfer_coding_stays_close_delimited() {
        // `Transfer-Encoding: gzip` (not chunked last): the body runs to the connection close,
        // the length is ignored (it used to frame the body as 3 bytes and truncate it).
        let h = head(
            HttpVersion::Http11,
            200,
            &[("Transfer-Encoding", "gzip"), ("Content-Length", "3")],
        );
        for kind in [BodyKind::Stream, BodyKind::Full(5)] {
            let plan = plan_response(&h, &ctx(HttpVersion::Http11), kind);
            assert_eq!(plan.framing, OutFraming::CloseDelimited, "{kind:?}");
            assert_eq!(
                text(&plan),
                "HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n",
                "{kind:?}"
            );
            assert!(plan.close_after);
            let plan = plan_response(&h, &ctx(HttpVersion::Http10), kind);
            assert_eq!(plan.framing, OutFraming::CloseDelimited);
            assert!(!text(&plan).contains("Content-Length"));
        }
    }

    #[test]
    fn no_body_cases() {
        let h = head(HttpVersion::Http11, 204, &[]);
        assert_eq!(
            plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Stream).framing,
            OutFraming::None
        );
        let h = head(HttpVersion::Http11, 200, &[("Content-Length", "1234")]);
        let c = ResponseContext {
            head_request: true,
            ..ctx(HttpVersion::Http11)
        };
        let plan = plan_response(&h, &c, BodyKind::Stream);
        assert_eq!(plan.framing, OutFraming::None);
        assert!(text(&plan).contains("Content-Length: 1234"));
        let h = head(
            HttpVersion::Http11,
            101,
            &[("Upgrade", "websocket"), ("Connection", "Upgrade")],
        );
        assert_eq!(
            plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::None).framing,
            OutFraming::None
        );
        let h = head(HttpVersion::H2, 200, &[]);
        assert_eq!(
            plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::None).framing,
            OutFraming::None
        );
    }

    #[test]
    fn drops_injection_attempts_and_honours_connection_close() {
        let h = head(
            HttpVersion::Http11,
            200,
            &[
                ("Good", "1"),
                ("Bad", "x\r\nInjected: y"),
                ("Bad Name", "z"),
                ("Connection", "close"),
            ],
        );
        let plan = plan_response(&h, &ctx(HttpVersion::Http11), BodyKind::Full(0));
        let t = text(&plan);
        assert!(t.contains("Good: 1") && !t.contains("Injected") && !t.contains("Bad Name"));
        assert!(plan.close_after);
    }

    #[test]
    fn chunk_helpers() {
        assert_eq!(chunk_header(255), b"FF\r\n");
        let mut t = Headers::new();
        t.append("X-Sum", "abc");
        assert_eq!(last_chunk(Some(&t)), b"0\r\nX-Sum: abc\r\n\r\n");
        assert_eq!(last_chunk(None), b"0\r\n\r\n");
        assert_eq!(
            informational(100, &Headers::new()),
            b"HTTP/1.1 100 Continue\r\n\r\n"
        );
    }
}
