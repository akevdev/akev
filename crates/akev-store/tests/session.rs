//! Native `.akev` session files.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{ExchangePatch, Store, StoreError};
use bytes::Bytes;
use common::*;
use std::io::{Read, Write};
use std::sync::Arc;

/// A store with modified, streaming, failed, truncated and annotated exchanges.
fn populated() -> Arc<Store> {
    let store = store();
    complete_exchange(
        &store,
        1,
        "https://a.example/json",
        "application/json",
        br#"{"a":1}"#,
    );

    // Modified request & response with originals, gzip response, trailers, upstream info.
    let id = ExchangeId(2);
    store.ingest(started(
        2,
        "POST",
        "http://b.example/in",
        &[("Content-Type", "text/plain")],
    ));
    store.ingest(ProxyEvent::RequestCompleted {
        id,
        body: CapturedBody::complete(Bytes::from_static(b"original")),
        trailers: Some(headers(&[("X-Sum", "1")])),
        at: 1.0,
    });
    store.ingest(ProxyEvent::RequestModified {
        id,
        head: head(
            "POST",
            "http://b.example/rewritten",
            &[("Content-Type", "text/plain"), ("X-Rule", "yes")],
        ),
        body: Some(CapturedBody::complete(Bytes::from_static(b"rewritten"))),
    });
    store.ingest(ProxyEvent::RuleMatched {
        id,
        rule: "rule-1".into(),
        mocked: false,
    });
    store.ingest(ProxyEvent::Upstream {
        id,
        info: UpstreamInfo {
            remote_addr: Some("10.0.0.2:80".into()),
            reused: true,
            ..UpstreamInfo::default()
        },
        dns_duration: None,
        connect_duration: Some(1.0),
        tls_duration: None,
        request_sent: Some(3.0),
    });
    store.ingest(resp_start(
        2,
        200,
        &[("Content-Encoding", "gzip"), ("Content-Type", "text/plain")],
        9.0,
    ));
    store.ingest(ProxyEvent::ResponseModified {
        id,
        original_head: resp_head(503, &[("Retry-After", "5")]),
        original_body: Some(CapturedBody::complete(Bytes::from_static(b"busy"))),
    });
    store.ingest(ProxyEvent::ResponseCompleted {
        id,
        body: CapturedBody::complete(Bytes::from(gzip(b"compressed text"))),
        trailers: Some(headers(&[("Grpc-Status", "0")])),
        at: 11.0,
    });

    // WebSocket still streaming (state must survive as-is).
    store.ingest(started(3, "GET", "ws://c.example/s", &[]));
    store.ingest(resp_start(3, 101, &[], 1.0));
    store.ingest(ProxyEvent::StreamOpened {
        id: ExchangeId(3),
        kind: ExchangeKind::WebSocket,
    });
    store.ingest(ws_message(3, 0, Direction::Sent, MessageKind::Text, b"hi"));
    store.ingest(ws_message(
        3,
        1,
        Direction::Received,
        MessageKind::Binary,
        &[0xde, 0xad],
    ));

    // Closed WebSocket (close info round trip).
    store.ingest(started(4, "GET", "ws://c.example/closed", &[]));
    store.ingest(resp_start(4, 101, &[], 1.0));
    store.ingest(ProxyEvent::StreamOpened {
        id: ExchangeId(4),
        kind: ExchangeKind::WebSocket,
    });
    store.ingest(ProxyEvent::StreamClosed {
        id: ExchangeId(4),
        code: Some(4000),
        reason: Some("custom".into()),
        by: Direction::Sent,
        at: 77.0,
    });

    // Truncated capture, aborted, paused, TLS failure, tunnel.
    store.ingest(started(5, "PUT", "http://d.example/big", &[]));
    store.ingest(ProxyEvent::RequestCompleted {
        id: ExchangeId(5),
        body: CapturedBody {
            bytes: Bytes::from_static(b"head"),
            wire_size: 9_000_000,
            truncated: true,
        },
        trailers: None,
        at: 100.0,
    });
    store.ingest(ProxyEvent::Aborted {
        id: ExchangeId(5),
        error: ExchangeError {
            kind: ErrorKind::Timeout,
            message: "timed out".into(),
            code: Some("ETIMEDOUT".into()),
        },
        at: 30_000.0,
    });
    store.ingest(started(6, "GET", "http://e.example/held", &[]));
    store.ingest(ProxyEvent::Paused {
        id: ExchangeId(6),
        stage: BreakpointStage::Request,
    });
    store.ingest(ProxyEvent::TlsFailed {
        id: ExchangeId(7),
        client: client(),
        host: "f.example".into(),
        port: 443,
        error: ExchangeError {
            kind: ErrorKind::ClientTlsRejected,
            message: "bad cert".into(),
            code: None,
        },
        at: T0,
    });
    store.ingest(ProxyEvent::TunnelOpened {
        id: ExchangeId(8),
        client: client(),
        host: "g.example".into(),
        port: 5222,
        reason: "non-http".into(),
        at: T0,
    });

    store.patch(
        &[ExchangeId(1), ExchangeId(3)],
        &ExchangePatch {
            pinned: Some(true),
            color: Some(Some("purple".into())),
            note: Some(Some("keep\nthis".into())),
        },
    );
    store
}

#[test]
fn session_roundtrip_is_lossless() {
    let source = populated();
    let mut file = Vec::new();
    source.save_session(&mut file, None).unwrap();
    assert_eq!(&file[..4], &[0x28, 0xb5, 0x2f, 0xfd], "zstd-compressed");

    let target = store();
    skip_ids_of(&source);
    let ids = target.load_session(&mut file.as_slice()).unwrap();
    let originals = source.summaries();
    assert_eq!(ids.len(), originals.len());

    for (orig, new_id) in originals.iter().zip(&ids) {
        let mut a = source.detail(orig.id).unwrap();
        let b = target.detail(*new_id).unwrap();
        assert_ne!(a.summary.id, b.summary.id);
        a.summary.id = b.summary.id;
        assert_eq!(a, b, "detail of {:?}", orig.id);
        for side in [Side::Request, Side::Response] {
            assert_eq!(
                source.body(orig.id, side),
                target.body(*new_id, side),
                "{side:?} body of {:?}",
                orig.id
            );
            assert_eq!(
                source.original_body(orig.id, side),
                target.original_body(*new_id, side)
            );
        }
        let (ma, mb) = (source.messages(orig.id), target.messages(*new_id));
        assert_eq!(ma.len(), mb.len());
        for (x, y) in ma.iter().zip(&mb) {
            assert_eq!((&x.message, &x.payload), (&y.message, &y.payload));
        }
    }
    // Spot checks on things that are easy to lose.
    let s3 = target.summary(ids[2]).unwrap();
    assert_eq!(
        (s3.state, s3.pinned, s3.color.as_deref()),
        (ExchangeState::Streaming, true, Some("purple"))
    );
    assert_eq!(target.summary(ids[3]).unwrap().duration, Some(77.0));
    assert_eq!(
        target.summary(ids[5]).unwrap().paused,
        Some(BreakpointStage::Request)
    );
    let big = target.body(ids[4], Side::Request).unwrap();
    assert_eq!((big.wire_size, big.truncated), (9_000_000, true));

    // Saving the loaded copy again produces an equivalent session.
    let mut file2 = Vec::new();
    target.save_session(&mut file2, None).unwrap();
    let third = store();
    let ids3 = third.load_session(&mut file2.as_slice()).unwrap();
    for (a, b) in ids.iter().zip(&ids3) {
        let mut da = target.detail(*a).unwrap();
        let db = third.detail(*b).unwrap();
        da.summary.id = db.summary.id;
        assert_eq!(da, db);
    }
    // The original HAR export of a loaded session still includes the close info.
    let har = target.export_har(Some(&[ids[3]]));
    assert_eq!(har["log"]["entries"][0]["_webSocketClose"]["code"], 4000);
}

#[test]
fn session_subset_and_spilled_bodies() {
    let dir = tempfile::tempdir().unwrap();
    let settings = akev_core::settings::CaptureSettings {
        memory_budget_bytes: 64,
        ..Default::default()
    };
    let source = Store::new(settings, Some(dir.path()));
    let payload = noise(5000, 3);
    complete_exchange(
        &source,
        1,
        "http://s.example/1",
        "application/octet-stream",
        &payload,
    );
    complete_exchange(
        &source,
        2,
        "http://s.example/2",
        "application/octet-stream",
        b"small",
    );
    assert!(source.stats().spilled_body_bytes >= 5000);
    let mut file = Vec::new();
    source
        .save_session(&mut file, Some(&[ExchangeId(1), ExchangeId(404)]))
        .unwrap();
    let target = store();
    let ids = target.load_session(&mut file.as_slice()).unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(
        &target.body(ids[0], Side::Response).unwrap().bytes[..],
        &payload[..]
    );
}

#[test]
fn load_accepts_uncompressed_jsonl_and_rejects_invalid_files() {
    let source = populated();
    let mut compressed = Vec::new();
    source
        .save_session(&mut compressed, Some(&[ExchangeId(1)]))
        .unwrap();
    let mut plain = String::new();
    zstd::stream::read::Decoder::new(compressed.as_slice())
        .unwrap()
        .read_to_string(&mut plain)
        .unwrap();
    let header: serde_json::Value = serde_json::from_str(plain.lines().next().unwrap()).unwrap();
    assert_eq!(header["format"], "akev-session");
    assert_eq!(header["version"], 1);
    assert_eq!(header["app"], akev_core::VERSION);
    assert!(UnixMicros::from_rfc3339(header["createdAt"].as_str().unwrap()).is_some());

    // Plain JSON lines with blank lines (and CRLF) are fine.
    let crlf = plain.replace('\n', "\r\n\r\n");
    let target = store();
    assert_eq!(target.load_session(&mut crlf.as_bytes()).unwrap().len(), 1);

    let reject = |bytes: &[u8]| match store().load_session(&mut &bytes[..]) {
        Err(StoreError::InvalidFile(msg)) => msg,
        other => panic!("expected InvalidFile, got {other:?}"),
    };
    reject(b"");
    reject(b"   \n\n");
    reject(br#"{"format":"something-else","version":1}"#);
    assert!(
        reject(br#"{"format":"akev-session","version":2}"#).contains("unsupported session version")
    );
    let mut bad_line = plain.lines().next().unwrap().to_string();
    bad_line.push_str("\n{\"detail\": 5}\n");
    assert!(reject(bad_line.as_bytes()).contains("line 2"));
    let mut bad_b64 = plain.clone();
    bad_b64 = bad_b64.replacen("\"data\":\"", "\"data\":\"!!!", 1);
    reject(bad_b64.as_bytes());
    // Truncated zstd stream.
    reject(&compressed[..compressed.len() / 2]);

    // Nothing was inserted by failed loads.
    let probe = store();
    let _ = probe.load_session(&mut bad_line.as_bytes());
    assert!(probe.summaries().is_empty());
}

#[test]
fn save_reports_writer_errors() {
    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("disk full"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let source = populated();
    assert!(matches!(
        source.save_session(&mut Failing, None),
        Err(StoreError::Io(_))
    ));
}

/// Regression: a small `.akev` file must not expand into gigabytes while loading.
mod limits {
    use super::*;
    use akev_store::SessionLimits;

    const HEADER: &[u8] =
        br#"{"format":"akev-session","version":1,"createdAt":"2026-01-01T00:00:00Z","app":"x"}"#;

    fn invalid(result: akev_store::Result<Vec<ExchangeId>>) -> String {
        match result {
            Err(StoreError::InvalidFile(msg)) => msg,
            other => panic!("expected InvalidFile, got {other:?}"),
        }
    }

    /// The header line, then one unterminated string of `size` `A`s, zstd-compressed without
    /// ever holding it in memory.
    fn bomb(size: usize) -> Vec<u8> {
        let mut enc = zstd::stream::write::Encoder::new(Vec::new(), 19).unwrap();
        enc.write_all(HEADER).unwrap();
        enc.write_all(b"\n{\"detail\":{\"x\":\"").unwrap();
        let chunk = vec![b'A'; 1 << 20];
        for _ in 0..size >> 20 {
            enc.write_all(&chunk).unwrap();
        }
        enc.finish().unwrap()
    }

    #[test]
    fn decompression_bomb_is_rejected_early() {
        // 128 MiB in a few KB: stopped by the expansion limit (64 MiB floor), not read whole.
        let file = bomb(128 << 20);
        assert!(file.len() < 64 * 1024, "{} bytes", file.len());
        let store = store();
        let msg = invalid(store.load_session(&mut file.as_slice()));
        assert!(msg.contains("compressed size"), "{msg}");
        assert!(store.summaries().is_empty());
    }

    #[test]
    fn long_lines_and_large_files_are_rejected() {
        let mut plain = HEADER.to_vec();
        plain.push(b'\n');
        plain.extend_from_slice(b"{\"detail\":{\"x\":\"");
        plain.extend(std::iter::repeat_n(b'A', 5000));
        let tight = SessionLimits {
            max_line_bytes: 1000,
            ..SessionLimits::default()
        };
        let msg = invalid(store().load_session_with_limits(&mut plain.as_slice(), tight));
        assert_eq!(msg, "line 2 is longer than 1000 bytes");
        // A blank (whitespace-only) line counts too.
        let mut blank = HEADER.to_vec();
        blank.push(b'\n');
        blank.extend(std::iter::repeat_n(b' ', 5000));
        let msg = invalid(store().load_session_with_limits(&mut blank.as_slice(), tight));
        assert!(msg.contains("longer than 1000 bytes"), "{msg}");

        let small_total = SessionLimits {
            max_total_bytes: 2000,
            ..SessionLimits::default()
        };
        let msg = invalid(store().load_session_with_limits(&mut plain.as_slice(), small_total));
        assert!(msg.contains("larger than 2000 bytes"), "{msg}");
    }

    #[test]
    fn real_sessions_load_within_the_limits() {
        let source = populated();
        let mut file = Vec::new();
        source.save_session(&mut file, None).unwrap();
        let mut plain = Vec::new();
        zstd::stream::read::Decoder::new(file.as_slice())
            .unwrap()
            .read_to_end(&mut plain)
            .unwrap();
        let longest = plain.split(|&b| b == b'\n').map(<[u8]>::len).max().unwrap();
        // Limits exactly at the file's size are enough...
        let exact = SessionLimits {
            max_line_bytes: longest,
            max_total_bytes: plain.len() as u64,
            ..SessionLimits::default()
        };
        let n = source.summaries().len();
        assert_eq!(
            store()
                .load_session_with_limits(&mut file.as_slice(), exact)
                .unwrap()
                .len(),
            n
        );
        assert_eq!(
            store()
                .load_session_with_limits(&mut plain.as_slice(), exact)
                .unwrap()
                .len(),
            n
        );
        // ...one byte less isn't.
        let short_line = SessionLimits {
            max_line_bytes: longest - 1,
            ..exact
        };
        invalid(store().load_session_with_limits(&mut file.as_slice(), short_line));
        // The expansion limit only applies to compressed files, past its floor.
        let no_expansion = SessionLimits {
            max_expansion: 1,
            expansion_floor_bytes: 16,
            ..SessionLimits::default()
        };
        let msg = invalid(store().load_session_with_limits(&mut file.as_slice(), no_expansion));
        assert!(msg.contains("compressed size"), "{msg}");
        assert_eq!(
            store()
                .load_session_with_limits(&mut plain.as_slice(), no_expansion)
                .unwrap()
                .len(),
            n
        );
    }
}
