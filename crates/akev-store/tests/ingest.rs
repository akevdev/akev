//! Event sequences → summaries, details, bodies and broadcasts.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{ExchangePatch, StoreEvent};
use bytes::Bytes;
use common::*;
use std::time::Duration;

#[test]
fn normal_exchange_lifecycle() {
    let store = store();
    let id = ExchangeId(1);
    let mut rx = store.subscribe();

    store.ingest(started(
        1,
        "POST",
        "https://api.example.com/v1/items?x=1",
        &[
            ("Host", "api.example.com"),
            ("Content-Type", "application/json"),
        ],
    ));
    let s = store.summary(id).unwrap();
    assert_eq!(s.state, ExchangeState::Requesting);
    assert_eq!(s.kind, ExchangeKind::Http);
    assert_eq!(
        (
            s.method.as_str(),
            s.scheme.as_str(),
            s.host.as_str(),
            s.port
        ),
        ("POST", "https", "api.example.com", 443)
    );
    assert_eq!(s.path, "/v1/items?x=1");
    assert_eq!(s.started_at, T0.as_millis_f64());
    assert_eq!(
        s.source.as_deref(),
        Some("curl"),
        "process name wins over interceptor"
    );

    store.ingest(req_done(1, br#"{"name":"widget"}"#, 2.0));
    assert_eq!(store.summary(id).unwrap().state, ExchangeState::Waiting);
    assert_eq!(store.summary(id).unwrap().request_size, 17);

    store.ingest(ProxyEvent::RuleMatched {
        id,
        rule: "r-modifier".into(),
        mocked: false,
    });
    store.ingest(ProxyEvent::RuleMatched {
        id,
        rule: "r-pass".into(),
        mocked: false,
    });
    store.ingest(ProxyEvent::Upstream {
        id,
        info: UpstreamInfo {
            remote_addr: Some("93.184.216.34:443".into()),
            ..UpstreamInfo::default()
        },
        dns_duration: Some(1.5),
        connect_duration: Some(3.0),
        tls_duration: Some(4.0),
        request_sent: Some(9.0),
    });
    store.ingest(resp_start(
        1,
        201,
        &[("Content-Type", "application/json; charset=utf-8")],
        20.0,
    ));
    let s = store.summary(id).unwrap();
    assert_eq!(s.state, ExchangeState::Responding);
    assert_eq!(s.status, Some(201));
    assert_eq!(s.content_type.as_deref(), Some("application/json"));
    assert_eq!(s.content_kind, ContentKind::Json);
    assert_eq!(s.duration, None);

    store.ingest(resp_done(1, br#"{"id":1}"#, 25.0));
    let s = store.summary(id).unwrap();
    assert_eq!(s.state, ExchangeState::Complete);
    assert_eq!(s.response_size, 8);
    assert_eq!(s.duration, Some(25.0));
    assert_eq!(s.rule.as_deref(), Some("r-pass"), "terminal (last) rule");
    assert!(!s.modified && !s.mocked && !s.pinned);

    let d = store.detail(id).unwrap();
    assert_eq!(d.summary, s);
    assert_eq!(d.request.url, "https://api.example.com/v1/items?x=1");
    assert_eq!(
        d.request.headers.get("content-type"),
        Some("application/json")
    );
    assert_eq!(
        d.matched_rules,
        vec!["r-modifier".to_string(), "r-pass".to_string()]
    );
    assert_eq!(d.timing.dns_duration, Some(1.5));
    assert_eq!(d.timing.request_sent, Some(9.0));
    assert_eq!(d.timing.response_headers, Some(20.0));
    assert_eq!(
        d.upstream.as_ref().unwrap().remote_addr.as_deref(),
        Some("93.184.216.34:443")
    );
    assert_eq!(d.request_body.wire_size, 17);
    assert_eq!(d.request_body.captured_size, 17);
    assert_eq!(d.request_body.decoded_size, Some(17));
    assert_eq!(
        d.response_body.content_type.as_deref(),
        Some("application/json")
    );
    assert!(d.response_body.encodings.is_empty());
    assert_eq!(d.client, client());
    assert!(d.original_request.is_none() && d.original_response.is_none());

    assert_eq!(
        &store.body(id, Side::Response).unwrap().bytes[..],
        br#"{"id":1}"#
    );
    assert_eq!(
        &store.body(id, Side::Request).unwrap().bytes[..],
        br#"{"name":"widget"}"#
    );
    assert!(store.original_body(id, Side::Request).is_none());

    let events = drain(&mut rx);
    // Body completion, two rule changes, response head, response completion; the Upstream
    // event changes nothing visible → no Updated for it.
    assert_eq!(
        kinds(&events),
        vec![
            "added", "updated", "updated", "updated", "updated", "updated"
        ]
    );
    match events.last() {
        Some(StoreEvent::Updated(s)) => assert_eq!(s.state, ExchangeState::Complete),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn aborted_exchange() {
    let store = store();
    let id = ExchangeId(5);
    store.ingest(started(5, "GET", "http://down.example/", &[]));
    store.ingest(req_done(5, b"", 1.0));
    store.ingest(ProxyEvent::Aborted {
        id,
        error: ExchangeError {
            kind: ErrorKind::ConnectionRefused,
            message: "refused".into(),
            code: Some("ECONNREFUSED".into()),
        },
        at: 3.25,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(s.state, ExchangeState::Aborted);
    assert_eq!(s.error, Some(ErrorKind::ConnectionRefused));
    assert_eq!(s.duration, Some(3.25));
    assert_eq!(s.status, None);
    let d = store.detail(id).unwrap();
    assert_eq!(d.error.unwrap().code.as_deref(), Some("ECONNREFUSED"));
    assert_eq!(d.timing.aborted, Some(3.25));
    // Late events for an aborted exchange don't resurrect it.
    store.ingest(ProxyEvent::Paused {
        id,
        stage: BreakpointStage::Response,
    });
    store.ingest(ProxyEvent::Aborted {
        id,
        error: ExchangeError {
            kind: ErrorKind::ConnectionRefused,
            message: "refused".into(),
            code: None,
        },
        at: 3.25,
    });
    assert_eq!(store.summary(id).unwrap().state, ExchangeState::Aborted);
}

#[test]
fn modified_and_mocked_exchange() {
    let store = store();
    let id = ExchangeId(9);
    store.ingest(started(
        9,
        "POST",
        "http://orig.example/a",
        &[("Host", "orig.example")],
    ));
    store.ingest(req_done(9, b"original body", 1.0));
    store.ingest(ProxyEvent::RequestModified {
        id,
        head: head(
            "PUT",
            "http://new.example:8080/b",
            &[("Host", "new.example:8080"), ("X-Added", "1")],
        ),
        body: Some(CapturedBody::complete(Bytes::from_static(b"modified body"))),
    });
    store.ingest(ProxyEvent::RuleMatched {
        id,
        rule: "mock".into(),
        mocked: true,
    });
    store.ingest(resp_start(9, 200, &[("Content-Type", "text/plain")], 5.0));
    store.ingest(ProxyEvent::ResponseModified {
        id,
        original_head: resp_head(500, &[("Content-Type", "text/plain")]),
        original_body: Some(CapturedBody::complete(Bytes::from_static(
            b"upstream error",
        ))),
    });
    store.ingest(resp_done(9, b"patched", 6.0));

    let s = store.summary(id).unwrap();
    assert!(s.modified && s.mocked);
    assert_eq!(
        (s.method.as_str(), s.host.as_str(), s.port, s.path.as_str()),
        ("PUT", "new.example", 8080, "/b")
    );
    assert_eq!(s.status, Some(200));
    assert_eq!(s.request_size, 13);
    let d = store.detail(id).unwrap();
    let orig = d.original_request.unwrap();
    assert_eq!(
        (orig.method.as_str(), orig.host.as_str(), orig.path.as_str()),
        ("POST", "orig.example", "/a")
    );
    assert_eq!(d.request.headers.get("x-added"), Some("1"));
    assert_eq!(d.original_response.unwrap().status, 500);
    assert_eq!(
        &store.body(id, Side::Request).unwrap().bytes[..],
        b"modified body"
    );
    assert_eq!(
        &store.original_body(id, Side::Request).unwrap().bytes[..],
        b"original body"
    );
    assert_eq!(
        &store.body(id, Side::Response).unwrap().bytes[..],
        b"patched"
    );
    assert_eq!(
        &store.original_body(id, Side::Response).unwrap().bytes[..],
        b"upstream error"
    );
}

#[test]
fn request_modified_before_body_completes() {
    let store = store();
    let id = ExchangeId(10);
    store.ingest(started(10, "POST", "http://h.example/", &[]));
    store.ingest(ProxyEvent::RequestModified {
        id,
        head: head("POST", "http://h.example/", &[]),
        body: Some(CapturedBody::complete(Bytes::from_static(b"replacement"))),
    });
    store.ingest(req_done(10, b"what the client sent", 2.0));
    assert_eq!(
        &store.body(id, Side::Request).unwrap().bytes[..],
        b"replacement"
    );
    assert_eq!(
        &store.original_body(id, Side::Request).unwrap().bytes[..],
        b"what the client sent"
    );
    assert_eq!(store.summary(id).unwrap().state, ExchangeState::Waiting);
}

#[test]
fn websocket_exchange() {
    let store = store();
    let id = ExchangeId(20);
    let mut rx = store.subscribe();
    store.ingest(started(
        20,
        "GET",
        "ws://chat.example/socket",
        &[("Upgrade", "websocket"), ("Connection", "Upgrade")],
    ));
    store.ingest(req_done(20, b"", 0.5));
    store.ingest(resp_start(20, 101, &[("Upgrade", "websocket")], 3.0));
    store.ingest(resp_done(20, b"", 3.0));
    store.ingest(ProxyEvent::StreamOpened {
        id,
        kind: ExchangeKind::WebSocket,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.kind, s.state, s.duration),
        (ExchangeKind::WebSocket, ExchangeState::Streaming, None)
    );

    store.ingest(ws_message(
        20,
        0,
        Direction::Sent,
        MessageKind::Text,
        b"hello",
    ));
    store.ingest(ws_message(
        20,
        1,
        Direction::Received,
        MessageKind::Binary,
        &[0, 159, 146, 150],
    ));
    store.ingest(ws_message(
        20,
        2,
        Direction::Received,
        MessageKind::Text,
        b"world",
    ));
    // A late message for an unknown exchange is ignored safely.
    store.ingest(ws_message(
        999,
        0,
        Direction::Sent,
        MessageKind::Text,
        b"orphan",
    ));
    let s = store.summary(id).unwrap();
    assert_eq!(s.message_count, 3);
    assert_eq!(s.state, ExchangeState::Streaming);
    let msgs = store.messages(id);
    assert_eq!(msgs.len(), 3);
    assert_eq!(&msgs[0].payload[..], b"hello");
    assert_eq!(msgs[1].message.kind, MessageKind::Binary);
    assert_eq!(&msgs[1].payload[..], &[0, 159, 146, 150]);
    assert_eq!(msgs[2].message.seq, 2);
    let tail = store.messages_after(id, 0);
    assert_eq!(
        tail.iter().map(|m| m.message.seq).collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert!(store.messages_after(id, 2).is_empty());
    assert!(store.messages_after(ExchangeId(999), 0).is_empty());
    assert!(store.messages(ExchangeId(999)).is_empty());

    store.ingest(ProxyEvent::StreamClosed {
        id,
        code: Some(1000),
        reason: Some("bye".into()),
        by: Direction::Sent,
        at: 120.0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.state, s.duration),
        (ExchangeState::Complete, Some(120.0))
    );

    let events = drain(&mut rx);
    let messages: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            StoreEvent::Message { id, message } => Some((*id, message.seq)),
            _ => None,
        })
        .collect();
    assert_eq!(messages, vec![(id, 0), (id, 1), (id, 2)]);
    // Every message is followed by an Updated carrying the new count.
    let counts: Vec<u32> = events
        .iter()
        .filter_map(|e| match e {
            StoreEvent::Updated(s) if s.kind == ExchangeKind::WebSocket => Some(s.message_count),
            _ => None,
        })
        .collect();
    assert!(counts.windows(2).all(|w| w[0] <= w[1]));
    assert_eq!(counts.last(), Some(&3));
}

#[test]
fn event_stream_completes_with_response() {
    let store = store();
    let id = ExchangeId(21);
    store.ingest(started(
        21,
        "GET",
        "https://sse.example/events",
        &[("Accept", "text/event-stream")],
    ));
    store.ingest(req_done(21, b"", 0.5));
    store.ingest(resp_start(
        21,
        200,
        &[("Content-Type", "text/event-stream")],
        2.0,
    ));
    store.ingest(ProxyEvent::StreamOpened {
        id,
        kind: ExchangeKind::EventStream,
    });
    store.ingest(ProxyEvent::StreamMessage {
        id,
        message: StreamMessage {
            seq: 0,
            direction: Direction::Received,
            kind: MessageKind::Event,
            at: T0,
            size: 4,
            event: Some("tick".into()),
            event_id: Some("1".into()),
            modified: false,
        },
        payload: Bytes::from_static(b"data"),
    });
    assert_eq!(store.summary(id).unwrap().state, ExchangeState::Streaming);
    store.ingest(resp_done(21, b"event: tick\nid: 1\ndata: data\n\n", 50.0));
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.kind, s.state, s.duration, s.message_count),
        (
            ExchangeKind::EventStream,
            ExchangeState::Complete,
            Some(50.0),
            1
        )
    );
    assert_eq!(s.content_kind, ContentKind::EventStream);
}

#[test]
fn tunnel_rows() {
    let store = store();
    let id = ExchangeId(30);
    let mut c = client();
    c.via = ProxyMode::Connect;
    store.ingest(ProxyEvent::TunnelOpened {
        id,
        client: c,
        host: "pinned.example".into(),
        port: 443,
        reason: "tls passthrough".into(),
        at: T0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (
            s.kind,
            s.state,
            s.method.as_str(),
            s.scheme.as_str(),
            s.port
        ),
        (
            ExchangeKind::Tunnel,
            ExchangeState::Streaming,
            "CONNECT",
            "https",
            443
        )
    );
    assert_eq!(s.host, "pinned.example");
    store.ingest(ProxyEvent::TunnelClosed {
        id,
        stats: TunnelStats {
            bytes_sent: 512,
            bytes_received: 4096,
            reason: String::new(),
        },
        at: 999.0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.state, s.request_size, s.response_size, s.duration),
        (ExchangeState::Complete, 512, 4096, Some(999.0))
    );
    let t = store.detail(id).unwrap().tunnel.unwrap();
    assert_eq!(
        t.reason, "tls passthrough",
        "reason kept when the close event has none"
    );
}

#[test]
fn tls_failure_rows() {
    let store = store();
    let id = ExchangeId(40);
    let mut c = client();
    c.tls = Some(TlsInfo {
        sni: Some("bank.example".into()),
        ..TlsInfo::default()
    });
    store.ingest(ProxyEvent::TlsFailed {
        id,
        client: c,
        host: "bank.example".into(),
        port: 443,
        error: ExchangeError {
            kind: ErrorKind::ClientTlsRejected,
            message: "unknown ca".into(),
            code: Some("cert-rejected".into()),
        },
        at: T0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.kind, s.state, s.error),
        (
            ExchangeKind::TlsFailure,
            ExchangeState::Aborted,
            Some(ErrorKind::ClientTlsRejected)
        )
    );
    assert_eq!(
        (s.host.as_str(), s.port, s.scheme.as_str()),
        ("bank.example", 443, "https")
    );
    assert_eq!(s.started_at, T0.as_millis_f64());
    let d = store.detail(id).unwrap();
    assert_eq!(d.request.url, "https://bank.example");
    assert_eq!(d.client.tls.unwrap().sni.as_deref(), Some("bank.example"));
}

#[test]
fn client_error_rows() {
    let store = store();
    let id = ExchangeId(50);
    store.ingest(ProxyEvent::ClientError {
        id,
        client: client(),
        error: ExchangeError {
            kind: ErrorKind::ClientProtocolError,
            message: "bad header".into(),
            code: None,
        },
        raw_prefix: Bytes::from_static(
            b"GET /x HTTP/1.1\r\nHost: weird.example\r\nBad Header\r\n\r\n",
        ),
        at: T0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.kind, s.state),
        (ExchangeKind::ClientError, ExchangeState::Aborted)
    );
    assert_eq!(
        (s.method.as_str(), s.host.as_str(), s.path.as_str()),
        ("GET", "weird.example", "/x")
    );
    assert!(
        store
            .body(id, Side::Request)
            .unwrap()
            .bytes
            .starts_with(b"GET /x")
    );
}

#[test]
fn breakpoint_pause_and_resume() {
    let store = store();
    let id = ExchangeId(60);
    store.ingest(started(60, "GET", "http://bp.example/", &[]));
    store.ingest(ProxyEvent::Paused {
        id,
        stage: BreakpointStage::Request,
    });
    let s = store.summary(id).unwrap();
    assert_eq!(
        (s.state, s.paused),
        (ExchangeState::Paused, Some(BreakpointStage::Request))
    );
    store.ingest(ProxyEvent::Resumed {
        id,
        paused_ms: 1500.0,
    });
    store.ingest(req_done(60, b"", 1600.0));
    store.ingest(ProxyEvent::Paused {
        id,
        stage: BreakpointStage::Response,
    });
    store.ingest(ProxyEvent::Resumed {
        id,
        paused_ms: 500.0,
    });
    let s = store.summary(id).unwrap();
    assert_eq!((s.state, s.paused), (ExchangeState::Waiting, None));
    assert_eq!(
        store.detail(id).unwrap().timing.paused_duration,
        Some(2000.0)
    );
}

#[test]
fn duplicates_and_unknown_ids_are_ignored() {
    let store = store();
    let mut rx = store.subscribe();
    store.ingest(resp_done(77, b"early", 1.0));
    store.ingest(ProxyEvent::Aborted {
        id: ExchangeId(78),
        error: ExchangeError {
            kind: ErrorKind::Internal,
            message: "x".into(),
            code: None,
        },
        at: 1.0,
    });
    assert!(store.summaries().is_empty());
    store.ingest(started(80, "GET", "http://a.example/", &[]));
    store.ingest(started(80, "DELETE", "http://b.example/", &[]));
    assert_eq!(store.summaries().len(), 1);
    assert_eq!(store.summary(ExchangeId(80)).unwrap().method, "GET");
    assert_eq!(kinds(&drain(&mut rx)), vec!["added"]);
}

#[test]
fn capture_settings_filters() {
    let store = store_with(|s| {
        s.ignore_hosts = vec!["*.noise.example".into(), "telemetry.example".into()];
        s.record_tunnels = false;
        s.record_tls_failures = false;
    });
    store.ingest(started(1, "GET", "https://cdn.noise.example/x", &[]));
    store.ingest(resp_done(1, b"dropped", 1.0));
    store.ingest(started(2, "GET", "https://TELEMETRY.example/x", &[]));
    store.ingest(started(3, "GET", "https://kept.example/x", &[]));
    store.ingest(ProxyEvent::TunnelOpened {
        id: ExchangeId(4),
        client: client(),
        host: "t.example".into(),
        port: 443,
        reason: "r".into(),
        at: T0,
    });
    store.ingest(ProxyEvent::TlsFailed {
        id: ExchangeId(5),
        client: client(),
        host: "f.example".into(),
        port: 443,
        error: ExchangeError {
            kind: ErrorKind::ClientTlsRejected,
            message: "x".into(),
            code: None,
        },
        at: T0,
    });
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    // `TELEMETRY.example` is uppercase in the URL helper's host; globs are case-insensitive.
    assert_eq!(ids, vec![3]);

    // Live update: tunnels recorded again, ignore list cleared.
    store.update_settings(akev_core::settings::CaptureSettings::default());
    store.ingest(ProxyEvent::TunnelOpened {
        id: ExchangeId(6),
        client: client(),
        host: "t.example".into(),
        port: 443,
        reason: "r".into(),
        at: T0,
    });
    store.ingest(started(7, "GET", "https://cdn.noise.example/y", &[]));
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    assert_eq!(ids, vec![3, 6, 7]);
}

#[test]
fn truncated_bodies_are_flagged() {
    let store = store_with(|s| s.max_body_bytes = 8);
    let id = ExchangeId(1);
    store.ingest(started(1, "POST", "http://t.example/", &[]));
    // The proxy already truncated this one.
    store.ingest(ProxyEvent::RequestCompleted {
        id,
        body: CapturedBody {
            bytes: Bytes::from_static(b"1234"),
            wire_size: 1000,
            truncated: true,
        },
        trailers: Some(headers(&[("X-Checksum", "abc")])),
        at: 1.0,
    });
    // This one exceeds max_body_bytes and is truncated by the store.
    store.ingest(resp_start(1, 200, &[("Content-Type", "text/plain")], 2.0));
    store.ingest(resp_done(1, b"0123456789abcdef", 3.0));
    let d = store.detail(id).unwrap();
    assert_eq!(
        (
            d.request_body.wire_size,
            d.request_body.captured_size,
            d.request_body.truncated
        ),
        (1000, 4, true)
    );
    assert_eq!(
        (
            d.response_body.wire_size,
            d.response_body.captured_size,
            d.response_body.truncated
        ),
        (16, 8, true)
    );
    assert_eq!(d.request_trailers.unwrap().get("x-checksum"), Some("abc"));
    let body = store.body(id, Side::Response).unwrap();
    assert_eq!(
        (&body.bytes[..], body.wire_size, body.truncated),
        (&b"01234567"[..], 16, true)
    );
    assert_eq!(
        store.summary(id).unwrap().response_size,
        16,
        "summary shows wire size"
    );
}

#[test]
fn gzip_bodies_report_decoded_size_and_sniffed_kind() {
    let store = store();
    let json = br#"{"items":[1,2,3],"more":"data data data data data data"}"#;
    let gz = gzip(json);
    store.ingest(started(1, "GET", "http://z.example/data", &[]));
    store.ingest(req_done(1, b"", 1.0));
    // No content type: the kind comes from sniffing the decoded body.
    store.ingest(resp_start(1, 200, &[("Content-Encoding", "gzip")], 2.0));
    store.ingest(resp_done(1, &gz, 3.0));
    let d = store.detail(ExchangeId(1)).unwrap();
    assert_eq!(d.response_body.encodings, vec!["gzip".to_string()]);
    assert_eq!(d.response_body.wire_size, gz.len() as u64);
    assert_eq!(d.response_body.decoded_size, Some(json.len() as u64));
    assert_eq!(d.summary.content_kind, ContentKind::Json);
    assert_eq!(d.summary.content_type, None);
    assert_eq!(
        store.body(ExchangeId(1), Side::Response).unwrap().bytes,
        Bytes::from(gz)
    );
}

#[test]
fn patch_updates_user_attributes() {
    let store = store();
    complete_exchange(&store, 1, "http://p.example/", "text/plain", b"x");
    complete_exchange(&store, 2, "http://p.example/2", "text/plain", b"y");
    let mut rx = store.subscribe();
    let ids = [ExchangeId(1), ExchangeId(2), ExchangeId(404)];
    store.patch(
        &ids,
        &ExchangePatch {
            pinned: Some(true),
            color: Some(Some("red".into())),
            note: Some(Some("look here".into())),
        },
    );
    let s = store.summary(ExchangeId(1)).unwrap();
    assert!(s.pinned && s.has_note);
    assert_eq!(s.color.as_deref(), Some("red"));
    assert_eq!(
        store.detail(ExchangeId(2)).unwrap().note.as_deref(),
        Some("look here")
    );
    assert_eq!(kinds(&drain(&mut rx)), vec!["updated", "updated"]);

    // Re-applying the same patch changes nothing → no events.
    store.patch(
        &ids,
        &ExchangePatch {
            pinned: Some(true),
            color: Some(Some("red".into())),
            note: None,
        },
    );
    assert!(drain(&mut rx).is_empty());

    // Clearing via `Some(None)` / empty strings.
    store.patch(
        &[ExchangeId(1)],
        &ExchangePatch {
            pinned: Some(false),
            color: Some(None),
            note: Some(Some("  ".into())),
        },
    );
    let s = store.summary(ExchangeId(1)).unwrap();
    assert!(!s.pinned && !s.has_note && s.color.is_none());
    assert_eq!(store.detail(ExchangeId(1)).unwrap().note, None);

    // JSON shape used by the API.
    let p: ExchangePatch = serde_json::from_str(r#"{"color":null,"note":"n"}"#).unwrap();
    assert_eq!(
        (p.pinned, p.color, p.note),
        (None, Some(None), Some(Some("n".into())))
    );
    let p: ExchangePatch = serde_json::from_str(r#"{}"#).unwrap();
    assert_eq!((p.pinned, p.color, p.note), (None, None, None));
}

#[test]
fn delete_and_clear_broadcast() {
    let store = store();
    for id in 1..=4 {
        complete_exchange(&store, id, "http://d.example/", "text/plain", b"body");
    }
    store.patch(
        &[ExchangeId(2)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    let mut rx = store.subscribe();

    store.delete(&[ExchangeId(1), ExchangeId(1), ExchangeId(99)]);
    match drain(&mut rx).as_slice() {
        [StoreEvent::Removed(ids)] => assert_eq!(ids, &vec![ExchangeId(1)]),
        other => panic!("unexpected {other:?}"),
    }
    store.delete(&[ExchangeId(99)]);
    assert!(drain(&mut rx).is_empty(), "nothing deleted → no event");

    store.clear(true);
    match drain(&mut rx).as_slice() {
        [StoreEvent::Removed(ids)] => assert_eq!(ids, &vec![ExchangeId(3), ExchangeId(4)]),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(
        store.summaries().iter().map(|s| s.id).collect::<Vec<_>>(),
        vec![ExchangeId(2)]
    );
    assert_eq!(store.stats().memory_body_bytes, 4);

    store.clear(false);
    assert_eq!(kinds(&drain(&mut rx)), vec!["cleared"]);
    assert!(store.summaries().is_empty());
    let stats = store.stats();
    assert_eq!(
        (
            stats.exchanges,
            stats.memory_body_bytes,
            stats.spilled_body_bytes
        ),
        (0, 0, 0)
    );
    // Events for cleared exchanges are ignored.
    store.ingest(resp_done(3, b"late", 50.0));
    assert!(store.summaries().is_empty());
}

#[test]
fn inline_sink_without_runtime() {
    let store = store();
    let sink = store.sink();
    sink.emit(started(1, "GET", "http://inline.example/", &[]));
    assert!(
        store.summary(ExchangeId(1)).is_some(),
        "no runtime → applied inline"
    );
    drop(store);
    // The sink must not keep the store alive or panic once it's gone.
    sink.emit(started(2, "GET", "http://inline.example/", &[]));
}

#[tokio::test]
async fn background_sink_applies_in_order() {
    let store = store();
    let mut rx = store.subscribe();
    let sink = store.sink();
    let sink2 = store.sink();
    for i in 0..500u64 {
        sink.emit(started(i + 1, "GET", "http://bg.example/", &[]));
        sink2.emit(req_done(i + 1, b"", 1.0));
        sink.emit(resp_start(i + 1, 204, &[], 2.0));
        sink.emit(resp_done(i + 1, b"", 3.0));
    }
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let done = store
            .summaries()
            .iter()
            .filter(|s| s.state == ExchangeState::Complete)
            .count();
        if done == 500 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "only {done} completed"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let first = rx.recv().await.unwrap();
    assert!(matches!(first, StoreEvent::Added(s) if s.id == ExchangeId(1)));
}

#[tokio::test]
async fn store_drops_with_live_sink() {
    let store = store();
    let weak = std::sync::Arc::downgrade(&store);
    let sink = store.sink();
    sink.emit(started(1, "GET", "http://drop.example/", &[]));
    drop(store);
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert!(
        weak.upgrade().is_none(),
        "background task must not keep the store alive"
    );
    sink.emit(started(2, "GET", "http://drop.example/", &[]));
}

#[test]
fn llm_calls_are_tagged_with_provider_and_model() {
    let store = store();
    // A well-known provider: tagged as soon as the request starts, model once the body is in.
    store.ingest(started(
        1,
        "POST",
        "https://api.anthropic.com/v1/messages",
        &[("Content-Type", "application/json")],
    ));
    let tag = store.summary(ExchangeId(1)).unwrap().llm.unwrap();
    assert_eq!(
        (tag.provider.as_str(), tag.model.as_deref()),
        ("anthropic", None)
    );
    store.ingest(req_done(
        1,
        br#"{"model":"claude-sonnet-4-5","max_tokens":1024,"messages":[{"role":"user","content":"Hi"}]}"#,
        2.0,
    ));
    let tag = store.summary(ExchangeId(1)).unwrap().llm.unwrap();
    assert_eq!(tag.model.as_deref(), Some("claude-sonnet-4-5"));
    // Serialized on the summary.
    let v = serde_json::to_value(store.summary(ExchangeId(1)).unwrap()).unwrap();
    assert_eq!(
        v["llm"],
        serde_json::json!({"provider": "anthropic", "model": "claude-sonnet-4-5"})
    );

    // A local OpenAI-compatible server: only once the body confirms the shape (gzip-encoded).
    store.ingest(started(
        2,
        "POST",
        "http://localhost:11434/v1/chat/completions",
        &[("Content-Encoding", "gzip")],
    ));
    assert!(store.summary(ExchangeId(2)).unwrap().llm.is_none());
    store.ingest(req_done(
        2,
        &gzip(br#"{"model":"llama3.2","messages":[{"role":"user","content":"Hi"}],"stream":true}"#),
        1.0,
    ));
    let tag = store.summary(ExchangeId(2)).unwrap().llm.unwrap();
    assert_eq!(
        (tag.provider.as_str(), tag.model.as_deref()),
        ("ollama", Some("llama3.2"))
    );

    // Look-alike paths with other bodies, and ordinary APIs, stay untagged.
    store.ingest(started(
        3,
        "POST",
        "https://chat.example.com/v1/messages",
        &[],
    ));
    store.ingest(req_done(
        3,
        br#"{"to":"ada","messages":[{"text":"hi"}]}"#,
        1.0,
    ));
    assert!(store.summary(ExchangeId(3)).unwrap().llm.is_none());
    complete_exchange(
        &store,
        4,
        "https://api.example.com/v1/items",
        "application/json",
        b"[]",
    );
    assert!(store.summary(ExchangeId(4)).unwrap().llm.is_none());

    // The filter language sees the tag.
    let ids = |q: &str| {
        store
            .query(&akev_filter::Filter::parse(q).unwrap(), None)
            .iter()
            .map(|s| s.id.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids("is:llm"), [1, 2]);
    assert_eq!(ids("model:claude"), [1]);
    assert_eq!(ids("provider:ollama"), [2]);
    assert_eq!(ids("-is:llm"), [3, 4]);
}
