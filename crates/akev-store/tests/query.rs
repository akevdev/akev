//! Filtering and autocompletion context.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_filter::Filter;
use akev_store::ExchangePatch;
use common::*;

fn seeded() -> std::sync::Arc<akev_store::Store> {
    let store = store();
    complete_exchange(
        &store,
        1,
        "https://api.example.com/users",
        "application/json",
        br#"{"user":"alice"}"#,
    );
    complete_exchange(
        &store,
        2,
        "https://cdn.other.org/logo.png",
        "image/png",
        &[0x89, b'P', b'N', b'G', 0, 1, 2],
    );
    complete_exchange(
        &store,
        3,
        "https://api.example.com/orders",
        "application/json",
        br#"{"order":42,"needle":true}"#,
    );
    store.ingest(started(
        4,
        "POST",
        "https://api.example.com/upload",
        &[("Content-Type", "text/plain")],
    ));
    store.ingest(req_done(4, b"request needle text", 1.0));
    store
}

fn ids(v: &[ExchangeSummary]) -> Vec<u64> {
    v.iter().map(|s| s.id.0).collect()
}

#[test]
fn query_by_fields_and_limit() {
    let store = seeded();
    let all = Filter::all();
    assert_eq!(ids(&store.query(&all, None)), vec![1, 2, 3, 4]);
    assert_eq!(
        ids(&store.query(&all, Some(2))),
        vec![3, 4],
        "limit keeps the last N"
    );
    assert!(store.query(&all, Some(0)).is_empty());
    assert_eq!(
        store.query_ids(&all),
        vec![ExchangeId(1), ExchangeId(2), ExchangeId(3), ExchangeId(4)]
    );

    let host = Filter::parse("host:example.com").unwrap();
    assert_eq!(ids(&store.query(&host, None)), vec![1, 3, 4]);
    assert_eq!(ids(&store.query(&host, Some(1))), vec![4]);

    let method = Filter::parse("method:post").unwrap();
    assert_eq!(store.query_ids(&method), vec![ExchangeId(4)]);
}

#[test]
fn body_filters_decode_lazily() {
    let store = seeded();
    // gzip-encoded body that only matches once decoded.
    store.ingest(started(5, "GET", "https://api.example.com/gz", &[]));
    store.ingest(req_done(5, b"", 1.0));
    store.ingest(resp_start(
        5,
        200,
        &[
            ("Content-Type", "application/json"),
            ("Content-Encoding", "gzip"),
        ],
        2.0,
    ));
    store.ingest(resp_done(5, &gzip(br#"{"hidden":"needle"}"#), 3.0));

    let body = Filter::parse("body:needle").unwrap();
    assert!(body.needs_body());
    assert_eq!(ids(&store.query(&body, None)), vec![3, 4, 5]);
    // Binary bodies never match text searches.
    let png = Filter::parse("body:PNG").unwrap();
    assert!(store.query(&png, None).is_empty());
}

#[test]
fn body_filters_scale_across_threads_in_order() {
    let store = store();
    for id in 1..=1500u64 {
        let body = if id % 7 == 0 {
            format!("item {id} needle")
        } else {
            format!("item {id}")
        };
        complete_exchange(
            &store,
            id,
            "http://bulk.example/",
            "text/plain",
            body.as_bytes(),
        );
    }
    let f = Filter::parse("body:needle").unwrap();
    let expected: Vec<ExchangeId> = (1..=1500u64)
        .filter(|i| i % 7 == 0)
        .map(ExchangeId)
        .collect();
    assert_eq!(store.query_ids(&f), expected);
    let last = store.query(&f, Some(3));
    assert_eq!(ids(&last), vec![1484, 1491, 1498]);
}

#[test]
fn single_exchange_matching() {
    let store = seeded();
    let f = Filter::parse("host:cdn.other.org").unwrap();
    assert_eq!(store.matches(ExchangeId(2), &f), Some(true));
    assert_eq!(store.matches(ExchangeId(1), &f), Some(false));
    assert_eq!(store.matches(ExchangeId(99), &f), None);
    let picked = store.filter_ids(
        &[ExchangeId(4), ExchangeId(2), ExchangeId(99), ExchangeId(1)],
        &Filter::all(),
    );
    assert_eq!(picked, vec![ExchangeId(4), ExchangeId(2), ExchangeId(1)]);
    let picked = store.filter_ids(&[ExchangeId(1), ExchangeId(2), ExchangeId(3)], &f);
    assert_eq!(picked, vec![ExchangeId(2)]);
}

#[test]
fn filter_sees_user_attributes() {
    let store = seeded();
    store.patch(
        &[ExchangeId(2)],
        &ExchangePatch {
            pinned: Some(true),
            note: Some(Some("check logo".into())),
            ..ExchangePatch::default()
        },
    );
    let pinned = Filter::parse("is:pinned").unwrap();
    assert_eq!(store.query_ids(&pinned), vec![ExchangeId(2)]);
    let note = Filter::parse("note:logo").unwrap();
    assert_eq!(store.query_ids(&note), vec![ExchangeId(2)]);
}

#[test]
fn suggest_context_ranks_by_frequency() {
    let store = seeded();
    store.ingest(ProxyEvent::TlsFailed {
        id: ExchangeId(10),
        client: client(),
        host: "pinned.example".into(),
        port: 443,
        error: ExchangeError {
            kind: ErrorKind::ClientTlsRejected,
            message: "x".into(),
            code: None,
        },
        at: T0,
    });
    let ctx = store.suggest_context();
    assert_eq!(
        ctx.hosts,
        vec!["api.example.com", "cdn.other.org", "pinned.example"]
    );
    assert_eq!(
        ctx.methods,
        vec!["GET", "POST"],
        "empty methods (TLS failure) skipped"
    );
    assert_eq!(ctx.sources, vec!["curl"]);
    assert_eq!(ctx.content_types, vec!["application/json", "image/png"]);
    assert!(ctx.header_names.contains(&"content-type".to_string()));
    assert!(ctx.header_names.contains(&"host".to_string()));
    assert!(
        ctx.header_names
            .iter()
            .all(|h| h.chars().all(|c| !c.is_ascii_uppercase()))
    );
    assert_eq!(
        ctx.header_names.first().map(String::as_str),
        Some("content-type")
    );

    // Cached until something changes, then recomputed.
    let again = store.suggest_context();
    assert_eq!(again.hosts, ctx.hosts);
    complete_exchange(
        &store,
        11,
        "https://new.example/",
        "text/html",
        b"<html></html>",
    );
    assert!(
        store
            .suggest_context()
            .hosts
            .contains(&"new.example".to_string())
    );
}

#[test]
fn suggest_context_caps_hosts() {
    let store = store();
    for i in 0..250u64 {
        store.ingest(started(i + 1, "GET", &format!("http://h{i}.example/"), &[]));
    }
    store.ingest(started(1000, "GET", "http://h7.example/again", &[]));
    let ctx = store.suggest_context();
    assert_eq!(ctx.hosts.len(), 200);
    assert_eq!(ctx.hosts[0], "h7.example", "most frequent first");
}
