//! End-to-end: WebSocket relaying with message events, permessage-deflate stripping and the
//! `ws-echo` rule.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use common::*;
use futures::{SinkExt, StreamExt};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::protocol::CloseFrame;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;

fn messages(events: &[ProxyEvent]) -> Vec<(Direction, MessageKind, Vec<u8>)> {
    events
        .iter()
        .filter_map(|e| match e {
            ProxyEvent::StreamMessage {
                message, payload, ..
            } => Some((message.direction, message.kind, payload.to_vec())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn websocket_relay_records_messages_both_ways() {
    let upstream = spawn_ws_upstream().await;
    let mut proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let tunnel = connect_tunnel(&proxy, &upstream.to_string()).await;
    let (mut ws, resp) = tokio_tungstenite::client_async(format!("ws://{upstream}/chat"), tunnel)
        .await
        .expect("ws handshake");
    assert_eq!(resp.status(), 101);
    ws.send(Message::text("hello")).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::text("hello"));
    ws.send(Message::binary(vec![1u8, 2, 3])).await.unwrap();
    assert_eq!(
        ws.next().await.unwrap().unwrap(),
        Message::binary(vec![1u8, 2, 3])
    );
    ws.close(Some(CloseFrame {
        code: CloseCode::Normal,
        reason: "bye".into(),
    }))
    .await
    .unwrap();
    while let Some(Ok(_)) = ws.next().await {}

    let (_, events) = wait_exchange(&mut proxy, "/chat", |e| {
        matches!(e, ProxyEvent::StreamClosed { .. })
    })
    .await;
    let ProxyEvent::RequestStarted { head, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.url, format!("ws://{upstream}/chat"));
    assert_eq!(head.scheme, "ws");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, ProxyEvent::ResponseStarted { head, .. } if head.status == 101))
    );
    assert!(events.iter().any(|e| matches!(
        e,
        ProxyEvent::StreamOpened {
            kind: ExchangeKind::WebSocket,
            ..
        }
    )));
    let msgs = messages(&events);
    assert_eq!(
        msgs[0],
        (Direction::Sent, MessageKind::Text, b"hello".to_vec())
    );
    assert_eq!(
        msgs[1],
        (Direction::Received, MessageKind::Text, b"hello".to_vec())
    );
    assert_eq!(
        msgs[2],
        (Direction::Sent, MessageKind::Binary, vec![1, 2, 3])
    );
    assert_eq!(
        msgs[3],
        (Direction::Received, MessageKind::Binary, vec![1, 2, 3])
    );
    assert_eq!(
        (msgs[4].0, msgs[4].1),
        (Direction::Sent, MessageKind::Close)
    );
    assert_eq!(&msgs[4].2[2..], b"bye");
    match events.last() {
        Some(ProxyEvent::StreamClosed {
            code, reason, by, ..
        }) => {
            assert_eq!(*code, Some(1000));
            assert_eq!(reason.as_deref(), Some("bye"));
            // The client sent the first close frame.
            assert_eq!(*by, Direction::Sent);
        }
        other => panic!("{other:?}"),
    }
}

/// Upstream that accepts any WebSocket upgrade and sends the request head it received as
/// the first (text) message.
async fn spawn_head_reporting_ws_upstream() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        while let Ok((mut s, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let Some((head, _)) = read_request(&mut s, &mut buf).await else {
                    return;
                };
                let text = String::from_utf8_lossy(&head).to_string();
                let key = text
                    .lines()
                    .find_map(|l| {
                        l.split_once(':')
                            .filter(|(k, _)| k.eq_ignore_ascii_case("sec-websocket-key"))
                            .map(|(_, v)| v.trim().to_string())
                    })
                    .unwrap_or_default();
                let accept =
                    tokio_tungstenite::tungstenite::handshake::derive_accept_key(key.as_bytes());
                let mut out = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n").into_bytes();
                let payload = text.as_bytes();
                out.push(0x81);
                out.push(126);
                out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
                out.extend_from_slice(payload);
                let _ = s.write_all(&out).await;
                let mut sink = [0u8; 1024];
                while matches!(s.read(&mut sink).await, Ok(n) if n > 0) {}
            });
        }
    });
    addr
}

#[tokio::test]
async fn absolute_form_upgrade_strips_permessage_deflate() {
    let upstream = spawn_head_reporting_ws_upstream().await;
    let mut proxy = start_proxy(
        akev_core::rules::RuleSet::default(),
        Arc::new(NoBreakpoints),
    )
    .await;
    let mut tcp = tokio::net::TcpStream::connect(proxy.addr()).await.unwrap();
    let request = format!(
        "GET http://{upstream}/live HTTP/1.1\r\nHost: {upstream}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits\r\nX-Keep: me\r\n\r\n"
    );
    tcp.write_all(request.as_bytes()).await.unwrap();
    let mut raw = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while !String::from_utf8_lossy(&raw).contains("X-Keep") {
        let mut tmp = [0u8; 4096];
        let n = tokio::time::timeout_at(deadline, tcp.read(&mut tmp))
            .await
            .expect("upgrade response")
            .unwrap();
        assert!(n > 0, "closed: {:?}", String::from_utf8_lossy(&raw));
        raw.extend_from_slice(&tmp[..n]);
    }
    let text = String::from_utf8_lossy(&raw);
    assert!(text.starts_with("HTTP/1.1 101"), "{text}");
    assert!(
        text.contains("Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo="),
        "{text}"
    );
    // The upstream saw the client's handshake minus the compression offer.
    assert!(
        !text
            .to_ascii_lowercase()
            .contains("sec-websocket-extensions"),
        "{text}"
    );
    assert!(text.contains("Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n"));
    let (_, events) = wait_exchange(&mut proxy, "/live", |e| {
        matches!(e, ProxyEvent::StreamMessage { .. })
    })
    .await;
    let ProxyEvent::RequestStarted { head, client, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(head.url, format!("ws://{upstream}/live"));
    assert_eq!(client.via, ProxyMode::HttpProxy);
    // The original request (as recorded) still shows what the client offered.
    assert!(head.headers.contains("sec-websocket-extensions"));
}

#[tokio::test]
async fn ws_echo_rule_answers_locally() {
    let rules = rules(
        r#"{"items":[{"type":"rule","id":"echo","matchers":[{"type":"path","pattern":{"op":"equals","value":"/echo-ws"}}],"action":{"type":"ws-echo"}}]}"#,
    );
    let mut proxy = start_proxy(rules, Arc::new(NoBreakpoints)).await;
    // Port 9 (discard) is never contacted: the rule answers.
    let tunnel = connect_tunnel(&proxy, "ws-echo.invalid:9").await;
    let (mut ws, _) = tokio_tungstenite::client_async("ws://ws-echo.invalid:9/echo-ws", tunnel)
        .await
        .expect("ws handshake");
    ws.send(Message::text("ping me")).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(), Message::text("ping me"));
    ws.close(None).await.unwrap();
    while let Some(Ok(_)) = ws.next().await {}
    let (_, events) = wait_exchange(&mut proxy, "/echo-ws", |e| {
        matches!(e, ProxyEvent::StreamClosed { .. })
    })
    .await;
    assert!(events.iter().any(
        |e| matches!(e, ProxyEvent::RuleMatched { rule, mocked: true, .. } if rule == "echo")
    ));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, ProxyEvent::Upstream { .. }))
    );
    let msgs = messages(&events);
    assert_eq!(
        msgs[0],
        (Direction::Sent, MessageKind::Text, b"ping me".to_vec())
    );
    assert_eq!(
        msgs[1],
        (Direction::Received, MessageKind::Text, b"ping me".to_vec())
    );
}
