//! WebSocket relaying after a `101 Switching Protocols`.
//!
//! Frames are forwarded byte-for-byte as they arrive (fragmentation, masking keys and RSV bits
//! untouched, no buffering of large frames) while an incremental parser reassembles messages
//! for `StreamMessage` events. `Sec-WebSocket-Extensions` is stripped from the forwarded
//! handshake, so payloads are never compressed and can be captured in clear.
//!
//! Each direction runs a frame-aware [`crate::ws_pump::Pump`], so live WebSockets can take
//! injected messages ([`crate::ws_control`]) and `ws-breakpoint` rules can pause messages; a
//! paused message only holds back its own direction.

use crate::events::ExchangeEvents;
use crate::io::ClientIo;
use crate::pipeline::{UpgradeJob, UpgradeMode};
use crate::shared::Shared;
use crate::ws_control::Injection;
use crate::ws_pump::{Breakpoints, Pump};
use akev_core::{CapturedBody, Direction, MessageKind, RuleId};
use base64::Engine;
use bytes::{Bytes, BytesMut};
use sha1::{Digest, Sha1};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWriteExt};

const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
/// How long to keep relaying server → client after the client hung up first (the server may
/// still owe its close frame).
const CLOSE_GRACE: Duration = Duration::from_secs(10);
/// How long to keep relaying client → server after the server hung up first (only an
/// in-flight close reply is still interesting).
const UPSTREAM_GONE_GRACE: Duration = Duration::from_millis(500);
/// Largest frame accepted by the local echo endpoint.
const MAX_ECHO_FRAME: u64 = 16 * 1024 * 1024;

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key`.
pub fn accept_key(key: &str) -> String {
    let mut h = Sha1::new();
    h.update(key.trim().as_bytes());
    h.update(GUID.as_bytes());
    base64::engine::general_purpose::STANDARD.encode(h.finalize())
}

/// A complete message reassembled by [`FrameParser`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WsMessage {
    pub kind: MessageKind,
    /// Unmasked payload (possibly truncated to the capture limit).
    pub payload: Bytes,
    /// Full payload size.
    pub size: u64,
    /// Close frames: status code and reason.
    pub close: Option<(Option<u16>, String)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Header,
    Payload,
    /// Unparseable stream: keep forwarding, stop parsing.
    Broken,
}

/// Incremental WebSocket frame parser over arbitrary chunk boundaries.
pub struct FrameParser {
    state: State,
    hdr: [u8; 14],
    hdr_len: usize,
    fin: bool,
    opcode: u8,
    mask: Option<[u8; 4]>,
    remaining: u64,
    mask_pos: usize,
    /// Current data message (text/binary + continuations).
    msg_kind: Option<MessageKind>,
    msg: BytesMut,
    msg_size: u64,
    /// Current control frame.
    ctrl: BytesMut,
    ctrl_size: u64,
    limit: usize,
}

impl FrameParser {
    pub fn new(limit: usize) -> Self {
        FrameParser {
            state: State::Header,
            hdr: [0; 14],
            hdr_len: 0,
            fin: false,
            opcode: 0,
            mask: None,
            remaining: 0,
            mask_pos: 0,
            msg_kind: None,
            msg: BytesMut::new(),
            msg_size: 0,
            ctrl: BytesMut::new(),
            ctrl_size: 0,
            limit,
        }
    }

    fn header_len(&self) -> Option<usize> {
        if self.hdr_len < 2 {
            return None;
        }
        let len7 = self.hdr[1] & 0x7f;
        let masked = self.hdr[1] & 0x80 != 0;
        let ext = match len7 {
            126 => 2,
            127 => 8,
            _ => 0,
        };
        Some(2 + ext + if masked { 4 } else { 0 })
    }

    /// Feed bytes; completed messages are appended to `out`.
    pub fn feed(&mut self, mut data: &[u8], out: &mut Vec<WsMessage>) {
        while !data.is_empty() {
            match self.state {
                State::Broken => return,
                State::Header => {
                    let need = self.header_len().unwrap_or(2);
                    let take = (need - self.hdr_len).min(data.len());
                    self.hdr[self.hdr_len..self.hdr_len + take].copy_from_slice(&data[..take]);
                    self.hdr_len += take;
                    data = &data[take..];
                    match self.header_len() {
                        Some(n) if self.hdr_len == n => self.start_frame(out),
                        _ => continue,
                    }
                }
                State::Payload => {
                    let take = (self.remaining.min(data.len() as u64)) as usize;
                    self.payload(&data[..take]);
                    data = &data[take..];
                    self.remaining -= take as u64;
                    if self.remaining == 0 {
                        self.end_frame(out);
                    }
                }
            }
        }
    }

    fn start_frame(&mut self, out: &mut Vec<WsMessage>) {
        let b0 = self.hdr[0];
        let b1 = self.hdr[1];
        self.fin = b0 & 0x80 != 0;
        self.opcode = b0 & 0x0f;
        let len7 = (b1 & 0x7f) as u64;
        let (len, mask_at) = match len7 {
            126 => (u16::from_be_bytes([self.hdr[2], self.hdr[3]]) as u64, 4),
            127 => {
                let mut b = [0u8; 8];
                b.copy_from_slice(&self.hdr[2..10]);
                (u64::from_be_bytes(b), 10)
            }
            n => (n, 2),
        };
        self.mask = (b1 & 0x80 != 0).then(|| {
            [
                self.hdr[mask_at],
                self.hdr[mask_at + 1],
                self.hdr[mask_at + 2],
                self.hdr[mask_at + 3],
            ]
        });
        self.mask_pos = 0;
        self.remaining = len;
        self.hdr_len = 0;
        let valid = match self.opcode {
            0 => self.msg_kind.is_some(),
            1 | 2 => true,
            8..=10 => self.fin && len <= 125,
            _ => false,
        };
        if !valid {
            self.state = State::Broken;
            return;
        }
        if self.opcode == 1 || self.opcode == 2 {
            self.msg_kind = Some(if self.opcode == 1 {
                MessageKind::Text
            } else {
                MessageKind::Binary
            });
            self.msg.clear();
            self.msg_size = 0;
        }
        if self.opcode >= 8 {
            self.ctrl.clear();
            self.ctrl_size = 0;
        }
        self.state = State::Payload;
        if len == 0 {
            self.end_frame(out);
        }
    }

    fn payload(&mut self, chunk: &[u8]) {
        let control = self.opcode >= 8;
        let (buf, size) = if control {
            (&mut self.ctrl, &mut self.ctrl_size)
        } else {
            (&mut self.msg, &mut self.msg_size)
        };
        *size += chunk.len() as u64;
        let room = self.limit.saturating_sub(buf.len());
        let keep = chunk.len().min(if control { 125 } else { room });
        let start = buf.len();
        buf.extend_from_slice(&chunk[..keep]);
        if let Some(mask) = self.mask {
            for (i, b) in buf[start..].iter_mut().enumerate() {
                *b ^= mask[(self.mask_pos + i) % 4];
            }
        }
        self.mask_pos = (self.mask_pos + chunk.len()) % 4;
    }

    fn end_frame(&mut self, out: &mut Vec<WsMessage>) {
        self.state = State::Header;
        match self.opcode {
            8 => {
                let p = self.ctrl.split().freeze();
                let (code, reason) = if p.len() >= 2 {
                    (
                        Some(u16::from_be_bytes([p[0], p[1]])),
                        String::from_utf8_lossy(&p[2..]).into_owned(),
                    )
                } else {
                    (None, String::new())
                };
                out.push(WsMessage {
                    kind: MessageKind::Close,
                    size: self.ctrl_size,
                    payload: p,
                    close: Some((code, reason)),
                });
            }
            9 | 10 => {
                let kind = if self.opcode == 9 {
                    MessageKind::Ping
                } else {
                    MessageKind::Pong
                };
                out.push(WsMessage {
                    kind,
                    payload: self.ctrl.split().freeze(),
                    size: self.ctrl_size,
                    close: None,
                });
            }
            _ => {
                if self.fin
                    && let Some(kind) = self.msg_kind.take()
                {
                    out.push(WsMessage {
                        kind,
                        payload: self.msg.split().freeze(),
                        size: self.msg_size,
                        close: None,
                    });
                }
            }
        }
    }
}

/// Close frames seen on a relayed connection.
#[derive(Default)]
pub(crate) struct CloseState {
    /// The first close frame: who sent it (the initiator of the closing handshake), code and
    /// reason.
    first: Option<(Direction, Option<u16>, String)>,
    sent: bool,
    received: bool,
}

pub(crate) type SharedClose = parking_lot::Mutex<CloseState>;

/// Remember a close frame relayed in `direction`.
pub(crate) fn note_close(close: &SharedClose, direction: Direction, m: &WsMessage) {
    if let Some((code, reason)) = &m.close {
        let mut st = close.lock();
        st.first
            .get_or_insert_with(|| (direction, *code, reason.clone()));
        match direction {
            Direction::Sent => st.sent = true,
            Direction::Received => st.received = true,
        }
    }
}

/// Run an upgraded connection to completion.
pub(crate) async fn run_upgrade(
    shared: Arc<Shared>,
    client: ClientIo,
    client_leftover: Bytes,
    job: UpgradeJob,
) {
    let UpgradeJob {
        events,
        mode,
        capture_limit,
        ws_breakpoints,
    } = job;
    match mode {
        UpgradeMode::WebSocket(up) => {
            relay(
                shared,
                (client, client_leftover),
                (up.io, up.leftover),
                events,
                capture_limit,
                ws_breakpoints,
            )
            .await
        }
        UpgradeMode::Raw(up) => {
            let mut client = crate::io::rewind(client, BytesMut::from(&client_leftover[..]));
            let mut upstream = crate::io::rewind(up.io, BytesMut::from(&up.leftover[..]));
            tokio::select! {
                _ = tokio::io::copy_bidirectional(&mut client, &mut upstream) => {}
                _ = shared.hard_stop.cancelled() => {}
            }
            events.response_completed(CapturedBody::empty(), None);
        }
        UpgradeMode::Echo => echo(shared, client, client_leftover, events, capture_limit).await,
    }
}

async fn relay(
    shared: Arc<Shared>,
    (client, client_leftover): (ClientIo, Bytes),
    (upstream, upstream_leftover): (ClientIo, Bytes),
    events: Arc<ExchangeEvents>,
    limit: usize,
    ws_breakpoints: Vec<RuleId>,
) {
    let (crd, cwr) = tokio::io::split(client);
    let (urd, uwr) = tokio::io::split(upstream);
    let close = SharedClose::default();
    let (queues, _registration) = shared.ws.register(events.id, true);
    let breaks = (!ws_breakpoints.is_empty()).then(|| Breakpoints {
        rules: shared.rules.clone(),
        rule_ids: ws_breakpoints,
        handler: shared.breakpoints.clone(),
    });
    // Once one direction is done, the other one stops pausing (nobody could act on it).
    let stop_up = tokio_util::sync::CancellationToken::new();
    let stop_down = tokio_util::sync::CancellationToken::new();
    let up = Pump::new(
        events.clone(),
        Direction::Sent,
        limit,
        &close,
        breaks.as_ref(),
        queues.to_server,
        stop_up.clone(),
    )
    .run(crd, uwr, client_leftover);
    let down = Pump::new(
        events.clone(),
        Direction::Received,
        limit,
        &close,
        breaks.as_ref(),
        Some(queues.to_client),
        stop_down.clone(),
    )
    .run(urd, cwr, upstream_leftover);
    tokio::pin!(up, down);
    let first_done = tokio::select! {
        _ = &mut up => Some(Direction::Sent),
        _ = &mut down => Some(Direction::Received),
        _ = shared.hard_stop.cancelled() => None,
    };
    if let Some(first) = first_done {
        match first {
            Direction::Sent => stop_down.cancel(),
            Direction::Received => stop_up.cancel(),
        }
        // The ended side's EOF was already propagated as a write shutdown to the other peer.
        // Keep relaying the remaining direction only while it can still matter.
        let grace = {
            let st = close.lock();
            match first {
                _ if st.sent && st.received => None,
                Direction::Received => Some(UPSTREAM_GONE_GRACE),
                Direction::Sent => Some(CLOSE_GRACE),
            }
        };
        if let Some(grace) = grace {
            let remaining = if first == Direction::Sent {
                down.as_mut()
            } else {
                up.as_mut()
            };
            tokio::select! {
                _ = remaining => {}
                _ = tokio::time::sleep(grace) => {}
                _ = shared.hard_stop.cancelled() => {}
            }
        }
    }
    // Without any close frame, whoever hung up first closed the connection.
    let first_close = close.lock().first.take();
    let (by, code, reason) = match first_close {
        Some((by, code, reason)) => (by, code, (!reason.is_empty()).then_some(reason)),
        None => (first_done.unwrap_or(Direction::Received), None, None),
    };
    events.stream_closed(code, reason, by);
    events.response_completed(CapturedBody::empty(), None);
}

async fn next_injection(
    rx: &mut Option<tokio::sync::mpsc::Receiver<Injection>>,
) -> Option<Injection> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

/// Read one complete frame (header + payload) for the echo endpoint.
async fn read_frame<R: AsyncRead + Unpin>(
    io: &mut R,
    buf: &mut BytesMut,
) -> std::io::Result<Option<(bool, u8, Bytes)>> {
    loop {
        if buf.len() >= 2 {
            let len7 = (buf[1] & 0x7f) as usize;
            let masked = buf[1] & 0x80 != 0;
            let ext = match len7 {
                126 => 2,
                127 => 8,
                _ => 0,
            };
            let hlen = 2 + ext + if masked { 4 } else { 0 };
            if buf.len() >= hlen {
                let len = match len7 {
                    126 => u16::from_be_bytes([buf[2], buf[3]]) as u64,
                    127 => {
                        let mut b = [0u8; 8];
                        b.copy_from_slice(&buf[2..10]);
                        u64::from_be_bytes(b)
                    }
                    n => n as u64,
                };
                if len > MAX_ECHO_FRAME {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "frame too large",
                    ));
                }
                let total = hlen + len as usize;
                if buf.len() >= total {
                    let fin = buf[0] & 0x80 != 0;
                    let opcode = buf[0] & 0x0f;
                    let mask = masked
                        .then(|| [buf[hlen - 4], buf[hlen - 3], buf[hlen - 2], buf[hlen - 1]]);
                    let frame = buf.split_to(total);
                    let mut payload = BytesMut::from(&frame[hlen..]);
                    if let Some(m) = mask {
                        for (i, b) in payload.iter_mut().enumerate() {
                            *b ^= m[i % 4];
                        }
                    }
                    return Ok(Some((fin, opcode, payload.freeze())));
                }
            }
        }
        if crate::io::read_more(io, buf).await? == 0 {
            return Ok(None);
        }
    }
}

/// Encode an unmasked (server → client) frame.
pub fn encode_frame(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(if fin { 0x80 } else { 0 } | (opcode & 0x0f));
    match payload.len() {
        n if n < 126 => out.push(n as u8),
        n if n <= u16::MAX as usize => {
            out.push(126);
            out.extend_from_slice(&(n as u16).to_be_bytes());
        }
        n => {
            out.push(127);
            out.extend_from_slice(&(n as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    out
}

fn kind_of(opcode: u8, current: Option<MessageKind>) -> Option<MessageKind> {
    match opcode {
        0 => current,
        1 => Some(MessageKind::Text),
        2 => Some(MessageKind::Binary),
        8 => Some(MessageKind::Close),
        9 => Some(MessageKind::Ping),
        10 => Some(MessageKind::Pong),
        _ => None,
    }
}

/// `ws-echo`: accept locally and echo every message back.
async fn echo(
    shared: Arc<Shared>,
    client: ClientIo,
    leftover: Bytes,
    events: Arc<ExchangeEvents>,
    limit: usize,
) {
    let (mut rd, mut wr) = tokio::io::split(client);
    let mut buf = BytesMut::from(&leftover[..]);
    let mut message: Option<(MessageKind, BytesMut)> = None;
    let mut closed: (Option<u16>, Option<String>, Direction) = (None, None, Direction::Sent);
    // Messages can be injected towards the client (there is no server).
    let (queues, _registration) = shared.ws.register(events.id, false);
    let mut inject = Some(queues.to_client);
    loop {
        let frame = tokio::select! {
            f = read_frame(&mut rd, &mut buf) => f,
            inj = next_injection(&mut inject), if message.is_none() => {
                match inj {
                    Some(inj) => {
                        let frame = crate::ws_pump::data_frame(inj.kind, &inj.payload, false);
                        if wr.write_all(&frame).await.is_err() || wr.flush().await.is_err() {
                            break;
                        }
                        let size = inj.payload.len() as u64;
                        let captured = inj.payload.slice(..inj.payload.len().min(limit));
                        let seq = events.modified_message(Direction::Received, inj.kind, captured, size);
                        let _ = inj.done.send(seq);
                    }
                    None => inject = None,
                }
                continue;
            }
            _ = shared.hard_stop.cancelled() => break,
        };
        let Ok(Some((fin, opcode, payload))) = frame else {
            break;
        };
        let reply: Option<Vec<u8>> = match opcode {
            8 => {
                let (code, reason) = if payload.len() >= 2 {
                    (
                        Some(u16::from_be_bytes([payload[0], payload[1]])),
                        String::from_utf8_lossy(&payload[2..]).into_owned(),
                    )
                } else {
                    (None, String::new())
                };
                events.stream_message(
                    Direction::Sent,
                    MessageKind::Close,
                    payload.clone(),
                    payload.len() as u64,
                    None,
                    None,
                );
                events.stream_message(
                    Direction::Received,
                    MessageKind::Close,
                    payload.clone(),
                    payload.len() as u64,
                    None,
                    None,
                );
                closed = (
                    code,
                    (!reason.is_empty()).then_some(reason),
                    Direction::Sent,
                );
                let _ = wr.write_all(&encode_frame(true, 8, &payload)).await;
                break;
            }
            9 => {
                events.stream_message(
                    Direction::Sent,
                    MessageKind::Ping,
                    payload.clone(),
                    payload.len() as u64,
                    None,
                    None,
                );
                events.stream_message(
                    Direction::Received,
                    MessageKind::Pong,
                    payload.clone(),
                    payload.len() as u64,
                    None,
                    None,
                );
                Some(encode_frame(true, 10, &payload))
            }
            10 => None,
            0..=2 => {
                let kind = kind_of(opcode, message.as_ref().map(|m| m.0));
                if let Some(kind) = kind {
                    let entry = message.get_or_insert_with(|| (kind, BytesMut::new()));
                    entry.1.extend_from_slice(&payload);
                    if fin && let Some((kind, data)) = message.take() {
                        let data = data.freeze();
                        let captured = data.slice(..data.len().min(limit));
                        events.stream_message(
                            Direction::Sent,
                            kind,
                            captured.clone(),
                            data.len() as u64,
                            None,
                            None,
                        );
                        events.stream_message(
                            Direction::Received,
                            kind,
                            captured,
                            data.len() as u64,
                            None,
                            None,
                        );
                    }
                }
                Some(encode_frame(fin, opcode, &payload))
            }
            _ => break,
        };
        if let Some(r) = reply
            && (wr.write_all(&r).await.is_err() || wr.flush().await.is_err())
        {
            break;
        }
    }
    let _ = wr.shutdown().await;
    events.stream_closed(closed.0, closed.1, closed.2);
    events.response_completed(CapturedBody::empty(), None);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked_frame(fin: bool, opcode: u8, payload: &[u8], mask: [u8; 4]) -> Vec<u8> {
        let mut f = vec![if fin { 0x80 } else { 0 } | opcode];
        match payload.len() {
            n if n < 126 => f.push(0x80 | n as u8),
            n if n <= 65535 => {
                f.push(0x80 | 126);
                f.extend_from_slice(&(n as u16).to_be_bytes());
            }
            n => {
                f.push(0x80 | 127);
                f.extend_from_slice(&(n as u64).to_be_bytes());
            }
        }
        f.extend_from_slice(&mask);
        f.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
        f
    }

    #[test]
    fn accept_key_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn parses_masked_fragmented_and_control_frames_any_split() {
        let mut wire = Vec::new();
        wire.extend(masked_frame(false, 1, b"Hel", [1, 2, 3, 4]));
        wire.extend(masked_frame(true, 9, b"ping!", [9, 9, 9, 9])); // interleaved control frame
        wire.extend(masked_frame(true, 0, b"lo", [5, 6, 7, 8]));
        wire.extend(encode_frame(true, 2, &[0xde, 0xad]));
        let big = vec![b'x'; 70_000];
        wire.extend(encode_frame(true, 2, &big));
        let mut close = 1000u16.to_be_bytes().to_vec();
        close.extend_from_slice(b"bye");
        wire.extend(masked_frame(true, 8, &close, [0, 1, 0, 1]));
        for step in [1, 3, 17, 4096, wire.len()] {
            let mut p = FrameParser::new(1024);
            let mut out = Vec::new();
            for chunk in wire.chunks(step) {
                p.feed(chunk, &mut out);
            }
            assert_eq!(out.len(), 5, "step {step}");
            assert_eq!(
                (out[0].kind, &out[0].payload[..]),
                (MessageKind::Ping, &b"ping!"[..])
            );
            assert_eq!(
                (out[1].kind, &out[1].payload[..]),
                (MessageKind::Text, &b"Hello"[..])
            );
            assert_eq!(
                (out[2].kind, &out[2].payload[..]),
                (MessageKind::Binary, &[0xde, 0xad][..])
            );
            assert_eq!((out[3].payload.len(), out[3].size), (1024, 70_000));
            assert_eq!(out[4].close, Some((Some(1000), "bye".to_string())));
        }
    }

    #[test]
    fn garbage_stops_parsing_without_panicking() {
        let mut p = FrameParser::new(100);
        let mut out = Vec::new();
        p.feed(&[0x83, 0x00, 0xff, 0xff, 0xff], &mut out); // reserved opcode 3
        p.feed(&encode_frame(true, 1, b"x"), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn frame_encoding_lengths() {
        assert_eq!(encode_frame(true, 1, b"hi"), vec![0x81, 2, b'h', b'i']);
        assert_eq!(&encode_frame(true, 2, &[0; 200])[..4], &[0x82, 126, 0, 200]);
        assert_eq!(&encode_frame(false, 2, &[0; 70_000])[..2], &[0x02, 127]);
    }

    #[tokio::test]
    async fn read_frame_unmasks() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&masked_frame(true, 1, b"echo me", [7, 7, 7, 7]))
            .await
            .unwrap();
        let mut buf = BytesMut::new();
        let (fin, op, payload) = read_frame(&mut b, &mut buf).await.unwrap().unwrap();
        assert!(fin);
        assert_eq!(op, 1);
        assert_eq!(&payload[..], b"echo me");
    }
}
