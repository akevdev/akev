//! Internal unit tests (access to private state): ledger invariants and spill reclamation.

use super::*;
use crate::record::BodyData;
use akev_core::settings::CaptureSettings;
use std::collections::BTreeSet;

const T0: UnixMicros = UnixMicros(1_700_000_000_000_000);

fn head(path: &str) -> RequestHead {
    RequestHead {
        method: "POST".into(),
        url: format!("http://inv.example{path}"),
        scheme: "http".into(),
        host: "inv.example".into(),
        port: 80,
        path: path.into(),
        http_version: HttpVersion::Http11,
        headers: Headers(vec![("Content-Type".into(), "text/plain".into())]),
    }
}

fn body(len: usize, seed: u8) -> CapturedBody {
    CapturedBody::complete(Bytes::from(
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect::<Vec<_>>(),
    ))
}

fn started(id: u64) -> ProxyEvent {
    ProxyEvent::RequestStarted {
        id: ExchangeId(id),
        kind: ExchangeKind::Http,
        head: head("/x"),
        client: ClientInfo {
            connection_id: ConnectionId(id % 3),
            ..ClientInfo::default()
        },
        timing: Timing {
            start: T0,
            ..Timing::default()
        },
    }
}

/// Recompute the ledger from scratch and compare with the incremental one.
fn check_invariants(store: &Store) {
    let inner = store.inner.read();
    let mut memory = 0u64;
    let mut spilled = 0u64;
    let mut index = BTreeSet::new();
    let mut spill_index = BTreeSet::new();
    let mut pinned = 0usize;
    for (id, rec) in &inner.records {
        assert_eq!(*id, rec.id());
        if rec.summary.pinned {
            pinned += 1;
        }
        for slot in [
            Slot::Request,
            Slot::Response,
            Slot::OriginalRequest,
            Slot::OriginalResponse,
        ] {
            if let Some(b) = rec.slot(slot) {
                match &b.data {
                    BodyData::Memory(bytes) if !bytes.is_empty() => {
                        memory += bytes.len() as u64;
                        index.insert((*id, slot));
                    }
                    BodyData::Memory(_) => {}
                    BodyData::Spilled(h) => {
                        spilled += h.raw_len;
                        spill_index.insert((*id, slot));
                        assert_eq!(store.spill.read(h).unwrap().len() as u64, h.raw_len);
                    }
                }
            }
        }
        let mut msg_in_memory = false;
        let mut msg_spilled = false;
        for m in &rec.messages {
            match &m.payload {
                Payload::Memory(b) => {
                    memory += b.len() as u64;
                    msg_in_memory |= !b.is_empty();
                }
                Payload::Spilled { len, .. } => {
                    spilled += len;
                    msg_spilled = true;
                }
            }
        }
        if msg_in_memory {
            index.insert((*id, Slot::Messages));
        }
        if msg_spilled {
            spill_index.insert((*id, Slot::Messages));
        }
    }
    assert_eq!(inner.ledger.memory_bytes, memory, "memory accounting");
    assert_eq!(inner.ledger.spilled_bytes, spilled, "spill accounting");
    assert_eq!(inner.ledger.mem_index, index, "spill index");
    assert_eq!(inner.ledger.spill_index, spill_index, "disk-cap index");
    assert_eq!(inner.pinned, pinned, "pinned counter");
}

#[test]
fn ledger_stays_consistent_through_every_operation() {
    let dir = tempfile::tempdir().unwrap();
    let settings = CaptureSettings {
        memory_budget_bytes: 3000,
        max_exchanges: 25,
        ..CaptureSettings::default()
    };
    let store = Store::new(settings, Some(dir.path()));
    for round in 0..4u64 {
        for i in 0..20u64 {
            let id = round * 100 + i + 1;
            let eid = ExchangeId(id);
            store.ingest(started(id));
            store.ingest(ProxyEvent::RequestCompleted {
                id: eid,
                body: body(100 + i as usize * 10, 1),
                trailers: None,
                at: 1.0,
            });
            if i % 3 == 0 {
                store.ingest(ProxyEvent::RequestModified {
                    id: eid,
                    head: head("/y"),
                    body: Some(body(50, 2)),
                });
            }
            store.ingest(ProxyEvent::ResponseStarted {
                id: eid,
                head: ResponseHead {
                    status: 200,
                    status_message: "OK".into(),
                    http_version: HttpVersion::Http11,
                    headers: Headers::new(),
                },
                at: 2.0,
            });
            if i % 4 == 0 {
                store.ingest(ProxyEvent::ResponseModified {
                    id: eid,
                    original_head: ResponseHead {
                        status: 500,
                        status_message: String::new(),
                        http_version: HttpVersion::Http11,
                        headers: Headers::new(),
                    },
                    original_body: Some(body(70, 3)),
                });
            }
            store.ingest(ProxyEvent::ResponseCompleted {
                id: eid,
                body: body(200, 4),
                trailers: None,
                at: 3.0,
            });
            if i % 5 == 0 {
                store.ingest(ProxyEvent::StreamOpened {
                    id: eid,
                    kind: ExchangeKind::WebSocket,
                });
                for seq in 0..5 {
                    store.ingest(ProxyEvent::StreamMessage {
                        id: eid,
                        message: StreamMessage {
                            seq,
                            direction: Direction::Received,
                            kind: MessageKind::Binary,
                            at: T0,
                            size: 40,
                            event: None,
                            event_id: None,
                            modified: false,
                        },
                        payload: body(40, seq as u8).bytes,
                    });
                }
            }
            if i % 7 == 0 {
                store.patch(
                    &[eid],
                    &ExchangePatch {
                        pinned: Some(true),
                        ..ExchangePatch::default()
                    },
                );
            }
            check_invariants(&store);
        }
        store.delete(&[ExchangeId(round * 100 + 3), ExchangeId(round * 100 + 4)]);
        check_invariants(&store);
        store.patch(
            &[ExchangeId(round * 100 + 1)],
            &ExchangePatch {
                pinned: Some(false),
                ..ExchangePatch::default()
            },
        );
        check_invariants(&store);
        if round == 2 {
            store.clear(true);
            check_invariants(&store);
        }
    }
    store.update_settings(CaptureSettings {
        memory_budget_bytes: 0,
        max_exchanges: 10,
        ..CaptureSettings::default()
    });
    check_invariants(&store);
    assert_eq!(store.stats().memory_body_bytes, 0);

    // Import + session insert paths account bodies too.
    let mut session = Vec::new();
    store.save_session(&mut session, None).unwrap();
    store.update_settings(CaptureSettings {
        memory_budget_bytes: 1 << 20,
        ..CaptureSettings::default()
    });
    store.load_session(&mut session.as_slice()).unwrap();
    check_invariants(&store);
    let har = serde_json::to_vec(&store.export_har(None)).unwrap();
    store.import_har(&har).unwrap();
    check_invariants(&store);

    store.clear(false);
    check_invariants(&store);
    assert_eq!(
        store.spill_disk_bytes(),
        0,
        "spill file truncated when the store is cleared"
    );
}

#[test]
fn spill_space_is_reclaimed_by_deletes() {
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        ..CaptureSettings::default()
    };
    let store = Store::new(settings, None);
    for id in 1..=10u64 {
        store.ingest(started(id));
        store.ingest(ProxyEvent::RequestCompleted {
            id: ExchangeId(id),
            body: body(1000, id as u8),
            trailers: None,
            at: 1.0,
        });
    }
    assert!(store.spill_disk_bytes() > 0);
    let all: Vec<ExchangeId> = (1..=10).map(ExchangeId).collect();
    store.delete(&all[..5]);
    assert!(store.spill_disk_bytes() > 0, "live blobs remain");
    store.delete(&all[5..]);
    assert_eq!(store.spill_disk_bytes(), 0);
    check_invariants(&store);
}

#[test]
fn client_info_is_interned_per_connection() {
    let store = Store::new(CaptureSettings::default(), None);
    for id in 1..=6u64 {
        store.ingest(started(id));
    }
    let inner = store.inner.read();
    let a = &inner.records[&ExchangeId(1)].client;
    let b = &inner.records[&ExchangeId(4)].client;
    let c = &inner.records[&ExchangeId(2)].client;
    assert!(Arc::ptr_eq(a, b), "same connection → shared");
    assert!(!Arc::ptr_eq(a, c));
}

#[test]
fn spilled_bodies_still_report_decoded_size() {
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        ..CaptureSettings::default()
    };
    let store = Store::new(settings, None);
    let json = br#"{"k":"vvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvvv"}"#;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut gz, json).unwrap();
    let gz = gz.finish().unwrap();
    store.ingest(started(1));
    store.ingest(ProxyEvent::ResponseStarted {
        id: ExchangeId(1),
        head: ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers(vec![("Content-Encoding".into(), "gzip".into())]),
        },
        at: 1.0,
    });
    store.ingest(ProxyEvent::ResponseCompleted {
        id: ExchangeId(1),
        body: CapturedBody::complete(Bytes::from(gz)),
        trailers: None,
        at: 2.0,
    });
    assert_eq!(store.stats().memory_body_bytes, 0);
    assert_eq!(
        store
            .detail(ExchangeId(1))
            .unwrap()
            .response_body
            .decoded_size,
        Some(json.len() as u64)
    );
}

#[test]
fn store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
    assert_send_sync::<StoreEvent>();
    assert_send_sync::<MessageWithPayload>();
}

/// Bytes that zstd can't shrink (so spill sizes are predictable).
fn noise(len: usize, seed: u32) -> CapturedBody {
    let mut x = seed.wrapping_mul(0x9e37_79b9) | 1;
    CapturedBody::complete(Bytes::from(
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect::<Vec<_>>(),
    ))
}

fn response(id: u64, body: CapturedBody) -> [ProxyEvent; 2] {
    [
        ProxyEvent::ResponseStarted {
            id: ExchangeId(id),
            head: ResponseHead {
                status: 200,
                status_message: "OK".into(),
                http_version: HttpVersion::Http11,
                headers: Headers(vec![("Content-Type".into(), "text/plain".into())]),
            },
            at: 1.0,
        },
        ProxyEvent::ResponseCompleted {
            id: ExchangeId(id),
            body,
            trailers: None,
            at: 2.0,
        },
    ]
}

#[test]
fn disk_cap_drops_the_oldest_spilled_bodies_and_keeps_the_exchanges() {
    let dir = tempfile::tempdir().unwrap();
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        spill_max_bytes: 40_000,
        ..CaptureSettings::default()
    };
    // 8 KiB segments: the cap covers about five of them.
    let store = Store::with_segment_target(settings, Some(dir.path()), 8 * 1024);
    for id in 1..=40u64 {
        store.ingest(started(id));
        for ev in response(id, noise(4000, id as u32)) {
            store.ingest(ev);
        }
        assert!(
            store.spill_disk_bytes() <= 40_000,
            "disk stays under the cap ({} bytes after {id})",
            store.spill_disk_bytes()
        );
        check_invariants(&store);
    }
    let stats = store.stats();
    assert_eq!(stats.exchanges, 40, "exchanges are kept");
    assert!(stats.disk_dropped_bodies > 0);
    assert_eq!(stats.spill_disk_bytes, store.spill_disk_bytes());
    // The oldest lost their bodies, the newest still have them.
    let oldest = store.detail(ExchangeId(1)).unwrap();
    assert!(oldest.response_body.dropped);
    assert!(oldest.response_body.truncated);
    assert_eq!(oldest.response_body.wire_size, 4000);
    assert_eq!(oldest.response_body.captured_size, 0);
    assert_eq!(
        oldest.notices,
        vec!["Response body deleted to stay within the spill disk limit".to_string()]
    );
    assert!(
        store
            .body(ExchangeId(1), Side::Response)
            .unwrap()
            .bytes
            .is_empty()
    );
    let newest = store.detail(ExchangeId(40)).unwrap();
    assert!(!newest.response_body.dropped);
    assert!(newest.notices.is_empty());
    assert_eq!(
        store.body(ExchangeId(40), Side::Response).unwrap(),
        noise(4000, 40)
    );
    // Sessions keep the notices.
    let mut session = Vec::new();
    store
        .save_session(&mut session, Some(&[ExchangeId(1)]))
        .unwrap();
    let copy = Store::new(CaptureSettings::default(), None);
    let ids = copy.load_session(&mut session.as_slice()).unwrap();
    assert_eq!(copy.detail(ids[0]).unwrap().notices, oldest.notices);
}

#[test]
fn disk_cap_spares_pinned_exchanges() {
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        spill_max_bytes: 20_000,
        ..CaptureSettings::default()
    };
    let store = Store::with_segment_target(settings, None, 4096);
    store.ingest(started(1));
    for ev in response(1, noise(4000, 1)) {
        store.ingest(ev);
    }
    store.patch(
        &[ExchangeId(1)],
        &ExchangePatch {
            pinned: Some(true),
            ..ExchangePatch::default()
        },
    );
    for id in 2..=30u64 {
        store.ingest(started(id));
        for ev in response(id, noise(4000, id as u32)) {
            store.ingest(ev);
        }
    }
    check_invariants(&store);
    let pinned = store.detail(ExchangeId(1)).unwrap();
    assert!(!pinned.response_body.dropped);
    assert_eq!(
        store.body(ExchangeId(1), Side::Response).unwrap(),
        noise(4000, 1)
    );
    assert!(store.detail(ExchangeId(2)).unwrap().response_body.dropped);
    // Unlimited: nothing more is dropped.
    let before = store.stats().disk_dropped_bodies;
    store.update_settings(CaptureSettings {
        memory_budget_bytes: 0,
        spill_max_bytes: 0,
        ..CaptureSettings::default()
    });
    for id in 31..=40u64 {
        store.ingest(started(id));
        for ev in response(id, noise(4000, id as u32)) {
            store.ingest(ev);
        }
    }
    assert_eq!(store.stats().disk_dropped_bodies, before);
}

#[test]
fn disk_cap_drops_spilled_message_payloads() {
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        spill_max_bytes: 10_000,
        ..CaptureSettings::default()
    };
    let store = Store::with_segment_target(settings, None, 2048);
    for id in 1..=10u64 {
        store.ingest(started(id));
        store.ingest(ProxyEvent::StreamOpened {
            id: ExchangeId(id),
            kind: ExchangeKind::WebSocket,
        });
        for seq in 0..3u32 {
            store.ingest(ProxyEvent::StreamMessage {
                id: ExchangeId(id),
                message: StreamMessage {
                    seq,
                    direction: Direction::Received,
                    kind: MessageKind::Binary,
                    at: T0,
                    size: 1000,
                    event: None,
                    event_id: None,
                    modified: false,
                },
                payload: noise(1000, id as u32 * 10 + seq).bytes,
            });
        }
        check_invariants(&store);
    }
    assert!(store.spill_disk_bytes() <= 10_000);
    let first = store.messages(ExchangeId(1));
    assert_eq!(first.len(), 3, "messages are kept");
    assert!(first.iter().all(|m| m.payload.is_empty()));
    assert_eq!(first[0].message.size, 1000);
    assert_eq!(
        store.detail(ExchangeId(1)).unwrap().notices,
        vec!["Message payloads deleted to stay within the spill disk limit".to_string()]
    );
    let last = store.messages(ExchangeId(10));
    assert_eq!(last[2].payload, noise(1000, 102).bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overloaded_queue_drops_bodies_but_keeps_exchanges() {
    let settings = CaptureSettings {
        event_queue_bytes: akev_core::settings::MIN_EVENT_QUEUE_BYTES,
        ..CaptureSettings::default()
    };
    let store = Store::new(settings, None);
    let sink = store.sink();
    let state = sink.backpressure().expect("bounded sink");
    // Hold the records lock (on another thread) so the ingest thread can't drain the queue.
    let (locked_tx, locked_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let holder = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            let _guard = store.inner.write();
            locked_tx.send(()).unwrap();
            let _ = release_rx.recv();
        })
    };
    locked_rx.recv().unwrap();
    for id in 1..=8u64 {
        sink.emit(started(id));
        for ev in response(id, noise(300 * 1024, id as u32)) {
            sink.emit(ev);
        }
    }
    assert!(state.queued_bytes() <= state.capacity() + 64 * 1024);
    assert!(state.is_congested());
    assert!(
        !state.wait_for_space(Duration::from_millis(20)).await,
        "proxy waits briefly, then gives up"
    );
    let dropped = state.dropped_bodies();
    assert!(dropped >= 4, "dropped {dropped}");
    release_tx.send(()).unwrap();
    holder.join().unwrap();
    assert!(state.wait_for_space(Duration::from_secs(5)).await);
    let deadline = Instant::now() + Duration::from_secs(5);
    while store.stats().exchanges < 8 || state.queued_bytes() > 0 {
        assert!(Instant::now() < deadline, "ingest caught up");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let stats = store.stats();
    assert_eq!(stats.dropped_bodies, dropped);
    let kept = store.detail(ExchangeId(1)).unwrap();
    assert!(!kept.response_body.dropped);
    assert_eq!(kept.response_body.captured_size, 300 * 1024);
    let lost = store.detail(ExchangeId(8)).unwrap();
    assert!(lost.response_body.dropped);
    assert_eq!(lost.response_body.wire_size, 300 * 1024);
    assert_eq!(
        lost.notices,
        vec!["Response body not recorded: capture was overloaded".to_string()]
    );
}

#[test]
fn body_filters_read_spilled_bodies_outside_the_lock() {
    let settings = CaptureSettings {
        memory_budget_bytes: 0,
        ..CaptureSettings::default()
    };
    let store = Store::new(settings, None);
    let n = view::SCAN_CHUNK as u64 * 2 + 10;
    for id in 1..=n {
        store.ingest(started(id));
        let text = if id.is_multiple_of(100) {
            "needle"
        } else {
            "hay"
        };
        let body = CapturedBody::complete(Bytes::from(format!("{text} {id}")));
        for ev in response(id, body) {
            store.ingest(ev);
        }
    }
    assert_eq!(store.stats().memory_body_bytes, 0, "everything spilled");
    let filter = akev_filter::Filter::parse("resbody:needle").unwrap();
    let all: Vec<ExchangeId> = (1..=n)
        .filter(|i| i.is_multiple_of(100))
        .map(ExchangeId)
        .collect();
    assert_eq!(store.query_ids(&filter), all);
    let last3: Vec<ExchangeId> = all[all.len() - 3..].to_vec();
    let summaries = store.query(&filter, Some(3));
    assert_eq!(summaries.iter().map(|s| s.id).collect::<Vec<_>>(), last3);
    assert_eq!(store.matches(ExchangeId(100), &filter), Some(true));
    assert_eq!(store.matches(ExchangeId(101), &filter), Some(false));
    assert_eq!(
        store.filter_ids(
            &[ExchangeId(101), ExchangeId(200), ExchangeId(999_999)],
            &filter
        ),
        vec![ExchangeId(200)]
    );
    // A query running while the store changes underneath it stays consistent: the
    // evaluator holds copies, so writers never wait for body reads.
    let writer = {
        let store = Arc::clone(&store);
        std::thread::spawn(move || {
            for id in (1..=n).step_by(2) {
                store.delete(&[ExchangeId(id)]);
            }
        })
    };
    let during = store.query_ids(&filter);
    writer.join().unwrap();
    assert!(during.iter().all(|id| id.0.is_multiple_of(100)));
    check_invariants(&store);
}

#[test]
fn message_pages() {
    let store = Store::new(CaptureSettings::default(), None);
    store.ingest(started(1));
    store.ingest(ProxyEvent::StreamOpened {
        id: ExchangeId(1),
        kind: ExchangeKind::WebSocket,
    });
    for seq in 0..10u32 {
        store.ingest(ProxyEvent::StreamMessage {
            id: ExchangeId(1),
            message: StreamMessage {
                seq,
                direction: Direction::Sent,
                kind: MessageKind::Text,
                at: T0,
                size: 1,
                event: None,
                event_id: None,
                modified: false,
            },
            payload: Bytes::from(seq.to_string()),
        });
    }
    let seqs = |page: MessagePage| -> Vec<u32> {
        store
            .messages_page(ExchangeId(1), page)
            .unwrap()
            .iter()
            .map(|m| m.message.seq)
            .collect()
    };
    assert_eq!(seqs(MessagePage::default()), (0..10).collect::<Vec<_>>());
    let first = MessagePage {
        limit: Some(3),
        ..MessagePage::default()
    };
    assert_eq!(seqs(first), vec![0, 1, 2]);
    let newest = MessagePage {
        limit: Some(3),
        tail: true,
        ..MessagePage::default()
    };
    assert_eq!(seqs(newest), vec![7, 8, 9]);
    let older = MessagePage {
        before: Some(7),
        limit: Some(3),
        tail: true,
        ..MessagePage::default()
    };
    assert_eq!(seqs(older), vec![4, 5, 6]);
    let after = MessagePage {
        after: Some(5),
        limit: Some(2),
        ..MessagePage::default()
    };
    assert_eq!(seqs(after), vec![6, 7]);
    let window = MessagePage {
        after: Some(2),
        before: Some(5),
        ..MessagePage::default()
    };
    assert_eq!(seqs(window), vec![3, 4]);
    assert_eq!(
        store.messages_page(ExchangeId(1), first).unwrap()[1].payload,
        Bytes::from_static(b"1")
    );
    assert!(store.messages_page(ExchangeId(2), first).is_none());
}

#[test]
fn newest_summaries_and_paged_snapshots() {
    let store = Store::new(CaptureSettings::default(), None);
    for id in 1..=10u64 {
        store.ingest(started(id));
    }
    let ids = |v: Vec<ExchangeSummary>| v.into_iter().map(|s| s.id.0).collect::<Vec<_>>();
    assert_eq!(ids(store.newest_summaries(3)), vec![8, 9, 10]);
    assert_eq!(ids(store.newest_summaries(50)).len(), 10);
    assert_eq!(store.ids().len(), 10);
    store.delete(&[ExchangeId(9)]);
    assert_eq!(
        ids(store.summaries_of(&[ExchangeId(8), ExchangeId(9), ExchangeId(10)])),
        vec![8, 10],
        "deleted ones are left out"
    );
}
