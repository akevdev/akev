//! Importing other tools' captures: Charles JSON sessions and mitmproxy flow files.
//!
//! `fixtures/charles.chlsj` follows Charles 4.x's JSON session export; `fixtures/mitmproxy.flows`
//! was written by a real `mitmdump` 12.3 (`-w`) proxying curl and a WebSocket client to a
//! local server: gzip JSON, a POST with a comment, a marked 404, HTTP/2 over TLS, a refused
//! connection and a WebSocket with text and binary messages.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{Store, StoreError};
use common::*;

const CHARLES: &[u8] = include_bytes!("fixtures/charles.chlsj");
const MITMPROXY: &[u8] = include_bytes!("fixtures/mitmproxy.flows");

fn details(store: &Store, ids: &[ExchangeId]) -> Vec<ExchangeDetail> {
    ids.iter()
        .map(|id| store.detail(*id).expect("detail"))
        .collect()
}

fn header<'a>(h: &'a Headers, name: &str) -> Option<&'a str> {
    h.get(name)
}

#[test]
fn charles_session_imports_every_kind_of_transaction() {
    let store = store();
    let ids = store.import_charles(CHARLES).unwrap();
    assert_eq!(ids.len(), 7);
    let d = details(&store, &ids);

    // 1. HTTP/2 GET; Charles decoded the gzip body, so the header goes; pseudo-headers too.
    let gh = &d[0];
    assert_eq!(gh.request.url, "https://api.github.com/users/octocat");
    assert_eq!(gh.request.http_version, HttpVersion::H2);
    assert!(gh.request.headers.iter().all(|(n, _)| !n.starts_with(':')));
    assert_eq!(
        header(&gh.request.headers, "accept"),
        Some("application/vnd.github+json")
    );
    let resp = gh.response.as_ref().unwrap();
    assert_eq!((resp.status, resp.status_message.as_str()), (200, "OK"));
    assert_eq!(header(&resp.headers, "content-encoding"), None);
    assert!(resp.headers.iter().all(|(n, _)| !n.starts_with(':')));
    let body = store.body(ids[0], Side::Response).unwrap();
    assert!(body.bytes.starts_with(b"{\"login\":\"octocat\""));
    assert!(!body.truncated);
    assert_eq!(gh.summary.content_kind, ContentKind::Json);
    assert_eq!(gh.summary.state, ExchangeState::Complete);
    assert_eq!(
        gh.timing.start,
        UnixMicros::from_rfc3339("2024-03-04T10:21:36.512Z").unwrap()
    );
    assert_eq!(gh.timing.dns_duration, Some(3.0));
    assert_eq!(gh.timing.connect_duration, Some(17.0));
    assert_eq!(gh.timing.tls_duration, Some(29.0));
    assert_eq!(gh.timing.request_sent, Some(51.0));
    assert_eq!(gh.timing.response_headers, Some(189.0));
    assert_eq!(gh.timing.response_body, Some(191.0));
    assert_eq!(gh.summary.duration, Some(191.0));
    assert_eq!(gh.client.remote_addr, "127.0.0.1:52178");
    let tls = gh.client.tls.as_ref().unwrap();
    assert_eq!(tls.version.as_deref(), Some("TLSv1.3"));
    assert_eq!(tls.alpn.as_deref(), Some("h2"));
    let up = gh.upstream.as_ref().unwrap();
    assert_eq!(up.remote_addr.as_deref(), Some("140.82.121.6:443"));
    assert_eq!(up.cipher.as_deref(), Some("TLS_AES_128_GCM_SHA256"));

    // 2. POST with a query, a form body and a note; non-default port kept in the URL.
    let post = &d[1];
    assert_eq!(post.request.method, "POST");
    assert_eq!(
        post.request.url,
        "http://shop.example.test:8080/api/orders?source=web"
    );
    assert_eq!(post.request.path, "/api/orders?source=web");
    assert_eq!(
        store.body(ids[1], Side::Request).unwrap().bytes.as_ref(),
        b"item=book&qty=2&price=19.90"
    );
    assert_eq!(post.response.as_ref().unwrap().status_message, "Created");
    assert_eq!(post.note.as_deref(), Some("Checkout bug: total is wrong"));
    assert!(post.summary.has_note);
    assert_eq!(post.timing.dns_duration, None);

    // 3. Binary body from base64.
    let png = store.body(ids[2], Side::Response).unwrap();
    assert!(png.bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert_eq!(d[2].summary.content_kind, ContentKind::Image);
    assert_eq!(png.wire_size, png.bytes.len() as u64);

    // 4. Still-compressed base64 gzip body: the header stays and the body decodes.
    let legacy = &d[3];
    assert_eq!(
        header(
            &legacy.response.as_ref().unwrap().headers,
            "content-encoding"
        ),
        Some("gzip")
    );
    let raw = store.body(ids[3], Side::Response).unwrap();
    assert!(raw.bytes.starts_with(&[0x1f, 0x8b]));
    assert_eq!(legacy.client.remote_addr, "127.0.0.1:52192");
    assert_eq!(legacy.timing.tls_duration, None, "-1 means not applicable");

    // 5. Failed DNS lookup: aborted with a mapped error kind.
    let failed = &d[4];
    assert_eq!(failed.summary.state, ExchangeState::Aborted);
    let err = failed.error.as_ref().unwrap();
    assert_eq!(err.kind, ErrorKind::DnsFailed);
    assert!(err.message.contains("UnknownHostException"));
    assert!(failed.response.is_none());
    assert_eq!(failed.timing.aborted, Some(18.0));

    // 6. CONNECT without SSL proxying: a tunnel row with byte counts.
    let tunnel = &d[5];
    assert_eq!(tunnel.summary.kind, ExchangeKind::Tunnel);
    assert_eq!(tunnel.summary.host, "updates.example.test");
    assert_eq!(tunnel.summary.port, 443);
    let stats = tunnel.tunnel.as_ref().unwrap();
    assert_eq!((stats.bytes_sent, stats.bytes_received), (5120, 48213));
    assert_eq!(tunnel.summary.state, ExchangeState::Complete);

    // 7. WebSocket with messages.
    let ws = &d[6];
    assert_eq!(ws.summary.kind, ExchangeKind::WebSocket);
    assert_eq!(ws.request.url, "https://chat.example.test/socket?v=2");
    assert_eq!(ws.response.as_ref().unwrap().status, 101);
    let messages = store.messages(ids[6]);
    assert_eq!(messages.len(), 3);
    assert_eq!(messages[0].message.direction, Direction::Sent);
    assert_eq!(
        messages[0].payload.as_ref(),
        br#"{"op":"subscribe","room":"general"}"#
    );
    assert_eq!(messages[1].message.direction, Direction::Received);
    assert_eq!(messages[2].message.kind, MessageKind::Binary);
    assert_eq!(messages[2].payload.as_ref(), &[0, 1, 2, 255]);
    assert_eq!(
        messages[1].message.at,
        UnixMicros::from_rfc3339("2024-03-04T10:21:43Z").unwrap()
    );
    assert_eq!(ws.summary.message_count, 3);
}

#[test]
fn charles_variants_and_errors() {
    // Durations only (no `times`), a numeric-string port, a wrapped array and an incomplete
    // transaction.
    let doc = br#"{"transactions": [
      {"status": "RECEIVING_RESPONSE", "method": "GET", "scheme": "https", "host": "slow.example",
       "actualPort": "8443", "path": "data", "durations": {"total": 900, "dns": 1, "connect": 2,
       "ssl": 3, "request": 4, "latency": 500, "response": -1},
       "request": {"header": {"firstLine": "GET /data HTTP/1.1", "headers": []}},
       "response": null},
      {"method": "GET", "host": "", "path": "/skipped"}
    ]}"#;
    let store = store();
    let ids = store.import_charles(doc).unwrap();
    assert_eq!(ids.len(), 1, "the transaction without a host is skipped");
    let d = store.detail(ids[0]).unwrap();
    assert_eq!(d.request.url, "https://slow.example:8443/data");
    assert_eq!(d.summary.state, ExchangeState::Aborted);
    assert_eq!(d.error.as_ref().unwrap().kind, ErrorKind::ClientAborted);
    assert!(
        d.error
            .as_ref()
            .unwrap()
            .message
            .contains("receiving response")
    );
    assert_eq!(d.timing.request_sent, Some(10.0));
    assert_eq!(d.timing.aborted, Some(900.0));

    for bad in [
        &b"{\"log\":{\"entries\":[]}}"[..],
        b"not json",
        b"[{\"host\":5}]",
    ] {
        assert!(
            matches!(store.import_charles(bad), Err(StoreError::InvalidFile(_))),
            "{}",
            String::from_utf8_lossy(bad)
        );
    }
}

#[test]
fn mitmproxy_flows_from_a_real_mitmdump() {
    let store = store();
    let ids = store.import_mitmproxy(MITMPROXY).unwrap();
    assert_eq!(ids.len(), 6);
    let d = details(&store, &ids);

    // gzip JSON: mitmproxy keeps raw (encoded) content, so the header stays.
    let users = &d[0];
    assert_eq!(users.request.method, "GET");
    assert!(
        users.request.url.ends_with("/api/users?page=2"),
        "{}",
        users.request.url
    );
    assert_eq!(users.request.http_version, HttpVersion::Http11);
    let resp = users.response.as_ref().unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(header(&resp.headers, "Content-Encoding"), Some("gzip"));
    let body = store.body(ids[0], Side::Response).unwrap();
    assert!(body.bytes.starts_with(&[0x1f, 0x8b]));
    assert_eq!(users.summary.content_kind, ContentKind::Json);
    assert!(users.timing.response_body.unwrap() > users.timing.response_headers.unwrap());
    assert!(users.timing.connect_duration.is_some());
    assert!(users.client.remote_addr.starts_with("127.0.0.1:"));
    assert!(users.upstream.as_ref().unwrap().remote_addr.is_some());

    // POST with comment.
    let post = &d[1];
    assert_eq!(post.request.method, "POST");
    assert_eq!(
        store.body(ids[1], Side::Request).unwrap().bytes.as_ref(),
        br#"{"item":"book","qty":2}"#
    );
    assert_eq!(post.response.as_ref().unwrap().status_message, "Created");
    assert_eq!(
        post.note.as_deref(),
        Some("Order placed from the checkout page")
    );

    // Marked 404 → red tag.
    assert_eq!(d[2].response.as_ref().unwrap().status, 404);
    assert_eq!(d[2].summary.color.as_deref(), Some("red"));

    // HTTP/2 over TLS.
    let h2 = &d[3];
    assert_eq!(h2.request.http_version, HttpVersion::H2);
    assert_eq!(h2.request.scheme, "https");
    assert_eq!(h2.request.host, "localhost");
    let tls = h2.client.tls.as_ref().unwrap();
    assert_eq!(tls.sni.as_deref(), Some("localhost"));
    assert_eq!(tls.alpn.as_deref(), Some("h2"));
    assert_eq!(tls.version.as_deref(), Some("TLSv1.3"));
    assert!(h2.timing.tls_duration.is_some());

    // Refused connection.
    let refused = &d[4];
    assert_eq!(refused.summary.state, ExchangeState::Aborted);
    assert_eq!(
        refused.error.as_ref().unwrap().kind,
        ErrorKind::ConnectionRefused
    );
    assert!(refused.timing.aborted.is_some());

    // WebSocket.
    let ws = &d[5];
    assert_eq!(ws.summary.kind, ExchangeKind::WebSocket);
    assert_eq!(ws.response.as_ref().unwrap().status, 101);
    let messages = store.messages(ids[5]);
    assert_eq!(messages.len(), 4);
    assert_eq!(messages[0].payload.as_ref(), b"hello");
    assert_eq!(messages[0].message.direction, Direction::Sent);
    assert_eq!(messages[1].payload.as_ref(), b"echo: hello");
    assert_eq!(messages[1].message.direction, Direction::Received);
    assert_eq!(messages[2].message.kind, MessageKind::Binary);
    assert_eq!(messages[2].payload.as_ref(), &[0, 1, 2, 255]);

    // Exchanges are in chronological order.
    let starts: Vec<f64> = d.iter().map(|x| x.summary.started_at).collect();
    assert!(starts.windows(2).all(|w| w[0] <= w[1]), "{starts:?}");
}

#[test]
fn mitmproxy_rejects_other_files_and_truncation() {
    let store = store();
    assert!(store.import_mitmproxy(b"{\"log\":{}}").is_err());
    assert!(store.import_mitmproxy(CHARLES).is_err());
    // A truncated file fails as a whole (nothing inserted).
    let cut = &MITMPROXY[..MITMPROXY.len() - 100];
    assert!(matches!(
        store.import_mitmproxy(cut),
        Err(StoreError::InvalidFile(_))
    ));
    assert_eq!(store.stats().exchanges, 0);
}

#[test]
fn imported_exchanges_round_trip_through_sessions() {
    let source = store();
    source.import_charles(CHARLES).unwrap();
    source.import_mitmproxy(MITMPROXY).unwrap();
    let mut file = Vec::new();
    source.save_session(&mut file, None).unwrap();
    let target = store();
    let ids = target.load_session(&mut file.as_slice()).unwrap();
    assert_eq!(ids.len(), 13);
    let before: Vec<_> = source
        .summaries()
        .into_iter()
        .map(|s| (s.method, s.host, s.status, s.kind))
        .collect();
    let after: Vec<_> = target
        .summaries()
        .into_iter()
        .map(|s| (s.method, s.host, s.status, s.kind))
        .collect();
    assert_eq!(before, after);
}
