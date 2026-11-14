//! Per-exchange event emission with causal-order guarantees, timing bookkeeping and body
//! capture (bounded prefix + full wire size).
//!
//! The request body may still be streaming upstream when rules have already matched, the
//! upstream connection is up, or even the response has arrived (servers can answer early).
//! [`ExchangeEvents`] queues everything that follows `RequestCompleted` in the documented order
//! until it has been emitted, so consumers always see
//! `RequestStarted → RequestCompleted → RuleMatched/RequestModified → Upstream →
//! ResponseStarted → ResponseModified → ResponseCompleted` (with `Aborted` possibly ending the
//! sequence early).

use crate::upstream::ConnectTimings;
use akev_core::*;
use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use std::sync::Arc;

/// Bounded body capture: keeps the first `limit` bytes (zero-copy slices of the received
/// chunks) and counts the full wire size.
pub(crate) struct Capture {
    parts: Vec<Bytes>,
    captured: usize,
    limit: usize,
    wire: u64,
}

impl Capture {
    pub(crate) fn new(limit: usize) -> Self {
        Capture {
            parts: Vec::new(),
            captured: 0,
            limit,
            wire: 0,
        }
    }

    pub(crate) fn push(&mut self, chunk: &Bytes) {
        self.wire += chunk.len() as u64;
        if self.captured < self.limit && !chunk.is_empty() {
            let take = chunk.len().min(self.limit - self.captured);
            self.parts.push(chunk.slice(..take));
            self.captured += take;
        }
    }

    pub(crate) fn finish(&mut self) -> CapturedBody {
        let parts = std::mem::take(&mut self.parts);
        let bytes = match parts.len() {
            0 => Bytes::new(),
            1 => parts.into_iter().next().unwrap_or_default(),
            _ => {
                let mut out = BytesMut::with_capacity(self.captured);
                for p in parts {
                    out.extend_from_slice(&p);
                }
                out.freeze()
            }
        };
        let truncated = (bytes.len() as u64) < self.wire;
        let body = CapturedBody {
            bytes,
            wire_size: self.wire,
            truncated,
        };
        self.captured = 0;
        self.wire = 0;
        body
    }
}

/// Capture a fully-buffered body.
pub(crate) fn capture_full(bytes: &Bytes, limit: usize) -> CapturedBody {
    let truncated = bytes.len() > limit;
    CapturedBody {
        bytes: if truncated {
            bytes.slice(..limit)
        } else {
            bytes.clone()
        },
        wire_size: bytes.len() as u64,
        truncated,
    }
}

struct EvState {
    request_done: bool,
    finished: bool,
    /// Request-phase events (rule matches, request modifications, upstream details) waiting
    /// for `RequestCompleted`. Flushed even when the exchange aborts.
    pending: Vec<ProxyEvent>,
    /// Response-phase events waiting for `RequestCompleted`. Dropped on abort.
    queue: Vec<ProxyEvent>,
    timing: Timing,
    msg_seq: u32,
    error: Option<ExchangeError>,
}

/// Event emitter for one exchange.
pub(crate) struct ExchangeEvents {
    pub id: ExchangeId,
    sink: Arc<dyn EventSink>,
    record: bool,
    pub sw: Stopwatch,
    state: Mutex<EvState>,
}

impl ExchangeEvents {
    /// `record = false` (ignored host) suppresses all events but keeps timing bookkeeping.
    pub(crate) fn new(
        sink: Arc<dyn EventSink>,
        record: bool,
        sw: Stopwatch,
        request_headers_ms: f64,
    ) -> Arc<Self> {
        let timing = Timing {
            start: sw.started_at(),
            request_headers: Some(request_headers_ms),
            ..Default::default()
        };
        Arc::new(ExchangeEvents {
            id: ExchangeId::next(),
            sink,
            record,
            sw,
            state: Mutex::new(EvState {
                request_done: false,
                finished: false,
                pending: Vec::new(),
                queue: Vec::new(),
                timing,
                msg_seq: 0,
                error: None,
            }),
        })
    }

    pub(crate) fn now_ms(&self) -> f64 {
        self.sw.elapsed_ms()
    }

    fn emit(&self, ev: ProxyEvent) {
        if self.record {
            self.sink.emit(ev);
        }
    }

    /// Emit now if the request is complete, else queue (request phase) until it is.
    fn emit_request_phase(&self, st: &mut EvState, ev: ProxyEvent) {
        if st.request_done || st.finished {
            self.emit(ev);
        } else {
            st.pending.push(ev);
        }
    }

    /// Emit now if the request is complete, else queue until it is.
    fn emit_response_phase(&self, st: &mut EvState, ev: ProxyEvent) {
        if st.request_done {
            self.emit(ev);
        } else {
            st.queue.push(ev);
        }
    }

    pub(crate) fn request_started(
        &self,
        kind: ExchangeKind,
        head: RequestHead,
        client: ClientInfo,
    ) {
        let timing = self.state.lock().timing.clone();
        self.emit(ProxyEvent::RequestStarted {
            id: self.id,
            kind,
            head,
            client,
            timing,
        });
    }

    /// Emit `RequestCompleted` (once) and flush queued response events.
    pub(crate) fn request_completed(&self, body: CapturedBody, trailers: Option<Headers>) {
        let at = self.now_ms();
        let mut st = self.state.lock();
        if st.request_done || st.finished {
            return;
        }
        st.request_done = true;
        st.timing.request_body = Some(at);
        self.emit(ProxyEvent::RequestCompleted {
            id: self.id,
            body,
            trailers,
            at,
        });
        for ev in std::mem::take(&mut st.pending)
            .into_iter()
            .chain(std::mem::take(&mut st.queue))
        {
            self.emit(ev);
        }
    }

    pub(crate) fn request_modified(&self, head: RequestHead, body: Option<CapturedBody>) {
        let mut st = self.state.lock();
        self.emit_request_phase(
            &mut st,
            ProxyEvent::RequestModified {
                id: self.id,
                head,
                body,
            },
        );
    }

    pub(crate) fn rule_matched(&self, rule: RuleId, mocked: bool) {
        let mut st = self.state.lock();
        self.emit_request_phase(
            &mut st,
            ProxyEvent::RuleMatched {
                id: self.id,
                rule,
                mocked,
            },
        );
    }

    /// Explain why the exchange was handled differently (shown with it).
    pub(crate) fn notice(&self, message: &str) {
        let mut st = self.state.lock();
        self.emit_request_phase(
            &mut st,
            ProxyEvent::Notice {
                id: self.id,
                message: message.to_string(),
                dropped: None,
            },
        );
    }

    pub(crate) fn highlighted(&self, color: String, note: Option<String>) {
        let mut st = self.state.lock();
        self.emit_request_phase(
            &mut st,
            ProxyEvent::Highlighted {
                id: self.id,
                color,
                note,
            },
        );
    }

    /// `send_started_ms` = exchange-relative time when `UpstreamClient::send` was called
    /// (the upstream reports `request_sent` relative to that call).
    pub(crate) fn upstream(
        &self,
        info: UpstreamInfo,
        timings: &ConnectTimings,
        request_sent: Option<f64>,
        send_started_ms: f64,
    ) {
        let request_sent = request_sent.map(|ms| send_started_ms + ms);
        let mut st = self.state.lock();
        st.timing.dns_duration = timings.dns_ms;
        st.timing.connect_duration = timings.connect_ms;
        st.timing.tls_duration = timings.tls_ms;
        st.timing.request_sent = request_sent;
        let ev = ProxyEvent::Upstream {
            id: self.id,
            info,
            dns_duration: timings.dns_ms,
            connect_duration: timings.connect_ms,
            tls_duration: timings.tls_ms,
            request_sent,
        };
        self.emit_request_phase(&mut st, ev);
    }

    pub(crate) fn response_started(&self, head: ResponseHead) {
        let at = self.now_ms();
        let mut st = self.state.lock();
        if st.finished {
            return;
        }
        st.timing.response_headers = Some(at);
        self.emit_response_phase(
            &mut st,
            ProxyEvent::ResponseStarted {
                id: self.id,
                head,
                at,
            },
        );
    }

    pub(crate) fn response_modified(
        &self,
        original_head: ResponseHead,
        original_body: Option<CapturedBody>,
    ) {
        let mut st = self.state.lock();
        if st.finished {
            return;
        }
        self.emit_response_phase(
            &mut st,
            ProxyEvent::ResponseModified {
                id: self.id,
                original_head,
                original_body,
            },
        );
    }

    pub(crate) fn response_completed(&self, body: CapturedBody, trailers: Option<Headers>) {
        let at = self.now_ms();
        let mut st = self.state.lock();
        if st.finished {
            return;
        }
        st.finished = true;
        st.timing.response_body = Some(at);
        self.emit_response_phase(
            &mut st,
            ProxyEvent::ResponseCompleted {
                id: self.id,
                body,
                trailers,
                at,
            },
        );
    }

    pub(crate) fn paused(&self, stage: BreakpointStage) {
        self.emit(ProxyEvent::Paused { id: self.id, stage });
    }

    pub(crate) fn resumed(&self, paused_ms: f64) {
        {
            let mut st = self.state.lock();
            st.timing.paused_duration = Some(st.timing.paused_duration.unwrap_or(0.0) + paused_ms);
        }
        self.emit(ProxyEvent::Resumed {
            id: self.id,
            paused_ms,
        });
    }

    /// Terminate the exchange with an error (first call wins). Queued request-phase events are
    /// flushed first; queued response events are dropped: they would violate the causal order
    /// without a `RequestCompleted`.
    pub(crate) fn aborted(&self, error: ExchangeError) {
        let at = self.now_ms();
        let mut st = self.state.lock();
        if st.finished {
            return;
        }
        st.finished = true;
        for ev in std::mem::take(&mut st.pending) {
            self.emit(ev);
        }
        st.queue.clear();
        st.timing.aborted = Some(at);
        st.error = Some(error.clone());
        self.emit(ProxyEvent::Aborted {
            id: self.id,
            error,
            at,
        });
    }

    /// Terminate an exchange whose response body was cut short (upstream failure, client gone,
    /// shutdown). The bytes received so far are recorded first, as a `ResponseCompleted`
    /// whose body is marked truncated, so the partial body stays inspectable; then `Aborted`
    /// records the error. Nothing extra is emitted when no body byte arrived.
    pub(crate) fn aborted_mid_response(
        &self,
        error: ExchangeError,
        mut partial: CapturedBody,
        trailers: Option<Headers>,
    ) {
        if partial.wire_size > 0 {
            let at = self.now_ms();
            let mut st = self.state.lock();
            if st.finished {
                return;
            }
            // Only a prefix of the body exists: the capture holds what arrived, not the whole
            // body.
            partial.truncated = true;
            st.timing.response_body = Some(at);
            self.emit_response_phase(
                &mut st,
                ProxyEvent::ResponseCompleted {
                    id: self.id,
                    body: partial,
                    trailers,
                    at,
                },
            );
        }
        self.aborted(error);
    }

    pub(crate) fn stream_opened(&self, kind: ExchangeKind) {
        let mut st = self.state.lock();
        self.emit_response_phase(&mut st, ProxyEvent::StreamOpened { id: self.id, kind });
    }

    pub(crate) fn stream_message(
        &self,
        direction: Direction,
        kind: MessageKind,
        payload: Bytes,
        size: u64,
        event: Option<String>,
        event_id: Option<String>,
    ) {
        self.push_message(direction, kind, payload, size, event, event_id, false);
    }

    /// Record a WebSocket message Akev wrote itself (injected by the user, or edited at a
    /// `ws-breakpoint`): flagged `modified`. Returns its `seq`.
    pub(crate) fn modified_message(
        &self,
        direction: Direction,
        kind: MessageKind,
        payload: Bytes,
        size: u64,
    ) -> u32 {
        self.push_message(direction, kind, payload, size, None, None, true)
    }

    #[allow(clippy::too_many_arguments)]
    fn push_message(
        &self,
        direction: Direction,
        kind: MessageKind,
        payload: Bytes,
        size: u64,
        event: Option<String>,
        event_id: Option<String>,
        modified: bool,
    ) -> u32 {
        let at = self.sw.now();
        let mut st = self.state.lock();
        let seq = st.msg_seq;
        st.msg_seq = st.msg_seq.wrapping_add(1);
        let message = StreamMessage {
            seq,
            direction,
            kind,
            at,
            size,
            event,
            event_id,
            modified,
        };
        self.emit_response_phase(
            &mut st,
            ProxyEvent::StreamMessage {
                id: self.id,
                message,
                payload,
            },
        );
        seq
    }

    pub(crate) fn stream_closed(&self, code: Option<u16>, reason: Option<String>, by: Direction) {
        let at = self.now_ms();
        let mut st = self.state.lock();
        self.emit_response_phase(
            &mut st,
            ProxyEvent::StreamClosed {
                id: self.id,
                code,
                reason,
                by,
                at,
            },
        );
    }

    pub(crate) fn timing(&self) -> Timing {
        self.state.lock().timing.clone()
    }

    /// The error the exchange was aborted with, if any.
    pub(crate) fn error(&self) -> Option<ExchangeError> {
        self.state.lock().error.clone()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tokio::sync::mpsc;

    pub(crate) fn channel_events(
        record: bool,
    ) -> (Arc<ExchangeEvents>, mpsc::UnboundedReceiver<ProxyEvent>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            ExchangeEvents::new(Arc::new(ChannelSink(tx)), record, Stopwatch::start(), 0.1),
            rx,
        )
    }

    fn head() -> ResponseHead {
        ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        }
    }

    fn names(rx: &mut mpsc::UnboundedReceiver<ProxyEvent>) -> Vec<&'static str> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(match ev {
                ProxyEvent::RequestCompleted { .. } => "req-done",
                ProxyEvent::ResponseStarted { .. } => "res-start",
                ProxyEvent::ResponseCompleted { .. } => "res-done",
                ProxyEvent::StreamOpened { .. } => "stream-open",
                ProxyEvent::Aborted { .. } => "aborted",
                ProxyEvent::RuleMatched { .. } => "rule",
                ProxyEvent::Upstream { .. } => "upstream",
                _ => "other",
            });
        }
        out
    }

    #[test]
    fn capture_keeps_prefix_and_counts_wire_size() {
        let mut c = Capture::new(5);
        c.push(&Bytes::from_static(b"abc"));
        c.push(&Bytes::from_static(b"defgh"));
        c.push(&Bytes::from_static(b"ij"));
        let body = c.finish();
        assert_eq!(&body.bytes[..], b"abcde");
        assert_eq!(body.wire_size, 10);
        assert!(body.truncated);
        let mut c = Capture::new(100);
        let chunk = Bytes::from_static(b"single");
        c.push(&chunk);
        let body = c.finish();
        assert_eq!(body.bytes, chunk);
        assert!(!body.truncated);
        let full = capture_full(&Bytes::from_static(b"0123456789"), 4);
        assert_eq!(
            (&full.bytes[..], full.wire_size, full.truncated),
            (&b"0123"[..], 10, true)
        );
    }

    #[test]
    fn response_events_wait_for_request_completion() {
        let (ev, mut rx) = channel_events(true);
        ev.response_started(head());
        ev.stream_opened(ExchangeKind::EventStream);
        assert!(names(&mut rx).is_empty());
        ev.request_completed(CapturedBody::empty(), None);
        ev.request_completed(CapturedBody::empty(), None); // idempotent
        ev.response_completed(CapturedBody::empty(), None);
        assert_eq!(
            names(&mut rx),
            vec!["req-done", "res-start", "stream-open", "res-done"]
        );
        let t = ev.timing();
        assert!(
            t.request_body.is_some() && t.response_headers.is_some() && t.response_body.is_some()
        );
    }

    #[test]
    fn request_phase_events_wait_for_request_completion() {
        let (ev, mut rx) = channel_events(true);
        ev.rule_matched("r1".into(), false);
        ev.upstream(
            UpstreamInfo::default(),
            &ConnectTimings::default(),
            Some(1.0),
            2.0,
        );
        ev.response_started(head());
        assert!(names(&mut rx).is_empty());
        ev.request_completed(CapturedBody::empty(), None);
        assert_eq!(
            names(&mut rx),
            vec!["req-done", "rule", "upstream", "res-start"]
        );
        // After completion everything is emitted immediately.
        ev.rule_matched("r2".into(), false);
        assert_eq!(names(&mut rx), vec!["rule"]);
        assert_eq!(ev.timing().request_sent, Some(3.0));
    }

    #[test]
    fn abort_flushes_request_phase_drops_response_phase() {
        let (ev, mut rx) = channel_events(true);
        ev.rule_matched("r1".into(), true);
        ev.response_started(head());
        ev.aborted(ExchangeError {
            kind: ErrorKind::ClientAborted,
            message: "x".into(),
            code: None,
        });
        ev.aborted(ExchangeError {
            kind: ErrorKind::Internal,
            message: "y".into(),
            code: None,
        });
        ev.request_completed(CapturedBody::empty(), None);
        ev.response_completed(CapturedBody::empty(), None);
        assert_eq!(names(&mut rx), vec!["rule", "aborted"]);
        assert!(ev.timing().aborted.is_some());
        assert_eq!(ev.error().map(|e| e.kind), Some(ErrorKind::ClientAborted));
    }

    #[test]
    fn mid_response_abort_keeps_the_partial_body() {
        let error = || ExchangeError {
            kind: ErrorKind::ConnectionReset,
            message: "reset".into(),
            code: None,
        };
        let (ev, mut rx) = channel_events(true);
        ev.request_completed(CapturedBody::empty(), None);
        ev.response_started(head());
        let mut c = Capture::new(100);
        c.push(&Bytes::from_static(b"0123456789"));
        ev.aborted_mid_response(error(), c.finish(), None);
        let events: Vec<ProxyEvent> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        match &events[..] {
            [
                ProxyEvent::RequestCompleted { .. },
                ProxyEvent::ResponseStarted { .. },
                ProxyEvent::ResponseCompleted { body, .. },
                ProxyEvent::Aborted { error, .. },
            ] => {
                assert_eq!(&body.bytes[..], b"0123456789");
                assert_eq!(body.wire_size, 10);
                assert!(body.truncated, "only a prefix of the body arrived");
                assert_eq!(error.kind, ErrorKind::ConnectionReset);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(ev.error().map(|e| e.kind), Some(ErrorKind::ConnectionReset));
        assert!(ev.timing().aborted.is_some());

        // No body byte received: only the abort.
        let (ev, mut rx) = channel_events(true);
        ev.request_completed(CapturedBody::empty(), None);
        ev.aborted_mid_response(error(), Capture::new(100).finish(), None);
        assert_eq!(names(&mut rx), vec!["req-done", "aborted"]);

        // Request still in flight: response events are dropped as for any abort.
        let (ev, mut rx) = channel_events(true);
        ev.response_started(head());
        let mut c = Capture::new(100);
        c.push(&Bytes::from_static(b"partial"));
        ev.aborted_mid_response(error(), c.finish(), None);
        assert_eq!(names(&mut rx), vec!["aborted"]);
    }

    #[test]
    fn unrecorded_exchanges_emit_nothing() {
        let (ev, mut rx) = channel_events(false);
        ev.request_completed(CapturedBody::empty(), None);
        ev.response_started(head());
        assert!(names(&mut rx).is_empty());
        assert!(ev.timing().response_headers.is_some());
    }
}
