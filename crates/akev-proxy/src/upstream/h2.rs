//! HTTP/2 client (h2 crate): one multiplexed connection per pool key.
//!
//! Request headers are converted for HTTP/2: names lowercased, connection-specific headers
//! dropped (`connection` and the headers it lists, `keep-alive`, `proxy-connection`,
//! `transfer-encoding`, `upgrade`, `http2-settings`, `host`, and `te` unless it is exactly
//! `trailers`), `:authority` taken from `Host`. Bodies and trailers stream in both directions
//! with flow control; stream resets map to [`ErrorKind::ConnectionReset`].
//!
//! Builds with the `impersonate` feature use the `http2` crate (a fork of `h2` with the same
//! API that can also order SETTINGS and pseudo-headers and set HEADERS priorities) so the
//! connection preface can match a browser's ([`H2Spec`]); native connections are configured
//! exactly as with `h2`.

use super::error::{error, from_io, internal, protocol, timeout as timeout_error};
use super::fingerprint::profiles::H2Spec;
use super::pool::ConnMeta;
use super::{BodyChunk, BodyStream, BoxIo, RequestBody, UpstreamError};
#[cfg(not(feature = "impersonate"))]
use ::h2 as h2c;
#[cfg(feature = "impersonate")]
use ::http2 as h2c;
use akev_core::{
    ErrorKind, Headers, HttpVersion, RequestHead, ResponseHead, canonical_reason,
    string_to_header_bytes,
};
use bytes::Bytes;
use futures::{Stream, StreamExt};
use h2c::client::SendRequest;
use h2c::{Reason, RecvStream, SendStream};
use http::{HeaderName, HeaderValue};
use parking_lot::Mutex;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};
use std::time::Duration;
use tokio::time::{Instant, Sleep};

/// Per-stream receive window: the most response data buffered for a stream whose consumer
/// doesn't read (capacity is released as the consumer pulls data, so backpressure reaches the
/// server).
const STREAM_WINDOW: u32 = 2 * 1024 * 1024;
/// Connection receive window. The connection is shared by every client of the proxy, and a
/// stalled stream (slow client, `throttle` rule, paused download) keeps up to `STREAM_WINDOW`
/// of it unreleased. With a connection window barely larger than a few stream windows, a
/// handful of stalled streams would stop the server from sending anything on *any* stream of
/// the connection. So it is large (as in Go's HTTP/2 transport), making stream windows the
/// only effective limit, and [`max_streams_for`] keeps that true: a pooled
/// connection never carries more streams than the connection window can serve in full.
const CONNECTION_WINDOW: u32 = 1 << 30;
/// Most concurrent streams on one pooled connection with this stream window: even if all of
/// them stall holding a full stream window, a [`CONNECTION_WINDOW`]-sized connection window
/// still has room for another full stream window. Past this (or the server's
/// `SETTINGS_MAX_CONCURRENT_STREAMS`), an additional connection is opened.
const fn max_streams_for(stream_window: u32) -> usize {
    let window = if stream_window == 0 { 1 } else { stream_window };
    let n = (CONNECTION_WINDOW / window) as usize;
    if n > 1 { n - 1 } else { 1 }
}

/// Akamai fingerprint of the preface a connection sends: the browser's, or Akev's own.
pub(crate) fn preface_fingerprint(spec: Option<&H2Spec>) -> String {
    match spec {
        Some(spec) => spec.akamai(),
        None => super::fingerprint::profiles::native_h2_akamai(STREAM_WINDOW, CONNECTION_WINDOW),
    }
}

struct H2Shared {
    meta: Arc<ConnMeta>,
    closed: AtomicBool,
    active: AtomicUsize,
    last_used: Mutex<Instant>,
    /// Per-stream receive window (`SETTINGS_INITIAL_WINDOW_SIZE`).
    stream_window: u32,
    /// Streams allowed on this connection (see [`max_streams_for`]).
    max_streams: usize,
    /// Connection receive window currently targeted. Browser profiles start with the
    /// browser's (small) connection window for an authentic preface; it grows as streams are
    /// opened so the invariant behind [`max_streams_for`] keeps holding.
    connection_window: std::sync::atomic::AtomicU32,
    /// Wakes the connection driver to apply a new `connection_window`.
    grow: tokio::sync::Notify,
}

/// A shared handle to a multiplexed HTTP/2 connection.
#[derive(Clone)]
pub(crate) struct H2Handle {
    send: SendRequest<Bytes>,
    shared: Arc<H2Shared>,
}

impl H2Handle {
    /// Connection metadata (pool key, base upstream info).
    pub(crate) fn meta(&self) -> &Arc<ConnMeta> {
        &self.shared.meta
    }

    /// Whether the connection has terminated (GOAWAY, IO error, closed).
    pub(crate) fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Whether this is the same connection as `other`.
    pub(crate) fn same(&self, other: &H2Handle) -> bool {
        Arc::ptr_eq(&self.shared, &other.shared)
    }

    /// Whether another stream can be opened on this connection without waiting for streams
    /// that may be stalled by slow consumers: fewer active streams than both the server's
    /// concurrency limit and [`max_streams_for`] the connection's stream window.
    pub(crate) fn has_capacity(&self) -> bool {
        let limit = self
            .shared
            .max_streams
            .min(self.send.current_max_send_streams());
        !self.is_closed() && self.active_streams() < limit
    }

    /// Streams whose response is still being received (or awaited).
    pub(crate) fn active_streams(&self) -> usize {
        self.shared.active.load(Ordering::Acquire)
    }

    /// Idle duration (zero while streams are active).
    pub(crate) fn idle_for(&self) -> Duration {
        if self.shared.active.load(Ordering::Acquire) > 0 {
            return Duration::ZERO;
        }
        self.shared.last_used.lock().elapsed()
    }

    fn touch(&self) {
        *self.shared.last_used.lock() = Instant::now();
    }
}

/// Decrements the active-stream count when a response body finishes or is dropped.
struct ActiveGuard(Arc<H2Shared>);

impl ActiveGuard {
    fn new(shared: &Arc<H2Shared>) -> Self {
        shared.active.fetch_add(1, Ordering::AcqRel);
        ActiveGuard(shared.clone())
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        *self.0.last_used.lock() = Instant::now();
        self.0.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Apply a browser's preface (SETTINGS values and order, connection window, pseudo-header
/// order, HEADERS priority).
#[cfg(feature = "impersonate")]
fn apply_spec(builder: &mut h2c::client::Builder, spec: &H2Spec) {
    use super::fingerprint::profiles::{Pseudo, setting};
    use h2c::frame::{PseudoId, PseudoOrder, SettingId, SettingsOrder, StreamDependency, StreamId};
    let mut order = SettingsOrder::builder();
    for &(id, value) in spec.settings {
        let id = match id {
            setting::HEADER_TABLE_SIZE => {
                builder.header_table_size(value);
                SettingId::HeaderTableSize
            }
            setting::ENABLE_PUSH => {
                builder.enable_push(value != 0);
                SettingId::EnablePush
            }
            setting::MAX_CONCURRENT_STREAMS => {
                builder.max_concurrent_streams(value);
                SettingId::MaxConcurrentStreams
            }
            setting::INITIAL_WINDOW_SIZE => {
                builder.initial_window_size(value);
                SettingId::InitialWindowSize
            }
            setting::MAX_FRAME_SIZE => {
                builder.max_frame_size(value);
                SettingId::MaxFrameSize
            }
            setting::MAX_HEADER_LIST_SIZE => {
                builder.max_header_list_size(value);
                SettingId::MaxHeaderListSize
            }
            setting::NO_RFC7540_PRIORITIES => {
                builder.no_rfc7540_priorities(value != 0);
                SettingId::NoRfc7540Priorities
            }
            other => {
                tracing::warn!(
                    id = other,
                    "unsupported HTTP/2 setting in a browser profile"
                );
                continue;
            }
        };
        order = order.push(id);
    }
    builder.settings_order(order.build());
    builder.initial_connection_window_size(spec.connection_window);
    let pseudo = spec.pseudo_order.iter().map(|p| match p {
        Pseudo::Method => PseudoId::Method,
        Pseudo::Scheme => PseudoId::Scheme,
        Pseudo::Authority => PseudoId::Authority,
        Pseudo::Path => PseudoId::Path,
    });
    builder.headers_pseudo_order(PseudoOrder::builder().extend(pseudo).build());
    if let Some((weight, exclusive)) = spec.headers_priority {
        builder.headers_stream_dependency(StreamDependency::new(
            StreamId::zero(),
            weight,
            exclusive,
        ));
    }
}

/// Perform the HTTP/2 handshake on an established (ALPN `h2`) connection and spawn its driver.
/// `spec` = the browser preface to send (impersonation), `None` for Akev's own.
pub(crate) async fn handshake(
    io: BoxIo,
    meta: Arc<ConnMeta>,
    spec: Option<&'static H2Spec>,
) -> Result<H2Handle, UpstreamError> {
    let mut builder = h2c::client::Builder::new();
    let (stream_window, connection_window) = match spec {
        #[cfg(feature = "impersonate")]
        Some(spec) => {
            apply_spec(&mut builder, spec);
            (spec.stream_window(), spec.connection_window)
        }
        _ => {
            builder
                .initial_window_size(STREAM_WINDOW)
                .initial_connection_window_size(CONNECTION_WINDOW)
                .enable_push(false);
            (STREAM_WINDOW, CONNECTION_WINDOW)
        }
    };
    let (send, mut connection) = builder
        .handshake::<_, Bytes>(io)
        .await
        .map_err(|e| map_h2_error(&e, &meta.peer))?;
    let shared = Arc::new(H2Shared {
        meta,
        closed: AtomicBool::new(false),
        active: AtomicUsize::new(0),
        last_used: Mutex::new(Instant::now()),
        stream_window,
        max_streams: max_streams_for(stream_window),
        connection_window: std::sync::atomic::AtomicU32::new(connection_window),
        grow: tokio::sync::Notify::new(),
    });
    let driver_shared = shared.clone();
    tokio::spawn(async move {
        let result = loop {
            tokio::select! {
                r = &mut connection => break r,
                _ = driver_shared.grow.notified() => {
                    let target = driver_shared.connection_window.load(Ordering::Acquire);
                    connection.set_target_window_size(target);
                }
            }
        };
        if let Err(e) = result {
            tracing::debug!(peer = %driver_shared.meta.peer, error = %e, "HTTP/2 connection ended with an error");
        }
        driver_shared.closed.store(true, Ordering::Release);
    });
    Ok(H2Handle { send, shared })
}

/// Before opening a stream: make sure the connection window can hold a full stream window for
/// every active stream plus one, so streams stalled by slow consumers never block the others
/// (a no-op for native connections, whose window is sized for the stream limit up front).
fn ensure_connection_window(shared: &H2Shared) {
    let active = shared.active.load(Ordering::Acquire) as u64;
    let needed = ((active + 1) * u64::from(shared.stream_window)).min(i32::MAX as u64) as u32;
    if shared.connection_window.fetch_max(needed, Ordering::AcqRel) < needed {
        shared.grow.notify_one();
    }
}

/// Map an h2 error.
pub(crate) fn map_h2_error(e: &h2c::Error, peer: &str) -> UpstreamError {
    if let Some(io) = e.get_io() {
        return from_io(io, format_args!("HTTP/2 connection to {peer}"));
    }
    match e.reason() {
        Some(reason) if e.is_go_away() => error(
            ErrorKind::ConnectionReset,
            format!("{peer} closed the HTTP/2 connection (GOAWAY {reason:?})"),
            &format!("H2_GOAWAY_{reason:?}"),
        ),
        Some(reason) if e.is_reset() && e.is_remote() => error(
            ErrorKind::ConnectionReset,
            format!("HTTP/2 stream reset by {peer} ({reason:?})"),
            &format!("H2_{reason:?}"),
        ),
        Some(reason) if e.is_library() || !e.is_remote() => error(
            ErrorKind::UpstreamProtocolError,
            format!("HTTP/2 protocol error talking to {peer}: {e}"),
            &format!("H2_{reason:?}"),
        ),
        Some(reason) => error(
            ErrorKind::ConnectionReset,
            format!("HTTP/2 error from {peer}: {e}"),
            &format!("H2_{reason:?}"),
        ),
        None => protocol(format!("HTTP/2 error talking to {peer}: {e}")),
    }
}

/// Whether a request whose response failed with this error can be retried on a new
/// connection: the server guarantees it didn't process the stream (`REFUSED_STREAM`, graceful
/// `GOAWAY` past our stream id), or the connection died underneath (a stale connection; the
/// same rule browsers apply to reused connections).
fn retryable(e: &h2c::Error) -> bool {
    e.reason() == Some(Reason::REFUSED_STREAM)
        || (e.is_go_away() && e.reason() == Some(Reason::NO_ERROR))
        || e.is_io()
}

/// Whether opening a stream failed because of the connection (GOAWAY received, IO error...)
/// rather than the request itself. Nothing was sent, so a new connection may be tried.
fn retryable_open(e: &h2c::Error) -> bool {
    retryable(e) || e.is_go_away()
}

const DROPPED: &[&str] = &[
    "host",
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
    "http2-settings",
];

/// Build the HTTP/2 request (pseudo-headers from the URI) with converted headers.
/// `default_authority` (the normalised target) is used when the request has neither
/// `:authority` nor `Host`.
pub(crate) fn build_request(
    head: &RequestHead,
    default_authority: &str,
    body: &RequestBody,
) -> Result<http::Request<()>, UpstreamError> {
    let authority = head
        .headers
        .get(":authority")
        .or_else(|| head.headers.get("host"))
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .unwrap_or(default_authority);
    let mut path = String::with_capacity(head.path.len() + 1);
    if !head.path.starts_with('/') && head.path != "*" {
        path.push('/');
    }
    for ch in head.path.chars() {
        if ch.is_ascii_graphic() {
            path.push(ch);
        } else {
            let mut buf = [0u8; 4];
            for b in ch.encode_utf8(&mut buf).bytes() {
                path.push_str(&format!("%{b:02X}"));
            }
        }
    }
    let uri = http::Uri::builder()
        .scheme("https")
        .authority(authority)
        .path_and_query(path.as_str())
        .build()
        .map_err(|e| {
            internal(format!(
                "cannot build HTTP/2 request URI from {authority:?} + {path:?}: {e}"
            ))
        })?;
    let method = http::Method::from_bytes(head.method.as_bytes())
        .map_err(|_| internal(format!("invalid request method {:?}", head.method)))?;
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .version(http::Version::HTTP_2)
        .body(())
        .map_err(|e| internal(format!("cannot build HTTP/2 request: {e}")))?;

    let connection_tokens: Vec<String> = head
        .headers
        .get_all("connection")
        .flat_map(|v| v.split(','))
        .map(|t| t.trim().to_ascii_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    let map = request.headers_mut();
    for (name, value) in head.headers.iter() {
        if name.starts_with(':') {
            continue;
        }
        let lower = name.to_ascii_lowercase();
        if DROPPED.contains(&lower.as_str()) || connection_tokens.contains(&lower) {
            continue;
        }
        if lower == "te" && !value.trim().eq_ignore_ascii_case("trailers") {
            continue;
        }
        match (
            HeaderName::from_bytes(lower.as_bytes()),
            HeaderValue::from_bytes(&string_to_header_bytes(value)),
        ) {
            (Ok(n), Ok(v)) => {
                map.append(n, v);
            }
            _ => tracing::warn!(name, "dropping request header invalid for HTTP/2"),
        }
    }
    // Keep content-length consistent with the actual body (h2 enforces it).
    let actual = match body {
        RequestBody::Empty => Some(0),
        RequestBody::Full(b) => Some(b.len() as u64),
        RequestBody::Stream { length, .. } => *length,
    };
    if let (Some(n), true) = (actual, map.contains_key(http::header::CONTENT_LENGTH)) {
        map.insert(http::header::CONTENT_LENGTH, HeaderValue::from(n));
    }
    Ok(request)
}

/// Send `data` respecting flow control.
async fn send_data_all(
    stream: &mut SendStream<Bytes>,
    mut data: Bytes,
    end: bool,
) -> Result<(), h2c::Error> {
    if data.is_empty() {
        if end {
            stream.send_data(Bytes::new(), true)?;
        }
        return Ok(());
    }
    while !data.is_empty() {
        stream.reserve_capacity(data.len());
        let capacity = match std::future::poll_fn(|cx| stream.poll_capacity(cx)).await {
            Some(c) => c?,
            None => return Err(h2c::Error::from(Reason::STREAM_CLOSED)),
        };
        if capacity == 0 {
            continue;
        }
        let chunk = data.split_to(capacity.min(data.len()));
        stream.send_data(chunk, end && data.is_empty())?;
    }
    Ok(())
}

enum BodyError {
    Source(UpstreamError),
    H2(h2c::Error),
}

async fn send_body(mut stream: SendStream<Bytes>, body: RequestBody) -> Result<Instant, BodyError> {
    match body {
        RequestBody::Empty => stream
            .send_data(Bytes::new(), true)
            .map_err(BodyError::H2)?,
        RequestBody::Full(b) => send_data_all(&mut stream, b, true)
            .await
            .map_err(BodyError::H2)?,
        RequestBody::Stream {
            stream: mut source, ..
        } => {
            let mut trailers: Option<Headers> = None;
            while let Some(item) = source.next().await {
                match item {
                    Ok(BodyChunk::Data(d)) => send_data_all(&mut stream, d, false)
                        .await
                        .map_err(BodyError::H2)?,
                    Ok(BodyChunk::Trailers(t)) => match trailers.as_mut() {
                        Some(existing) => existing.0.extend(t.0),
                        None => trailers = Some(t),
                    },
                    Err(e) => {
                        stream.send_reset(Reason::CANCEL);
                        return Err(BodyError::Source(e));
                    }
                }
            }
            match trailers {
                Some(t) => stream
                    .send_trailers(t.to_header_map())
                    .map_err(BodyError::H2)?,
                None => stream
                    .send_data(Bytes::new(), true)
                    .map_err(BodyError::H2)?,
            }
        }
    }
    Ok(Instant::now())
}

/// Request parameters.
pub(crate) struct H2Request<'a> {
    pub head: &'a RequestHead,
    /// Normalised target authority (`:authority` fallback).
    pub authority: &'a str,
    pub body: RequestBody,
    pub head_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub started: Instant,
}

/// Failure of an HTTP/2 exchange.
pub(crate) enum H2Error {
    /// Not processed by the server (refused stream, connection gone before sending): retry
    /// on a new connection.
    Retryable {
        error: UpstreamError,
        body: RequestBody,
    },
    Fatal(UpstreamError),
}

/// Successful exchange.
pub(crate) struct H2Outcome {
    pub head: ResponseHead,
    pub body: BodyStream,
    pub request_sent_ms: Option<f64>,
}

/// Send a request on a shared connection.
pub(crate) async fn send(handle: &H2Handle, req: H2Request<'_>) -> Result<H2Outcome, H2Error> {
    let peer = handle.shared.meta.peer.clone();
    let request = build_request(req.head, req.authority, &req.body).map_err(H2Error::Fatal)?;
    let body = req.body;
    let send = match handle.send.clone().ready().await {
        Ok(s) => s,
        Err(e) => {
            return Err(H2Error::Retryable {
                error: map_h2_error(&e, &peer),
                body,
            });
        }
    };
    let mut send = send;
    let end_of_stream = match &body {
        RequestBody::Empty => true,
        RequestBody::Full(b) => b.is_empty(),
        RequestBody::Stream { .. } => false,
    };
    let (response, stream) = match send.send_request(request, end_of_stream) {
        Ok(r) => r,
        Err(e) if retryable_open(&e) => {
            return Err(H2Error::Retryable {
                error: map_h2_error(&e, &peer),
                body,
            });
        }
        Err(e) => return Err(H2Error::Fatal(map_h2_error(&e, &peer))),
    };
    handle.touch();
    let guard = ActiveGuard::new(&handle.shared);
    ensure_connection_window(&handle.shared);
    // Keep a copy of replayable bodies for a retry after REFUSED_STREAM.
    let replay = match &body {
        RequestBody::Empty => Some(RequestBody::Empty),
        RequestBody::Full(b) => Some(RequestBody::Full(b.clone())),
        RequestBody::Stream { .. } => None,
    };
    let mut sent: Option<Instant> = None;
    let mut sender = if end_of_stream {
        sent = Some(Instant::now());
        drop(stream);
        None
    } else {
        Some(tokio::spawn(send_body(stream, body)))
    };
    let mut deadline: Option<(Instant, Duration)> = if sent.is_some() {
        req.head_timeout.map(|t| (Instant::now() + t, t))
    } else {
        None
    };
    tokio::pin!(response);
    let result = loop {
        let timer = async {
            match deadline {
                Some((at, _)) => tokio::time::sleep_until(at).await,
                None => std::future::pending().await,
            }
        };
        let join = async {
            match sender.as_mut() {
                Some(h) => h.await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            biased;
            res = &mut response => break res,
            joined = join => {
                sender = None;
                match joined {
                    Ok(Ok(at)) => {
                        sent = Some(at);
                        deadline = req.head_timeout.map(|t| (Instant::now() + t, t));
                    }
                    Ok(Err(BodyError::Source(e))) => return Err(H2Error::Fatal(e)),
                    // The server may still answer (e.g. it reset the stream after responding
                    // early); keep waiting for the response.
                    Ok(Err(BodyError::H2(e))) => tracing::debug!(peer = %peer, error = %e, "HTTP/2 request body not fully sent"),
                    Err(join) => return Err(H2Error::Fatal(internal(format!("request body sender failed: {join}")))),
                }
            }
            _ = timer => {
                if let Some(h) = sender.take() {
                    h.abort();
                }
                let after = deadline.map(|(_, d)| d).unwrap_or_default();
                return Err(H2Error::Fatal(timeout_error(format_args!("waiting for a response from {peer}"), after)));
            }
        }
    };
    let response = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(h) = sender.take() {
                h.abort();
            }
            return Err(match (retryable(&e), replay) {
                (true, Some(body)) => H2Error::Retryable {
                    error: map_h2_error(&e, &peer),
                    body,
                },
                _ => H2Error::Fatal(map_h2_error(&e, &peer)),
            });
        }
    };
    let (parts, recv) = response.into_parts();
    let status = parts.status.as_u16();
    let head = ResponseHead {
        status,
        status_message: canonical_reason(status).to_owned(),
        http_version: HttpVersion::H2,
        headers: Headers::from_header_map(&parts.headers),
    };
    let request_sent_ms =
        sent.map(|at| at.saturating_duration_since(req.started).as_secs_f64() * 1000.0);
    let body = H2Body {
        recv,
        phase: Phase::Data,
        idle_timeout: req.idle_timeout,
        idle: None,
        peer,
        _sender: sender.map(AbortOnDrop),
        _guard: guard,
    };
    Ok(H2Outcome {
        head,
        body: Box::pin(body),
        request_sent_ms,
    })
}

/// Aborts a still-running request body sender when the response is dropped.
struct AbortOnDrop(tokio::task::JoinHandle<Result<Instant, BodyError>>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

enum Phase {
    Data,
    Trailers,
    Done,
}

struct H2Body {
    recv: RecvStream,
    phase: Phase,
    idle_timeout: Option<Duration>,
    idle: Option<Pin<Box<Sleep>>>,
    peer: Arc<str>,
    _sender: Option<AbortOnDrop>,
    _guard: ActiveGuard,
}

impl H2Body {
    fn idle_expired(&mut self, cx: &mut Context<'_>) -> Option<UpstreamError> {
        let dur = self.idle_timeout?;
        let sleep = self
            .idle
            .get_or_insert_with(|| Box::pin(tokio::time::sleep(dur)));
        if sleep.as_mut().poll(cx).is_ready() {
            self.phase = Phase::Done;
            return Some(timeout_error(
                format_args!("waiting for response body data from {}", self.peer),
                dur,
            ));
        }
        None
    }

    fn reset_idle(&mut self) {
        if let (Some(dur), Some(sleep)) = (self.idle_timeout, self.idle.as_mut()) {
            sleep.as_mut().reset(Instant::now() + dur);
        }
    }
}

impl Stream for H2Body {
    type Item = Result<BodyChunk, UpstreamError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();
        loop {
            match this.phase {
                Phase::Done => return Poll::Ready(None),
                Phase::Data => match this.recv.poll_data(cx) {
                    Poll::Ready(Some(Ok(data))) => {
                        let _ = this.recv.flow_control().release_capacity(data.len());
                        if data.is_empty() {
                            continue;
                        }
                        this.reset_idle();
                        return Poll::Ready(Some(Ok(BodyChunk::Data(data))));
                    }
                    Poll::Ready(Some(Err(e))) => {
                        this.phase = Phase::Done;
                        return Poll::Ready(Some(Err(map_h2_error(&e, &this.peer))));
                    }
                    Poll::Ready(None) => this.phase = Phase::Trailers,
                    Poll::Pending => {
                        return match this.idle_expired(cx) {
                            Some(e) => Poll::Ready(Some(Err(e))),
                            None => Poll::Pending,
                        };
                    }
                },
                Phase::Trailers => match this.recv.poll_trailers(cx) {
                    Poll::Ready(Ok(Some(map))) => {
                        this.phase = Phase::Done;
                        return Poll::Ready(Some(Ok(BodyChunk::Trailers(
                            Headers::from_header_map(&map),
                        ))));
                    }
                    Poll::Ready(Ok(None)) => {
                        this.phase = Phase::Done;
                        return Poll::Ready(None);
                    }
                    Poll::Ready(Err(e)) => {
                        this.phase = Phase::Done;
                        return Poll::Ready(Some(Err(map_h2_error(&e, &this.peer))));
                    }
                    Poll::Pending => {
                        return match this.idle_expired(cx) {
                            Some(e) => Poll::Ready(Some(Err(e))),
                            None => Poll::Pending,
                        };
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(headers: &[(&str, &str)]) -> RequestHead {
        RequestHead {
            method: "POST".into(),
            url: "https://example.test/a?b=c".into(),
            scheme: "https".into(),
            host: "example.test".into(),
            port: 443,
            path: "/a?b=c".into(),
            http_version: HttpVersion::Http11,
            headers: Headers(
                headers
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect(),
            ),
        }
    }

    #[test]
    fn converts_headers_for_h2() {
        let h = head(&[
            ("Host", "example.test:8443"),
            ("Connection", "keep-alive, X-Hop"),
            ("X-Hop", "1"),
            ("Keep-Alive", "timeout=5"),
            ("Proxy-Connection", "keep-alive"),
            ("Transfer-Encoding", "chunked"),
            ("Upgrade", "h2c"),
            ("TE", "trailers"),
            ("Content-Type", "text/plain"),
            ("X-Multi", "a"),
            ("x-multi", "b"),
            ("Content-Length", "99"),
        ]);
        let req = build_request(
            &h,
            "unused.test",
            &RequestBody::Full(Bytes::from_static(b"hello")),
        )
        .unwrap();
        assert_eq!(req.uri().authority().unwrap().as_str(), "example.test:8443");
        assert_eq!(req.uri().path_and_query().unwrap().as_str(), "/a?b=c");
        assert_eq!(req.uri().scheme_str(), Some("https"));
        let names: Vec<&str> = req.headers().keys().map(|k| k.as_str()).collect();
        assert!(!names.contains(&"host"));
        assert!(!names.contains(&"connection"));
        assert!(!names.contains(&"x-hop"));
        assert!(!names.contains(&"keep-alive"));
        assert!(!names.contains(&"proxy-connection"));
        assert!(!names.contains(&"transfer-encoding"));
        assert!(!names.contains(&"upgrade"));
        assert_eq!(req.headers()["te"], "trailers");
        assert_eq!(req.headers().get_all("x-multi").iter().count(), 2);
        assert_eq!(req.headers()["content-length"], "5");
    }

    #[test]
    fn te_other_than_trailers_is_dropped_and_authority_falls_back() {
        let h = head(&[("te", "gzip")]);
        let req = build_request(&h, "example.test:8443", &RequestBody::Empty).unwrap();
        assert!(req.headers().get("te").is_none());
        assert_eq!(req.uri().authority().unwrap().as_str(), "example.test:8443");
    }

    #[test]
    fn non_ascii_paths_are_encoded() {
        let mut h = head(&[]);
        h.path = "/caf\u{e9} x".into();
        let req = build_request(&h, "example.test", &RequestBody::Empty).unwrap();
        assert_eq!(req.uri().path(), "/caf%C3%A9%20x");
    }

    #[test]
    fn error_mapping() {
        let e = h2c::Error::from(Reason::REFUSED_STREAM);
        assert!(retryable(&e));
        let mapped = map_h2_error(&e, "h:443");
        assert_eq!(mapped.code.as_deref(), Some("H2_REFUSED_STREAM"));
        let goaway = h2c::Error::from(Reason::NO_ERROR);
        assert!(!map_h2_error(&goaway, "h:443").message.is_empty());
    }
}
