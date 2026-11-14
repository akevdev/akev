//! HTTP/2 server side (h2 crate) for clients speaking h2 over TLS (ALPN) or h2c with prior
//! knowledge. Every stream is an exchange; `CONNECT` streams become tunnels that are sniffed
//! again like a fresh connection.

use crate::body::ClientBody;
use crate::conn::{ConnCtx, client_error};
use crate::errors::exchange_error;
use crate::io::ClientIo;
use crate::local::{MAGIC_HOST, is_local_target};
use crate::pipeline::{self, ExchangeEnv, ExchangeRequest};
use crate::responder::{AbortKind, BodyKind, Responder};
use crate::shared::Shared;
use crate::target::{RequestTarget, build_url, parse_authority, resolve_target};
use crate::upstream::{BodyChunk, BodyStream, UpstreamError};
use akev_core::*;
use bytes::{Buf, Bytes, BytesMut};
use h2::server::SendResponse;
use h2::{Reason, RecvStream, SendStream};
use std::future::poll_fn;
use std::io;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

fn h2_io_error(e: h2::Error) -> io::Error {
    if e.is_io() {
        e.into_io()
            .unwrap_or_else(|| io::Error::other("http2 io error"))
    } else {
        io::Error::other(e)
    }
}

fn h2_body_error(e: h2::Error) -> UpstreamError {
    UpstreamError {
        kind: ErrorKind::ClientAborted,
        message: format!("HTTP/2 request body error: {e}"),
        code: e.reason().map(|r| format!("{r:?}")),
    }
}

/// Request body from an h2 stream; capacity is released as the consumer pulls data, so
/// HTTP/2 flow control propagates backpressure.
fn h2_body(recv: RecvStream) -> BodyStream {
    Box::pin(futures::stream::unfold(Some(recv), |state| async move {
        let mut recv = state?;
        match recv.data().await {
            Some(Ok(data)) => {
                let _ = recv.flow_control().release_capacity(data.len());
                Some((Ok(BodyChunk::Data(data)), Some(recv)))
            }
            Some(Err(e)) => Some((Err(h2_body_error(e)), None)),
            None => match recv.trailers().await {
                Ok(Some(t)) => Some((Ok(BodyChunk::Trailers(Headers::from_header_map(&t))), None)),
                Ok(None) => None,
                Err(e) => Some((Err(h2_body_error(e)), None)),
            },
        }
    }))
}

/// How long a request whose HEADERS frame didn't end the stream may take to show whether a
/// body follows, for methods without request-body semantics. Clients that open such streams
/// (Node's `endStream: false`, gRPC-style stacks...) normally send the END_STREAM DATA frame
/// right away.
const BODY_PEEK_GRACE: Duration = Duration::from_millis(500);

/// The start of a request body of unknown length.
enum Peeked {
    /// The stream ended without any data or trailers: there is no body.
    Empty,
    /// The first body item (data, trailers or an error).
    First(Result<BodyChunk, UpstreamError>),
    /// Nothing arrived within [`BODY_PEEK_GRACE`].
    Pending,
}

/// Wait (briefly) for the first item of a request body.
async fn peek_body(recv: &mut RecvStream) -> Peeked {
    let peek = async {
        loop {
            match recv.data().await {
                Some(Ok(data)) => {
                    let _ = recv.flow_control().release_capacity(data.len());
                    if data.is_empty() {
                        // An empty DATA frame (typically the one carrying END_STREAM).
                        continue;
                    }
                    return Peeked::First(Ok(BodyChunk::Data(data)));
                }
                Some(Err(e)) => return Peeked::First(Err(h2_body_error(e))),
                None => {
                    return match recv.trailers().await {
                        Ok(Some(t)) => {
                            Peeked::First(Ok(BodyChunk::Trailers(Headers::from_header_map(&t))))
                        }
                        Ok(None) => Peeked::Empty,
                        Err(e) => Peeked::First(Err(h2_body_error(e))),
                    };
                }
            }
        }
    };
    tokio::time::timeout(BODY_PEEK_GRACE, peek)
        .await
        .unwrap_or(Peeked::Pending)
}

/// The request body stream, starting with an already peeked item.
fn h2_body_after(first: Option<Result<BodyChunk, UpstreamError>>, recv: RecvStream) -> BodyStream {
    use futures::StreamExt;
    match first {
        None => h2_body(recv),
        Some(Ok(BodyChunk::Data(data))) => {
            Box::pin(futures::stream::iter([Ok(BodyChunk::Data(data))]).chain(h2_body(recv)))
        }
        // Trailers and errors end the body.
        Some(last) => Box::pin(futures::stream::iter([last])),
    }
}

/// Methods whose requests normally carry a body: their streams are forwarded as soon as
/// they open (streaming uploads, gRPC), with chunked framing towards HTTP/1 upstreams when
/// the length is unknown.
fn method_has_request_body(method: &str) -> bool {
    ["POST", "PUT", "PATCH"]
        .iter()
        .any(|m| method.eq_ignore_ascii_case(m))
}

/// Connection-specific headers that must not appear in HTTP/2 messages.
fn is_connection_specific(name: &str) -> bool {
    [
        "connection",
        "keep-alive",
        "proxy-connection",
        "transfer-encoding",
        "upgrade",
        "host",
    ]
    .iter()
    .any(|h| name.eq_ignore_ascii_case(h))
}

/// Writes responses on an HTTP/2 stream.
pub(crate) struct H2Responder {
    respond: SendResponse<Bytes>,
    send: Option<SendStream<Bytes>>,
    head_request: bool,
    expect_continue: bool,
    continue_sent: bool,
    head_sent: bool,
}

impl H2Responder {
    fn new(respond: SendResponse<Bytes>, head_request: bool, expect_continue: bool) -> Self {
        H2Responder {
            respond,
            send: None,
            head_request,
            expect_continue,
            continue_sent: false,
            head_sent: false,
        }
    }

    pub(crate) fn head_sent(&self) -> bool {
        self.head_sent
    }

    pub(crate) fn send_continue(&mut self) {
        if self.expect_continue && !self.continue_sent && !self.head_sent {
            self.continue_sent = true;
            if let Ok(resp) = ::http::Response::builder().status(100).body(()) {
                let _ = self.respond.send_informational(resp);
            }
        }
    }

    pub(crate) fn send_head(&mut self, head: &ResponseHead, body: BodyKind) -> io::Result<()> {
        let mut builder = ::http::Response::builder().status(head.status);
        // Transfer-Encoding overrides Content-Length (RFC 9112 §6.3): a length that came with
        // it is meaningless and must not be forwarded.
        let drop_length = head.headers.contains("transfer-encoding");
        for (name, value) in head.headers.iter() {
            if name.starts_with(':') || is_connection_specific(name) {
                continue;
            }
            if drop_length && name.eq_ignore_ascii_case("content-length") {
                continue;
            }
            if name.eq_ignore_ascii_case("te") && !value.eq_ignore_ascii_case("trailers") {
                continue;
            }
            match (
                ::http::HeaderName::from_bytes(name.as_bytes()),
                ::http::HeaderValue::from_bytes(&string_to_header_bytes(value)),
            ) {
                (Ok(n), Ok(v)) => builder = builder.header(n, v),
                _ => tracing::debug!(header = name, "dropping invalid HTTP/2 response header"),
            }
        }
        let resp = builder.body(()).map_err(io::Error::other)?;
        let no_body = self.head_request
            || (100..200).contains(&head.status)
            || head.status == 204
            || head.status == 304;
        // An empty `Full` body keeps the stream open: `finish` then ends it with the trailers
        // (gRPC sends grpc-status as HEADERS + trailers without DATA) or with an empty
        // END_STREAM DATA frame.
        let end = no_body || body == BodyKind::None;
        self.head_sent = true;
        let stream = self.respond.send_response(resp, end).map_err(h2_io_error)?;
        if !end {
            self.send = Some(stream);
        }
        Ok(())
    }

    pub(crate) async fn send_data(&mut self, mut data: Bytes) -> io::Result<()> {
        let Some(stream) = self.send.as_mut() else {
            return Ok(());
        };
        while !data.is_empty() {
            stream.reserve_capacity(data.len());
            match poll_fn(|cx| stream.poll_capacity(cx)).await {
                None => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "HTTP/2 stream closed",
                    ));
                }
                Some(Err(e)) => return Err(h2_io_error(e)),
                Some(Ok(0)) => continue,
                Some(Ok(n)) => {
                    let chunk = data.split_to(n.min(data.len()));
                    stream.send_data(chunk, false).map_err(h2_io_error)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn finish(&mut self, trailers: Option<&Headers>) -> io::Result<()> {
        let Some(mut stream) = self.send.take() else {
            return Ok(());
        };
        match trailers.filter(|t| !t.is_empty()) {
            Some(t) => stream.send_trailers(t.to_header_map()).map_err(h2_io_error),
            None => stream.send_data(Bytes::new(), true).map_err(h2_io_error),
        }
    }

    pub(crate) fn abort(&mut self, kind: AbortKind) {
        let reason = match kind {
            AbortKind::Close => Reason::CANCEL,
            AbortKind::Reset | AbortKind::Incomplete => Reason::INTERNAL_ERROR,
        };
        match self.send.as_mut() {
            Some(s) => s.send_reset(reason),
            None => self.respond.send_reset(reason),
        }
    }

    pub(crate) async fn client_gone(&mut self) {
        let _ = poll_fn(|cx| match self.send.as_mut() {
            Some(s) => s.poll_reset(cx),
            None => self.respond.poll_reset(cx),
        })
        .await;
    }
}

/// Merge `cookie` headers into one (HTTP/1 servers expect a single Cookie header).
fn merge_cookies(headers: &mut Headers) {
    let cookies: Vec<String> = headers.get_all("cookie").map(str::to_string).collect();
    if cookies.len() > 1 {
        headers.set("cookie", cookies.join("; "));
    }
}

/// Serve an HTTP/2 connection.
pub(crate) async fn serve(shared: Arc<Shared>, io: ClientIo, prefix: BytesMut, ctx: ConnCtx) {
    let io = crate::io::rewind(io, prefix);
    let mut builder = h2::server::Builder::new();
    builder
        .initial_window_size(1 << 20)
        .initial_connection_window_size(4 << 20)
        .max_concurrent_streams(256)
        .max_header_list_size(256 * 1024)
        .max_send_buffer_size(1 << 20);
    let handshake = tokio::select! {
        r = tokio::time::timeout(HANDSHAKE_TIMEOUT, builder.handshake::<_, Bytes>(io)) => r,
        _ = shared.hard_stop.cancelled() => return,
    };
    let mut conn = match handshake {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            if !e.is_io() {
                let err = exchange_error(
                    ErrorKind::ClientProtocolError,
                    format!("HTTP/2 handshake failed: {e}"),
                    Some("ERR_HTTP2_ERROR"),
                );
                client_error(&shared, &ctx, err, &[]).await;
            }
            return;
        }
        Err(_) => return,
    };
    let ctx = Arc::new(ctx);
    let mut graceful = false;
    // `accept` also drives the connection (frames for every stream), so it's polled until the
    // connection ends. A graceful shutdown sends GOAWAY and lets in-flight streams finish.
    loop {
        tokio::select! {
            accepted = conn.accept() => match accepted {
                Some(Ok((req, respond))) => {
                    let task_shared = shared.clone();
                    let ctx = ctx.clone();
                    shared.tracker.spawn(async move { stream_task(task_shared, ctx, req, respond).await });
                }
                Some(Err(e)) => {
                    tracing::debug!(conn = %ctx.conn.id, error = %e, "HTTP/2 connection error");
                    return;
                }
                None => return,
            },
            _ = shared.shutdown.cancelled(), if !graceful => {
                graceful = true;
                conn.graceful_shutdown();
            }
            _ = shared.hard_stop.cancelled() => return,
        }
    }
}

async fn stream_task(
    shared: Arc<Shared>,
    ctx: Arc<ConnCtx>,
    req: ::http::Request<RecvStream>,
    mut respond: SendResponse<Bytes>,
) {
    let sw = Stopwatch::start();
    let (parts, recv) = req.into_parts();
    if parts.method == ::http::Method::CONNECT {
        connect_stream(shared, ctx, parts, recv, respond).await;
        return;
    }
    let headers = Headers::from_header_map(&parts.headers);
    let authority = parts
        .uri
        .authority()
        .map(|a| a.as_str().to_string())
        .or_else(|| headers.get("host").map(str::to_string));
    let raw_path = parts
        .uri
        .path_and_query()
        .map(|p| p.as_str())
        .unwrap_or("/");
    let target = if raw_path == "*" {
        RequestTarget::Asterisk
    } else {
        RequestTarget::Origin(raw_path.to_string())
    };
    let resolved = match resolve_target(
        &ctx.target_context(),
        &target,
        authority.as_deref(),
        parts.uri.scheme_str(),
        false,
    ) {
        Ok(r) => r,
        Err(e) => {
            let err = exchange_error(
                ErrorKind::ClientProtocolError,
                e.to_string(),
                Some("ERR_INVALID_URL"),
            );
            client_error(&shared, &ctx, err, raw_path.as_bytes()).await;
            if let Ok(resp) = ::http::Response::builder().status(400).body(()) {
                let _ = respond.send_response(resp, true);
            }
            return;
        }
    };
    let head = RequestHead {
        method: parts.method.as_str().to_string(),
        url: build_url(
            &resolved.scheme,
            &resolved.host,
            resolved.port,
            &resolved.path,
        ),
        scheme: resolved.scheme.clone(),
        host: resolved.host.clone(),
        port: resolved.port,
        path: resolved.path.clone(),
        http_version: HttpVersion::H2,
        headers,
    };
    // HTTP/2 → HTTP/1 translation for the forwarded request (Host from :authority, one
    // Cookie header). Harmless for HTTP/2 upstreams.
    let mut forward = head.clone();
    if !forward.headers.contains("host") {
        let authority = forward.authority();
        forward.headers.0.insert(0, ("host".to_string(), authority));
    }
    merge_cookies(&mut forward.headers);
    let expect_continue = crate::http1::parse::expects_continue(&head.headers);
    let request_headers_ms = sw.elapsed_ms();
    let mut recv = recv;
    let mut no_body = recv.is_end_stream();
    let mut first = None;
    let unknown_length = !no_body
        && !forward.headers.contains("content-length")
        && !forward.headers.contains("transfer-encoding");
    if unknown_length && !method_has_request_body(&head.method) && !expect_continue {
        // HEADERS without END_STREAM doesn't mean there is a body: many clients end a GET
        // with a separate empty DATA frame. Find out before choosing the HTTP/1 framing, so
        // a bodiless request isn't forwarded as `Transfer-Encoding: chunked` (which strict
        // origins reject on GET).
        match peek_body(&mut recv).await {
            Peeked::Empty => no_body = true,
            Peeked::First(item) => first = Some(item),
            Peeked::Pending => {}
        }
    }
    // An HTTP/1 upstream needs explicit framing for a body of unknown length (HTTP/2 upstreams
    // drop the header again).
    if unknown_length && !no_body {
        forward.headers.append("transfer-encoding", "chunked");
    }
    let local = is_local_target(&head.host, head.port, resolved.mode, ctx.local).then(|| {
        if head.host.eq_ignore_ascii_case(MAGIC_HOST) {
            crate::local::display_addr(ctx.local)
        } else {
            authority
                .clone()
                .unwrap_or_else(|| crate::local::display_addr(ctx.local))
        }
    });
    let body = if no_body {
        ClientBody::Empty
    } else {
        ClientBody::Stream {
            length: head.headers.content_length(),
            stream: h2_body_after(first, recv),
        }
    };
    let head_request = head.method.eq_ignore_ascii_case("HEAD");
    let (client, source) = ctx.client_info(&shared, resolved.mode).await;
    let env = ExchangeEnv {
        client,
        source,
        request_headers_ms,
        sw,
        apply_rules: true,
        options: ctx.request_options(&shared),
        local,
    };
    let req = ExchangeRequest {
        head,
        forward,
        body,
        upgrade: None,
    };
    let mut responder = H2Responder::new(respond, head_request, expect_continue);
    let mut r = Responder::H2(&mut responder);
    let _ = pipeline::run(&shared, env, req, &mut r).await;
}

/// `CONNECT` on an HTTP/2 stream: answer 200 and treat the stream as a new connection.
async fn connect_stream(
    shared: Arc<Shared>,
    ctx: Arc<ConnCtx>,
    parts: ::http::request::Parts,
    recv: RecvStream,
    mut respond: SendResponse<Bytes>,
) {
    let Some(target) = parts
        .uri
        .authority()
        .and_then(|a| parse_authority(a.as_str(), None))
    else {
        if let Ok(resp) = ::http::Response::builder().status(400).body(()) {
            let _ = respond.send_response(resp, true);
        }
        return;
    };
    let Ok(resp) = ::http::Response::builder().status(200).body(()) else {
        return;
    };
    let Ok(send) = respond.send_response(resp, false) else {
        return;
    };
    let mut inner = (*ctx).clone();
    inner.enter_tunnel(target, ProxyMode::Connect);
    let io: ClientIo = Box::new(H2StreamIo {
        recv,
        send,
        buf: Bytes::new(),
        eof: false,
    });
    crate::conn::handle(shared, io, BytesMut::new(), inner).await;
}

/// An HTTP/2 stream used as a byte stream (CONNECT tunnels).
struct H2StreamIo {
    recv: RecvStream,
    send: SendStream<Bytes>,
    buf: Bytes,
    eof: bool,
}

impl AsyncRead for H2StreamIo {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        out: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        loop {
            if !self.buf.is_empty() {
                let n = self.buf.len().min(out.remaining());
                out.put_slice(&self.buf[..n]);
                self.buf.advance(n);
                return Poll::Ready(Ok(()));
            }
            if self.eof {
                return Poll::Ready(Ok(()));
            }
            match ready!(self.recv.poll_data(cx)) {
                Some(Ok(data)) => {
                    let _ = self.recv.flow_control().release_capacity(data.len());
                    self.buf = data;
                }
                Some(Err(e)) => return Poll::Ready(Err(h2_io_error(e))),
                None => self.eof = true,
            }
        }
    }
}

impl AsyncWrite for H2StreamIo {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        data: &[u8],
    ) -> Poll<io::Result<usize>> {
        if data.is_empty() {
            return Poll::Ready(Ok(0));
        }
        self.send.reserve_capacity(data.len());
        loop {
            match ready!(self.send.poll_capacity(cx)) {
                None => {
                    return Poll::Ready(Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "HTTP/2 stream closed",
                    )));
                }
                Some(Err(e)) => return Poll::Ready(Err(h2_io_error(e))),
                Some(Ok(0)) => continue,
                Some(Ok(n)) => {
                    let n = n.min(data.len());
                    self.send
                        .send_data(Bytes::copy_from_slice(&data[..n]), false)
                        .map_err(h2_io_error)?;
                    return Poll::Ready(Ok(n));
                }
            }
        }
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let _ = self.send.send_data(Bytes::new(), true);
        Poll::Ready(Ok(()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_merging() {
        let mut h: Headers = vec![
            ("cookie".to_string(), "a=1".to_string()),
            ("x".to_string(), "y".to_string()),
            ("cookie".to_string(), "b=2".to_string()),
        ]
        .into_iter()
        .collect();
        merge_cookies(&mut h);
        assert_eq!(
            h.0,
            vec![
                ("cookie".to_string(), "a=1; b=2".to_string()),
                ("x".to_string(), "y".to_string())
            ]
        );
    }

    #[test]
    fn connection_specific_headers() {
        assert!(is_connection_specific("Transfer-Encoding"));
        assert!(is_connection_specific("keep-alive"));
        assert!(!is_connection_specific("content-length"));
    }
}
