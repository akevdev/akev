//! Hand-written HTTP/1.1 client codec.
//!
//! * Requests are written with the request line in origin-form (absolute-form for plain-HTTP
//!   requests through an HTTP proxy) and `head.headers` **verbatim** (casing, order,
//!   duplicates). Only framing headers are fixed up so the message is well-formed: a missing
//!   `Host` is inserted first, `Content-Length` is corrected/added for known-length bodies and
//!   `Transfer-Encoding: chunked` added for streams of unknown length. Header lines that would
//!   corrupt framing (CR/LF/NUL in values, invalid names) are dropped. Requests that arrived
//!   over HTTP/2 get their split `cookie` fields joined into one (HTTP/1.1 allows only one).
//! * Responses: interim 1xx responses are skipped (`100 Continue` drives `Expect`), the head
//!   is parsed with httparse preserving header casing/order, and the body is decoded
//!   (Content-Length, chunked with trailers, close-delimited; none for HEAD/1xx/204/304) as a
//!   true stream: bytes are read from the socket only when the consumer polls (backpressure).
//!   A TLS peer closing without `close_notify` ends a close-delimited body normally.
//! * Request bodies that stream (or are large) are written by a separate task concurrently
//!   with reading the response, so early responses (`413`, auth failures) are delivered
//!   instead of deadlocking or turning into write errors. Chunks already available are
//!   coalesced into one write; nothing is held back while waiting for the next chunk.
//! * The connection returns to the pool only after the response body was fully read, the
//!   request was fully written and both sides allow keep-alive. A connection that carried
//!   connection-oriented authentication (NTLM / Negotiate) returns pinned to the request's
//!   client connection (see [`super::ConnectionOwner`]), or not at all without one.
//! * Transfer-Encoding overrides Content-Length: a received Content-Length is removed from
//!   such responses before they are forwarded (RFC 9112 §6.3), and the connection isn't reused.

use super::error::{error, from_io, internal, protocol, timeout as timeout_error};
use super::pool::{ConnMeta, H1Conn, Pool};
use super::{BodyChunk, BodyStream, BoxIo, ConnectionOwner, RequestBody, UpstreamError, WeakOwner};
use akev_core::{
    ErrorKind, Headers, HttpVersion, RequestHead, header_value_to_string, string_to_header_bytes,
};
use bytes::{Buf, Bytes, BytesMut};
use futures::{FutureExt, Stream, StreamExt};
use std::fmt::Write as _;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, OnceLock, Weak};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::io::{
    AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufWriter, ReadHalf, WriteHalf,
};
use tokio::task::JoinHandle;
use tokio::time::{Instant, Sleep};

/// Largest response head accepted.
const MAX_HEAD_SIZE: usize = 1024 * 1024;
/// Largest chunk-size line / trailer block accepted.
const MAX_CHUNK_LINE: usize = 4096;
const MAX_TRAILERS_SIZE: usize = 64 * 1024;
/// Read granularity.
const READ_CHUNK: usize = 32 * 1024;
/// Full bodies up to this size are written together with the head in one write.
const INLINE_BODY_MAX: usize = 64 * 1024;
/// How long to wait for `100 Continue` before sending the body anyway (RFC 9110 §10.1.1).
const EXPECT_CONTINUE_WAIT: Duration = Duration::from_secs(1);
/// After a request write error, how long to keep looking for an (early) response.
const EARLY_RESPONSE_GRACE: Duration = Duration::from_secs(2);

/// A parsed response head.
#[derive(Clone, Debug)]
pub(crate) struct ParsedHead {
    pub status: u16,
    pub reason: String,
    pub version: HttpVersion,
    pub headers: Headers,
}

/// Failure reading a response head.
#[derive(Debug)]
pub(crate) enum HeadError {
    /// The peer closed the connection before sending a single byte.
    ClosedBeforeResponse,
    Io(io::Error),
    Protocol(String),
}

impl HeadError {
    /// Convert to an [`UpstreamError`], with `peer` naming the server for messages.
    pub(crate) fn into_upstream(self, peer: &str) -> UpstreamError {
        match self {
            HeadError::ClosedBeforeResponse => error(
                ErrorKind::ConnectionReset,
                format!("{peer} closed the connection without sending a response"),
                "ECONNRESET",
            ),
            HeadError::Io(e) => from_io(&e, format_args!("reading response from {peer}")),
            HeadError::Protocol(msg) => protocol(format!("invalid response from {peer}: {msg}")),
        }
    }
}

fn parser_config() -> &'static httparse::ParserConfig {
    static CONFIG: OnceLock<httparse::ParserConfig> = OnceLock::new();
    CONFIG.get_or_init(|| {
        let mut c = httparse::ParserConfig::default();
        // Real-world servers are sloppy; be lenient in what we accept (but never ambiguous
        // about framing).
        c.allow_spaces_after_header_name_in_responses(true)
            .allow_obsolete_multiline_headers_in_responses(true)
            .allow_multiple_spaces_in_response_status_delimiters(true);
        c
    })
}

/// Parse a response head from the start of `buf`. `Ok(None)` = need more bytes.
pub(crate) fn parse_head(buf: &[u8]) -> Result<Option<(ParsedHead, usize)>, HeadError> {
    let mut headers = [httparse::EMPTY_HEADER; 128];
    match try_parse_head(buf, &mut headers) {
        Err(HeadError::Protocol(msg)) if msg == "too many headers" => {
            let mut many = vec![httparse::EMPTY_HEADER; 8192];
            try_parse_head(buf, &mut many)
        }
        other => other,
    }
}

fn try_parse_head<'b>(
    buf: &'b [u8],
    headers: &mut [httparse::Header<'b>],
) -> Result<Option<(ParsedHead, usize)>, HeadError> {
    let mut resp = httparse::Response::new(headers);
    match parser_config().parse_response(&mut resp, buf) {
        Ok(httparse::Status::Complete(len)) => {
            let status = resp.code.unwrap_or(0);
            let version = match resp.version {
                Some(0) => HttpVersion::Http10,
                _ => HttpVersion::Http11,
            };
            let mut out = Headers::with_capacity(resp.headers.len());
            for h in resp.headers.iter() {
                let mut value = header_value_to_string(h.value);
                if value.contains(['\r', '\n']) {
                    // obs-fold: replace with SP (RFC 9112 §5.2).
                    value = value.replace(['\r', '\n'], " ");
                }
                out.append(h.name, value);
            }
            Ok(Some((
                ParsedHead {
                    status,
                    reason: resp.reason.unwrap_or("").to_owned(),
                    version,
                    headers: out,
                },
                len,
            )))
        }
        Ok(httparse::Status::Partial) => Ok(None),
        Err(httparse::Error::TooManyHeaders) => Err(HeadError::Protocol("too many headers".into())),
        Err(e) => Err(HeadError::Protocol(format!(
            "malformed response head ({e})"
        ))),
    }
}

/// Read one response head (which may be an interim 1xx).
pub(crate) async fn read_head<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
) -> Result<ParsedHead, HeadError> {
    loop {
        if !buf.is_empty() {
            if let Some((head, len)) = parse_head(buf)? {
                buf.advance(len);
                return Ok(head);
            }
            if buf.len() > MAX_HEAD_SIZE {
                return Err(HeadError::Protocol("response head too large".into()));
            }
        }
        buf.reserve(READ_CHUNK);
        let n = io.read_buf(buf).await.map_err(HeadError::Io)?;
        if n == 0 {
            return Err(if buf.is_empty() {
                HeadError::ClosedBeforeResponse
            } else {
                HeadError::Protocol("connection closed in the middle of the response head".into())
            });
        }
    }
}

/// Read heads until a final one, skipping interim responses (100, 102, 103...). A `101` is
/// final for upgrade requests and an error otherwise.
async fn read_final_head<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
    upgrade: bool,
) -> Result<ParsedHead, HeadError> {
    loop {
        let head = read_head(io, buf).await?;
        match head.status {
            101 if upgrade => return Ok(head),
            101 => {
                return Err(HeadError::Protocol(
                    "unexpected 101 Switching Protocols".into(),
                ));
            }
            100..=199 => continue,
            _ => return Ok(head),
        }
    }
}

/// How a request body is framed on the wire.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Framing {
    /// No body and no framing headers.
    None,
    Length(u64),
    Chunked,
}

fn declares_chunked(headers: &Headers) -> bool {
    headers
        .get_all("transfer-encoding")
        .last()
        .and_then(|v| v.rsplit(',').next())
        .is_some_and(|last| last.trim().eq_ignore_ascii_case("chunked"))
}

fn method_expects_body(method: &str) -> bool {
    ["POST", "PUT", "PATCH"]
        .iter()
        .any(|m| method.eq_ignore_ascii_case(m))
}

/// Decide request framing from the body and the declared headers.
pub(crate) fn decide_framing(head: &RequestHead, body: &RequestBody) -> Framing {
    let chunked = declares_chunked(&head.headers);
    let has_cl = head.headers.contains("content-length");
    match body {
        _ if chunked => Framing::Chunked,
        RequestBody::Empty => {
            if has_cl || method_expects_body(&head.method) {
                Framing::Length(0)
            } else {
                Framing::None
            }
        }
        RequestBody::Full(b) => {
            if b.is_empty() && !has_cl && !method_expects_body(&head.method) {
                Framing::None
            } else {
                Framing::Length(b.len() as u64)
            }
        }
        RequestBody::Stream {
            length: Some(n), ..
        } => Framing::Length(*n),
        RequestBody::Stream { length: None, .. } => match head.headers.content_length() {
            Some(n) => Framing::Length(n),
            None => Framing::Chunked,
        },
    }
}

/// Request-target form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TargetForm {
    Origin,
    /// `http://authority/path` (plain HTTP through an HTTP proxy).
    Absolute,
}

/// Facts about the encoded request that the exchange logic needs.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct HeadFlags {
    pub expect_continue: bool,
    pub close: bool,
}

fn is_token_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b)
}

fn write_target(out: &mut Vec<u8>, path: &str) {
    if path.is_empty() {
        out.push(b'/');
        return;
    }
    if !path.starts_with('/') && path != "*" {
        out.push(b'/');
    }
    for &b in path.as_bytes() {
        // Bytes that would break the request line are percent-encoded; everything else is
        // sent exactly as given.
        if b <= 0x20 || b >= 0x7F {
            let _ = write!(ByteWriter(out), "%{b:02X}");
        } else {
            out.push(b);
        }
    }
}

struct ByteWriter<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for ByteWriter<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn write_name(out: &mut Vec<u8>, name: &str, title_case: bool) {
    if !title_case {
        out.extend_from_slice(name.as_bytes());
        return;
    }
    let mut upper = true;
    for &b in name.as_bytes() {
        out.push(if upper {
            b.to_ascii_uppercase()
        } else {
            b.to_ascii_lowercase()
        });
        upper = b == b'-';
    }
}

fn write_header(out: &mut Vec<u8>, name: &str, value: &str, title_case: bool) {
    write_name(out, name, title_case);
    out.extend_from_slice(b": ");
    if value.is_ascii() {
        out.extend_from_slice(value.as_bytes());
    } else {
        out.extend_from_slice(&string_to_header_bytes(value));
    }
    out.extend_from_slice(b"\r\n");
}

/// How to encode a request head on a particular connection.
#[derive(Clone, Copy, Debug)]
pub(crate) struct EncodeOptions<'a> {
    pub form: TargetForm,
    /// Normalised `host[:port]` of the target: written as `Host` when the request has none
    /// (and no `:authority`), and used for absolute-form request targets.
    pub authority: &'a str,
    /// `Proxy-Authorization` to inject (replacing the client's) on absolute-form requests.
    pub proxy_auth: Option<&'a str>,
    /// Normalise header names to Title-Case.
    pub title_case: bool,
}

fn is_valid_value(value: &str) -> bool {
    !value.contains(['\r', '\n', '\0'])
}

/// Encode the request head.
pub(crate) fn encode_head(
    head: &RequestHead,
    framing: Framing,
    opts: &EncodeOptions<'_>,
    out: &mut Vec<u8>,
) -> Result<HeadFlags, UpstreamError> {
    let title_case = opts.title_case;
    if head.method.is_empty() || !head.method.bytes().all(is_token_byte) {
        return Err(internal(format!(
            "invalid request method {:?}",
            head.method
        )));
    }
    out.reserve(head.headers.wire_size() + head.path.len() + 64);
    out.extend_from_slice(head.method.as_bytes());
    out.push(b' ');
    match opts.form {
        TargetForm::Origin => write_target(out, &head.path),
        TargetForm::Absolute => {
            out.extend_from_slice(b"http://");
            out.extend_from_slice(opts.authority.as_bytes());
            if head.path != "*" {
                write_target(out, &head.path);
            }
        }
    }
    out.extend_from_slice(b" HTTP/1.1\r\n");

    let mut flags = HeadFlags::default();
    if !head.headers.contains("host") {
        let authority = head
            .headers
            .get(":authority")
            .filter(|a| !a.is_empty() && is_valid_value(a))
            .unwrap_or(opts.authority);
        write_header(out, "Host", authority, false);
    }
    // HTTP/2 clients may split `cookie` into several fields; HTTP/1.1 allows only one
    // (RFC 9113 §8.2.3, RFC 6265 §5.4), so they're joined at the first one's position.
    let merge_cookies = matches!(head.http_version, HttpVersion::H2 | HttpVersion::H3)
        && head.headers.get_all("cookie").nth(1).is_some();
    let mut cookie_written = false;
    let mut cl_written = false;
    let mut te_written = false;
    for (name, value) in head.headers.iter() {
        if name.starts_with(':') {
            continue;
        }
        if name.is_empty() || !name.bytes().all(is_token_byte) {
            tracing::warn!(name, "dropping request header with an invalid name");
            continue;
        }
        if !is_valid_value(value) {
            tracing::warn!(name, "dropping request header with CR/LF/NUL in its value");
            continue;
        }
        if name.eq_ignore_ascii_case("content-length") {
            if let Framing::Length(n) = framing
                && !cl_written
            {
                write_header(out, name, &n.to_string(), title_case);
                cl_written = true;
            }
            continue;
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            if framing == Framing::Chunked {
                te_written = true;
                write_header(out, name, value, title_case);
            }
            continue;
        }
        if opts.proxy_auth.is_some() && name.eq_ignore_ascii_case("proxy-authorization") {
            continue;
        }
        if merge_cookies && name.eq_ignore_ascii_case("cookie") {
            if !cookie_written {
                let joined = head
                    .headers
                    .get_all("cookie")
                    .filter(|v| is_valid_value(v) && !v.trim().is_empty())
                    .collect::<Vec<_>>()
                    .join("; ");
                write_header(out, name, &joined, title_case);
                cookie_written = true;
            }
            continue;
        }
        if name.eq_ignore_ascii_case("connection")
            && value
                .split(',')
                .any(|t| t.trim().eq_ignore_ascii_case("close"))
        {
            flags.close = true;
        }
        if name.eq_ignore_ascii_case("expect") && value.trim().eq_ignore_ascii_case("100-continue")
        {
            flags.expect_continue = true;
        }
        write_header(out, name, value, title_case);
    }
    match framing {
        Framing::Length(n) if !cl_written => {
            write_header(out, "Content-Length", &n.to_string(), false)
        }
        Framing::Chunked if !te_written => write_header(out, "Transfer-Encoding", "chunked", false),
        _ => {}
    }
    if let Some(auth) = opts.proxy_auth {
        write_header(out, "Proxy-Authorization", auth, false);
    }
    out.extend_from_slice(b"\r\n");
    Ok(flags)
}

/// Failure while writing a request body.
#[derive(Debug)]
pub(crate) enum WriteError {
    /// The request body stream itself failed (client aborted...).
    Body(UpstreamError),
    /// Writing to the upstream failed.
    Io(io::Error),
    /// The body didn't match its declared length.
    Length(String),
}

impl WriteError {
    fn into_upstream(self, peer: &str) -> UpstreamError {
        match self {
            WriteError::Body(e) => e,
            WriteError::Io(e) => from_io(&e, format_args!("sending request to {peer}")),
            WriteError::Length(msg) => {
                error(ErrorKind::ClientProtocolError, msg, "BODY_LENGTH_MISMATCH")
            }
        }
    }
}

fn encode_chunk_header(out: &mut Vec<u8>, len: usize) {
    let _ = write!(ByteWriter(out), "{len:X}\r\n");
}

fn encode_trailers(out: &mut Vec<u8>, trailers: Option<&Headers>, title_case: bool) {
    out.extend_from_slice(b"0\r\n");
    if let Some(t) = trailers {
        for (name, value) in t.iter() {
            if !name.is_empty() && name.bytes().all(is_token_byte) && is_valid_value(value) {
                write_header(out, name, value, title_case);
            }
        }
    }
    out.extend_from_slice(b"\r\n");
}

/// Append a fully-known body (Empty/Full) to the head buffer with the right framing.
fn encode_full_body(out: &mut Vec<u8>, body: &[u8], framing: Framing) {
    match framing {
        Framing::Chunked => {
            if !body.is_empty() {
                encode_chunk_header(out, body.len());
                out.extend_from_slice(body);
                out.extend_from_slice(b"\r\n");
            }
            encode_trailers(out, None, false);
        }
        Framing::Length(_) | Framing::None => out.extend_from_slice(body),
    }
}

/// Write a request body with the given framing, then flush.
async fn write_body<W: AsyncWrite + Unpin>(
    w: W,
    body: RequestBody,
    framing: Framing,
    title_case: bool,
) -> Result<(), WriteError> {
    let mut w = BufWriter::with_capacity(16 * 1024, w);
    match body {
        RequestBody::Empty => {
            if framing == Framing::Chunked {
                w.write_all(b"0\r\n\r\n").await.map_err(WriteError::Io)?;
            }
        }
        RequestBody::Full(bytes) if framing == Framing::Chunked => {
            if !bytes.is_empty() {
                let mut header = Vec::with_capacity(16);
                encode_chunk_header(&mut header, bytes.len());
                w.write_all(&header).await.map_err(WriteError::Io)?;
                w.write_all(&bytes).await.map_err(WriteError::Io)?;
                w.write_all(b"\r\n").await.map_err(WriteError::Io)?;
            }
            w.write_all(b"0\r\n\r\n").await.map_err(WriteError::Io)?;
        }
        RequestBody::Full(bytes) => w.write_all(&bytes).await.map_err(WriteError::Io)?,
        RequestBody::Stream { mut stream, .. } => {
            let declared = match framing {
                Framing::Length(n) => Some(n),
                _ => None,
            };
            let mut sent: u64 = 0;
            let mut trailers: Option<Headers> = None;
            let mut header = Vec::with_capacity(16);
            loop {
                // Coalesce chunks that are already available, but never hold data back while
                // waiting for more: a slow streaming upload (long-lived chunked requests,
                // client-streaming APIs) must reach the server as it is produced.
                let next = match stream.next().now_or_never() {
                    Some(next) => next,
                    None => {
                        w.flush().await.map_err(WriteError::Io)?;
                        stream.next().await
                    }
                };
                let Some(item) = next else { break };
                match item.map_err(WriteError::Body)? {
                    BodyChunk::Data(data) => {
                        if data.is_empty() {
                            continue;
                        }
                        sent += data.len() as u64;
                        if let Some(limit) = declared
                            && sent > limit
                        {
                            return Err(WriteError::Length(format!(
                                "request body is longer than its declared length of {limit} bytes"
                            )));
                        }
                        if framing == Framing::Chunked {
                            header.clear();
                            encode_chunk_header(&mut header, data.len());
                            w.write_all(&header).await.map_err(WriteError::Io)?;
                            w.write_all(&data).await.map_err(WriteError::Io)?;
                            w.write_all(b"\r\n").await.map_err(WriteError::Io)?;
                        } else {
                            w.write_all(&data).await.map_err(WriteError::Io)?;
                        }
                    }
                    BodyChunk::Trailers(t) => match trailers.as_mut() {
                        Some(existing) => existing.0.extend(t.0),
                        None => trailers = Some(t),
                    },
                }
            }
            if let Some(limit) = declared
                && sent < limit
            {
                return Err(WriteError::Length(format!(
                    "request body ended after {sent} of its declared {limit} bytes"
                )));
            }
            if framing == Framing::Chunked {
                let mut end = Vec::new();
                encode_trailers(&mut end, trailers.as_ref(), title_case);
                w.write_all(&end).await.map_err(WriteError::Io)?;
            } else if trailers.is_some() {
                tracing::debug!("dropping request trailers: body is not chunked");
            }
        }
    }
    w.flush().await.map_err(WriteError::Io)
}

/// How a response body is framed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseFraming {
    Empty,
    Length(u64),
    Chunked,
    Close,
}

/// RFC 9112 §6.3 body length rules.
pub(crate) fn response_framing(
    method: &str,
    status: u16,
    headers: &Headers,
) -> Result<ResponseFraming, UpstreamError> {
    if method.eq_ignore_ascii_case("HEAD")
        || (100..200).contains(&status)
        || status == 204
        || status == 304
    {
        return Ok(ResponseFraming::Empty);
    }
    if headers.contains("transfer-encoding") {
        return Ok(if declares_chunked(headers) {
            ResponseFraming::Chunked
        } else {
            ResponseFraming::Close
        });
    }
    let mut length: Option<u64> = None;
    for value in headers.get_all("content-length") {
        for part in value.split(',') {
            let n: u64 = part
                .trim()
                .parse()
                .map_err(|_| protocol(format!("invalid Content-Length {value:?}")))?;
            if length.is_some_and(|l| l != n) {
                return Err(protocol("conflicting Content-Length headers"));
            }
            length = Some(n);
        }
    }
    Ok(match length {
        Some(0) => ResponseFraming::Empty,
        Some(n) => ResponseFraming::Length(n),
        None => ResponseFraming::Close,
    })
}

/// RFC 9112 §6.3: Transfer-Encoding overrides Content-Length, and an intermediary forwarding
/// a message that carries both MUST remove the received Content-Length first (otherwise
/// downstream parsers that trust the length read a truncated or overlong body). Returns
/// whether the framing was suspicious — both headers present (a smuggling / response
/// splitting symptom), or Transfer-Encoding in an HTTP/1.0 message — in which case the
/// connection must not be reused.
pub(crate) fn normalize_framing_headers(head: &mut ParsedHead) -> bool {
    if !head.headers.contains("transfer-encoding") {
        return false;
    }
    let had_length = head.headers.remove("content-length");
    had_length || head.version == HttpVersion::Http10
}

/// Connection pinning state of an exchange.
struct Pinning<'a> {
    /// The connection is already pinned (to the request's owner).
    pinned: Option<WeakOwner>,
    /// The client connection the request came from.
    owner: Option<&'a ConnectionOwner>,
}

/// Whether a header value names an authentication scheme that authenticates the connection
/// rather than the request (NTLM, Negotiate/Kerberos). Challenges may be listed
/// comma-separated.
fn is_connection_auth(value: &str) -> bool {
    value.split(',').any(|challenge| {
        let scheme = challenge.split_whitespace().next().unwrap_or_default();
        scheme.eq_ignore_ascii_case("ntlm") || scheme.eq_ignore_ascii_case("negotiate")
    })
}

/// Whether an exchange used connection-oriented authentication: credentials sent with the
/// request, or a challenge for them in the response.
pub(crate) fn uses_connection_auth(request: &Headers, response: &Headers) -> bool {
    let request_auth = request
        .get_all("authorization")
        .chain(request.get_all("proxy-authorization"));
    let challenges = response
        .get_all("www-authenticate")
        .chain(response.get_all("proxy-authenticate"));
    request_auth.chain(challenges).any(is_connection_auth)
}

/// Whether the connection may be reused after this response.
pub(crate) fn response_keep_alive(version: HttpVersion, headers: &Headers) -> bool {
    let mut keep_alive = false;
    for token in headers.get_all("connection").flat_map(|v| v.split(',')) {
        let token = token.trim();
        if token.eq_ignore_ascii_case("close") {
            return false;
        }
        keep_alive |= token.eq_ignore_ascii_case("keep-alive");
    }
    version != HttpVersion::Http10 || keep_alive
}

/// The request writer running concurrently with the response.
enum Writer {
    Done(WriteHalf<BoxIo>),
    Running(JoinHandle<Result<(WriteHalf<BoxIo>, Instant), WriteError>>),
    Failed,
}

enum ReadSide {
    Whole(BoxIo),
    Split(ReadHalf<BoxIo>, Writer),
}

impl ReadSide {
    fn reader(&mut self) -> Pin<&mut (dyn AsyncRead + Send + Unpin)> {
        match self {
            ReadSide::Whole(io) => Pin::new(io),
            ReadSide::Split(r, _) => Pin::new(r),
        }
    }

    fn abort_writer(&self) {
        if let ReadSide::Split(_, Writer::Running(handle)) = self {
            handle.abort();
        }
    }

    /// Reassemble the connection if the request side finished cleanly.
    fn into_io(self) -> Option<BoxIo> {
        match self {
            ReadSide::Whole(io) => Some(io),
            ReadSide::Split(r, Writer::Done(w)) => Some(r.unsplit(w)),
            ReadSide::Split(r, Writer::Running(mut handle)) => {
                if handle.is_finished() {
                    if let Some(Ok(Ok((w, _)))) = (&mut handle).now_or_never() {
                        return Some(r.unsplit(w));
                    }
                } else {
                    // The server answered before consuming the whole request body: abandon
                    // the upload and the connection.
                    handle.abort();
                }
                None
            }
            ReadSide::Split(_, Writer::Failed) => None,
        }
    }
}

enum ChunkState {
    Size,
    Data(u64),
    DataEnd,
    Trailers,
}

enum BodyState {
    Length(u64),
    Chunked(ChunkState),
    Close,
    Done,
}

/// Streaming HTTP/1 response body; returns the connection to the pool once complete.
pub(crate) struct H1Body {
    state: BodyState,
    read: Option<ReadSide>,
    buf: BytesMut,
    meta: Arc<ConnMeta>,
    /// Client connection the connection is pinned to (returned to the pool with it).
    pinned: Option<WeakOwner>,
    keep_alive: bool,
    pool: Option<Weak<Pool>>,
    idle_timeout: Option<Duration>,
    idle: Option<Pin<Box<Sleep>>>,
    received: u64,
}

impl Drop for H1Body {
    fn drop(&mut self) {
        if let Some(read) = &self.read {
            read.abort_writer();
        }
    }
}

impl H1Body {
    #[allow(clippy::too_many_arguments)]
    fn new(
        framing: ResponseFraming,
        read: ReadSide,
        buf: BytesMut,
        meta: Arc<ConnMeta>,
        pinned: Option<WeakOwner>,
        keep_alive: bool,
        pool: Option<Weak<Pool>>,
        idle_timeout: Option<Duration>,
    ) -> Self {
        let state = match framing {
            ResponseFraming::Empty => BodyState::Length(0),
            ResponseFraming::Length(n) => BodyState::Length(n),
            ResponseFraming::Chunked => BodyState::Chunked(ChunkState::Size),
            ResponseFraming::Close => BodyState::Close,
        };
        let mut body = H1Body {
            state,
            read: Some(read),
            buf,
            meta,
            pinned,
            keep_alive: keep_alive && framing != ResponseFraming::Close,
            pool,
            idle_timeout,
            idle: None,
            received: 0,
        };
        if matches!(body.state, BodyState::Length(0)) {
            body.finish();
        }
        body
    }

    /// Body complete: release the connection to the pool when reusable.
    fn finish(&mut self) {
        self.state = BodyState::Done;
        self.idle = None;
        let Some(read) = self.read.take() else { return };
        // Bytes after the end of the body mean the server is confused: never reuse.
        if !self.keep_alive || !self.buf.is_empty() {
            read.abort_writer();
            return;
        }
        let Some(pool) = self.pool.as_ref().and_then(Weak::upgrade) else {
            read.abort_writer();
            return;
        };
        if let Some(io) = read.into_io() {
            pool.put_h1(H1Conn {
                io,
                buf: std::mem::take(&mut self.buf),
                meta: self.meta.clone(),
                pinned: self.pinned.take(),
            });
        }
    }

    fn fail(&mut self) {
        self.state = BodyState::Done;
        self.idle = None;
        if let Some(read) = self.read.take() {
            read.abort_writer();
        }
    }

    /// Produce the next item from buffered bytes, if possible.
    fn decode(&mut self) -> Result<Option<BodyChunk>, UpstreamError> {
        loop {
            match &mut self.state {
                BodyState::Done => return Ok(None),
                BodyState::Length(remaining) => {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    let n = (self.buf.len() as u64).min(*remaining) as usize;
                    let data = self.buf.split_to(n).freeze();
                    *remaining -= n as u64;
                    if *remaining == 0 {
                        self.finish();
                    }
                    return Ok(Some(BodyChunk::Data(data)));
                }
                BodyState::Close => {
                    if self.buf.is_empty() {
                        return Ok(None);
                    }
                    return Ok(Some(BodyChunk::Data(self.buf.split().freeze())));
                }
                BodyState::Chunked(cs) => match cs {
                    ChunkState::Size => {
                        let Some(nl) = memchr::memchr(b'\n', &self.buf) else {
                            if self.buf.len() > MAX_CHUNK_LINE {
                                return Err(protocol("chunk size line too long"));
                            }
                            return Ok(None);
                        };
                        let line = &self.buf[..nl];
                        let line = line.strip_suffix(b"\r").unwrap_or(line);
                        let size_part = line.split(|&b| b == b';').next().unwrap_or(b"");
                        let size_str = std::str::from_utf8(size_part)
                            .map_err(|_| protocol("invalid chunk size"))?
                            .trim();
                        if size_str.is_empty() || size_str.len() > 16 {
                            return Err(protocol(format!("invalid chunk size {size_str:?}")));
                        }
                        let size = u64::from_str_radix(size_str, 16)
                            .map_err(|_| protocol(format!("invalid chunk size {size_str:?}")))?;
                        self.buf.advance(nl + 1);
                        *cs = if size == 0 {
                            ChunkState::Trailers
                        } else {
                            ChunkState::Data(size)
                        };
                    }
                    ChunkState::Data(remaining) => {
                        if self.buf.is_empty() {
                            return Ok(None);
                        }
                        let n = (self.buf.len() as u64).min(*remaining) as usize;
                        let data = self.buf.split_to(n).freeze();
                        *remaining -= n as u64;
                        if *remaining == 0 {
                            *cs = ChunkState::DataEnd;
                        }
                        return Ok(Some(BodyChunk::Data(data)));
                    }
                    ChunkState::DataEnd => {
                        if self.buf.starts_with(b"\r\n") {
                            self.buf.advance(2);
                        } else if self.buf.starts_with(b"\n") {
                            self.buf.advance(1);
                        } else if self.buf.is_empty() || &self.buf[..] == b"\r" {
                            return Ok(None);
                        } else {
                            return Err(protocol("missing CRLF after chunk data"));
                        }
                        *cs = ChunkState::Size;
                    }
                    ChunkState::Trailers => {
                        if self.buf.starts_with(b"\r\n") || self.buf.starts_with(b"\n") {
                            let n = if self.buf[0] == b'\r' { 2 } else { 1 };
                            self.buf.advance(n);
                            self.finish();
                            return Ok(None);
                        }
                        let mut headers = [httparse::EMPTY_HEADER; 64];
                        match httparse::parse_headers(&self.buf, &mut headers) {
                            Ok(httparse::Status::Complete((len, parsed))) => {
                                let trailers: Headers = parsed
                                    .iter()
                                    .map(|h| (h.name.to_owned(), header_value_to_string(h.value)))
                                    .collect();
                                self.buf.advance(len);
                                self.finish();
                                return Ok(
                                    (!trailers.is_empty()).then_some(BodyChunk::Trailers(trailers))
                                );
                            }
                            Ok(httparse::Status::Partial) => {
                                if self.buf.len() > MAX_TRAILERS_SIZE {
                                    return Err(protocol("response trailers too large"));
                                }
                                return Ok(None);
                            }
                            Err(e) => {
                                return Err(protocol(format!("malformed response trailers ({e})")));
                            }
                        }
                    }
                },
            }
        }
    }

    fn reset_idle(&mut self) {
        if let (Some(dur), Some(sleep)) = (self.idle_timeout, self.idle.as_mut()) {
            sleep.as_mut().reset(Instant::now() + dur);
        }
    }
}

impl Stream for H1Body {
    type Item = Result<BodyChunk, UpstreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.decode() {
                Ok(Some(item)) => {
                    if let BodyChunk::Data(d) = &item {
                        this.received += d.len() as u64;
                    }
                    this.reset_idle();
                    return Poll::Ready(Some(Ok(item)));
                }
                Ok(None) => {}
                Err(e) => {
                    this.fail();
                    return Poll::Ready(Some(Err(e)));
                }
            }
            if matches!(this.state, BodyState::Done) {
                return Poll::Ready(None);
            }
            let Some(read) = this.read.as_mut() else {
                this.state = BodyState::Done;
                return Poll::Ready(None);
            };
            this.buf.reserve(READ_CHUNK);
            match tokio_util::io::poll_read_buf(read.reader(), cx, &mut this.buf) {
                Poll::Pending => {
                    if let Some(dur) = this.idle_timeout {
                        let sleep = this
                            .idle
                            .get_or_insert_with(|| Box::pin(tokio::time::sleep(dur)));
                        if sleep.as_mut().poll(cx).is_ready() {
                            this.fail();
                            return Poll::Ready(Some(Err(timeout_error(
                                format_args!(
                                    "waiting for response body data from {}",
                                    this.meta.peer
                                ),
                                dur,
                            ))));
                        }
                    }
                    return Poll::Pending;
                }
                Poll::Ready(Ok(0)) => {
                    if matches!(this.state, BodyState::Close) {
                        this.keep_alive = false;
                        this.finish();
                        return Poll::Ready(None);
                    }
                    let received = this.received;
                    this.fail();
                    return Poll::Ready(Some(Err(error(
                        ErrorKind::ConnectionReset,
                        format!(
                            "{} closed the connection before the response body was complete ({received} bytes received)",
                            this.meta.peer
                        ),
                        "ECONNRESET",
                    ))));
                }
                Poll::Ready(Ok(_)) => continue,
                // TLS peers that close without `close_notify` surface as UnexpectedEof. For a
                // close-delimited body that *is* the end of the body (browsers and Node accept
                // it); for every other framing it is a truncation, reported below.
                Poll::Ready(Err(e))
                    if e.kind() == io::ErrorKind::UnexpectedEof
                        && matches!(this.state, BodyState::Close) =>
                {
                    this.keep_alive = false;
                    this.finish();
                    return Poll::Ready(None);
                }
                Poll::Ready(Err(e)) => {
                    this.fail();
                    return Poll::Ready(Some(Err(from_io(
                        &e,
                        format_args!("reading response body from {}", this.meta.peer),
                    ))));
                }
            }
        }
    }
}

/// Everything the exchange needs to send one request on a connection.
pub(crate) struct H1Request<'a> {
    pub head: &'a RequestHead,
    /// Normalised target authority (see [`EncodeOptions::authority`]).
    pub authority: &'a str,
    pub body: RequestBody,
    pub title_case: bool,
    /// Response-head timeout (armed once the request is fully written).
    pub head_timeout: Option<Duration>,
    /// Idle timeout between response body reads.
    pub idle_timeout: Option<Duration>,
    /// Start of the `send` call (for `request_sent_ms`).
    pub started: Instant,
    /// Upgrade request: a `101` is a final response.
    pub upgrade: bool,
    /// The client connection the request came from (connection pinning).
    pub owner: Option<&'a ConnectionOwner>,
}

/// Result of a successful exchange.
pub(crate) struct H1Outcome {
    pub head: ParsedHead,
    /// Response body (absent for a `101` upgrade).
    pub body: Option<BodyStream>,
    /// Upgraded stream + bytes already read past the head.
    pub upgraded: Option<(BoxIo, Bytes)>,
    pub request_sent_ms: Option<f64>,
}

/// Failure of an exchange.
pub(crate) enum H1Error {
    /// Nothing was received and the body wasn't consumed: safe to retry on a fresh
    /// connection if this one was reused (stale keep-alive connection).
    Retryable {
        error: UpstreamError,
        body: RequestBody,
    },
    Fatal(UpstreamError),
}

enum ContinueOutcome {
    Continue,
    Final(ParsedHead),
    TimedOut,
}

async fn await_continue<R: AsyncRead + Unpin + ?Sized>(
    io: &mut R,
    buf: &mut BytesMut,
    upgrade: bool,
) -> Result<ContinueOutcome, HeadError> {
    let wait = async {
        loop {
            let head = read_head(io, buf).await?;
            match head.status {
                100 => return Ok(ContinueOutcome::Continue),
                101 if upgrade => return Ok(ContinueOutcome::Final(head)),
                102..=199 => continue,
                _ => return Ok(ContinueOutcome::Final(head)),
            }
        }
    };
    match tokio::time::timeout(EXPECT_CONTINUE_WAIT, wait).await {
        Ok(r) => r,
        Err(_) => Ok(ContinueOutcome::TimedOut),
    }
}

fn is_reset(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::UnexpectedEof
    )
}

async fn write_all_flush<W: AsyncWrite + Unpin + ?Sized>(
    w: &mut W,
    bytes: &[u8],
) -> io::Result<()> {
    w.write_all(bytes).await?;
    w.flush().await
}

/// Map a head-read failure, flagging stale-connection symptoms as retryable.
fn head_failure(e: HeadError, buf_empty: bool, peer: &str, body: RequestBody) -> H1Error {
    match e {
        HeadError::ClosedBeforeResponse => H1Error::Retryable {
            error: e.into_upstream(peer),
            body,
        },
        HeadError::Io(io) if buf_empty && is_reset(&io) => H1Error::Retryable {
            error: from_io(&io, format_args!("reading response from {peer}")),
            body,
        },
        other => H1Error::Fatal(other.into_upstream(peer)),
    }
}

/// Send one request on `conn` and read the response head.
pub(crate) async fn send(
    conn: H1Conn,
    req: H1Request<'_>,
    pool: Option<Weak<Pool>>,
) -> Result<H1Outcome, H1Error> {
    let H1Conn {
        mut io,
        mut buf,
        meta,
        pinned,
    } = conn;
    let peer = meta.peer.clone();
    let framing = decide_framing(req.head, &req.body);
    let form = if meta.absolute_form {
        TargetForm::Absolute
    } else {
        TargetForm::Origin
    };
    let mut head_bytes = Vec::new();
    let opts = EncodeOptions {
        form,
        authority: req.authority,
        proxy_auth: meta.proxy_auth.as_deref(),
        title_case: req.title_case,
    };
    let flags = encode_head(req.head, framing, &opts, &mut head_bytes).map_err(H1Error::Fatal)?;
    let body_len = match &req.body {
        RequestBody::Empty => Some(0),
        RequestBody::Full(b) => Some(b.len()),
        RequestBody::Stream { .. } => None,
    };
    let keep_alive_request = !flags.close;
    let mut body = req.body;
    let finish = |read: ReadSide,
                  buf: BytesMut,
                  head: ParsedHead,
                  keep_alive: bool,
                  sent: Option<Instant>| {
        finish_response(
            meta.clone(),
            read,
            buf,
            head,
            req.head,
            req.upgrade,
            req.idle_timeout,
            keep_alive,
            sent,
            req.started,
            pool.clone(),
            Pinning {
                pinned: pinned.clone(),
                owner: req.owner,
            },
        )
        .map_err(H1Error::Fatal)
    };

    if flags.expect_continue && body_len != Some(0) {
        // Send the head alone and give the server a chance to reject the request before the
        // body is transmitted.
        if let Err(e) = write_all_flush(&mut io, &head_bytes).await {
            return Err(H1Error::Retryable {
                error: from_io(&e, format_args!("sending request to {peer}")),
                body,
            });
        }
        head_bytes.clear();
        match await_continue(&mut io, &mut buf, req.upgrade).await {
            Ok(ContinueOutcome::Final(head)) => {
                // Answered without the body. The server's view of the framing is now
                // undefined, so the connection is not reused.
                return finish(ReadSide::Whole(io), buf, head, false, None);
            }
            Ok(ContinueOutcome::Continue | ContinueOutcome::TimedOut) => {}
            Err(e) => {
                let empty = buf.is_empty();
                return Err(head_failure(e, empty, &peer, body));
            }
        }
    }

    if matches!(body_len, Some(n) if n <= INLINE_BODY_MAX) {
        // Head (unless already sent) + the whole body in one write.
        if let RequestBody::Full(bytes) = &body {
            encode_full_body(&mut head_bytes, bytes, framing);
        } else {
            encode_full_body(&mut head_bytes, &[], framing);
        }
        let write_result = write_all_flush(&mut io, &head_bytes).await;
        drop(head_bytes);
        if let Err(e) = write_result {
            // The server may have answered (and closed) without reading the request; prefer
            // its response over the write error.
            let early = tokio::time::timeout(
                EARLY_RESPONSE_GRACE,
                read_final_head(&mut io, &mut buf, req.upgrade),
            )
            .await;
            return match early {
                Ok(Ok(head)) => finish(ReadSide::Whole(io), buf, head, false, None),
                _ if buf.is_empty() => Err(H1Error::Retryable {
                    error: from_io(&e, format_args!("sending request to {peer}")),
                    body,
                }),
                _ => Err(H1Error::Fatal(from_io(
                    &e,
                    format_args!("sending request to {peer}"),
                ))),
            };
        }
        let sent = Instant::now();
        let read = read_final_head(&mut io, &mut buf, req.upgrade);
        let head = match req.head_timeout {
            Some(t) => match tokio::time::timeout(t, read).await {
                Ok(r) => r,
                Err(_) => {
                    return Err(H1Error::Fatal(timeout_error(
                        format_args!("waiting for a response from {peer}"),
                        t,
                    )));
                }
            },
            None => read.await,
        };
        return match head {
            Ok(head) => finish(
                ReadSide::Whole(io),
                buf,
                head,
                keep_alive_request,
                Some(sent),
            ),
            Err(e) => {
                let empty = buf.is_empty();
                Err(head_failure(e, empty, &peer, body))
            }
        };
    }

    // Streaming / large body: write the head, then the body from a separate task while the
    // response is read concurrently.
    if !head_bytes.is_empty()
        && let Err(e) = write_all_flush(&mut io, &head_bytes).await
    {
        return Err(H1Error::Retryable {
            error: from_io(&e, format_args!("sending request to {peer}")),
            body,
        });
    }
    drop(head_bytes);
    let (mut rh, wh) = tokio::io::split(io);
    let body = std::mem::replace(&mut body, RequestBody::Empty);
    let title_case = req.title_case;
    let mut handle = Some(tokio::spawn(async move {
        let mut wh = wh;
        write_body(&mut wh, body, framing, title_case).await?;
        Ok::<_, WriteError>((wh, Instant::now()))
    }));
    let mut writer: Option<Writer> = None;
    let mut write_error: Option<UpstreamError> = None;
    let mut sent: Option<Instant> = None;
    let mut deadline: Option<(Instant, Duration)> = None;
    let head = {
        let read = read_final_head(&mut rh, &mut buf, req.upgrade);
        tokio::pin!(read);
        loop {
            let timer = async {
                match deadline {
                    Some((at, _)) => tokio::time::sleep_until(at).await,
                    None => std::future::pending().await,
                }
            };
            let join = async {
                match handle.as_mut() {
                    Some(h) => h.await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                biased;
                // Writer first: when both are ready, record the request completion too.
                joined = join => {
                    handle = None;
                    match joined {
                        Ok(Ok((wh, at))) => {
                            sent = Some(at);
                            writer = Some(Writer::Done(wh));
                            deadline = req.head_timeout.map(|t| (Instant::now() + t, t));
                        }
                        Ok(Err(WriteError::Io(e))) => {
                            // The server may have answered early and closed; look for it briefly.
                            writer = Some(Writer::Failed);
                            write_error = Some(from_io(&e, format_args!("sending request body to {peer}")));
                            deadline = Some((Instant::now() + EARLY_RESPONSE_GRACE, EARLY_RESPONSE_GRACE));
                        }
                        Ok(Err(other)) => return Err(H1Error::Fatal(other.into_upstream(&peer))),
                        Err(join) => {
                            return Err(H1Error::Fatal(internal(format!("request body writer failed: {join}"))));
                        }
                    }
                }
                res = &mut read => break res,
                _ = timer => {
                    return Err(H1Error::Fatal(match write_error.take() {
                        Some(e) => e,
                        None => timeout_error(
                            format_args!("waiting for a response from {peer}"),
                            deadline.map(|(_, d)| d).unwrap_or_default(),
                        ),
                    }));
                }
            }
        }
    };
    let head = match head {
        Ok(h) => h,
        Err(e) => {
            if let Some(h) = handle {
                h.abort();
            }
            return Err(H1Error::Fatal(
                write_error.unwrap_or_else(|| e.into_upstream(&peer)),
            ));
        }
    };
    // The writer may have finished between the last poll and the head arriving.
    if writer.is_none()
        && handle.as_ref().is_some_and(JoinHandle::is_finished)
        && let Some(Some(joined)) = handle.take().map(FutureExt::now_or_never)
    {
        writer = Some(match joined {
            Ok(Ok((wh, at))) => {
                sent = Some(at);
                Writer::Done(wh)
            }
            _ => Writer::Failed,
        });
    }
    let writer = match (writer, handle) {
        (Some(w), _) => w,
        (None, Some(h)) => Writer::Running(h),
        (None, None) => Writer::Failed,
    };
    let keep_alive = keep_alive_request && !matches!(writer, Writer::Failed);
    finish(ReadSide::Split(rh, writer), buf, head, keep_alive, sent)
}

#[allow(clippy::too_many_arguments)]
fn finish_response(
    meta: Arc<ConnMeta>,
    read: ReadSide,
    buf: BytesMut,
    head: ParsedHead,
    request: &RequestHead,
    upgrade: bool,
    idle_timeout: Option<Duration>,
    keep_alive_request: bool,
    sent: Option<Instant>,
    started: Instant,
    pool: Option<Weak<Pool>>,
    pinning: Pinning<'_>,
) -> Result<H1Outcome, UpstreamError> {
    let request_sent_ms =
        sent.map(|at| at.saturating_duration_since(started).as_secs_f64() * 1000.0);
    if upgrade && head.status == 101 {
        let io = read
            .into_io()
            .ok_or_else(|| internal("upgraded connection unavailable"))?;
        return Ok(H1Outcome {
            head,
            body: None,
            upgraded: Some((io, buf.freeze())),
            request_sent_ms,
        });
    }
    let framing = match response_framing(&request.method, head.status, &head.headers) {
        Ok(f) => f,
        Err(e) => {
            read.abort_writer();
            return Err(e);
        }
    };
    let mut head = head;
    let suspicious_framing = normalize_framing_headers(&mut head);
    let mut keep_alive = keep_alive_request
        && !suspicious_framing
        && response_keep_alive(head.version, &head.headers);
    let pinned = match pinning.pinned {
        Some(owner) => Some(owner),
        None if uses_connection_auth(&request.headers, &head.headers) => match pinning.owner {
            Some(owner) => Some(owner.downgrade()),
            // Nobody to pin the authenticated connection to: never reuse it.
            None => {
                keep_alive = false;
                None
            }
        },
        None => None,
    };
    let body = H1Body::new(
        framing,
        read,
        buf,
        meta,
        pinned,
        keep_alive,
        pool,
        idle_timeout,
    );
    Ok(H1Outcome {
        head,
        body: Some(Box::pin(body)),
        upgraded: None,
        request_sent_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upstream::pool::PoolKey;
    use tokio::io::AsyncWriteExt;

    fn req(method: &str, headers: &[(&str, &str)]) -> RequestHead {
        RequestHead {
            method: method.into(),
            url: "http://example.test:8080/p?q=1".into(),
            scheme: "http".into(),
            host: "example.test".into(),
            port: 8080,
            path: "/p?q=1".into(),
            http_version: HttpVersion::Http11,
            headers: Headers(
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        }
    }

    fn encode(
        head: &RequestHead,
        framing: Framing,
        form: TargetForm,
        auth: Option<&str>,
    ) -> (String, HeadFlags) {
        let authority = head.authority();
        let opts = EncodeOptions {
            form,
            authority: &authority,
            proxy_auth: auth,
            title_case: false,
        };
        let mut out = Vec::new();
        let flags = encode_head(head, framing, &opts, &mut out).unwrap();
        (String::from_utf8(out).unwrap(), flags)
    }

    #[test]
    fn cookies_from_http2_clients_are_merged() {
        let mut h = req(
            "GET",
            &[
                ("cookie", "a=1"),
                ("accept", "*/*"),
                ("cookie", "b=2"),
                ("cookie", "c=3"),
            ],
        );
        h.http_version = HttpVersion::H2;
        let (text, _) = encode(&h, Framing::None, TargetForm::Origin, None);
        assert_eq!(
            text,
            "GET /p?q=1 HTTP/1.1\r\nHost: example.test:8080\r\ncookie: a=1; b=2; c=3\r\naccept: */*\r\n\r\n"
        );
        // HTTP/1.1 requests are forwarded verbatim, even with (non-conforming) duplicates.
        h.http_version = HttpVersion::Http11;
        let (text, _) = encode(&h, Framing::None, TargetForm::Origin, None);
        assert_eq!(text.matches("cookie: ").count(), 3);
        // A single cookie header from an HTTP/2 client is untouched.
        let mut h = req("GET", &[("cookie", "only=1")]);
        h.http_version = HttpVersion::H2;
        assert!(
            encode(&h, Framing::None, TargetForm::Origin, None)
                .0
                .contains("\r\ncookie: only=1\r\n")
        );
    }

    #[test]
    fn host_falls_back_to_the_normalised_authority() {
        let h = req("GET", &[]);
        let opts = EncodeOptions {
            form: TargetForm::Absolute,
            authority: "example.test",
            proxy_auth: None,
            title_case: true,
        };
        let mut out = Vec::new();
        encode_head(&h, Framing::None, &opts, &mut out).unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            "GET http://example.test/p?q=1 HTTP/1.1\r\nHost: example.test\r\n\r\n"
        );
        // An unusable :authority is ignored.
        let h = req("GET", &[(":authority", "")]);
        assert!(
            encode(&h, Framing::None, TargetForm::Origin, None)
                .0
                .contains("Host: example.test:8080\r\n")
        );
    }

    fn empty_stream() -> BodyStream {
        Box::pin(futures::stream::empty())
    }

    #[test]
    fn framing_decisions() {
        let full = |b: &'static [u8]| RequestBody::Full(Bytes::from_static(b));
        assert_eq!(
            decide_framing(&req("GET", &[]), &RequestBody::Empty),
            Framing::None
        );
        assert_eq!(
            decide_framing(&req("POST", &[]), &RequestBody::Empty),
            Framing::Length(0)
        );
        assert_eq!(
            decide_framing(&req("GET", &[("Content-Length", "9")]), &RequestBody::Empty),
            Framing::Length(0)
        );
        assert_eq!(decide_framing(&req("GET", &[]), &full(b"")), Framing::None);
        assert_eq!(
            decide_framing(&req("GET", &[]), &full(b"abc")),
            Framing::Length(3)
        );
        assert_eq!(
            decide_framing(
                &req("POST", &[("Transfer-Encoding", "gzip, chunked")]),
                &full(b"abc")
            ),
            Framing::Chunked
        );
        assert_eq!(
            decide_framing(
                &req("POST", &[("Transfer-Encoding", "gzip")]),
                &full(b"abc")
            ),
            Framing::Length(3)
        );
        let stream = |length| RequestBody::Stream {
            stream: empty_stream(),
            length,
        };
        assert_eq!(
            decide_framing(&req("POST", &[]), &stream(Some(7))),
            Framing::Length(7)
        );
        assert_eq!(
            decide_framing(&req("POST", &[]), &stream(None)),
            Framing::Chunked
        );
        assert_eq!(
            decide_framing(&req("POST", &[("content-length", "12")]), &stream(None)),
            Framing::Length(12)
        );
    }

    #[test]
    fn encodes_heads() {
        // Absolute-form with injected proxy credentials replacing the client's.
        let h = req(
            "GET",
            &[
                ("Host", "example.test:8080"),
                ("Proxy-Authorization", "client"),
                (":authority", "x"),
            ],
        );
        let (text, _) = encode(&h, Framing::None, TargetForm::Absolute, Some("Basic abc"));
        assert_eq!(
            text,
            "GET http://example.test:8080/p?q=1 HTTP/1.1\r\nHost: example.test:8080\r\nProxy-Authorization: Basic abc\r\n\r\n"
        );
        // :authority supplies a missing Host (HTTP/2 downstream).
        let h = req("GET", &[(":authority", "h2.example"), ("accept", "*/*")]);
        let (text, _) = encode(&h, Framing::None, TargetForm::Origin, None);
        assert_eq!(
            text,
            "GET /p?q=1 HTTP/1.1\r\nHost: h2.example\r\naccept: */*\r\n\r\n"
        );
        // Chunked: TE kept verbatim, CL dropped; flags detected.
        let h = req(
            "POST",
            &[
                ("Host", "h"),
                ("Content-Length", "3"),
                ("transfer-encoding", "chunked"),
                ("Expect", "100-Continue"),
                ("Connection", "keep-alive, close"),
            ],
        );
        let (text, flags) = encode(&h, Framing::Chunked, TargetForm::Origin, None);
        assert!(!text.contains("Content-Length"));
        assert!(text.contains("transfer-encoding: chunked\r\n"));
        assert!(flags.expect_continue && flags.close);
        // Request-target sanitisation and OPTIONS *.
        let mut h = req("GET", &[("Host", "h")]);
        h.path = "/a b/\u{e9}?x=\"1\"".into();
        let (text, _) = encode(&h, Framing::None, TargetForm::Origin, None);
        assert!(
            text.starts_with("GET /a%20b/%C3%A9?x=\"1\" HTTP/1.1\r\n"),
            "{text}"
        );
        h.method = "OPTIONS".into();
        h.path = "*".into();
        assert!(
            encode(&h, Framing::None, TargetForm::Origin, None)
                .0
                .starts_with("OPTIONS * HTTP/1.1\r\n")
        );
        assert!(
            encode(&h, Framing::None, TargetForm::Absolute, None)
                .0
                .starts_with("OPTIONS http://example.test:8080 HTTP/1.1\r\n")
        );
        h.path = "?only=query".into();
        assert!(
            encode(&h, Framing::None, TargetForm::Origin, None)
                .0
                .starts_with("OPTIONS /?only=query HTTP/1.1\r\n")
        );
        // IPv6 authority gets brackets.
        let mut h = req("GET", &[]);
        h.host = "::1".into();
        assert!(
            encode(&h, Framing::None, TargetForm::Origin, None)
                .0
                .contains("Host: [::1]:8080\r\n")
        );
        // Invalid methods are rejected.
        let mut out = Vec::new();
        let opts = EncodeOptions {
            form: TargetForm::Origin,
            authority: "h",
            proxy_auth: None,
            title_case: false,
        };
        assert!(encode_head(&req("GE T", &[]), Framing::None, &opts, &mut out).is_err());
    }

    #[test]
    fn full_body_encoding() {
        let mut out = Vec::new();
        encode_full_body(&mut out, b"hello", Framing::Chunked);
        assert_eq!(out, b"5\r\nhello\r\n0\r\n\r\n");
        out.clear();
        encode_full_body(&mut out, b"", Framing::Chunked);
        assert_eq!(out, b"0\r\n\r\n");
        out.clear();
        encode_full_body(&mut out, b"raw", Framing::Length(3));
        assert_eq!(out, b"raw");
    }

    #[test]
    fn response_framing_rules() {
        let h = |pairs: &[(&str, &str)]| {
            Headers(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        assert_eq!(
            response_framing("HEAD", 200, &h(&[("Content-Length", "10")])).unwrap(),
            ResponseFraming::Empty
        );
        assert_eq!(
            response_framing("GET", 204, &h(&[])).unwrap(),
            ResponseFraming::Empty
        );
        assert_eq!(
            response_framing("GET", 304, &h(&[("Content-Length", "10")])).unwrap(),
            ResponseFraming::Empty
        );
        assert_eq!(
            response_framing(
                "GET",
                200,
                &h(&[("Content-Length", "10"), ("Transfer-Encoding", "chunked")])
            )
            .unwrap(),
            ResponseFraming::Chunked
        );
        assert_eq!(
            response_framing("GET", 200, &h(&[("Transfer-Encoding", "gzip")])).unwrap(),
            ResponseFraming::Close
        );
        assert_eq!(
            response_framing("GET", 200, &h(&[("Content-Length", "5, 5")])).unwrap(),
            ResponseFraming::Length(5)
        );
        assert_eq!(
            response_framing("GET", 200, &h(&[("Content-Length", "0")])).unwrap(),
            ResponseFraming::Empty
        );
        assert_eq!(
            response_framing("GET", 200, &h(&[])).unwrap(),
            ResponseFraming::Close
        );
        assert!(
            response_framing(
                "GET",
                200,
                &h(&[("Content-Length", "5"), ("content-length", "6")])
            )
            .is_err()
        );
        assert!(response_framing("GET", 200, &h(&[("Content-Length", "-1")])).is_err());
    }

    #[test]
    fn transfer_encoding_responses_lose_their_content_length() {
        let parsed = |version, headers: &[(&str, &str)]| ParsedHead {
            status: 200,
            reason: "OK".into(),
            version,
            headers: Headers(
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        };
        // Both: CL removed (RFC 9112 §6.3), connection not reused.
        let mut h = parsed(
            HttpVersion::Http11,
            &[
                ("Content-Length", "3"),
                ("Transfer-Encoding", "chunked"),
                ("X", "y"),
            ],
        );
        assert!(normalize_framing_headers(&mut h));
        assert_eq!(
            h.headers.0,
            vec![
                ("Transfer-Encoding".to_string(), "chunked".to_string()),
                ("X".to_string(), "y".to_string())
            ]
        );
        // Plain chunked: untouched and reusable.
        let mut h = parsed(HttpVersion::Http11, &[("Transfer-Encoding", "chunked")]);
        assert!(!normalize_framing_headers(&mut h));
        assert_eq!(h.headers.0.len(), 1);
        // Transfer-Encoding in an HTTP/1.0 message: faulty framing, don't reuse.
        let mut h = parsed(HttpVersion::Http10, &[("Transfer-Encoding", "chunked")]);
        assert!(normalize_framing_headers(&mut h));
        // Content-Length only: untouched.
        let mut h = parsed(HttpVersion::Http11, &[("Content-Length", "3")]);
        assert!(!normalize_framing_headers(&mut h));
        assert_eq!(h.headers.content_length(), Some(3));
    }

    #[test]
    fn connection_oriented_auth_is_detected() {
        let h = |pairs: &[(&str, &str)]| {
            Headers(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        let none = Headers::new();
        assert!(uses_connection_auth(
            &h(&[("Authorization", "NTLM TlRMTVNTUAABAAAA")]),
            &none
        ));
        assert!(uses_connection_auth(
            &h(&[("authorization", "Negotiate YIIGhgYGKwYBBQUC")]),
            &none
        ));
        assert!(uses_connection_auth(
            &none,
            &h(&[
                ("WWW-Authenticate", "Negotiate"),
                ("WWW-Authenticate", "NTLM")
            ])
        ));
        assert!(uses_connection_auth(
            &none,
            &h(&[("WWW-Authenticate", "Basic realm=\"x\", NTLM")])
        ));
        assert!(uses_connection_auth(
            &none,
            &h(&[("Proxy-Authenticate", "NTLM")])
        ));
        assert!(!uses_connection_auth(
            &h(&[("Authorization", "Basic dXNlcjpwYXNz")]),
            &h(&[("WWW-Authenticate", "Bearer realm=\"api\"")])
        ));
        assert!(!uses_connection_auth(&none, &none));
    }

    #[test]
    fn keep_alive_rules() {
        let h = |pairs: &[(&str, &str)]| {
            Headers(
                pairs
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            )
        };
        assert!(response_keep_alive(HttpVersion::Http11, &h(&[])));
        assert!(!response_keep_alive(
            HttpVersion::Http11,
            &h(&[("Connection", "Upgrade, Close")])
        ));
        assert!(!response_keep_alive(HttpVersion::Http10, &h(&[])));
        assert!(response_keep_alive(
            HttpVersion::Http10,
            &h(&[("Connection", "Keep-Alive")])
        ));
    }

    #[test]
    fn lenient_head_parsing() {
        let (head, len) = parse_head(
            b"HTTP/1.0 200 Fine Thanks\r\nX-Folded: a\r\n b\r\nServer : odd\r\n\r\nbody",
        )
        .unwrap()
        .unwrap();
        assert_eq!(head.version, HttpVersion::Http10);
        assert_eq!(head.reason, "Fine Thanks");
        assert_eq!(
            head.headers
                .get("x-folded")
                .map(|v| v.split_whitespace().collect::<Vec<_>>()),
            Some(vec!["a", "b"])
        );
        assert_eq!(head.headers.get("server"), Some("odd"));
        assert_eq!(len, 59);
        assert!(
            parse_head(b"HTTP/1.1 200 OK\r\nPartial: yes\r\n")
                .unwrap()
                .is_none()
        );
        assert!(matches!(
            parse_head(b"NOPE\r\n\r\n"),
            Err(HeadError::Protocol(_))
        ));
        // More headers than the stack buffer holds.
        let mut many = b"HTTP/1.1 200 OK\r\n".to_vec();
        for i in 0..300 {
            many.extend_from_slice(format!("X-H{i}: {i}\r\n").as_bytes());
        }
        many.extend_from_slice(b"\r\n");
        let (head, _) = parse_head(&many).unwrap().unwrap();
        assert_eq!(head.headers.len(), 300);
    }

    fn meta() -> Arc<ConnMeta> {
        Arc::new(ConnMeta {
            key: PoolKey {
                tls: false,
                host: "t".into(),
                port: 1,
                proxy: None,
                insecure: false,
                client_cert: None,
                h1_alpn: false,
                browser: None,
                restricted: false,
            },
            generation: 0,
            info: Default::default(),
            peer: "t:1".into(),
            absolute_form: false,
            proxy_auth: None,
        })
    }

    async fn decode_all(
        wire: &'static [u8],
        framing: ResponseFraming,
    ) -> Vec<Result<BodyChunk, UpstreamError>> {
        let (client, mut server) = tokio::io::duplex(64);
        tokio::spawn(async move {
            // Dribble the bytes to exercise partial parsing.
            for chunk in wire.chunks(3) {
                server.write_all(chunk).await.unwrap();
                tokio::task::yield_now().await;
            }
        });
        let body = H1Body::new(
            framing,
            ReadSide::Whole(Box::new(client)),
            BytesMut::new(),
            meta(),
            None,
            true,
            None,
            None,
        );
        Box::pin(body).collect().await
    }

    fn data_of(items: &[Result<BodyChunk, UpstreamError>]) -> Vec<u8> {
        items
            .iter()
            .filter_map(|i| match i {
                Ok(BodyChunk::Data(d)) => Some(d.to_vec()),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[tokio::test]
    async fn chunked_decoder_edge_cases() {
        // Extensions, bare LF line endings, uppercase hex, trailers.
        let items = decode_all(
            b"5;name=val\r\nhello\nA\n0123456789\r\n0\r\nX-T: 1\r\n\r\n",
            ResponseFraming::Chunked,
        )
        .await;
        assert_eq!(data_of(&items), b"hello0123456789");
        assert!(
            matches!(items.last(), Some(Ok(BodyChunk::Trailers(t))) if t.get("x-t") == Some("1"))
        );

        let items = decode_all(b"zz\r\n", ResponseFraming::Chunked).await;
        assert!(matches!(items.last(), Some(Err(e)) if e.kind == ErrorKind::UpstreamProtocolError));

        let items = decode_all(b"3\r\nabcXX0\r\n\r\n", ResponseFraming::Chunked).await;
        assert!(matches!(items.last(), Some(Err(e)) if e.kind == ErrorKind::UpstreamProtocolError));

        let items = decode_all(b"FFFFFFFFFFFFFFFFFF\r\n", ResponseFraming::Chunked).await;
        assert!(matches!(items.last(), Some(Err(e)) if e.kind == ErrorKind::UpstreamProtocolError));

        // EOF mid-body is a reset.
        let items = decode_all(b"10\r\nonly-part", ResponseFraming::Chunked).await;
        assert_eq!(data_of(&items), b"only-part");
        assert!(matches!(items.last(), Some(Err(e)) if e.kind == ErrorKind::ConnectionReset));

        // Close-delimited and length bodies.
        let items = decode_all(b"until close", ResponseFraming::Close).await;
        assert_eq!(data_of(&items), b"until close");
        assert!(items.iter().all(Result::is_ok));
        let items = decode_all(b"exactly", ResponseFraming::Length(7)).await;
        assert_eq!(data_of(&items), b"exactly");
    }
}
