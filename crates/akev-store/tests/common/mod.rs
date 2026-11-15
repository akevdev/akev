//! Shared builders for akev-store integration tests.
#![allow(dead_code)]

use akev_core::settings::CaptureSettings;
use akev_core::*;
use akev_store::{Store, StoreEvent};
use bytes::Bytes;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::broadcast;

/// A UTC instant used as the start of every test exchange (2024-01-02T03:04:05Z).
pub const T0: UnixMicros = UnixMicros(1_704_164_645_000_000);

pub fn store() -> Arc<Store> {
    Store::new(CaptureSettings::default(), None)
}

pub fn store_with(f: impl FnOnce(&mut CaptureSettings)) -> Arc<Store> {
    let mut s = CaptureSettings::default();
    f(&mut s);
    Store::new(s, None)
}

/// Imports and session loads take fresh ids from the process-wide counter, which starts at 1,
/// while test stores ingest hand-picked ids from 1 up. Move the counter past `store`'s ids so a
/// copy can't reuse them by chance when its test is the first to run.
pub fn skip_ids_of(store: &Store) {
    if let Some(max) = store.summaries().iter().map(|s| s.id.0).max() {
        ExchangeId::reserve(max);
    }
}

pub fn headers(pairs: &[(&str, &str)]) -> Headers {
    Headers(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    )
}

pub fn head(method: &str, url: &str, hdrs: &[(&str, &str)]) -> RequestHead {
    let (scheme, rest) = url.split_once("://").unwrap();
    let (authority, path) = match rest.find(['/', '?']) {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().unwrap()),
        None => (
            authority.to_string(),
            if scheme == "https" || scheme == "wss" {
                443
            } else {
                80
            },
        ),
    };
    RequestHead {
        method: method.into(),
        url: url.into(),
        scheme: scheme.into(),
        host,
        port,
        path: path.into(),
        http_version: HttpVersion::Http11,
        headers: headers(hdrs),
    }
}

pub fn client() -> ClientInfo {
    ClientInfo {
        remote_addr: "127.0.0.1:50000".into(),
        connection_id: ConnectionId(7),
        process: Some(ProcessInfo {
            pid: 42,
            name: "curl".into(),
            path: None,
        }),
        interceptor: Some("terminal".into()),
        via: ProxyMode::HttpProxy,
        tls: None,
    }
}

pub fn started(id: u64, method: &str, url: &str, hdrs: &[(&str, &str)]) -> ProxyEvent {
    ProxyEvent::RequestStarted {
        id: ExchangeId(id),
        kind: ExchangeKind::Http,
        head: head(method, url, hdrs),
        client: client(),
        timing: Timing {
            start: T0,
            request_headers: Some(0.5),
            ..Timing::default()
        },
    }
}

pub fn req_done(id: u64, body: &[u8], at: f64) -> ProxyEvent {
    ProxyEvent::RequestCompleted {
        id: ExchangeId(id),
        body: CapturedBody::complete(Bytes::copy_from_slice(body)),
        trailers: None,
        at,
    }
}

pub fn resp_head(status: u16, hdrs: &[(&str, &str)]) -> ResponseHead {
    ResponseHead {
        status,
        status_message: canonical_reason(status).into(),
        http_version: HttpVersion::Http11,
        headers: headers(hdrs),
    }
}

pub fn resp_start(id: u64, status: u16, hdrs: &[(&str, &str)], at: f64) -> ProxyEvent {
    ProxyEvent::ResponseStarted {
        id: ExchangeId(id),
        head: resp_head(status, hdrs),
        at,
    }
}

pub fn resp_done(id: u64, body: &[u8], at: f64) -> ProxyEvent {
    ProxyEvent::ResponseCompleted {
        id: ExchangeId(id),
        body: CapturedBody::complete(Bytes::copy_from_slice(body)),
        trailers: None,
        at,
    }
}

pub fn ws_message(
    id: u64,
    seq: u32,
    direction: Direction,
    kind: MessageKind,
    payload: &[u8],
) -> ProxyEvent {
    ProxyEvent::StreamMessage {
        id: ExchangeId(id),
        message: StreamMessage {
            seq,
            direction,
            kind,
            at: UnixMicros(T0.0 + 10_000 + seq as i64 * 1000),
            size: payload.len() as u64,
            event: None,
            event_id: None,
            modified: false,
        },
        payload: Bytes::copy_from_slice(payload),
    }
}

/// A complete GET → 200 exchange with the given response body/content type.
pub fn complete_exchange(store: &Store, id: u64, url: &str, content_type: &str, body: &[u8]) {
    store.ingest(started(
        id,
        "GET",
        url,
        &[("Host", "example.com"), ("Accept", "*/*")],
    ));
    store.ingest(req_done(id, b"", 1.0));
    store.ingest(resp_start(id, 200, &[("Content-Type", content_type)], 10.0));
    store.ingest(resp_done(id, body, 12.5));
}

pub fn gzip(data: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(data).unwrap();
    e.finish().unwrap()
}

/// Everything currently queued on a subscription.
pub fn drain(rx: &mut broadcast::Receiver<StoreEvent>) -> Vec<StoreEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

pub fn kinds(events: &[StoreEvent]) -> Vec<&'static str> {
    events
        .iter()
        .map(|e| match e {
            StoreEvent::Added(_) => "added",
            StoreEvent::Updated(_) => "updated",
            StoreEvent::Removed(_) => "removed",
            StoreEvent::Cleared => "cleared",
            StoreEvent::Message { .. } => "message",
        })
        .collect()
}

/// Pseudo-random, poorly compressible bytes.
pub fn noise(len: usize, seed: u32) -> Vec<u8> {
    let mut x = seed.wrapping_mul(2_654_435_761).wrapping_add(1);
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 8) as u8
        })
        .collect()
}
