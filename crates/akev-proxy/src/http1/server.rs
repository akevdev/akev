//! The HTTP/1 connection loop: reads requests (sequentially, supporting pipelining), handles
//! CONNECT by handing the stream back for re-sniffing, runs each exchange through the
//! pipeline, and carries out upgrades.

use super::decode::{Decoded, Decoder};
use super::parse::{
    Framing, MAX_HEAD_SIZE, ParseError, RawRequest, UpgradeRequest, client_wants_close,
    expects_continue, has_head_end, header_tokens, parse_request, request_framing, upgrade_request,
};
use super::write::{
    BodyKind, OutFraming, ResponseContext, chunk_header, informational, last_chunk, plan_response,
};
use crate::body::ClientBody;
use crate::conn::{ConnCtx, client_error};
use crate::errors::exchange_error;
use crate::io::{ClientIo, read_more, read_more_timeout};
use crate::local::{MAGIC_HOST, is_local_target};
use crate::pipeline::{self, ExchangeEnv, ExchangeRequest, Outcome, Upgrade};
use crate::responder::{AbortKind, Responder};
use crate::shared::Shared;
use crate::target::{RequestTarget, build_url, parse_authority, resolve_target};
use crate::upstream::{BodyChunk, BodyStream, UpstreamError};
use akev_core::*;
use bytes::{Bytes, BytesMut};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::sync::oneshot;
use tokio_util::sync::CancellationToken;

/// Idle keep-alive timeout between requests.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Time allowed to complete a request head once it started.
const HEAD_TIMEOUT: Duration = Duration::from_secs(60);
/// Per-read idle timeout while receiving a request body.
const BODY_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
/// Most pipelined bytes buffered while an exchange is in flight.
const MAX_PIPELINED: usize = 1024 * 1024;
/// Unread request bodies up to this size are drained to keep the connection alive.
const MAX_DRAIN: usize = 4 * 1024 * 1024;
const DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// Below this size the head parser runs on every read (cheap); above it only once the
/// blank line arrived, keeping big heads linear.
const EAGER_PARSE_LIMIT: usize = 8 * 1024;

/// The read side of a connection plus its buffer.
pub(crate) struct H1Reader {
    io: ReadHalf<ClientIo>,
    buf: BytesMut,
}

/// The reader handed back by a request body stream when it finishes or is dropped.
struct ReaderBack {
    reader: H1Reader,
    decoder: Decoder,
}

struct BodyState {
    reader: Option<H1Reader>,
    decoder: Decoder,
    ret: Option<oneshot::Sender<ReaderBack>>,
    failed: bool,
}

impl BodyState {
    fn give_back(&mut self) {
        if let (Some(reader), Some(tx)) = (self.reader.take(), self.ret.take()) {
            let _ = tx.send(ReaderBack {
                reader,
                decoder: self.decoder.clone(),
            });
        }
    }
}

impl Drop for BodyState {
    fn drop(&mut self) {
        self.give_back();
    }
}

fn client_body_error(kind: ErrorKind, message: impl Into<String>) -> UpstreamError {
    UpstreamError {
        kind,
        message: message.into(),
        code: None,
    }
}

/// Stream a request body off the connection. The reader comes back through the receiver as
/// soon as the body is complete (or the stream is dropped).
fn body_stream(reader: H1Reader, decoder: Decoder) -> (BodyStream, oneshot::Receiver<ReaderBack>) {
    let (tx, rx) = oneshot::channel();
    let state = BodyState {
        reader: Some(reader),
        decoder,
        ret: Some(tx),
        failed: false,
    };
    let stream = futures::stream::unfold(state, |mut st| async move {
        if st.failed {
            return None;
        }
        loop {
            let reader = st.reader.as_mut()?;
            match st.decoder.decode(&mut reader.buf) {
                Ok(Decoded::Data(b)) => return Some((Ok(BodyChunk::Data(b)), st)),
                Ok(Decoded::Trailers(t)) => return Some((Ok(BodyChunk::Trailers(t)), st)),
                Ok(Decoded::Done) => {
                    st.give_back();
                    return None;
                }
                Ok(Decoded::NeedMore) => {
                    match read_more_timeout(&mut reader.io, &mut reader.buf, BODY_IDLE_TIMEOUT)
                        .await
                    {
                        Ok(0) => {
                            st.failed = true;
                            let err = client_body_error(
                                ErrorKind::ClientAborted,
                                "the client closed the connection before sending the whole request body",
                            );
                            return Some((Err(err), st));
                        }
                        Ok(_) => continue,
                        Err(e) => {
                            st.failed = true;
                            return Some((
                                Err(client_body_error(
                                    ErrorKind::ClientAborted,
                                    format!("error reading request body: {e}"),
                                )),
                                st,
                            ));
                        }
                    }
                }
                Err(e) => {
                    st.failed = true;
                    return Some((
                        Err(client_body_error(
                            ErrorKind::ClientProtocolError,
                            format!("malformed request body: {e}"),
                        )),
                        st,
                    ));
                }
            }
        }
    });
    (Box::pin(stream), rx)
}

/// Writes responses to an HTTP/1 client.
pub(crate) struct H1Responder {
    io: WriteHalf<ClientIo>,
    client_version: HttpVersion,
    head_request: bool,
    client_close: bool,
    expect_continue: bool,
    continue_sent: bool,
    head_sent: bool,
    framing: OutFraming,
    written: u64,
    close_after: bool,
    broken: bool,
    gone: CancellationToken,
    reset: Arc<AtomicBool>,
}

/// What the response writer needs to know about the request it answers.
#[derive(Clone, Copy)]
struct RequestFacts {
    version: HttpVersion,
    head_request: bool,
    client_close: bool,
    expect_continue: bool,
}

impl RequestFacts {
    fn of(req: &RawRequest) -> Self {
        RequestFacts {
            version: req.version,
            head_request: req.method.eq_ignore_ascii_case("HEAD"),
            client_close: client_wants_close(&req.headers, req.version),
            expect_continue: expects_continue(&req.headers),
        }
    }
}

impl H1Responder {
    fn new(
        io: WriteHalf<ClientIo>,
        req: RequestFacts,
        gone: CancellationToken,
        reset: Arc<AtomicBool>,
    ) -> Self {
        H1Responder {
            io,
            client_version: req.version,
            head_request: req.head_request,
            client_close: req.client_close,
            expect_continue: req.expect_continue,
            continue_sent: false,
            head_sent: false,
            framing: OutFraming::None,
            written: 0,
            close_after: false,
            broken: false,
            gone,
            reset,
        }
    }

    pub(crate) fn head_sent(&self) -> bool {
        self.head_sent
    }

    pub(crate) fn keep_alive(&self) -> bool {
        !self.close_after && !self.broken && !self.client_close
    }

    pub(crate) async fn send_continue(&mut self) {
        if self.expect_continue && !self.continue_sent && !self.head_sent {
            self.continue_sent = true;
            let bytes = informational(100, &Headers::new());
            if self.io.write_all(&bytes).await.is_err() || self.io.flush().await.is_err() {
                self.broken = true;
            }
        }
    }

    pub(crate) async fn send_head(
        &mut self,
        head: &ResponseHead,
        body: BodyKind,
    ) -> io::Result<()> {
        let ctx = ResponseContext {
            head_request: self.head_request,
            client_version: self.client_version,
            client_wants_close: self.client_close,
        };
        let plan = plan_response(head, &ctx, body);
        self.framing = plan.framing;
        self.close_after |= plan.close_after;
        self.head_sent = true;
        let res = async {
            self.io.write_all(&plan.head).await?;
            self.io.flush().await
        }
        .await;
        if res.is_err() {
            self.broken = true;
        }
        res
    }

    pub(crate) async fn send_data(&mut self, data: Bytes) -> io::Result<()> {
        if data.is_empty() {
            return Ok(());
        }
        let res = match self.framing {
            OutFraming::None => return Ok(()),
            OutFraming::Length(n) => {
                let allowed = n.saturating_sub(self.written);
                let data = if (data.len() as u64) > allowed {
                    // More data than declared: never corrupt the connection, truncate & close.
                    self.broken = true;
                    data.slice(..allowed as usize)
                } else {
                    data
                };
                self.written += data.len() as u64;
                self.io.write_all(&data).await
            }
            OutFraming::Chunked => {
                let header = chunk_header(data.len());
                self.written += data.len() as u64;
                if data.len() <= 16 * 1024 {
                    let mut v = Vec::with_capacity(header.len() + data.len() + 2);
                    v.extend_from_slice(&header);
                    v.extend_from_slice(&data);
                    v.extend_from_slice(b"\r\n");
                    self.io.write_all(&v).await
                } else {
                    async {
                        self.io.write_all(&header).await?;
                        self.io.write_all(&data).await?;
                        self.io.write_all(b"\r\n").await
                    }
                    .await
                }
            }
            OutFraming::CloseDelimited => {
                self.written += data.len() as u64;
                self.io.write_all(&data).await
            }
        };
        let res = match res {
            Ok(()) => self.io.flush().await,
            Err(e) => Err(e),
        };
        if res.is_err() {
            self.broken = true;
        }
        res
    }

    pub(crate) async fn finish(&mut self, trailers: Option<&Headers>) -> io::Result<()> {
        let res = match self.framing {
            OutFraming::Chunked => self.io.write_all(&last_chunk(trailers)).await,
            OutFraming::Length(n) if self.written < n => {
                // Body shorter than declared: the client can only notice through a close.
                self.broken = true;
                Ok(())
            }
            OutFraming::CloseDelimited => {
                self.close_after = true;
                Ok(())
            }
            _ => Ok(()),
        };
        let res = match res {
            Ok(()) => self.io.flush().await,
            Err(e) => Err(e),
        };
        if res.is_err() {
            self.broken = true;
        }
        res
    }

    pub(crate) async fn abort(&mut self, kind: AbortKind) {
        self.broken = true;
        match kind {
            AbortKind::Reset => self.reset.store(true, Ordering::Relaxed),
            // A response that started but can't be completed must not end with a graceful
            // close: for a close-delimited body a FIN (or a TLS close_notify) *is* the
            // end-of-body marker, so the client would accept the truncated body as complete.
            // Close abortively instead (TCP RST, no TLS shutdown), like HTTP/2's RST_STREAM.
            AbortKind::Incomplete if self.head_sent => self.reset.store(true, Ordering::Relaxed),
            AbortKind::Close | AbortKind::Incomplete => {
                let _ = self.io.shutdown().await;
            }
        }
    }

    pub(crate) async fn client_gone(&mut self) {
        self.gone.cancelled().await
    }
}

enum HeadRead {
    Request(RawRequest, Stopwatch, f64),
    Closed,
    Error(ParseError),
}

async fn read_head(reader: &mut H1Reader) -> HeadRead {
    let mut sw = (!reader.buf.is_empty()).then(Stopwatch::start);
    let mut checked = 0;
    loop {
        if !reader.buf.is_empty()
            && (reader.buf.len() <= EAGER_PARSE_LIMIT || has_head_end(&reader.buf, checked))
        {
            match parse_request(&reader.buf) {
                Ok(Some(raw)) => {
                    let sw = sw.unwrap_or_else(Stopwatch::start);
                    let ms = sw.elapsed_ms();
                    return HeadRead::Request(raw, sw, ms);
                }
                Ok(None) => {}
                Err(e) => return HeadRead::Error(e),
            }
        }
        if reader.buf.len() > MAX_HEAD_SIZE {
            return HeadRead::Error(ParseError::TooLarge);
        }
        checked = reader.buf.len();
        let timeout = if reader.buf.is_empty() {
            IDLE_TIMEOUT
        } else {
            HEAD_TIMEOUT
        };
        match read_more_timeout(&mut reader.io, &mut reader.buf, timeout).await {
            Ok(0) if reader.buf.is_empty() => return HeadRead::Closed,
            Ok(0) => {
                return HeadRead::Error(ParseError::Invalid {
                    message: "connection closed in the middle of a request head".into(),
                    code: "HPE_INVALID_EOF_STATE",
                });
            }
            Ok(_) => {
                sw.get_or_insert_with(Stopwatch::start);
            }
            Err(_) => return HeadRead::Closed,
        }
    }
}

/// Remove an `Upgrade: h2c` request's upgrade headers (it is then handled as a normal request).
fn strip_h2c_upgrade(headers: &mut Headers) {
    headers.remove("upgrade");
    headers.remove("http2-settings");
    let tokens: Vec<String> = header_tokens(headers, "connection")
        .into_iter()
        .filter(|t| t != "upgrade" && t != "http2-settings")
        .collect();
    if tokens.is_empty() {
        headers.remove("connection");
    } else {
        headers.set("connection", tokens.join(", "));
    }
}

/// `407` asking for the proxy credentials (`proxy.access`), then close.
async fn proxy_auth_required(writer: &mut WriteHalf<ClientIo>) {
    const BODY: &str = "This proxy needs a username and password. Enter the ones set in Akev (Settings > Proxy > Who can connect) in this device's proxy settings.\n";
    let msg = format!(
        "HTTP/1.1 407 Proxy Authentication Required\r\nProxy-Authenticate: Basic realm=\"Akev\", charset=\"UTF-8\"\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{BODY}",
        BODY.len()
    );
    let _ = writer.write_all(msg.as_bytes()).await;
    let _ = writer.shutdown().await;
}

async fn reject(writer: &mut WriteHalf<ClientIo>, status: u16) {
    let msg = format!(
        "HTTP/1.1 {status} {}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n",
        canonical_reason(status)
    );
    let _ = writer.write_all(msg.as_bytes()).await;
    let _ = writer.shutdown().await;
}

/// Drain the rest of an unread request body so the connection can be reused.
async fn drain_body(reader: &mut H1Reader, mut decoder: Decoder) -> bool {
    let deadline = tokio::time::Instant::now() + DRAIN_TIMEOUT;
    let mut total = 0usize;
    loop {
        match decoder.decode(&mut reader.buf) {
            Ok(Decoded::Data(b)) => {
                total += b.len();
                if total > MAX_DRAIN {
                    return false;
                }
            }
            Ok(Decoded::Trailers(_)) => {}
            Ok(Decoded::Done) => return true,
            Ok(Decoded::NeedMore) => {
                match tokio::time::timeout_at(deadline, read_more(&mut reader.io, &mut reader.buf))
                    .await
                {
                    Ok(Ok(n)) if n > 0 => {}
                    _ => return false,
                }
            }
            Err(_) => return false,
        }
    }
}

/// Serve HTTP/1 on a stream. Returns the stream (and leftover bytes / new context) when a
/// CONNECT tunnel was established and must be re-sniffed.
pub(crate) async fn serve(
    shared: Arc<Shared>,
    io: ClientIo,
    buf: BytesMut,
    ctx: ConnCtx,
) -> Option<(ClientIo, BytesMut, ConnCtx)> {
    let (rd, wr) = tokio::io::split(io);
    let mut reader = Some(H1Reader { io: rd, buf });
    let mut writer = wr;
    let mut ctx = ctx;
    loop {
        let r = reader.as_mut()?;
        let read = tokio::select! {
            h = read_head(r) => h,
            _ = shared.shutdown.cancelled(), if r.buf.is_empty() => HeadRead::Closed,
        };
        let (mut raw, sw, head_ms) = match read {
            HeadRead::Request(raw, sw, ms) => (raw, sw, ms),
            HeadRead::Closed => return None,
            HeadRead::Error(e) => {
                let err =
                    exchange_error(ErrorKind::ClientProtocolError, e.message(), Some(e.code()));
                client_error(&shared, &ctx, err, &r.buf).await;
                reject(
                    &mut writer,
                    if e == ParseError::TooLarge { 431 } else { 400 },
                )
                .await;
                return None;
            }
        };
        let head_bytes = r.buf.split_to(raw.head_len);
        // Proxy credentials (`proxy.access`): once presented, the connection (and any tunnel
        // opened on it) is authenticated.
        if !ctx.authenticated
            && raw
                .headers
                .get("proxy-authorization")
                .is_some_and(|v| shared.settings().access.check_basic(v))
        {
            ctx.authenticated = true;
        }

        // ---- CONNECT ----------------------------------------------------------------------
        if raw.method.eq_ignore_ascii_case("CONNECT") && !ctx.authenticated {
            shared.auth_failed(ctx.peer, "CONNECT without valid Proxy-Authorization");
            proxy_auth_required(&mut writer).await;
            return None;
        }
        if raw.method.eq_ignore_ascii_case("CONNECT") {
            let target = parse_authority(&raw.target, None).or_else(|| {
                raw.headers
                    .get("host")
                    .and_then(|h| parse_authority(h, None))
            });
            let Some(target) = target else {
                let err = exchange_error(
                    ErrorKind::ClientProtocolError,
                    format!("invalid CONNECT target {:?}", raw.target),
                    Some("ERR_INVALID_URL"),
                );
                client_error(&shared, &ctx, err, &head_bytes).await;
                reject(&mut writer, 400).await;
                return None;
            };
            let reply = format!(
                "{} 200 Connection Established\r\n\r\n",
                if raw.version == HttpVersion::Http10 {
                    "HTTP/1.0"
                } else {
                    "HTTP/1.1"
                }
            );
            if writer.write_all(reply.as_bytes()).await.is_err() || writer.flush().await.is_err() {
                return None;
            }
            let H1Reader { io: rd, buf } = reader.take()?;
            let io = rd.unsplit(writer);
            let mut next = ctx.clone();
            next.enter_tunnel(target, ProxyMode::Connect);
            return Some((io, buf, next));
        }

        // ---- request head -----------------------------------------------------------------
        let invalid = |message: String, code: &str| {
            exchange_error(ErrorKind::ClientProtocolError, message, Some(code))
        };
        let Some(target) = RequestTarget::parse(&raw.target) else {
            client_error(
                &shared,
                &ctx,
                invalid(
                    format!("invalid request target {:?}", raw.target),
                    "ERR_INVALID_URL",
                ),
                &head_bytes,
            )
            .await;
            reject(&mut writer, 400).await;
            return None;
        };
        let upgrade = upgrade_request(&raw.headers);
        let websocket = upgrade == Some(UpgradeRequest::WebSocket);
        let resolved = match resolve_target(
            &ctx.target_context(),
            &target,
            raw.headers.get("host"),
            None,
            websocket,
        ) {
            Ok(r) => r,
            Err(e) => {
                client_error(
                    &shared,
                    &ctx,
                    invalid(e.to_string(), "ERR_INVALID_URL"),
                    &head_bytes,
                )
                .await;
                reject(&mut writer, 400).await;
                return None;
            }
        };
        let framing = match request_framing(&raw.headers) {
            Ok(f) => f,
            Err(e) => {
                client_error(&shared, &ctx, invalid(e.message(), e.code()), &head_bytes).await;
                reject(&mut writer, 400).await;
                return None;
            }
        };
        let facts = RequestFacts::of(&raw);
        let mut headers = std::mem::take(&mut raw.headers);
        if resolved.mode == ProxyMode::HttpProxy {
            headers.remove("proxy-connection");
            headers.remove("proxy-authorization");
        } else if shared.settings().access.has_credentials() {
            // Never forward the proxy credentials.
            headers.remove("proxy-authorization");
        }
        let head = RequestHead {
            method: std::mem::take(&mut raw.method),
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
            http_version: raw.version,
            headers,
        };
        let mut forward = head.clone();
        if upgrade == Some(UpgradeRequest::H2c) {
            strip_h2c_upgrade(&mut forward.headers);
        }
        // Transfer-Encoding and Content-Length together (RFC 9112 §6.1): the body was framed as
        // chunked, so never forward the conflicting length (request smuggling), and don't reuse
        // the client connection afterwards.
        let ambiguous_framing =
            framing == Framing::Chunked && forward.headers.contains("content-length");
        if ambiguous_framing {
            forward.headers.remove("content-length");
        }
        // A proxy forwarding an absolute-form request must send a Host header (RFC 9112 §3.2.2);
        // HTTP/1.0 clients often omit it.
        if resolved.mode == ProxyMode::HttpProxy && !forward.headers.contains("host") {
            let authority = forward.authority();
            forward.headers.0.insert(0, ("Host".to_string(), authority));
        }
        let local = is_local_target(&head.host, head.port, resolved.mode, ctx.local).then(|| {
            if head.host.eq_ignore_ascii_case(MAGIC_HOST) {
                crate::local::display_addr(ctx.local)
            } else {
                head.headers
                    .get("host")
                    .map(str::to_string)
                    .unwrap_or_else(|| crate::local::display_addr(ctx.local))
            }
        });
        // The setup page (CA certificate, instructions) needs no credentials: a device fetches
        // it before it is configured. Everything else does.
        if !ctx.authenticated && local.is_none() {
            shared.auth_failed(ctx.peer, "request without valid Proxy-Authorization");
            proxy_auth_required(&mut writer).await;
            return None;
        }

        // ---- body -------------------------------------------------------------------------
        let length = match framing {
            Framing::Empty => Some(0),
            Framing::Length(n) => Some(n),
            Framing::Chunked => None,
        };
        let mut reader_rx = None;
        let body = if framing == Framing::Empty {
            ClientBody::Empty
        } else {
            let r = reader.take()?;
            let (stream, rx) = body_stream(r, Decoder::new(framing));
            reader_rx = Some(rx);
            ClientBody::Stream { stream, length }
        };

        let (client, source) = ctx.client_info(&shared, resolved.mode).await;
        let env = ExchangeEnv {
            client,
            source,
            sw,
            request_headers_ms: head_ms,
            apply_rules: true,
            options: ctx.request_options(&shared),
            local,
        };
        let req = ExchangeRequest {
            head,
            forward,
            body,
            upgrade: match upgrade {
                Some(UpgradeRequest::WebSocket) => Some(Upgrade::WebSocket),
                Some(UpgradeRequest::Other(_)) => Some(Upgrade::Other),
                _ => None,
            },
        };

        // ---- run the exchange while watching the connection -------------------------------
        let gone = CancellationToken::new();
        let mut responder = H1Responder::new(writer, facts, gone.clone(), ctx.conn.reset.clone());
        responder.client_close |= ambiguous_framing;
        let mut decoder_back: Option<Decoder> = None;
        let mut client_eof = false;
        let outcome = {
            let mut resp = Responder::H1(&mut responder);
            let fut = pipeline::run(&shared, env, req, &mut resp);
            tokio::pin!(fut);
            enum Event {
                Done(Outcome),
                ReaderBack(Option<ReaderBack>),
                Read(io::Result<usize>),
            }
            loop {
                let watch_reader =
                    reader.as_ref().is_some_and(|r| r.buf.len() < MAX_PIPELINED) && !client_eof;
                let event = tokio::select! {
                    res = &mut fut => Event::Done(res.1),
                    back = async {
                        match reader_rx.as_mut() {
                            Some(rx) => rx.await.ok(),
                            None => std::future::pending().await,
                        }
                    }, if reader_rx.is_some() => Event::ReaderBack(back),
                    n = async {
                        match reader.as_mut() {
                            Some(r) => read_more(&mut r.io, &mut r.buf).await,
                            None => std::future::pending().await,
                        }
                    }, if watch_reader => Event::Read(n),
                };
                match event {
                    Event::Done(outcome) => break outcome,
                    Event::ReaderBack(back) => {
                        reader_rx = None;
                        if let Some(b) = back {
                            reader = Some(b.reader);
                            decoder_back = Some(b.decoder);
                        }
                    }
                    // EOF (or error) while the exchange runs: the client is gone.
                    Event::Read(n) => {
                        if !matches!(n, Ok(k) if k > 0) {
                            client_eof = true;
                            gone.cancel();
                        }
                    }
                }
            }
        };
        let keep_alive = responder.keep_alive();
        let continue_sent = responder.continue_sent;
        let expect_continue = responder.expect_continue;
        writer = responder.io;

        // Get the reader back (the body stream may still be held by the upstream client).
        if reader.is_none()
            && let Some(rx) = reader_rx.take()
        {
            let back = tokio::select! {
                b = rx => b.ok(),
                _ = shared.hard_stop.cancelled() => None,
            };
            if let Some(b) = back {
                reader = Some(b.reader);
                decoder_back = Some(b.decoder);
            }
        }
        // A reset (rule / breakpoint) must reach the client as a TCP RST: no graceful FIN first.
        let reset = ctx.conn.reset.load(Ordering::Relaxed);
        let r = reader.as_mut()?;
        if let Some(dec) = decoder_back.take()
            && !dec.is_done()
        {
            let drainable = keep_alive
                && !client_eof
                && !(expect_continue && !continue_sent)
                && matches!(outcome, Outcome::Done { .. });
            if reset || !drainable || !drain_body(r, dec).await {
                if !reset {
                    let _ = writer.shutdown().await;
                }
                return None;
            }
        }
        match outcome {
            Outcome::Upgrade(job) => {
                let H1Reader { io: rd, buf } = reader.take()?;
                let io = rd.unsplit(writer);
                crate::websocket::run_upgrade(shared.clone(), io, buf.freeze(), job).await;
                return None;
            }
            Outcome::Done { keep_alive: ka } if !ka || !keep_alive || client_eof => {
                if !reset {
                    let _ = writer.shutdown().await;
                }
                return None;
            }
            Outcome::Done { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h2c_upgrade_headers_are_stripped() {
        let mut h: Headers = vec![
            ("Host".to_string(), "a".to_string()),
            (
                "Connection".to_string(),
                "Upgrade, HTTP2-Settings, keep-alive".to_string(),
            ),
            ("Upgrade".to_string(), "h2c".to_string()),
            (
                "HTTP2-Settings".to_string(),
                "AAMAAABkAARAAAAAAAIAAAAA".to_string(),
            ),
        ]
        .into_iter()
        .collect();
        strip_h2c_upgrade(&mut h);
        assert_eq!(
            h.0,
            vec![
                ("Host".to_string(), "a".to_string()),
                ("Connection".to_string(), "keep-alive".to_string())
            ]
        );
    }

    async fn reader_from(bytes: &'static [u8]) -> (H1Reader, tokio::io::DuplexStream) {
        let (client, server) = tokio::io::duplex(64 * 1024);
        let io: ClientIo = Box::new(server);
        let (rd, _wr) = tokio::io::split(io);
        let mut buf = BytesMut::new();
        buf.extend_from_slice(bytes);
        (H1Reader { io: rd, buf }, client)
    }

    #[tokio::test]
    async fn pipelined_requests_are_read_one_by_one() {
        let (mut reader, _client) =
            reader_from(b"GET /1 HTTP/1.1\r\nHost: a\r\n\r\nPOST /2 HTTP/1.1\r\nHost: a\r\nContent-Length: 3\r\n\r\nabcGET /3 HTTP/1.1\r\n\r\n").await;
        let HeadRead::Request(r1, _, _) = read_head(&mut reader).await else {
            panic!()
        };
        assert_eq!(r1.target, "/1");
        let _ = reader.buf.split_to(r1.head_len);
        let HeadRead::Request(r2, _, _) = read_head(&mut reader).await else {
            panic!()
        };
        assert_eq!(r2.target, "/2");
        let _ = reader.buf.split_to(r2.head_len);
        let (mut stream, rx) =
            body_stream(reader, Decoder::new(request_framing(&r2.headers).unwrap()));
        use futures::StreamExt;
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            if let Ok(BodyChunk::Data(d)) = chunk {
                body.extend_from_slice(&d);
            }
        }
        assert_eq!(body, b"abc");
        let back = rx.await.unwrap();
        assert!(back.decoder.is_done());
        let mut reader = back.reader;
        let HeadRead::Request(r3, _, _) = read_head(&mut reader).await else {
            panic!()
        };
        assert_eq!(r3.target, "/3");
    }

    #[tokio::test]
    async fn dropped_body_returns_reader_with_remaining_state() {
        let (reader, _client) = reader_from(b"0123456789").await;
        let (stream, rx) = body_stream(reader, Decoder::new(Framing::Length(100)));
        drop(stream);
        let back = rx.await.unwrap();
        assert!(!back.decoder.is_done());
        assert_eq!(&back.reader.buf[..], b"0123456789");
    }

    #[tokio::test]
    async fn truncated_body_errors() {
        let (reader, client) = reader_from(b"abc").await;
        drop(client);
        let (mut stream, _rx) = body_stream(reader, Decoder::new(Framing::Length(10)));
        use futures::StreamExt;
        assert!(matches!(stream.next().await, Some(Ok(BodyChunk::Data(_)))));
        match stream.next().await {
            Some(Err(e)) => assert_eq!(e.kind, ErrorKind::ClientAborted),
            other => panic!("{other:?}"),
        }
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn malformed_and_oversized_heads() {
        let (mut reader, _c) = reader_from(b"GET / HTTP/1.1\r\nBad Header\x01: x\r\n\r\n").await;
        // invalid header lines are ignored leniently
        assert!(matches!(
            read_head(&mut reader).await,
            HeadRead::Request(..)
        ));
        let (mut reader, _c) = reader_from(b"GET / HTTP/7.0\r\n\r\n").await;
        assert!(matches!(read_head(&mut reader).await, HeadRead::Error(_)));
        let (mut reader, c) = reader_from(b"GET / HTTP/1.1\r\nHost: a\r\n").await;
        drop(c);
        assert!(matches!(
            read_head(&mut reader).await,
            HeadRead::Error(ParseError::Invalid {
                code: "HPE_INVALID_EOF_STATE",
                ..
            })
        ));
    }
}
