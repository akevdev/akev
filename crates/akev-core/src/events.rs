//! Events emitted by the proxy engine as traffic flows. The store consumes these to build
//! [`crate::exchange::ExchangeDetail`] records; nothing else should need to parse raw traffic.
//!
//! Ordering guarantee: for a given `ExchangeId`, events are emitted in causal order
//! (`RequestStarted` → `RequestCompleted` → `ResponseStarted` → `ResponseCompleted`, with
//! `Aborted` possibly terminating the sequence at any point, and stream messages after the
//! upgrade). Events for different exchanges interleave freely.

use crate::body::CapturedBody;
use crate::exchange::*;
use crate::http::{Headers, RequestHead, ResponseHead};
use crate::ids::{ExchangeId, RuleId};
use crate::time::UnixMicros;
use bytes::Bytes;
use std::sync::Arc;

/// Variants differ in size by design (request/response heads vs. small notifications); events
/// are short-lived and moved through a channel once, so boxing would only add allocations.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug)]
pub enum ProxyEvent {
    /// Request head received.
    RequestStarted {
        id: ExchangeId,
        kind: ExchangeKind,
        head: RequestHead,
        client: ClientInfo,
        timing: Timing,
    },
    /// Request body fully received (and possibly forwarded).
    RequestCompleted {
        id: ExchangeId,
        body: CapturedBody,
        trailers: Option<Headers>,
        /// ms since start
        at: f64,
    },
    /// A rule or breakpoint changed the request before it was forwarded. `head`/`body` are the
    /// versions actually sent upstream; the originals stay as recorded by the earlier events.
    RequestModified {
        id: ExchangeId,
        head: RequestHead,
        body: Option<CapturedBody>,
    },
    /// Rule decision made for this exchange.
    RuleMatched {
        id: ExchangeId,
        rule: RuleId,
        /// Response generated locally (mock / map-local / file) instead of by upstream.
        mocked: bool,
    },
    /// A `highlight` rule matched: colour the row (and attach a note) unless the user already
    /// set their own colour/note on this exchange.
    Highlighted {
        id: ExchangeId,
        color: String,
        note: Option<String>,
    },
    /// Upstream connection details known.
    Upstream {
        id: ExchangeId,
        info: UpstreamInfo,
        dns_duration: Option<f64>,
        connect_duration: Option<f64>,
        tls_duration: Option<f64>,
        request_sent: Option<f64>,
    },
    /// Response head received (or generated).
    ResponseStarted {
        id: ExchangeId,
        head: ResponseHead,
        at: f64,
    },
    /// Response body fully sent to the client.
    ResponseCompleted {
        id: ExchangeId,
        body: CapturedBody,
        trailers: Option<Headers>,
        at: f64,
    },
    /// The response was modified (by rule/breakpoint) before reaching the client. `head`/`body`
    /// are what the client received; originals are from `ResponseStarted`/upstream.
    ResponseModified {
        id: ExchangeId,
        original_head: ResponseHead,
        original_body: Option<CapturedBody>,
    },
    /// Exchange paused at a breakpoint / resumed.
    Paused {
        id: ExchangeId,
        stage: BreakpointStage,
    },
    Resumed {
        id: ExchangeId,
        paused_ms: f64,
    },
    /// The exchange failed.
    Aborted {
        id: ExchangeId,
        error: ExchangeError,
        at: f64,
    },
    /// WebSocket upgrade accepted (exchange kind becomes WebSocket).
    StreamOpened {
        id: ExchangeId,
        kind: ExchangeKind,
    },
    /// One WebSocket/SSE message.
    StreamMessage {
        id: ExchangeId,
        message: StreamMessage,
        payload: Bytes,
    },
    /// Stream closed.
    StreamClosed {
        id: ExchangeId,
        code: Option<u16>,
        reason: Option<String>,
        by: Direction,
        at: f64,
    },
    /// Client TLS handshake failed (doesn't trust our certificate etc).
    TlsFailed {
        id: ExchangeId,
        client: ClientInfo,
        /// Target host (from SNI or CONNECT).
        host: String,
        port: u16,
        error: ExchangeError,
        at: UnixMicros,
    },
    /// A connection was tunnelled (not intercepted). Emitted once when opened and once when
    /// closed with final stats.
    TunnelOpened {
        id: ExchangeId,
        client: ClientInfo,
        host: String,
        port: u16,
        reason: String,
        at: UnixMicros,
    },
    TunnelClosed {
        id: ExchangeId,
        stats: TunnelStats,
        at: f64,
    },
    /// Akev handled the exchange differently from usual (a limit was reached, capture was
    /// overloaded, a breakpoint released itself). `message` is shown with the exchange;
    /// `dropped` names a body that was not recorded because of it.
    Notice {
        id: ExchangeId,
        message: String,
        dropped: Option<crate::body::Side>,
    },
    /// Garbage on the wire.
    ClientError {
        id: ExchangeId,
        client: ClientInfo,
        error: ExchangeError,
        /// Up to a few KB of what was received, for display.
        raw_prefix: Bytes,
        at: UnixMicros,
    },
}

impl ProxyEvent {
    pub fn exchange_id(&self) -> ExchangeId {
        match self {
            ProxyEvent::RequestStarted { id, .. }
            | ProxyEvent::RequestCompleted { id, .. }
            | ProxyEvent::RequestModified { id, .. }
            | ProxyEvent::RuleMatched { id, .. }
            | ProxyEvent::Highlighted { id, .. }
            | ProxyEvent::Upstream { id, .. }
            | ProxyEvent::ResponseStarted { id, .. }
            | ProxyEvent::ResponseCompleted { id, .. }
            | ProxyEvent::ResponseModified { id, .. }
            | ProxyEvent::Paused { id, .. }
            | ProxyEvent::Resumed { id, .. }
            | ProxyEvent::Aborted { id, .. }
            | ProxyEvent::StreamOpened { id, .. }
            | ProxyEvent::StreamMessage { id, .. }
            | ProxyEvent::StreamClosed { id, .. }
            | ProxyEvent::TlsFailed { id, .. }
            | ProxyEvent::TunnelOpened { id, .. }
            | ProxyEvent::TunnelClosed { id, .. }
            | ProxyEvent::Notice { id, .. }
            | ProxyEvent::ClientError { id, .. } => *id,
        }
    }

    /// Rough memory held by the event (body bytes and heads plus a fixed overhead), used to
    /// bound the queue between the proxy and the store.
    pub fn approx_bytes(&self) -> u64 {
        const BASE: u64 = 256;
        let heads =
            |h: &Headers| -> u64 { h.iter().map(|(k, v)| (k.len() + v.len() + 16) as u64).sum() };
        let extra = match self {
            ProxyEvent::RequestStarted { head, .. } => {
                (head.url.len() + head.path.len() + head.host.len()) as u64 + heads(&head.headers)
            }
            ProxyEvent::RequestCompleted { body, trailers, .. }
            | ProxyEvent::ResponseCompleted { body, trailers, .. } => {
                body.bytes.len() as u64 + trailers.as_ref().map_or(0, heads)
            }
            ProxyEvent::RequestModified { head, body, .. } => {
                heads(&head.headers)
                    + head.url.len() as u64
                    + body.as_ref().map_or(0, |b| b.bytes.len() as u64)
            }
            ProxyEvent::ResponseStarted { head, .. } => heads(&head.headers),
            ProxyEvent::ResponseModified {
                original_head,
                original_body,
                ..
            } => {
                heads(&original_head.headers)
                    + original_body.as_ref().map_or(0, |b| b.bytes.len() as u64)
            }
            ProxyEvent::StreamMessage { payload, .. } => payload.len() as u64,
            ProxyEvent::ClientError { raw_prefix, .. } => raw_prefix.len() as u64,
            ProxyEvent::Notice { message, .. } => message.len() as u64,
            _ => 0,
        };
        BASE + extra
    }

    /// Remove the captured body bytes this event carries (sizes are kept, the body is
    /// marked truncated). Returns which body was removed, if any.
    pub fn strip_body(&mut self) -> Option<StrippedBody> {
        use crate::body::Side;
        fn strip(body: &mut CapturedBody) -> bool {
            if body.bytes.is_empty() {
                return false;
            }
            body.bytes = Bytes::new();
            body.truncated = true;
            true
        }
        match self {
            ProxyEvent::RequestCompleted { body, .. }
            | ProxyEvent::RequestModified {
                body: Some(body), ..
            } => strip(body).then_some(StrippedBody::Body(Side::Request)),
            ProxyEvent::ResponseCompleted { body, .. } => {
                strip(body).then_some(StrippedBody::Body(Side::Response))
            }
            ProxyEvent::ResponseModified {
                original_body: Some(body),
                ..
            } => strip(body).then_some(StrippedBody::OriginalResponse),
            ProxyEvent::StreamMessage { payload, .. } if !payload.is_empty() => {
                *payload = Bytes::new();
                Some(StrippedBody::MessagePayload)
            }
            _ => None,
        }
    }
}

/// Which captured bytes [`ProxyEvent::strip_body`] removed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StrippedBody {
    /// The recorded request or response body.
    Body(crate::body::Side),
    /// The response body as received before a rule changed it.
    OriginalResponse,
    /// A WebSocket / SSE message payload.
    MessagePayload,
}

/// Receiver of proxy events. Implementations must be cheap and non-blocking (push to a channel
/// or update an in-memory structure under a short lock): `emit` is called on the hot path.
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: ProxyEvent);

    /// The bounded queue behind this sink, if any: the proxy waits on it briefly before
    /// starting new exchanges while the store is behind.
    fn backpressure(&self) -> Option<Arc<crate::queue::QueueState>> {
        None
    }
}

/// Sink that drops everything (useful in tests / benchmarks).
pub struct NullSink;
impl EventSink for NullSink {
    fn emit(&self, _event: ProxyEvent) {}
}

/// Sink that forwards events into an unbounded tokio channel.
pub struct ChannelSink(pub tokio::sync::mpsc::UnboundedSender<ProxyEvent>);
impl EventSink for ChannelSink {
    fn emit(&self, event: ProxyEvent) {
        let _ = self.0.send(event);
    }
}

impl<F: Fn(ProxyEvent) + Send + Sync + 'static> EventSink for F {
    fn emit(&self, event: ProxyEvent) {
        self(event)
    }
}
