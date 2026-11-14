//! One direction of a relayed WebSocket, frame-aware: forwards frames as they arrive while
//! letting Akev write messages of its own (user injections, messages edited at a
//! `ws-breakpoint`) between messages, and holds back complete messages a `ws-breakpoint` rule
//! wants paused.
//!
//! * **Streaming** (the default): header and payload bytes are forwarded as they arrive — large
//!   frames are never buffered. Frame headers are collected first (≤ 14 bytes) and control
//!   frames (≤ 125 bytes of payload) are forwarded whole, so the outgoing stream is only ever
//!   "inside" a data frame or a fragmented data message while one is being streamed.
//! * **Injection** happens only between messages (RFC 6455 forbids interleaving data frames with
//!   a fragmented message). Frames sent to the server are masked with a fresh random key.
//! * **Holding**: while a matched `ws-breakpoint` rule may apply to this direction, a data
//!   message is buffered until complete (up to [`MAX_PAUSE_MESSAGE`]; larger ones are released
//!   and streamed). If its payload matches, it is *paused*: the breakpoint handler is asked and
//!   the message is forwarded (as received when unchanged, re-framed when edited) or dropped.
//!   While paused, only this direction waits — and not even its pings/pongs: they are forwarded
//!   right away (control frames may precede the paused message), so keep-alive timeouts can't
//!   fire because of a pause. Everything else that arrives is kept, in order, for after the pause
//!   (up to [`MAX_PAUSE_BACKLOG`]; then the source isn't read until the pause ends).
//! * A pause ends without the user when the source hangs up, when the relay's other direction
//!   finished (`stop_pausing`) or on shutdown (the future is dropped): the message is then
//!   delivered as received and nothing more is paused on this connection.

use crate::events::ExchangeEvents;
use crate::websocket::{FrameParser, SharedClose, WsMessage, encode_frame, note_close};
use crate::ws_control::Injection;
use akev_core::{
    BreakpointHandler, Direction, MessageKind, MessageResume, PausedMessage, RuleId, UnixMicros,
};
use akev_rules::RulesEngine;
use bytes::{Buf, Bytes, BytesMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Largest message (frames incl. headers) held back for a `ws-breakpoint`; bigger messages
/// stream through without pausing.
pub(crate) const MAX_PAUSE_MESSAGE: usize = 4 * 1024 * 1024;
/// While a message is paused, at most this much of what follows it is buffered; beyond that
/// the source isn't read until the pause ends (back-pressure).
pub(crate) const MAX_PAUSE_BACKLOG: usize = 4 * 1024 * 1024;

/// Pause ids, unique per process.
static NEXT_PAUSE_ID: AtomicU64 = AtomicU64::new(1);

/// The `ws-breakpoint` rules a connection matched at its upgrade.
pub(crate) struct Breakpoints {
    pub rules: Arc<RulesEngine>,
    pub rule_ids: Vec<RuleId>,
    pub handler: Arc<dyn BreakpointHandler>,
}

/// Total header length once at least two header bytes are known (2 before that).
fn header_len(hdr: &[u8]) -> usize {
    if hdr.len() < 2 {
        return 2;
    }
    let ext = match hdr[1] & 0x7f {
        126 => 2,
        127 => 8,
        _ => 0,
    };
    2 + ext + if hdr[1] & 0x80 != 0 { 4 } else { 0 }
}

/// Payload length and masking key of a complete header.
fn header_fields(hdr: &[u8]) -> (u64, Option<[u8; 4]>) {
    let (len, mask_at) = match hdr[1] & 0x7f {
        126 => (u16::from_be_bytes([hdr[2], hdr[3]]) as u64, 4),
        127 => {
            let mut b = [0u8; 8];
            b.copy_from_slice(&hdr[2..10]);
            (u64::from_be_bytes(b), 10)
        }
        n => (n as u64, 2),
    };
    let mask = (hdr[1] & 0x80 != 0).then(|| {
        [
            hdr[mask_at],
            hdr[mask_at + 1],
            hdr[mask_at + 2],
            hdr[mask_at + 3],
        ]
    });
    (len, mask)
}

/// A complete single-frame text/binary message; masked (with a random key) when `mask`.
pub(crate) fn data_frame(kind: MessageKind, payload: &[u8], mask: bool) -> Vec<u8> {
    let opcode = if kind == MessageKind::Text { 1 } else { 2 };
    let mut frame = encode_frame(true, opcode, payload);
    if mask {
        let key: [u8; 4] = rand::random();
        let at = frame.len() - payload.len();
        frame[1] |= 0x80;
        frame.splice(at..at, key);
        for (i, b) in frame[at + 4..].iter_mut().enumerate() {
            *b ^= key[i % 4];
        }
    }
    frame
}

/// A data message being held back.
struct Held {
    kind: MessageKind,
    /// The frames exactly as received (headers, masking).
    raw: BytesMut,
    /// The unmasked payload.
    payload: BytesMut,
}

/// What the current frame's bytes are for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cur {
    /// Forwarded as they arrive.
    Stream,
    /// Part of the held message.
    Hold,
    /// A control frame, forwarded once complete.
    Control,
}

/// How the data message in progress is relayed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MsgRoute {
    Stream,
    Hold,
}

/// Output waiting to be reported once the bytes went out.
enum Emit {
    Parsed(WsMessage),
    Written {
        kind: MessageKind,
        payload: Bytes,
        done: Option<tokio::sync::oneshot::Sender<u32>>,
    },
}

/// A held message that matched a breakpoint, and the bytes after it.
struct Pause {
    held: Held,
    rule: RuleId,
    rest: Bytes,
}

/// Splits what arrives while a message is paused: complete pings/pongs go out right away,
/// everything else is kept (in order) for after the pause.
#[derive(Default)]
struct PauseScanner {
    hdr: Vec<u8>,
    in_payload: bool,
    remaining: u64,
    /// The current frame is a ping/pong being collected in `frame`.
    control: bool,
    frame: Vec<u8>,
    /// Unparseable: everything goes to the backlog (the relay notices later).
    broken: bool,
}

impl PauseScanner {
    fn feed(&mut self, mut data: &[u8], backlog: &mut BytesMut, mut forward: impl FnMut(&[u8])) {
        while !data.is_empty() {
            if self.broken {
                backlog.extend_from_slice(data);
                return;
            }
            if !self.in_payload {
                let take = (header_len(&self.hdr) - self.hdr.len()).min(data.len());
                self.hdr.extend_from_slice(&data[..take]);
                data = &data[take..];
                if self.hdr.len() < header_len(&self.hdr) {
                    continue;
                }
                let fin = self.hdr[0] & 0x80 != 0;
                let opcode = self.hdr[0] & 0x0f;
                let (len, _) = header_fields(&self.hdr);
                self.control = matches!(opcode, 9 | 10) && fin && len <= 125;
                self.broken = !matches!(opcode, 0..=2 | 8..=10);
                if self.control {
                    self.frame.extend_from_slice(&self.hdr);
                } else {
                    backlog.extend_from_slice(&self.hdr);
                }
                self.hdr.clear();
                self.remaining = len;
                self.in_payload = true;
            } else {
                let take = (self.remaining.min(data.len() as u64)) as usize;
                if self.control {
                    self.frame.extend_from_slice(&data[..take]);
                } else {
                    backlog.extend_from_slice(&data[..take]);
                }
                data = &data[take..];
                self.remaining -= take as u64;
            }
            if self.in_payload && self.remaining == 0 {
                self.in_payload = false;
                if self.control {
                    forward(&self.frame);
                    self.frame.clear();
                }
            }
        }
    }

    /// Bytes of an incomplete header or ping/pong (they belong after the backlog).
    fn pending(&mut self) -> Vec<u8> {
        let mut out = std::mem::take(&mut self.hdr);
        out.append(&mut self.frame);
        out
    }
}

async fn recv_injection(rx: &mut Option<mpsc::Receiver<Injection>>) -> Option<Injection> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// One direction of a relayed WebSocket.
pub(crate) struct Pump<'a> {
    events: Arc<ExchangeEvents>,
    direction: Direction,
    limit: usize,
    close: &'a SharedClose,
    breaks: Option<&'a Breakpoints>,
    inject: Option<mpsc::Receiver<Injection>>,
    /// Cancelled by the relay when the other direction finished: pauses end, none start.
    stop_pausing: CancellationToken,
    /// A pause ended without the user (source gone, relay stopping): no more pauses.
    no_more_pauses: bool,
    source_eof: bool,

    // Incoming framing.
    hdr: [u8; 14],
    hdr_len: usize,
    in_payload: bool,
    fin: bool,
    mask: Option<[u8; 4]>,
    mask_pos: usize,
    remaining: u64,
    cur: Cur,
    msg: Option<MsgRoute>,
    held: Option<Held>,
    ctrl: BytesMut,
    /// Unparseable stream: forward bytes verbatim, no more injections or pauses.
    broken: bool,

    /// Message events for what was forwarded unchanged.
    parser: FrameParser,
    parsed: Vec<WsMessage>,
    /// Bytes to write next, and what to report once they are written.
    out: BytesMut,
    emits: Vec<Emit>,
}

impl<'a> Pump<'a> {
    pub(crate) fn new(
        events: Arc<ExchangeEvents>,
        direction: Direction,
        limit: usize,
        close: &'a SharedClose,
        breaks: Option<&'a Breakpoints>,
        inject: Option<mpsc::Receiver<Injection>>,
        stop_pausing: CancellationToken,
    ) -> Self {
        Pump {
            events,
            direction,
            limit,
            close,
            breaks,
            inject,
            stop_pausing,
            no_more_pauses: false,
            source_eof: false,
            hdr: [0; 14],
            hdr_len: 0,
            in_payload: false,
            fin: false,
            mask: None,
            mask_pos: 0,
            remaining: 0,
            cur: Cur::Stream,
            msg: None,
            held: None,
            ctrl: BytesMut::new(),
            broken: false,
            parser: FrameParser::new(limit),
            parsed: Vec::new(),
            out: BytesMut::new(),
            emits: Vec::new(),
        }
    }

    /// Relay `from` → `to` until EOF or an IO error (propagated as a write shutdown).
    pub(crate) async fn run<R, W>(mut self, mut from: R, mut to: W, leftover: Bytes)
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut pending = leftover;
        let mut rbuf = BytesMut::new();
        loop {
            if !pending.is_empty() {
                let paused = self.process(std::mem::take(&mut pending));
                if !self.flush(&mut to).await {
                    break;
                }
                if let Some(p) = paused {
                    match self.pause(p, &mut from, &mut to).await {
                        Some(backlog) => pending = backlog,
                        None => break,
                    }
                }
                continue;
            }
            if self.source_eof {
                break;
            }
            let can_inject = self.can_inject();
            tokio::select! {
                biased;
                inj = recv_injection(&mut self.inject), if can_inject => match inj {
                    Some(inj) => {
                        self.inject_now(inj);
                        if !self.flush(&mut to).await {
                            break;
                        }
                    }
                    None => self.inject = None,
                },
                r = crate::io::read_more(&mut from, &mut rbuf) => match r {
                    Ok(0) | Err(_) => break,
                    Ok(_) => pending = rbuf.split().freeze(),
                },
            }
        }
        let _ = to.shutdown().await;
    }

    // ---- framing ---------------------------------------------------------------------------

    /// Route incoming bytes. Stops at a message that must be paused.
    fn process(&mut self, mut data: Bytes) -> Option<Pause> {
        while !data.is_empty() {
            if self.broken {
                self.forward(&data);
                return None;
            }
            if !self.in_payload {
                let need = header_len(&self.hdr[..self.hdr_len]);
                let take = (need - self.hdr_len).min(data.len());
                self.hdr[self.hdr_len..self.hdr_len + take].copy_from_slice(&data[..take]);
                self.hdr_len += take;
                data.advance(take);
                if self.hdr_len == header_len(&self.hdr[..self.hdr_len])
                    && let Some((held, rule)) = self.begin_frame()
                {
                    return Some(Pause {
                        held,
                        rule,
                        rest: data,
                    });
                }
            } else {
                let take = (self.remaining.min(data.len() as u64)) as usize;
                let seg = data.split_to(take);
                self.payload(&seg);
                self.remaining -= take as u64;
                if self.remaining == 0
                    && let Some((held, rule)) = self.end_frame()
                {
                    return Some(Pause {
                        held,
                        rule,
                        rest: data,
                    });
                }
            }
        }
        None
    }

    fn begin_frame(&mut self) -> Option<(Held, RuleId)> {
        let header = Bytes::copy_from_slice(&self.hdr[..self.hdr_len]);
        self.hdr_len = 0;
        let fin = header[0] & 0x80 != 0;
        let rsv = header[0] & 0x70;
        let opcode = header[0] & 0x0f;
        let (len, mask) = header_fields(&header);
        self.fin = fin;
        self.mask = mask;
        self.mask_pos = 0;
        self.remaining = len;
        match opcode {
            8..=10 if fin && len <= 125 => {
                self.ctrl.clear();
                self.ctrl.extend_from_slice(&header);
                self.cur = Cur::Control;
            }
            1 | 2 => {
                // A new message while a fragmented one is held: release that one as is.
                if let Some(held) = self.held.take() {
                    self.forward(&held.raw);
                }
                let kind = if opcode == 1 {
                    MessageKind::Text
                } else {
                    MessageKind::Binary
                };
                if rsv == 0 && self.may_hold() {
                    let mut raw =
                        BytesMut::with_capacity(header.len() + len.min(64 * 1024) as usize);
                    raw.extend_from_slice(&header);
                    self.held = Some(Held {
                        kind,
                        raw,
                        payload: BytesMut::new(),
                    });
                    self.msg = Some(MsgRoute::Hold);
                    self.cur = Cur::Hold;
                } else {
                    self.forward(&header);
                    self.msg = Some(MsgRoute::Stream);
                    self.cur = Cur::Stream;
                }
            }
            0 if self.msg.is_some() => match (self.msg, self.held.as_mut()) {
                (Some(MsgRoute::Hold), Some(held)) => {
                    held.raw.extend_from_slice(&header);
                    self.cur = Cur::Hold;
                }
                _ => {
                    self.forward(&header);
                    self.cur = Cur::Stream;
                }
            },
            _ => {
                // Not something we understand: forward everything verbatim from here on.
                if let Some(held) = self.held.take() {
                    self.forward(&held.raw);
                }
                self.forward(&header);
                self.broken = true;
                self.msg = None;
                return None;
            }
        }
        self.in_payload = true;
        if len == 0 {
            return self.end_frame();
        }
        None
    }

    fn payload(&mut self, seg: &[u8]) {
        match self.cur {
            Cur::Stream => self.forward(seg),
            Cur::Control => self.ctrl.extend_from_slice(seg),
            Cur::Hold => {
                if let Some(held) = self.held.as_mut() {
                    held.raw.extend_from_slice(seg);
                    let start = held.payload.len();
                    held.payload.extend_from_slice(seg);
                    if let Some(mask) = self.mask {
                        for (i, b) in held.payload[start..].iter_mut().enumerate() {
                            *b ^= mask[(self.mask_pos + i) % 4];
                        }
                    }
                    if held.raw.len() > MAX_PAUSE_MESSAGE {
                        // Too big to pause: release it and stream the rest of the message.
                        tracing::debug!(
                            exchange = %self.events.id,
                            "WebSocket message too large to pause; forwarding it"
                        );
                        if let Some(held) = self.held.take() {
                            self.forward(&held.raw);
                        }
                        self.msg = Some(MsgRoute::Stream);
                        self.cur = Cur::Stream;
                    }
                }
            }
        }
        self.mask_pos = (self.mask_pos + seg.len()) % 4;
    }

    fn end_frame(&mut self) -> Option<(Held, RuleId)> {
        self.in_payload = false;
        let cur = self.cur;
        self.cur = Cur::Stream;
        match cur {
            Cur::Control => {
                let frame = self.ctrl.split();
                self.forward(&frame);
            }
            Cur::Stream => {
                if self.fin {
                    self.msg = None;
                }
            }
            Cur::Hold => {
                if self.fin {
                    self.msg = None;
                    if let Some(held) = self.held.take() {
                        if let Some(rule) = self.breakpoint_for(&held) {
                            return Some((held, rule));
                        }
                        self.forward(&held.raw);
                    }
                }
            }
        }
        self.drain_injections();
        None
    }

    /// Forward received bytes unchanged (and parse them for message events).
    fn forward(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.out.extend_from_slice(bytes);
        self.parser.feed(bytes, &mut self.parsed);
        self.emits.extend(self.parsed.drain(..).map(Emit::Parsed));
    }

    // ---- injection -------------------------------------------------------------------------

    /// Whether the outgoing stream is between messages (a message of our own may go out).
    fn can_inject(&self) -> bool {
        !self.broken
            && self.msg != Some(MsgRoute::Stream)
            && !(self.in_payload && self.cur == Cur::Stream)
    }

    /// Write queued injections now, if between messages.
    fn drain_injections(&mut self) {
        while self.can_inject() {
            let Some(rx) = self.inject.as_mut() else {
                return;
            };
            match rx.try_recv() {
                Ok(inj) => self.inject_now(inj),
                Err(mpsc::error::TryRecvError::Empty) => return,
                Err(mpsc::error::TryRecvError::Disconnected) => self.inject = None,
            }
        }
    }

    fn inject_now(&mut self, inj: Injection) {
        self.write_message(inj.kind, inj.payload, Some(inj.done));
    }

    /// Write a message of our own (injected or edited), recorded as `modified`.
    fn write_message(
        &mut self,
        kind: MessageKind,
        payload: Bytes,
        done: Option<tokio::sync::oneshot::Sender<u32>>,
    ) {
        let frame = data_frame(kind, &payload, self.direction == Direction::Sent);
        self.out.extend_from_slice(&frame);
        self.emits.push(Emit::Written {
            kind,
            payload,
            done,
        });
    }

    /// Write pending output, then report it. `false` = the destination is gone.
    async fn flush<W: AsyncWrite + Unpin>(&mut self, to: &mut W) -> bool {
        if !self.out.is_empty() {
            let out = self.out.split();
            if to.write_all(&out).await.is_err() || to.flush().await.is_err() {
                self.emits.clear();
                return false;
            }
        }
        for e in self.emits.drain(..) {
            match e {
                Emit::Parsed(m) => {
                    note_close(self.close, self.direction, &m);
                    self.events.stream_message(
                        self.direction,
                        m.kind,
                        m.payload,
                        m.size,
                        None,
                        None,
                    );
                }
                Emit::Written {
                    kind,
                    payload,
                    done,
                } => {
                    let size = payload.len() as u64;
                    let captured = payload.slice(..payload.len().min(self.limit));
                    let seq = self
                        .events
                        .modified_message(self.direction, kind, captured, size);
                    if let Some(done) = done {
                        let _ = done.send(seq);
                    }
                }
            }
        }
        true
    }

    // ---- breakpoints -----------------------------------------------------------------------

    fn pausing(&self) -> bool {
        !self.no_more_pauses && !self.stop_pausing.is_cancelled()
    }

    /// Whether a message starting now must be held for a possible pause.
    fn may_hold(&self) -> bool {
        self.pausing()
            && self
                .breaks
                .is_some_and(|b| b.rules.ws_may_break(&b.rule_ids, self.direction))
    }

    fn breakpoint_for(&self, held: &Held) -> Option<RuleId> {
        if !self.pausing() {
            return None;
        }
        let b = self.breaks?;
        b.rules
            .ws_message_breakpoint(&b.rule_ids, self.direction, held.kind, &held.payload)
    }

    /// Hold a message at its breakpoint until the user decides. Returns what arrived meanwhile
    /// (to be processed next), or `None` when the destination is gone.
    async fn pause<R, W>(&mut self, p: Pause, from: &mut R, to: &mut W) -> Option<Bytes>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let Pause { held, rule, rest } = p;
        let breaks = self.breaks?;
        let original = held.payload.clone().freeze();
        let message = PausedMessage {
            id: self.events.id,
            pause_id: NEXT_PAUSE_ID.fetch_add(1, Ordering::Relaxed),
            direction: self.direction,
            kind: held.kind,
            payload: original.clone(),
            at: UnixMicros::now(),
            rule,
        };
        let mut scanner = PauseScanner::default();
        let mut backlog = BytesMut::new();
        scanner.feed(&rest, &mut backlog, |frame| self.forward(frame));
        if !self.flush(to).await {
            return None;
        }
        let handler = Arc::clone(&breaks.handler);
        let decision = handler.on_message(message);
        tokio::pin!(decision);
        let mut rbuf = BytesMut::new();
        let decision = loop {
            let read_more = !self.source_eof && backlog.len() < MAX_PAUSE_BACKLOG;
            tokio::select! {
                biased;
                d = &mut decision => break Some(d),
                _ = self.stop_pausing.cancelled() => break None,
                inj = recv_injection(&mut self.inject) => match inj {
                    // The paused message isn't out yet: we are between messages.
                    Some(inj) => {
                        self.inject_now(inj);
                        if !self.flush(to).await {
                            return None;
                        }
                    }
                    None => self.inject = None,
                },
                r = crate::io::read_more(from, &mut rbuf), if read_more => match r {
                    Ok(0) | Err(_) => {
                        self.source_eof = true;
                        break None;
                    }
                    Ok(_) => {
                        let chunk = rbuf.split().freeze();
                        scanner.feed(&chunk, &mut backlog, |frame| self.forward(frame));
                        if !self.flush(to).await {
                            return None;
                        }
                    }
                },
            }
        };
        let decision = decision.unwrap_or_else(|| {
            // Nobody can act on it anymore: deliver it as received.
            self.no_more_pauses = true;
            MessageResume::Forward {
                kind: held.kind,
                payload: original.clone(),
            }
        });
        match decision {
            MessageResume::Forward { kind, payload }
                if kind == held.kind && payload == original =>
            {
                self.forward(&held.raw);
            }
            MessageResume::Forward { kind, payload } => {
                let kind = match kind {
                    MessageKind::Text | MessageKind::Binary => kind,
                    _ => held.kind,
                };
                self.write_message(kind, payload, None);
            }
            MessageResume::Drop => {
                tracing::debug!(exchange = %self.events.id, "WebSocket message dropped at a breakpoint");
            }
        }
        backlog.extend_from_slice(&scanner.pending());
        if !self.flush(to).await {
            return None;
        }
        Some(backlog.freeze())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::websocket::encode_frame;

    #[test]
    fn masked_data_frames_decode() {
        for len in [0usize, 5, 125, 126, 70_000] {
            let payload: Vec<u8> = (0..len).map(|i| (i % 251) as u8).collect();
            let frame = data_frame(MessageKind::Binary, &payload, true);
            let mut parser = FrameParser::new(usize::MAX);
            let mut out = Vec::new();
            parser.feed(&frame, &mut out);
            assert_eq!(out.len(), 1, "len {len}");
            assert_eq!(out[0].kind, MessageKind::Binary);
            assert_eq!(&out[0].payload[..], &payload[..]);
            assert!(frame[1] & 0x80 != 0, "masked");
        }
        let plain = data_frame(MessageKind::Text, b"hi", false);
        assert_eq!(plain, encode_frame(true, 1, b"hi"));
    }

    #[test]
    fn pause_scanner_forwards_pings_and_keeps_the_rest_in_order() {
        let mut wire = Vec::new();
        wire.extend(encode_frame(false, 1, b"frag-"));
        wire.extend(encode_frame(true, 9, b"ping"));
        wire.extend(encode_frame(true, 0, b"ment"));
        wire.extend(encode_frame(true, 10, b"pong"));
        wire.extend(encode_frame(true, 8, &1000u16.to_be_bytes()));
        for step in [1, 2, 3, 7, wire.len()] {
            let mut scanner = PauseScanner::default();
            let mut backlog = BytesMut::new();
            let mut forwarded = Vec::new();
            for chunk in wire.chunks(step) {
                scanner.feed(chunk, &mut backlog, |f| forwarded.push(f.to_vec()));
            }
            assert!(scanner.pending().is_empty());
            assert_eq!(
                forwarded,
                vec![
                    encode_frame(true, 9, b"ping"),
                    encode_frame(true, 10, b"pong")
                ],
                "step {step}"
            );
            let mut expected = encode_frame(false, 1, b"frag-");
            expected.extend(encode_frame(true, 0, b"ment"));
            expected.extend(encode_frame(true, 8, &1000u16.to_be_bytes()));
            assert_eq!(&backlog[..], &expected[..], "step {step}");
        }
        // Incomplete trailing frames are handed back.
        let mut scanner = PauseScanner::default();
        let mut backlog = BytesMut::new();
        let ping = encode_frame(true, 9, b"ping");
        scanner.feed(&ping[..3], &mut backlog, |_| panic!("incomplete"));
        assert!(backlog.is_empty());
        assert_eq!(scanner.pending(), ping[..3].to_vec());
    }
}
