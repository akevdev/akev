//! End-to-end: injecting messages into live WebSockets and `ws-breakpoint` rules (pause, edit,
//! drop, forward), against a real WebSocket echo server.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_proxy::WsInjectError;
use bytes::Bytes;
use common::*;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

const WAIT: Duration = Duration::from_secs(5);

type Ws = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

/// Breakpoint handler that hands every paused message to the test.
struct Paused {
    tx: mpsc::UnboundedSender<(PausedMessage, oneshot::Sender<MessageResume>)>,
}

#[async_trait::async_trait]
impl BreakpointHandler for Paused {
    async fn on_request(
        &self,
        _id: ExchangeId,
        _head: &RequestHead,
        request: EditableRequest,
    ) -> RequestResume {
        RequestResume::Continue { request }
    }
    async fn on_response(
        &self,
        _id: ExchangeId,
        _request: &RequestHead,
        _head: &ResponseHead,
        response: EditableResponse,
    ) -> ResponseResume {
        ResponseResume::Continue { response }
    }
    async fn on_message(&self, message: PausedMessage) -> MessageResume {
        let (tx, rx) = oneshot::channel();
        let original = MessageResume::Forward {
            kind: message.kind,
            payload: message.payload.clone(),
        };
        let _ = self.tx.send((message, tx));
        rx.await.unwrap_or(original)
    }
}

type PauseRx = mpsc::UnboundedReceiver<(PausedMessage, oneshot::Sender<MessageResume>)>;

fn paused_handler() -> (Arc<Paused>, PauseRx) {
    let (tx, rx) = mpsc::unbounded_channel();
    (Arc::new(Paused { tx }), rx)
}

async fn next_pause(rx: &mut PauseRx) -> (PausedMessage, oneshot::Sender<MessageResume>) {
    tokio::time::timeout(WAIT, rx.recv())
        .await
        .expect("a message pauses")
        .expect("handler alive")
}

fn ws_break_rules(direction: &str, contains: Option<&str>) -> akev_core::rules::RuleSet {
    let pattern = contains
        .map(|v| format!(r#","pattern":{{"op":"contains","value":"{v}"}}"#))
        .unwrap_or_default();
    rules(&format!(
        r#"{{"items":[{{"type":"rule","id":"wsb","matchers":[],"action":{{"type":"ws-breakpoint","direction":"{direction}"{pattern}}}}}]}}"#
    ))
}

/// Open a WebSocket through the proxy to the echo upstream; returns it and its exchange id.
async fn open(
    proxy: &mut TestProxy,
    upstream: std::net::SocketAddr,
    path: &str,
) -> (Ws, ExchangeId) {
    let (ws, id, _) = open_with_events(proxy, upstream, path).await;
    (ws, id)
}

/// [`open`], also returning the exchange's events up to `StreamOpened`.
async fn open_with_events(
    proxy: &mut TestProxy,
    upstream: std::net::SocketAddr,
    path: &str,
) -> (Ws, ExchangeId, Vec<ProxyEvent>) {
    let tunnel = connect_tunnel(proxy, &upstream.to_string()).await;
    let (ws, resp) = tokio_tungstenite::client_async(format!("ws://{upstream}{path}"), tunnel)
        .await
        .expect("ws handshake");
    assert_eq!(resp.status(), 101);
    let (id, events) = wait_exchange(proxy, path, |e| {
        matches!(e, ProxyEvent::StreamOpened { .. })
    })
    .await;
    (ws, id, events)
}

async fn recv(ws: &mut Ws) -> Message {
    tokio::time::timeout(WAIT, ws.next())
        .await
        .expect("a message arrives")
        .expect("stream open")
        .expect("valid message")
}

/// Assert nothing arrives for a while.
async fn silent(ws: &mut Ws) {
    if let Ok(m) = tokio::time::timeout(Duration::from_millis(300), ws.next()).await {
        panic!("unexpected message: {m:?}");
    }
}

/// Stream messages recorded for `id` (direction, kind, payload, modified).
fn recorded(events: &[ProxyEvent], id: ExchangeId) -> Vec<(Direction, MessageKind, Vec<u8>, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProxyEvent::StreamMessage {
                id: i,
                message,
                payload,
            } if *i == id => Some((
                message.direction,
                message.kind,
                payload.to_vec(),
                message.modified,
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn inject_messages_both_ways() {
    let upstream = spawn_ws_upstream().await;
    let mut proxy = start_proxy(Default::default(), Arc::new(NoBreakpoints)).await;
    let (mut ws, id) = open(&mut proxy, upstream, "/inject").await;
    let handle = proxy.handle.clone();
    assert!(handle.ws_is_open(id));

    // To the server: the echo server only accepts masked client frames, and echoes it back.
    let seq = handle
        .inject_ws_message(
            id,
            Direction::Sent,
            MessageKind::Text,
            Bytes::from_static(b"{\"to\":\"server\"}"),
        )
        .await
        .expect("injected");
    assert_eq!(recv(&mut ws).await, Message::text("{\"to\":\"server\"}"));
    // To the client, binary.
    let seq2 = handle
        .inject_ws_message(
            id,
            Direction::Received,
            MessageKind::Binary,
            Bytes::from_static(&[0xde, 0xad, 0xbe, 0xef]),
        )
        .await
        .expect("injected");
    assert!(seq2 > seq);
    assert_eq!(
        recv(&mut ws).await,
        Message::binary(vec![0xde, 0xad, 0xbe, 0xef])
    );
    // Regular traffic keeps working around injections.
    ws.send(Message::text("normal")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("normal"));

    // Refusals.
    assert_eq!(
        handle
            .inject_ws_message(
                ExchangeId(u64::MAX),
                Direction::Sent,
                MessageKind::Text,
                Bytes::new()
            )
            .await,
        Err(WsInjectError::NotOpen(ExchangeId(u64::MAX)))
    );
    assert_eq!(
        handle
            .inject_ws_message(id, Direction::Sent, MessageKind::Ping, Bytes::new())
            .await,
        Err(WsInjectError::UnsupportedKind)
    );
    assert_eq!(
        handle
            .inject_ws_message(
                id,
                Direction::Sent,
                MessageKind::Text,
                Bytes::from_static(b"\xff")
            )
            .await,
        Err(WsInjectError::InvalidText)
    );

    ws.close(None).await.unwrap();
    while let Some(Ok(_)) = ws.next().await {}
    let events = proxy
        .wait_for(
            WAIT,
            |e| matches!(e, ProxyEvent::StreamClosed { id: i, .. } if *i == id),
        )
        .await;
    let msgs = recorded(&events, id);
    assert_eq!(
        &msgs[..5],
        &[
            (
                Direction::Sent,
                MessageKind::Text,
                b"{\"to\":\"server\"}".to_vec(),
                true
            ),
            (
                Direction::Received,
                MessageKind::Text,
                b"{\"to\":\"server\"}".to_vec(),
                false
            ),
            (
                Direction::Received,
                MessageKind::Binary,
                vec![0xde, 0xad, 0xbe, 0xef],
                true
            ),
            (
                Direction::Sent,
                MessageKind::Text,
                b"normal".to_vec(),
                false
            ),
            (
                Direction::Received,
                MessageKind::Text,
                b"normal".to_vec(),
                false
            ),
        ]
    );
    // Closed: no more injections.
    let deadline = tokio::time::Instant::now() + WAIT;
    while handle.ws_is_open(id) {
        assert!(
            tokio::time::Instant::now() < deadline,
            "registry entry removed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(matches!(
        handle
            .inject_ws_message(id, Direction::Received, MessageKind::Text, Bytes::new())
            .await,
        Err(WsInjectError::NotOpen(_))
    ));
}

#[tokio::test]
async fn ws_echo_connections_take_injections_towards_the_client() {
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"echo","matchers":[],"action":{"type":"ws-echo"}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    let tunnel = connect_tunnel(&proxy, "echo.invalid:9").await;
    let (mut ws, _) = tokio_tungstenite::client_async("ws://echo.invalid:9/e", tunnel)
        .await
        .expect("ws handshake");
    let (id, _) = wait_exchange(&mut proxy, "/e", |e| {
        matches!(e, ProxyEvent::StreamOpened { .. })
    })
    .await;
    let handle = proxy.handle.clone();
    handle
        .inject_ws_message(
            id,
            Direction::Received,
            MessageKind::Text,
            Bytes::from_static(b"hey"),
        )
        .await
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("hey"));
    assert_eq!(
        handle
            .inject_ws_message(
                id,
                Direction::Sent,
                MessageKind::Text,
                Bytes::from_static(b"x")
            )
            .await,
        Err(WsInjectError::NoServer)
    );
    ws.send(Message::text("still echoing")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("still echoing"));
}

#[tokio::test]
async fn breakpoint_edit_drop_and_forward() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("sent", Some("pause")), handler).await;
    let (mut ws, id, opened) = open_with_events(&mut proxy, upstream, "/bp").await;
    // The rule matched the upgrade (a modifier: the upgrade is relayed as usual).
    assert!(opened.iter().any(
        |e| matches!(e, ProxyEvent::RuleMatched { rule, mocked: false, .. } if rule == "wsb")
    ));
    let handle = proxy.handle.clone();

    // Not matching: flows.
    ws.send(Message::text("hello")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("hello"));

    // Matching: paused; later messages wait behind it, in order.
    ws.send(Message::text("pause-1")).await.unwrap();
    let (p, decide) = next_pause(&mut pauses).await;
    assert_eq!(
        (p.id, p.direction, p.kind, &p.payload[..], p.rule.as_str()),
        (
            id,
            Direction::Sent,
            MessageKind::Text,
            &b"pause-1"[..],
            "wsb"
        )
    );
    ws.send(Message::text("after")).await.unwrap();
    silent(&mut ws).await;
    // The other direction keeps flowing...
    handle
        .inject_ws_message(
            id,
            Direction::Received,
            MessageKind::Text,
            Bytes::from_static(b"down"),
        )
        .await
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("down"));
    // ...and so do pings of the paused direction (the server's pong comes back).
    ws.send(Message::Ping(Bytes::from_static(b"keepalive")))
        .await
        .unwrap();
    assert_eq!(
        recv(&mut ws).await,
        Message::Pong(Bytes::from_static(b"keepalive"))
    );
    // Injections towards the server go out while the message is paused.
    handle
        .inject_ws_message(
            id,
            Direction::Sent,
            MessageKind::Text,
            Bytes::from_static(b"up"),
        )
        .await
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("up"));

    // Edit & forward.
    decide
        .send(MessageResume::Forward {
            kind: MessageKind::Text,
            payload: Bytes::from_static(b"edited-1"),
        })
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("edited-1"));
    assert_eq!(recv(&mut ws).await, Message::text("after"));

    // Drop.
    ws.send(Message::text("pause-2")).await.unwrap();
    let (p, decide) = next_pause(&mut pauses).await;
    assert_eq!(&p.payload[..], b"pause-2");
    decide.send(MessageResume::Drop).unwrap();
    ws.send(Message::text("next")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("next"));

    // Forward unchanged (binary edits are fine too).
    ws.send(Message::text("pause-3")).await.unwrap();
    let (p, decide) = next_pause(&mut pauses).await;
    decide
        .send(MessageResume::Forward {
            kind: p.kind,
            payload: p.payload.clone(),
        })
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("pause-3"));
    ws.send(Message::text("pause-4")).await.unwrap();
    let (_, decide) = next_pause(&mut pauses).await;
    decide
        .send(MessageResume::Forward {
            kind: MessageKind::Binary,
            payload: Bytes::from_static(&[1, 2, 3]),
        })
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::binary(vec![1u8, 2, 3]));

    ws.close(None).await.unwrap();
    while let Some(Ok(_)) = ws.next().await {}
    let events = proxy
        .wait_for(
            WAIT,
            |e| matches!(e, ProxyEvent::StreamClosed { id: i, .. } if *i == id),
        )
        .await;
    let sent: Vec<(Vec<u8>, bool)> = recorded(&events, id)
        .into_iter()
        .filter(|m| {
            m.0 == Direction::Sent && matches!(m.1, MessageKind::Text | MessageKind::Binary)
        })
        .map(|m| (m.2, m.3))
        .collect();
    assert_eq!(
        sent,
        vec![
            (b"hello".to_vec(), false),
            (b"up".to_vec(), true),
            (b"edited-1".to_vec(), true),
            (b"after".to_vec(), false),
            (b"next".to_vec(), false),
            (b"pause-3".to_vec(), false),
            (vec![1, 2, 3], true),
        ],
        "the dropped message was never sent"
    );
}

#[tokio::test]
async fn received_direction_breakpoint_lets_the_client_side_flow() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("received", None), handler).await;
    let (mut ws, id) = open(&mut proxy, upstream, "/down").await;
    // Client → server isn't paused; the echo coming back is.
    ws.send(Message::text("one")).await.unwrap();
    let (p, decide) = next_pause(&mut pauses).await;
    assert_eq!(
        (p.direction, &p.payload[..]),
        (Direction::Received, &b"one"[..])
    );
    // Keep-alive works: the client's ping reaches the server, the pong passes the paused echo.
    ws.send(Message::Ping(Bytes::from_static(b"p")))
        .await
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::Pong(Bytes::from_static(b"p")));
    decide
        .send(MessageResume::Forward {
            kind: MessageKind::Text,
            payload: Bytes::from_static(b"ONE"),
        })
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("ONE"));
    let _ = id;
}

#[tokio::test]
async fn disconnecting_client_abandons_the_pause() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("sent", None), handler).await;
    let (mut ws, id) = open(&mut proxy, upstream, "/gone").await;
    ws.send(Message::text("stuck")).await.unwrap();
    let (_, mut decide) = next_pause(&mut pauses).await;
    // The client vanishes (no close frame).
    drop(ws);
    tokio::time::timeout(WAIT, decide.closed())
        .await
        .expect("the pause is abandoned when the client disconnects");
    proxy
        .wait_for(
            WAIT,
            |e| matches!(e, ProxyEvent::StreamClosed { id: i, .. } if *i == id),
        )
        .await;
    assert!(!proxy.handle.ws_is_open(id));
}

/// Upstream that completes the handshake and hangs up after a short while.
async fn spawn_short_lived_ws_upstream() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let Ok(ws) = tokio_tungstenite::accept_async(s).await else {
                    return;
                };
                tokio::time::sleep(Duration::from_millis(400)).await;
                drop(ws);
            });
        }
    });
    addr
}

#[tokio::test]
async fn disconnecting_server_abandons_the_pause() {
    let upstream = spawn_short_lived_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("both", None), handler).await;
    let (mut ws, id) = open(&mut proxy, upstream, "/server-gone").await;
    ws.send(Message::text("waiting")).await.unwrap();
    let (_, mut decide) = next_pause(&mut pauses).await;
    tokio::time::timeout(WAIT, decide.closed())
        .await
        .expect("the pause is abandoned when the server disconnects");
    proxy
        .wait_for(
            WAIT,
            |e| matches!(e, ProxyEvent::StreamClosed { id: i, .. } if *i == id),
        )
        .await;
    // The client sees the connection end.
    let end = tokio::time::timeout(WAIT, async {
        loop {
            match ws.next().await {
                None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            }
        }
    })
    .await;
    assert!(end.is_ok(), "client connection closed");
}

#[tokio::test]
async fn disabling_the_rule_stops_pausing_open_connections() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let engine = Arc::new(akev_rules::RulesEngine::new(ws_break_rules("sent", None)));
    let mut proxy = start_proxy_engine(
        engine.clone(),
        handler,
        proxy_settings(),
        Default::default(),
    )
    .await;
    let (mut ws, _) = open(&mut proxy, upstream, "/live-rules").await;
    ws.send(Message::text("a")).await.unwrap();
    let (_, decide) = next_pause(&mut pauses).await;
    decide
        .send(MessageResume::Forward {
            kind: MessageKind::Text,
            payload: Bytes::from_static(b"a"),
        })
        .unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("a"));
    engine.set_rules(Default::default());
    ws.send(Message::text("b")).await.unwrap();
    assert_eq!(recv(&mut ws).await, Message::text("b"));
    assert!(pauses.try_recv().is_err());
}

// ---- raw client: fragmentation & large messages -------------------------------------------

fn masked(fin: bool, opcode: u8, payload: &[u8]) -> Vec<u8> {
    let mask = [0x11, 0x22, 0x33, 0x44];
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

/// Handshake by hand through a CONNECT tunnel; returns the raw stream after the 101.
async fn raw_ws(
    proxy: &TestProxy,
    upstream: std::net::SocketAddr,
    path: &str,
) -> tokio::net::TcpStream {
    let mut s = connect_tunnel(proxy, &upstream.to_string()).await;
    s.write_all(
        format!(
            "GET {path} HTTP/1.1\r\nHost: {upstream}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
             Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\r\n"
        )
        .as_bytes(),
    )
    .await
    .unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut b = [0u8; 1];
        assert_eq!(s.read(&mut b).await.unwrap(), 1, "101 response");
        head.push(b[0]);
    }
    assert!(
        head.starts_with(b"HTTP/1.1 101"),
        "{}",
        String::from_utf8_lossy(&head)
    );
    s
}

/// Read one (unmasked) frame from the proxy: (fin, opcode, payload).
async fn read_frame(s: &mut tokio::net::TcpStream) -> (bool, u8, Vec<u8>) {
    tokio::time::timeout(WAIT, async {
        let mut h = [0u8; 2];
        s.read_exact(&mut h).await.unwrap();
        let len = match h[1] & 0x7f {
            126 => {
                let mut b = [0u8; 2];
                s.read_exact(&mut b).await.unwrap();
                u16::from_be_bytes(b) as usize
            }
            127 => {
                let mut b = [0u8; 8];
                s.read_exact(&mut b).await.unwrap();
                u64::from_be_bytes(b) as usize
            }
            n => n as usize,
        };
        let mut payload = vec![0u8; len];
        s.read_exact(&mut payload).await.unwrap();
        (h[0] & 0x80 != 0, h[0] & 0x0f, payload)
    })
    .await
    .expect("a frame arrives")
}

#[tokio::test]
async fn injections_wait_for_the_end_of_a_fragmented_message() {
    let upstream = spawn_ws_upstream().await;
    let mut proxy = start_proxy(Default::default(), Arc::new(NoBreakpoints)).await;
    let mut s = raw_ws(&proxy, upstream, "/frag").await;
    let (id, _) = wait_exchange(&mut proxy, "/frag", |e| {
        matches!(e, ProxyEvent::StreamOpened { .. })
    })
    .await;
    s.write_all(&masked(false, 1, b"Hel")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    let handle = proxy.handle.clone();
    let injected = tokio::spawn(async move {
        handle
            .inject_ws_message(
                id,
                Direction::Sent,
                MessageKind::Text,
                Bytes::from_static(b"inj"),
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(
        !injected.is_finished(),
        "held until the message is complete"
    );
    s.write_all(&masked(true, 0, b"lo")).await.unwrap();
    injected
        .await
        .unwrap()
        .expect("injected after the fragmented message");
    // The echo server saw "Hello" first, then the injected message.
    assert_eq!(read_frame(&mut s).await, (true, 1, b"Hello".to_vec()));
    assert_eq!(read_frame(&mut s).await, (true, 1, b"inj".to_vec()));
}

#[tokio::test]
async fn fragmented_messages_pause_whole_and_keep_their_framing() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("sent", Some("Hello")), handler).await;
    let mut s = raw_ws(&proxy, upstream, "/frag-bp").await;
    let mut wire = masked(false, 1, b"Hel");
    wire.extend(masked(true, 9, b"mid-ping")); // a control frame in the middle
    wire.extend(masked(true, 0, b"lo"));
    // Byte by byte: the pump must cope with any split.
    for b in wire {
        s.write_all(&[b]).await.unwrap();
    }
    // The ping isn't held back with the message: the server's pong arrives first.
    assert_eq!(read_frame(&mut s).await, (true, 10, b"mid-ping".to_vec()));
    let (p, decide) = next_pause(&mut pauses).await;
    assert_eq!(&p.payload[..], b"Hello");
    decide
        .send(MessageResume::Forward {
            kind: p.kind,
            payload: p.payload.clone(),
        })
        .unwrap();
    assert_eq!(read_frame(&mut s).await, (true, 1, b"Hello".to_vec()));
    let _ = proxy.drain(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn messages_too_large_to_pause_stream_through() {
    let upstream = spawn_ws_upstream().await;
    let (handler, mut pauses) = paused_handler();
    let mut proxy = start_proxy(ws_break_rules("sent", None), handler).await;
    let (mut ws, _) = open(&mut proxy, upstream, "/big").await;
    let big = vec![b'x'; 5 * 1024 * 1024];
    ws.send(Message::binary(big.clone())).await.unwrap();
    match recv(&mut ws).await {
        Message::Binary(b) => assert_eq!(b.len(), big.len()),
        other => panic!("{other:?}"),
    }
    assert!(pauses.try_recv().is_err(), "not paused");
    // Small ones still pause.
    ws.send(Message::text("small")).await.unwrap();
    let (p, _decide) = next_pause(&mut pauses).await;
    assert_eq!(&p.payload[..], b"small");
}
