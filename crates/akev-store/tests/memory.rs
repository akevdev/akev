//! Body memory budget, spill-to-disk, eviction and settings updates.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::settings::CaptureSettings;
use akev_core::*;
use akev_store::{ExchangePatch, Store, StoreEvent};
use common::*;
use std::sync::Arc;

fn spill_store(budget: u64, dir: &std::path::Path) -> Arc<Store> {
    let settings = CaptureSettings {
        memory_budget_bytes: budget,
        ..CaptureSettings::default()
    };
    Store::new(settings, Some(dir))
}

#[test]
fn bodies_spill_under_tiny_budget_and_read_back_identical() {
    let dir = tempfile::tempdir().unwrap();
    let store = spill_store(4096, &dir.path().join("spill"));
    let mut expected = Vec::new();
    for i in 0..40u64 {
        let id = i + 1;
        let req = noise(500 + i as usize * 13, i as u32);
        let resp = if i % 2 == 0 {
            b"compressible ".repeat(300 + i as usize)
        } else {
            noise(3000, 1000 + i as u32)
        };
        store.ingest(started(id, "POST", "http://spill.example/", &[]));
        store.ingest(req_done(id, &req, 1.0));
        store.ingest(resp_start(
            id,
            200,
            &[("Content-Type", "application/octet-stream")],
            2.0,
        ));
        store.ingest(resp_done(id, &resp, 3.0));
        expected.push((ExchangeId(id), req, resp));
    }
    let stats = store.stats();
    assert!(
        stats.memory_body_bytes <= 4096,
        "memory {} over budget",
        stats.memory_body_bytes
    );
    let total: u64 = expected
        .iter()
        .map(|(_, a, b)| (a.len() + b.len()) as u64)
        .sum();
    assert_eq!(
        stats.memory_body_bytes + stats.spilled_body_bytes,
        total,
        "every byte accounted"
    );
    assert!(stats.spilled_body_bytes > 0);
    assert!(
        std::fs::read_dir(dir.path().join("spill")).is_ok(),
        "spill dir created"
    );

    for (id, req, resp) in &expected {
        let r = store.body(*id, Side::Request).unwrap();
        assert_eq!(&r.bytes[..], &req[..], "request body {id:?}");
        assert_eq!(r.wire_size, req.len() as u64);
        assert_eq!(
            &store.body(*id, Side::Response).unwrap().bytes[..],
            &resp[..],
            "response body {id:?}"
        );
        // Metadata never needs the spill file.
        let d = store.detail(*id).unwrap();
        assert_eq!(d.response_body.captured_size, resp.len() as u64);
    }
    // The newest bodies are the ones still in memory (oldest spill first).
    let snapshot = store.export_har(Some(&[ExchangeId(1)]));
    assert_eq!(snapshot["log"]["entries"][0]["request"]["bodySize"], 500);

    // Deleting/clearing frees both memory and spill accounting.
    store.delete(&[ExchangeId(1), ExchangeId(2)]);
    let after = store.stats();
    assert!(after.memory_body_bytes + after.spilled_body_bytes < total);
    store.clear(false);
    let cleared = store.stats();
    assert_eq!(
        (cleared.memory_body_bytes, cleared.spilled_body_bytes),
        (0, 0)
    );

    // The store keeps working (and spilling) after a clear.
    let big = noise(10_000, 7);
    complete_exchange(
        &store,
        100,
        "http://spill.example/again",
        "application/octet-stream",
        &big,
    );
    complete_exchange(
        &store,
        101,
        "http://spill.example/again",
        "application/octet-stream",
        &big,
    );
    assert_eq!(
        &store.body(ExchangeId(100), Side::Response).unwrap().bytes[..],
        &big[..]
    );
    assert!(store.stats().spilled_body_bytes >= 10_000);
}

#[test]
fn websocket_payloads_spill_and_read_back() {
    let dir = tempfile::tempdir().unwrap();
    let store = spill_store(1024, dir.path());
    let id = ExchangeId(1);
    store.ingest(started(1, "GET", "ws://ws.example/", &[]));
    store.ingest(resp_start(1, 101, &[], 1.0));
    store.ingest(ProxyEvent::StreamOpened {
        id,
        kind: ExchangeKind::WebSocket,
    });
    let mut payloads = Vec::new();
    for seq in 0..60u32 {
        let p = noise(97 + seq as usize, seq);
        store.ingest(ws_message(
            1,
            seq,
            if seq % 2 == 0 {
                Direction::Sent
            } else {
                Direction::Received
            },
            MessageKind::Binary,
            &p,
        ));
        payloads.push(p);
    }
    let stats = store.stats();
    assert!(stats.memory_body_bytes <= 1024);
    assert!(stats.spilled_body_bytes > 0);
    let msgs = store.messages(id);
    assert_eq!(msgs.len(), 60);
    for (m, p) in msgs.iter().zip(&payloads) {
        assert_eq!(&m.payload[..], &p[..], "seq {}", m.message.seq);
    }
    let tail = store.messages_after(id, 55);
    assert_eq!(tail.len(), 4);
    for (m, p) in tail.iter().zip(&payloads[56..]) {
        assert_eq!(&m.payload[..], &p[..], "seq {}", m.message.seq);
    }
    store.delete(&[id]);
    assert_eq!(store.stats().spilled_body_bytes, 0);
    assert_eq!(store.stats().memory_body_bytes, 0);
}

#[test]
fn zero_budget_spills_everything() {
    let store = store_with(|s| s.memory_budget_bytes = 0);
    complete_exchange(&store, 1, "http://z.example/", "text/plain", b"hello world");
    assert_eq!(store.stats().memory_body_bytes, 0);
    assert_eq!(store.stats().spilled_body_bytes, 11);
    assert_eq!(
        &store.body(ExchangeId(1), Side::Response).unwrap().bytes[..],
        b"hello world"
    );
}

#[test]
fn eviction_by_max_exchanges_keeps_pinned() {
    let store = store_with(|s| s.max_exchanges = 3);
    let mut rx = store.subscribe();
    complete_exchange(&store, 1, "http://e.example/1", "text/plain", b"one");
    store.patch(
        &[ExchangeId(1)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    for id in 2..=6 {
        complete_exchange(&store, id, "http://e.example/n", "text/plain", b"many");
    }
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    assert_eq!(
        ids,
        vec![1, 4, 5, 6],
        "oldest unpinned evicted first; pinned kept and exempt"
    );
    let stats = store.stats();
    assert_eq!((stats.exchanges, stats.evicted), (4, 2));
    assert_eq!(stats.memory_body_bytes, 3 + 4 * 3);
    let removed: Vec<ExchangeId> = drain(&mut rx)
        .into_iter()
        .filter_map(|e| match e {
            StoreEvent::Removed(ids) => Some(ids),
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(removed, vec![ExchangeId(2), ExchangeId(3)]);
    // Late events for evicted exchanges are ignored.
    store.ingest(resp_done(2, b"late", 9.0));
    assert!(store.summary(ExchangeId(2)).is_none());

    // Pinned rows never crowd out new traffic.
    store.patch(
        &[ExchangeId(4), ExchangeId(5), ExchangeId(6)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    for id in 7..=10 {
        complete_exchange(&store, id, "http://e.example/new", "text/plain", b"x");
    }
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    assert_eq!(ids, vec![1, 4, 5, 6, 8, 9, 10]);

    // Unpinning re-applies the limit to the now-unpinned rows (oldest first).
    store.patch(
        &[ExchangeId(4), ExchangeId(5)],
        &ExchangePatch {
            pinned: Some(false),
            ..ExchangePatch::default()
        },
    );
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    assert_eq!(ids, vec![1, 6, 8, 9, 10]);

    // Clearing unpinned rows keeps pinned accounting right for later evictions.
    store.clear(true);
    assert_eq!(
        store.summaries().iter().map(|s| s.id.0).collect::<Vec<_>>(),
        vec![1, 6]
    );
    for id in 11..=15 {
        complete_exchange(&store, id, "http://e.example/after", "text/plain", b"y");
    }
    let ids: Vec<u64> = store.summaries().iter().map(|s| s.id.0).collect();
    assert_eq!(ids, vec![1, 6, 13, 14, 15]);
}

#[test]
fn update_settings_applies_limits_live() {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::new(CaptureSettings::default(), Some(dir.path()));
    for id in 1..=10 {
        complete_exchange(
            &store,
            id,
            "http://live.example/",
            "application/octet-stream",
            &noise(1000, id as u32),
        );
    }
    assert_eq!(store.stats().memory_body_bytes, 10_000);
    let mut rx = store.subscribe();
    store.update_settings(CaptureSettings {
        max_exchanges: 6,
        memory_budget_bytes: 2000,
        ..CaptureSettings::default()
    });
    let stats = store.stats();
    assert_eq!((stats.exchanges, stats.evicted), (6, 4));
    assert!(stats.memory_body_bytes <= 2000);
    assert_eq!(stats.memory_body_bytes + stats.spilled_body_bytes, 6000);
    assert!(matches!(drain(&mut rx).as_slice(), [StoreEvent::Removed(ids)] if ids.len() == 4));
    for id in 5..=10 {
        assert_eq!(
            &store.body(ExchangeId(id), Side::Response).unwrap().bytes[..],
            &noise(1000, id as u32)[..]
        );
    }
}

#[test]
fn concurrent_reads_while_ingesting_and_spilling() {
    let dir = tempfile::tempdir().unwrap();
    let store = spill_store(8 * 1024, dir.path());
    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            for id in 1..=300u64 {
                complete_exchange(
                    &store,
                    id,
                    "http://c.example/",
                    "application/octet-stream",
                    &noise(700, id as u32),
                );
                if id % 50 == 0 {
                    store.delete(&[ExchangeId(id - 25)]);
                }
            }
        })
    };
    let readers: Vec<_> = (0..4)
        .map(|_| {
            let store = Arc::clone(&store);
            std::thread::spawn(move || {
                for _ in 0..200 {
                    for s in store.summaries().iter().rev().take(20) {
                        if s.state == ExchangeState::Complete
                            && let Some(body) = store.body(s.id, Side::Response)
                        {
                            assert_eq!(&body.bytes[..], &noise(700, s.id.0 as u32)[..]);
                        }
                    }
                }
            })
        })
        .collect();
    writer.join().unwrap();
    for r in readers {
        r.join().unwrap();
    }
    let stats = store.stats();
    assert_eq!(stats.exchanges, 300 - 6);
    assert_eq!(
        stats.memory_body_bytes + stats.spilled_body_bytes,
        700 * (300 - 6)
    );
}
