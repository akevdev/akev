//! HAR 1.2 export, import (Chrome / Firefox / HTTP Toolkit / Charles / Fiddler shapes) and
//! round trips.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{ExchangePatch, Store};
use bytes::Bytes;
use common::*;
use serde_json::Value;
use std::sync::Arc;

fn fixture(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    std::fs::read(path).unwrap()
}

/// A store holding one of every kind of row.
fn rich_store() -> Arc<Store> {
    let store = store();
    // 1: JSON GET with query, cookies, set-cookie; upstream timings.
    store.ingest(started(
        1,
        "GET",
        "https://api.example.com/search?q=red+shoes&page=2",
        &[
            ("Host", "api.example.com"),
            ("Cookie", "sid=abc; theme=dark"),
        ],
    ));
    store.ingest(req_done(1, b"", 1.0));
    store.ingest(ProxyEvent::Upstream {
        id: ExchangeId(1),
        info: UpstreamInfo {
            remote_addr: Some("[2001:db8::1]:443".into()),
            ..UpstreamInfo::default()
        },
        dns_duration: Some(2.0),
        connect_duration: Some(5.0),
        tls_duration: Some(7.0),
        request_sent: Some(15.0),
    });
    store.ingest(resp_start(1, 200, &[
        ("Content-Type", "application/json"),
        ("Set-Cookie", "token=a=b; Path=/; Expires=Wed, 21 Oct 2015 07:28:00 GMT; HttpOnly; Secure; SameSite=Strict"),
    ], 40.0));
    store.ingest(resp_done(1, br#"{"results":["shoe"]}"#, 45.0));

    // 2: gzip-encoded HTML with a redirect header.
    let html = b"<!doctype html><title>moved</title>".repeat(20);
    store.ingest(started(
        2,
        "GET",
        "http://old.example/page",
        &[("Host", "old.example")],
    ));
    store.ingest(req_done(2, b"", 1.0));
    store.ingest(resp_start(
        2,
        301,
        &[
            ("Content-Type", "text/html"),
            ("Content-Encoding", "gzip"),
            ("Location", "https://new.example/page"),
        ],
        5.0,
    ));
    store.ingest(resp_done(2, &gzip(&html), 6.0));

    // 3: urlencoded POST (with charset parameter) and binary response.
    store.ingest(started(
        3,
        "POST",
        "https://forms.example/submit",
        &[(
            "Content-Type",
            "application/x-www-form-urlencoded; charset=utf-8",
        )],
    ));
    store.ingest(req_done(3, b"name=J%C3%B6rg&tags=a&tags=b+c", 1.0));
    store.ingest(resp_start(
        3,
        200,
        &[("Content-Type", "application/octet-stream")],
        5.0,
    ));
    store.ingest(resp_done(3, &[0x00, 0xff, 0xfe, 0x80, 0x7f], 6.0));

    // 4: aborted.
    store.ingest(started(4, "GET", "https://down.example/", &[]));
    store.ingest(ProxyEvent::Aborted {
        id: ExchangeId(4),
        error: ExchangeError {
            kind: ErrorKind::DnsFailed,
            message: "getaddrinfo ENOTFOUND down.example".into(),
            code: Some("ENOTFOUND".into()),
        },
        at: 4.0,
    });

    // 5: websocket with text + binary messages and a close.
    store.ingest(started(
        5,
        "GET",
        "wss://ws.example/live",
        &[("Upgrade", "websocket")],
    ));
    store.ingest(req_done(5, b"", 0.5));
    store.ingest(resp_start(5, 101, &[("Upgrade", "websocket")], 2.0));
    store.ingest(resp_done(5, b"", 2.0));
    store.ingest(ProxyEvent::StreamOpened {
        id: ExchangeId(5),
        kind: ExchangeKind::WebSocket,
    });
    store.ingest(ws_message(
        5,
        0,
        Direction::Sent,
        MessageKind::Text,
        "héllo".as_bytes(),
    ));
    store.ingest(ws_message(
        5,
        1,
        Direction::Received,
        MessageKind::Binary,
        &[1, 2, 3, 0xff],
    ));
    store.ingest(ws_message(
        5,
        2,
        Direction::Received,
        MessageKind::Ping,
        &[0x9f],
    ));
    store.ingest(ProxyEvent::StreamClosed {
        id: ExchangeId(5),
        code: Some(1000),
        reason: Some("done".into()),
        by: Direction::Received,
        at: 500.0,
    });

    // 6: TLS failure, 7: tunnel, 8: client error.
    let mut tls_client = client();
    tls_client.tls = Some(TlsInfo {
        sni: Some("pinned.example".into()),
        alpn_offered: vec!["h2".into()],
        ja3: Some("ja3hash".into()),
        ..TlsInfo::default()
    });
    store.ingest(ProxyEvent::TlsFailed {
        id: ExchangeId(6),
        client: tls_client,
        host: "pinned.example".into(),
        port: 8443,
        error: ExchangeError {
            kind: ErrorKind::ClientTlsRejected,
            message: "unknown ca".into(),
            code: Some("cert-rejected".into()),
        },
        at: UnixMicros(T0.0 + 1000),
    });
    store.ingest(ProxyEvent::TunnelOpened {
        id: ExchangeId(7),
        client: client(),
        host: "tunnel.example".into(),
        port: 443,
        reason: "passthrough".into(),
        at: UnixMicros(T0.0 + 2000),
    });
    store.ingest(ProxyEvent::TunnelClosed {
        id: ExchangeId(7),
        stats: TunnelStats {
            bytes_sent: 10,
            bytes_received: 20,
            reason: "passthrough".into(),
        },
        at: 50.0,
    });
    store.ingest(ProxyEvent::ClientError {
        id: ExchangeId(8),
        client: client(),
        error: ExchangeError {
            kind: ErrorKind::ClientProtocolError,
            message: "bad request line".into(),
            code: None,
        },
        raw_prefix: Bytes::from_static(b"GARBAGE\x00\x01"),
        at: UnixMicros(T0.0 + 3000),
    });

    // User attributes.
    store.patch(
        &[ExchangeId(1)],
        &ExchangePatch {
            pinned: Some(true),
            color: Some(Some("green".into())),
            note: Some(Some("important".into())),
        },
    );
    store
}

#[test]
fn export_structure() {
    let store = rich_store();
    let har = store.export_har(None);
    let log = &har["log"];
    assert_eq!(log["version"], "1.2");
    assert_eq!(log["creator"]["name"], "Akev");
    assert_eq!(log["creator"]["version"], akev_core::VERSION);
    assert!(log.get("pages").is_none());
    let entries = log["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 5, "HTTP + WS entries only");

    let e = &entries[0];
    assert_eq!(e["startedDateTime"], T0.to_rfc3339());
    assert_eq!(e["request"]["method"], "GET");
    assert_eq!(e["request"]["httpVersion"], "HTTP/1.1");
    assert_eq!(
        e["request"]["headers"][0],
        serde_json::json!({"name": "Host", "value": "api.example.com"})
    );
    assert_eq!(
        e["request"]["queryString"],
        serde_json::json!([{"name": "q", "value": "red shoes"}, {"name": "page", "value": "2"}])
    );
    assert_eq!(
        e["request"]["cookies"],
        serde_json::json!([{"name": "sid", "value": "abc"}, {"name": "theme", "value": "dark"}])
    );
    assert_eq!(e["request"]["bodySize"], 0);
    assert!(
        e["request"]["headersSize"].as_i64().unwrap() > 0,
        "computed for HTTP/1"
    );
    assert!(e["request"].get("postData").is_none());
    let cookie = &e["response"]["cookies"][0];
    assert_eq!(cookie["name"], "token");
    assert_eq!(cookie["value"], "a=b");
    assert_eq!(cookie["expires"], "2015-10-21T07:28:00.000Z");
    assert_eq!(
        (cookie["httpOnly"].as_bool(), cookie["secure"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(cookie["sameSite"], "Strict");
    assert_eq!(e["response"]["content"]["text"], r#"{"results":["shoe"]}"#);
    assert_eq!(e["response"]["content"]["mimeType"], "application/json");
    assert_eq!(e["serverIPAddress"], "2001:db8::1");
    assert_eq!(e["connection"], "7");
    assert_eq!(e["comment"], "important");
    assert_eq!(e["_pinned"], true);
    let t = &e["timings"];
    assert_eq!(
        (
            t["blocked"].as_f64(),
            t["dns"].as_f64(),
            t["connect"].as_f64(),
            t["ssl"].as_f64()
        ),
        (Some(-1.0), Some(2.0), Some(12.0), Some(7.0))
    );
    assert_eq!(
        (
            t["send"].as_f64(),
            t["wait"].as_f64(),
            t["receive"].as_f64()
        ),
        (Some(1.0), Some(25.0), Some(5.0))
    );
    assert_eq!(e["time"].as_f64(), Some(45.0));
    let ext = &e["_akev"];
    assert_eq!(ext["id"], 1);
    assert_eq!(ext["kind"], "http");
    assert_eq!(ext["pinned"], true);
    assert_eq!(ext["color"], "green");
    assert_eq!(ext["note"], "important");
    assert_eq!(ext["source"], "curl");

    // gzip: decoded text, compression, redirect.
    let e = &entries[1];
    let content = &e["response"]["content"];
    let html = "<!doctype html><title>moved</title>".repeat(20);
    assert_eq!(content["text"], html.as_str());
    assert_eq!(content["size"], html.len());
    let wire = e["response"]["bodySize"].as_u64().unwrap();
    assert!(wire < html.len() as u64);
    assert_eq!(content["compression"], html.len() as u64 - wire);
    assert_eq!(e["response"]["redirectURL"], "https://new.example/page");
    assert_eq!(e["response"]["status"], 301);

    // Form POST params (charset parameter allowed) + binary response as base64.
    let e = &entries[2];
    let post = &e["request"]["postData"];
    assert_eq!(
        post["mimeType"],
        "application/x-www-form-urlencoded; charset=utf-8"
    );
    assert_eq!(post["text"], "name=J%C3%B6rg&tags=a&tags=b+c");
    assert_eq!(
        post["params"],
        serde_json::json!([{"name": "name", "value": "Jörg"}, {"name": "tags", "value": "a"}, {"name": "tags", "value": "b c"}])
    );
    assert_eq!(e["response"]["content"]["encoding"], "base64");
    assert_eq!(e["response"]["content"]["text"], "AP/+gH8=");

    // Aborted.
    let e = &entries[3];
    assert_eq!(e["response"]["status"], 0);
    assert_eq!(
        e["response"]["_error"],
        "getaddrinfo ENOTFOUND down.example"
    );
    assert_eq!(e["_akev"]["error"]["kind"], "dns-failed");
    assert_eq!(e["_akev"]["state"], "aborted");

    // WebSocket (Chrome format: client perspective, epoch seconds).
    let e = &entries[4];
    assert_eq!(e["_resourceType"], "websocket");
    let msgs = e["_webSocketMessages"].as_array().unwrap();
    assert_eq!(msgs.len(), 3);
    assert_eq!(msgs[0]["type"], "send");
    assert_eq!(msgs[0]["opcode"], 1);
    assert_eq!(msgs[0]["data"], "héllo");
    assert_eq!(msgs[0]["time"].as_f64(), Some((T0.0 + 10_000) as f64 / 1e6));
    assert_eq!(msgs[1]["type"], "receive");
    assert_eq!(msgs[1]["opcode"], 2);
    assert_eq!(msgs[1]["data"], "AQID/w==");
    assert_eq!(msgs[2]["opcode"], 9);
    assert_eq!(msgs[2]["encoding"], "base64");
    assert_eq!(e["_webSocketClose"]["code"], 1000);
    assert_eq!(e["_webSocketClose"]["reason"], "done");

    // Extension arrays.
    let tls = &log["_tlsErrors"][0];
    assert_eq!(tls["hostname"], "pinned.example");
    assert_eq!(tls["cause"], "cert-rejected");
    assert_eq!(tls["clientIPAddress"], "127.0.0.1");
    assert_eq!(tls["clientPort"], 50000);
    assert_eq!(tls["tlsMetadata"]["sniHostname"], "pinned.example");
    assert_eq!(tls["tlsMetadata"]["ja3Fingerprint"], "ja3hash");
    let tunnel = &log["_rawTunnels"][0];
    assert_eq!(
        (
            tunnel["hostname"].as_str(),
            tunnel["port"].as_u64(),
            tunnel["time"].as_f64()
        ),
        (Some("tunnel.example"), Some(443), Some(50.0))
    );
    assert_eq!(log["_clientErrors"][0]["rawPrefix"], "R0FSQkFHRQAB");

    // Subset export.
    let subset = store.export_har(Some(&[
        ExchangeId(3),
        ExchangeId(1),
        ExchangeId(3),
        ExchangeId(404),
    ]));
    let ids: Vec<u64> = subset["log"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["_akev"]["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids, vec![1, 3]);
    assert!(subset["log"].get("_tlsErrors").is_none());
}

#[test]
fn har_roundtrip_preserves_traffic() {
    let source = rich_store();
    let har = serde_json::to_vec(&source.export_har(None)).unwrap();
    let target = store();
    let ids = target.import_har(&har).unwrap();
    assert_eq!(ids.len(), 8);
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "fresh ids in order");
    let originals = source.summaries();
    let imported = target.summaries();
    for (o, i) in originals.iter().zip(&imported) {
        assert_ne!(o.id, i.id);
        assert_eq!((o.kind, o.state), (i.kind, i.state), "{:?}", o.id);
        assert_eq!(
            (&o.method, &o.scheme, &o.host, o.port, &o.path),
            (&i.method, &i.scheme, &i.host, i.port, &i.path)
        );
        assert_eq!(
            (o.status, o.error, o.pinned, &o.color, o.has_note),
            (i.status, i.error, i.pinned, &i.color, i.has_note)
        );
        assert_eq!(
            (o.message_count, &o.source, o.started_at),
            (i.message_count, &i.source, i.started_at)
        );
        assert_eq!(o.content_type, i.content_type);
        assert_eq!(o.duration, i.duration, "{:?}", o.id);
    }
    let [a1, a2, a3, _a4, a5, a6, a7, a8] = ids[..] else {
        panic!()
    };

    // Timing and client info survive via `_akev`.
    let (d_src, d_dst) = (
        source.detail(ExchangeId(1)).unwrap(),
        target.detail(a1).unwrap(),
    );
    assert_eq!(d_src.timing, d_dst.timing);
    assert_eq!(d_src.client, d_dst.client);
    assert_eq!(d_src.upstream, d_dst.upstream);
    assert_eq!(d_dst.note.as_deref(), Some("important"));
    assert_eq!(d_src.request.headers, d_dst.request.headers);
    assert_eq!(
        d_src.response.unwrap().headers,
        d_dst.response.clone().unwrap().headers
    );

    // gzip body comes back decoded, Content-Encoding dropped (stored headers match stored bytes).
    let d2 = target.detail(a2).unwrap();
    let html = b"<!doctype html><title>moved</title>".repeat(20);
    assert!(
        d2.response
            .as_ref()
            .unwrap()
            .headers
            .get("content-encoding")
            .is_none()
    );
    assert_eq!(
        d2.response.unwrap().headers.get("location"),
        Some("https://new.example/page")
    );
    assert_eq!(
        &target.body(a2, Side::Response).unwrap().bytes[..],
        &html[..]
    );

    // Binary bodies are byte-identical; form bodies keep their raw text.
    assert_eq!(
        &target.body(a3, Side::Response).unwrap().bytes[..],
        &[0x00, 0xff, 0xfe, 0x80, 0x7f]
    );
    assert_eq!(
        &target.body(a3, Side::Request).unwrap().bytes[..],
        b"name=J%C3%B6rg&tags=a&tags=b+c"
    );

    // WebSocket messages incl. binary & control frames.
    let (m_src, m_dst) = (source.messages(ExchangeId(5)), target.messages(a5));
    assert_eq!(m_src.len(), m_dst.len());
    for (s, d) in m_src.iter().zip(&m_dst) {
        assert_eq!(s.message, d.message);
        assert_eq!(s.payload, d.payload);
    }
    assert_eq!(target.summary(a5).unwrap().duration, Some(500.0));

    // Non-HTTP rows.
    let d6 = target.detail(a6).unwrap();
    assert_eq!(
        (d6.summary.host.as_str(), d6.summary.port),
        ("pinned.example", 8443)
    );
    assert_eq!(
        d6.client.tls.as_ref().unwrap().ja3.as_deref(),
        Some("ja3hash")
    );
    assert_eq!(d6.error.unwrap().code.as_deref(), Some("cert-rejected"));
    let d7 = target.detail(a7).unwrap();
    assert_eq!(d7.tunnel.unwrap().bytes_received, 20);
    assert_eq!(
        d7.summary.method, "",
        "tunnel opened via HTTP proxy → no CONNECT label"
    );
    assert_eq!(
        &target.body(a8, Side::Request).unwrap().bytes[..],
        b"GARBAGE\x00\x01"
    );

    // Exporting the imported store again yields the same entries (modulo ids).
    let mut again = target.export_har(None);
    let mut first = source.export_har(None);
    for har in [&mut again, &mut first] {
        strip_volatile(har);
    }
    // The gzip entry differs by design (now stored decoded); compare the others.
    let (a, b) = (&again["log"]["entries"], &first["log"]["entries"]);
    for i in [0usize, 2, 3, 4] {
        assert_eq!(a[i], b[i], "entry {i}");
    }
    assert_eq!(again["log"]["_tlsErrors"], first["log"]["_tlsErrors"]);
    assert_eq!(again["log"]["_rawTunnels"], first["log"]["_rawTunnels"]);
}

fn strip_volatile(v: &mut Value) {
    match v {
        Value::Object(map) => {
            map.remove("id");
            for (_, child) in map.iter_mut() {
                strip_volatile(child);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(strip_volatile),
        _ => {}
    }
}

#[test]
fn import_chrome_har() {
    let store = store();
    let ids = store.import_har(&fixture("chrome.har")).unwrap();
    assert_eq!(ids.len(), 4);
    let s: Vec<_> = ids.iter().map(|id| store.detail(*id).unwrap()).collect();

    // Document: h2 + pseudo headers, gzip header dropped because text is decoded.
    let d = &s[0];
    assert_eq!(d.summary.http_version, HttpVersion::H2);
    assert_eq!(
        d.summary.path, "/?q=red%20shoes&page=2",
        "path kept verbatim"
    );
    assert_eq!(d.summary.state, ExchangeState::Complete);
    assert_eq!(d.summary.content_type.as_deref(), Some("text/html"));
    assert_eq!(d.summary.content_kind, ContentKind::Html);
    let resp = d.response.as_ref().unwrap();
    assert_eq!(resp.status_message, "");
    assert!(resp.headers.get("content-encoding").is_none());
    assert_eq!(
        resp.headers.get("set-cookie"),
        Some("seen=1; Path=/; HttpOnly")
    );
    assert_eq!(d.response_body.wire_size, 53, "wire size from bodySize");
    assert!(!d.response_body.truncated);
    assert_eq!(
        &store.body(ids[0], Side::Response).unwrap().bytes[..],
        b"<!doctype html><title>Shoes</title><p>ok"
    );
    assert_eq!(
        d.upstream.as_ref().unwrap().remote_addr.as_deref(),
        Some("[2606:4700::6810:84e5]:443")
    );
    assert_eq!(
        d.timing.start,
        UnixMicros::from_rfc3339("2024-05-01T10:00:00.123Z").unwrap()
    );
    assert_eq!(d.timing.response_body, Some(85.5));
    assert_eq!(d.timing.response_headers, Some(2.1 + 0.3 + 70.1));
    assert_eq!(d.timing.dns_duration, None, "-1 means unknown");
    assert_eq!(d.summary.duration, Some(85.5));

    // Binary image via base64.
    assert_eq!(store.body(ids[1], Side::Response).unwrap().bytes.len(), 21);
    assert_eq!(s[1].summary.content_kind, ContentKind::Image);

    // Failed request: status 0 + net::ERR → aborted with mapped kind; form body from text.
    let d = &s[2];
    assert_eq!(d.summary.http_version, HttpVersion::H3);
    assert_eq!(d.summary.state, ExchangeState::Aborted);
    assert_eq!(d.summary.error, Some(ErrorKind::DnsFailed));
    assert!(d.response.is_none());
    assert_eq!(
        d.error.as_ref().unwrap().message,
        "net::ERR_NAME_NOT_RESOLVED"
    );
    assert_eq!(
        &store.body(ids[2], Side::Request).unwrap().bytes[..],
        b"item=42&qty=2&note=a+b"
    );
    assert!(d.upstream.is_none(), "empty serverIPAddress ignored");

    // WebSocket: error rows dropped, binary decoded, timings with connect/ssl split.
    let d = &s[3];
    assert_eq!(d.summary.kind, ExchangeKind::WebSocket);
    assert_eq!(d.summary.message_count, 3);
    assert_eq!(
        (
            d.timing.dns_duration,
            d.timing.connect_duration,
            d.timing.tls_duration
        ),
        (Some(2.0), Some(7.0), Some(5.0))
    );
    let msgs = store.messages(ids[3]);
    assert_eq!(msgs[0].message.direction, Direction::Sent);
    assert_eq!(&msgs[0].payload[..], br#"{"subscribe":"prices"}"#);
    assert_eq!(msgs[2].message.kind, MessageKind::Binary);
    assert_eq!(&msgs[2].payload[..], &[0, 1, 2, 0xff]);
    assert_eq!(msgs[1].message.at, UnixMicros(1_714_557_601_500_000));
    assert_eq!(
        msgs.iter().map(|m| m.message.seq).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
}

#[test]
fn import_firefox_har() {
    let store = store();
    let ids = store.import_har(&fixture("firefox.har")).unwrap();
    assert_eq!(ids.len(), 3);
    let d = store.detail(ids[0]).unwrap();
    assert_eq!(d.summary.status, Some(200), "numeric strings accepted");
    assert_eq!(d.summary.http_version, HttpVersion::H2);
    let headers = &d.response.as_ref().unwrap().headers;
    assert!(
        headers.get("content-encoding").is_none(),
        "br dropped: text already decoded"
    );
    assert_eq!(headers.get("content-length"), Some("17"));
    assert_eq!(
        &store.body(ids[0], Side::Response).unwrap().bytes[..],
        br#"{"articles":[1,2,3,4,5]}"#
    );
    assert_eq!(
        d.response_body.wire_size, 24,
        "null bodySize → stored length"
    );
    assert_eq!(
        d.timing.start,
        UnixMicros::from_rfc3339("2024-05-02T06:00:00.100Z").unwrap()
    );
    assert_eq!(
        d.upstream.unwrap().remote_addr.as_deref(),
        Some("198.51.100.7:443")
    );

    // Blocked request: status 0, missing timings, comment → note.
    let d = store.detail(ids[1]).unwrap();
    assert_eq!(d.summary.state, ExchangeState::Aborted);
    assert_eq!(d.note.as_deref(), Some("blocked by an extension"));
    assert!(d.summary.has_note);
    assert_eq!(
        d.summary.http_version,
        HttpVersion::Http11,
        "empty version → guess"
    );

    // 304 with a cached size isn't a truncated body.
    let d = store.detail(ids[2]).unwrap();
    assert_eq!(d.summary.status, Some(304));
    assert!(!d.response_body.truncated);
}

#[test]
fn import_httptoolkit_har() {
    let store = store();
    let ids = store.import_har(&fixture("httptoolkit.har")).unwrap();
    assert_eq!(ids.len(), 4, "3 entries + 1 TLS error");
    let kinds: Vec<_> = ids
        .iter()
        .map(|id| store.summary(*id).unwrap().kind)
        .collect();
    // Chronological: the TLS error (12:00:01.5) sits between the first two entries.
    assert_eq!(
        kinds,
        vec![
            ExchangeKind::Http,
            ExchangeKind::TlsFailure,
            ExchangeKind::WebSocket,
            ExchangeKind::Http
        ]
    );

    let d = store.detail(ids[0]).unwrap();
    assert!(d.summary.pinned, "_pinned honoured");
    assert_eq!(
        &store.body(ids[0], Side::Request).unwrap().bytes[..],
        &[0x00, 0xff, 0x10, 0xff],
        "_content preferred"
    );
    assert_eq!(
        d.request_trailers.unwrap().get("x-upload-checksum"),
        Some("deadbeef")
    );
    assert_eq!(
        d.response_trailers.unwrap().get("server-timing"),
        Some("db;dur=3")
    );
    assert_eq!(d.summary.duration, Some(25.5));

    let tls = store.detail(ids[1]).unwrap();
    assert_eq!(tls.summary.host, "pinned.example");
    assert_eq!(tls.summary.error, Some(ErrorKind::ClientTlsRejected));
    let info = tls.client.tls.unwrap();
    assert_eq!(
        info.alpn_offered,
        vec!["h2".to_string(), "http/1.1".to_string()]
    );
    assert_eq!(info.ja4.as_deref(), Some("t13d1516h2"));
    assert_eq!(tls.client.remote_addr, "[::ffff:127.0.0.1]:61000");

    let ws = store.summary(ids[2]).unwrap();
    assert_eq!((ws.message_count, ws.state), (2, ExchangeState::Complete));
    assert_eq!(
        ws.duration,
        Some(5000.0),
        "close timestamp relative to start"
    );

    let big = store.detail(ids[3]).unwrap();
    assert!(big.response_body.truncated, "discarded body flagged");
    assert_eq!(big.response_body.wire_size, 500_000_000);
    assert_eq!(big.response_body.captured_size, 0);
}

#[test]
fn import_charles_har_skips_invalid_entries() {
    let store = store();
    let ids = store.import_har(&fixture("charles.har")).unwrap();
    assert_eq!(ids.len(), 2, "unparseable URL entry skipped");
    // Sorted chronologically: the entry without an offset (UTC 09:29:59) comes after
    // 09:30+01:00 (= 08:30 UTC).
    let login = store.detail(ids[0]).unwrap();
    assert_eq!(login.summary.method, "POST");
    assert_eq!(login.note.as_deref(), Some("login flow"));
    assert_eq!(
        login.response.as_ref().unwrap().status_message,
        "Found",
        "null statusText → canonical"
    );
    assert_eq!(
        login.request.headers.get("x-count"),
        Some("3"),
        "non-string header values stringified"
    );
    assert_eq!(
        &store.body(ids[0], Side::Request).unwrap().bytes[..],
        br#"{"user":"bob"}"#
    );
    // Base64 gzip bytes with a gzip header are kept encoded (header kept).
    let resp_headers = &login.response.as_ref().unwrap().headers;
    assert_eq!(resp_headers.get("content-encoding"), Some("gzip"));
    assert_eq!(login.response_body.decoded_size, Some(5));
    assert_eq!(store.body(ids[0], Side::Response).unwrap().bytes.len(), 25);
    assert!(login.upstream.is_none(), "invalid serverIPAddress ignored");
    assert_eq!(
        (login.timing.connect_duration, login.timing.tls_duration),
        (Some(8.0), Some(12.0))
    );

    let old = store.detail(ids[1]).unwrap();
    assert_eq!(old.summary.http_version, HttpVersion::Http10);
    assert_eq!(old.response.as_ref().unwrap().status_message, "OK");
    assert_eq!(
        old.timing.start,
        UnixMicros::from_rfc3339("2024-05-04T09:29:59Z").unwrap()
    );
    assert!(
        old.summary.duration.unwrap() >= 0.0,
        "negative time ignored"
    );
}

#[test]
fn import_fiddler_style_har_with_bom() {
    let mut har = b"\xEF\xBB\xBF".to_vec();
    har.extend_from_slice(
        br#"{"log":{"version":"1.2","creator":{"name":"Fiddler","version":"5.0"},"comment":"",
        "entries":[{"startedDateTime":"2024-05-05T10:00:00.1234567+02:00","time":12,"comment":"",
        "request":{"method":"GET","url":"http://fiddler.example/a b","httpVersion":"HTTP/1.1","headers":[{"name":"HOST","value":"fiddler.example"}],"queryString":[],"cookies":[],"headersSize":40,"bodySize":-1,"comment":""},
        "response":{"status":200,"statusText":"OK","httpVersion":"HTTP/1.1","headers":[{"name":"CONTENT-TYPE","value":"text/plain"}],"cookies":[],"content":{"size":2,"mimeType":"text/plain","text":"aGk=","encoding":"base64","comment":""},"redirectURL":"","headersSize":30,"bodySize":2,"comment":""},
        "cache":{},"timings":{"blocked":-1,"dns":-1,"connect":-1,"send":0,"wait":10,"receive":2}}]}}"#,
    );
    let store = store();
    let ids = store.import_har(&har).unwrap();
    let d = store.detail(ids[0]).unwrap();
    assert_eq!(
        d.timing.start,
        UnixMicros::from_rfc3339("2024-05-05T08:00:00.1234567Z").unwrap()
    );
    assert_eq!(d.request.headers.get("host"), Some("fiddler.example"));
    assert_eq!(d.summary.content_type.as_deref(), Some("text/plain"));
    assert_eq!(
        &store.body(ids[0], Side::Response).unwrap().bytes[..],
        b"hi"
    );
    assert_eq!(d.request_body.wire_size, 0, "bodySize -1 clamped");
    assert!(d.note.is_none(), "empty comment isn't a note");
}

#[test]
fn import_rejects_garbage_and_allocates_fresh_ids() {
    let store = store();
    assert!(store.import_har(b"<html>").is_err());
    assert!(store.import_har(br#"{"entries":[]}"#).is_err());
    let har = fixture("chrome.har");
    let first = store.import_har(&har).unwrap();
    let second = store.import_har(&har).unwrap();
    assert_eq!(first.len(), second.len());
    assert!(
        first.iter().all(|id| !second.contains(id)),
        "repeat imports never collide"
    );
    assert_eq!(store.summaries().len(), 8);
}

#[test]
fn import_broadcasts_added() {
    let store = store();
    let mut rx = store.subscribe();
    let ids = store.import_har(&fixture("firefox.har")).unwrap();
    let events = drain(&mut rx);
    assert_eq!(kinds(&events), vec!["added"; 3]);
    let added: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            akev_store::StoreEvent::Added(s) => Some(s.id),
            _ => None,
        })
        .collect();
    assert_eq!(added, ids);
}

/// Regressions of HTTP Toolkit HAR export quirks that Akev must not repeat.
#[test]
fn http_toolkit_har_bugs_are_fixed() {
    let store = store();
    // Response httpVersion must come from the response, not the request.
    let mut ev = started(1, "GET", "https://v.example/", &[]);
    if let ProxyEvent::RequestStarted { head, .. } = &mut ev {
        head.http_version = HttpVersion::Http11;
    }
    store.ingest(ev);
    store.ingest(ProxyEvent::ResponseStarted {
        id: ExchangeId(1),
        head: ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::H2,
            headers: headers(&[("Content-Type", "text/plain")]),
        },
        at: 1.0,
    });
    store.ingest(resp_done(1, b"ok", 2.0));
    let har = store.export_har(None);
    let e = &har["log"]["entries"][0];
    assert_eq!(e["request"]["httpVersion"], "HTTP/1.1");
    assert_eq!(e["response"]["httpVersion"], "HTTP/2");
    assert_eq!(
        e["response"]["headersSize"], -1,
        "HTTP/2 header blocks have no wire size"
    );

    // Import: unknown version strings don't abort; -1 sizes are clamped; duplicate headers
    // survive; one bad entry doesn't sink the file.
    let har = br#"{"log":{"entries":[
        {"startedDateTime":"2024-01-01T00:00:00Z","request":{"method":"GET","url":"https://x.example/","httpVersion":"SPDY/9","headers":[{"name":"X-Dup","value":"1"},{"name":"x-dup","value":"2"}],"bodySize":-1},
         "response":{"status":200,"httpVersion":"weird","headers":[{"name":"Set-Cookie","value":"a=1"},{"name":"Set-Cookie","value":"b=2"}],"content":{"size":-1,"text":"hi"},"bodySize":-1},"timings":{"send":-1,"wait":-1,"receive":-1}},
        {"startedDateTime":"2024-01-01T00:00:01Z","request":{"method":"GET"}},
        42
    ]}}"#;
    let ids = store.import_har(har).unwrap();
    assert_eq!(ids.len(), 1);
    let d = store.detail(ids[0]).unwrap();
    assert_eq!(d.summary.http_version, HttpVersion::Http11);
    assert_eq!(
        d.request.headers.get_all("x-dup").collect::<Vec<_>>(),
        vec!["1", "2"]
    );
    assert_eq!(d.response.unwrap().headers.get_all("set-cookie").count(), 2);
    assert_eq!(
        (d.request_body.wire_size, d.response_body.wire_size),
        (0, 2)
    );
    assert!(d.timing.response_body.unwrap() >= 0.0);
}

#[test]
fn websocket_message_times_survive_microsecond_precision() {
    let source = store();
    source.ingest(started(1, "GET", "ws://t.example/", &[]));
    source.ingest(resp_start(1, 101, &[], 1.0));
    source.ingest(ProxyEvent::StreamOpened {
        id: ExchangeId(1),
        kind: ExchangeKind::WebSocket,
    });
    for (seq, micros) in [123_457i64, 999_999, 1, 654_321].into_iter().enumerate() {
        source.ingest(ProxyEvent::StreamMessage {
            id: ExchangeId(1),
            message: StreamMessage {
                seq: seq as u32,
                direction: Direction::Sent,
                kind: MessageKind::Text,
                at: UnixMicros(1_714_557_601_000_000 + micros),
                size: 1,
                event: None,
                event_id: None,
                modified: false,
            },
            payload: Bytes::from_static(b"x"),
        });
    }
    let har = serde_json::to_vec(&source.export_har(None)).unwrap();
    let target = store();
    let ids = target.import_har(&har).unwrap();
    let a: Vec<_> = source
        .messages(ExchangeId(1))
        .into_iter()
        .map(|m| m.message.at)
        .collect();
    let b: Vec<_> = target
        .messages(ids[0])
        .into_iter()
        .map(|m| m.message.at)
        .collect();
    assert_eq!(a, b);
}

/// A GET whose response carries `body` under the given response headers.
fn exchange_with_response(store: &Store, id: u64, hdrs: &[(&str, &str)], body: &[u8]) {
    store.ingest(started(id, "GET", "https://dl.example/file", &[]));
    store.ingest(req_done(id, b"", 1.0));
    store.ingest(resp_start(id, 200, hdrs, 10.0));
    store.ingest(resp_done(id, body, 12.5));
}

#[test]
fn write_har_streams_exactly_the_pretty_export() {
    let source = rich_store();
    for ids in [
        None,
        Some(&[ExchangeId(3), ExchangeId(1), ExchangeId(404)][..]),
    ] {
        let mut streamed = Vec::new();
        let written = source.write_har(&mut streamed, ids).unwrap();
        let expected = serde_json::to_vec_pretty(&source.export_har(ids)).unwrap();
        assert_eq!(
            String::from_utf8(streamed).unwrap(),
            String::from_utf8(expected).unwrap()
        );
        assert_eq!(written, if ids.is_none() { 8 } else { 2 });
    }
    let empty = store();
    let mut streamed = Vec::new();
    assert_eq!(empty.write_har(&mut streamed, None).unwrap(), 0);
    assert_eq!(
        streamed,
        serde_json::to_vec_pretty(&empty.export_har(None)).unwrap()
    );
    // And it imports back.
    let target = store();
    assert_eq!(
        target
            .import_har(&{
                let mut v = Vec::new();
                source.write_har(&mut v, None).unwrap();
                v
            })
            .unwrap()
            .len(),
        8
    );
}

#[test]
fn doubly_encoded_bodies_survive_a_har_round_trip() {
    // A .tar.gz served with `Content-Encoding: gzip`: decoding once gives the .gz file.
    let inner = gzip(b"hello tarball contents, hello tarball contents");
    let wire = gzip(&inner);
    let source = store();
    exchange_with_response(
        &source,
        1,
        &[
            ("Content-Type", "application/gzip"),
            ("Content-Encoding", "gzip"),
        ],
        &wire,
    );
    let mut har = source.export_har(None);
    let entry = &har["log"]["entries"][0];
    // The flag is explicit whenever a content-coding is declared.
    assert_eq!(entry["_akev"]["responseBodyEncoded"], Value::Bool(false));
    assert_eq!(entry["response"]["content"]["encoding"], "base64");

    let check = |har: &Value| {
        let target = store();
        let ids = target
            .import_har(&serde_json::to_vec(har).unwrap())
            .unwrap();
        let detail = target.detail(ids[0]).unwrap();
        let headers = &detail.response.as_ref().unwrap().headers;
        // Stored decoded: the header is gone and the body is the .gz file, not its contents.
        assert!(!headers.contains("content-encoding"));
        assert_eq!(
            &target.body(ids[0], Side::Response).unwrap().bytes[..],
            &inner[..]
        );
    };
    check(&har);
    // A third-party HAR (no `_akev`) still gets the signature heuristic.
    har["log"]["entries"][0]
        .as_object_mut()
        .unwrap()
        .remove("_akev");
    let target = store();
    let ids = target
        .import_har(&serde_json::to_vec(&har).unwrap())
        .unwrap();
    assert!(
        target
            .detail(ids[0])
            .unwrap()
            .response
            .unwrap()
            .headers
            .contains("content-encoding")
    );
}

#[test]
fn oversized_decoded_bodies_are_exported_still_encoded() {
    // 65 MiB of zeros (65 KB gzipped) decodes past the 64 MiB per-body export cap.
    let wire = gzip(&vec![0u8; 65 << 20]);
    let source = store();
    exchange_with_response(
        &source,
        1,
        &[
            ("Content-Type", "application/octet-stream"),
            ("Content-Encoding", "gzip"),
        ],
        &wire,
    );
    let mut streamed = Vec::new();
    source.write_har(&mut streamed, None).unwrap();
    assert!(streamed.len() < 1 << 20, "{} bytes", streamed.len());
    let har: Value = serde_json::from_slice(&streamed).unwrap();
    let entry = &har["log"]["entries"][0];
    assert_eq!(entry["_akev"]["responseBodyEncoded"], Value::Bool(true));
    assert_eq!(entry["response"]["content"]["encoding"], "base64");
    // Round trip: the raw bytes and the header come back.
    let target = store();
    let ids = target.import_har(&streamed).unwrap();
    assert_eq!(
        &target.body(ids[0], Side::Response).unwrap().bytes[..],
        &wire[..]
    );
    assert!(
        target
            .detail(ids[0])
            .unwrap()
            .response
            .unwrap()
            .headers
            .contains("content-encoding")
    );
}

#[test]
fn control_heavy_text_is_exported_as_base64() {
    // Valid UTF-8, but JSON would escape every NUL as `\u0000` (6x).
    let body = vec![0u8; 10_000];
    let source = store();
    complete_exchange(&source, 1, "https://a.example/nul", "text/plain", &body);
    complete_exchange(
        &source,
        2,
        "https://a.example/text",
        "text/plain",
        b"line\n\ttab",
    );
    let har = source.export_har(None);
    let content = |i: usize| &har["log"]["entries"][i]["response"]["content"];
    assert_eq!(content(0)["encoding"], "base64");
    assert_eq!(content(1)["text"], "line\n\ttab");
    assert!(content(1).get("encoding").is_none());
    let target = store();
    let ids = target
        .import_har(&serde_json::to_vec(&har).unwrap())
        .unwrap();
    assert_eq!(
        &target.body(ids[0], Side::Response).unwrap().bytes[..],
        &body[..]
    );
}
