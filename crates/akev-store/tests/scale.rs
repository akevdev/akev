//! Scale measurements for the store: ingest rate, memory and query latency.
//!
//! * `bounded_memory` runs with every `cargo test` (CI-sized: 20k exchanges, debug build)
//!   and asserts that memory stays under a bound.
//! * `scale_100k`, `scale_1m_capped` and `scale_1m_uncapped` are the benchmarks behind the
//!   store numbers in `docs/PERFORMANCE.md`. Run them one at a time, in release mode:
//!   `cargo test -p akev-store --release --test scale -- --ignored --nocapture --test-threads=1`
//!
//! Each test is alone in this binary's process when run with `--test-threads=1`, so the
//! resident set size (RSS, read from `/proc/self/status` on Linux) is attributable to it.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_core::settings::CaptureSettings;
use akev_core::*;
use akev_filter::Filter;
use akev_store::Store;
use bytes::Bytes;
use std::sync::Arc;
use std::time::{Duration, Instant};

const T0: UnixMicros = UnixMicros(1_704_164_645_000_000);

/// Resident set size and its peak in bytes (`None` where `/proc` isn't available).
fn rss() -> Option<(u64, u64)> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| -> Option<u64> {
        let line = status.lines().find(|l| l.starts_with(name))?;
        let kb: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb * 1024)
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

fn headers(pairs: &[(&str, String)]) -> Headers {
    Headers(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// The four events of one realistic API exchange: browser-like request headers (cookie,
/// user agent), a JSON response of about 1 KB with typical response headers.
fn exchange(id: u64, body: &Bytes) -> [ProxyEvent; 4] {
    let host = format!("api{}.example.com", id % 50);
    let path = format!("/v1/items/{id}?page=2&sort=desc");
    let request = RequestHead {
        method: if id.is_multiple_of(5) { "POST" } else { "GET" }.into(),
        url: format!("https://{host}{path}"),
        scheme: "https".into(),
        host: host.clone(),
        port: 443,
        path,
        http_version: HttpVersion::H2,
        headers: headers(&[
            ("host", host),
            (
                "user-agent",
                "Mozilla/5.0 (X11; Linux x86_64; rv:125.0) Gecko/20100101 Firefox/125.0".into(),
            ),
            ("accept", "application/json, text/plain, */*".into()),
            ("accept-language", "en-GB,en;q=0.9".into()),
            ("accept-encoding", "gzip, deflate, br, zstd".into()),
            (
                "cookie",
                format!("session={id:016x}; theme=dark; consent=1; _ga=GA1.2.3.4"),
            ),
            ("x-request-id", format!("{id:032x}")),
        ]),
    };
    let client = ClientInfo {
        remote_addr: format!("127.0.0.1:{}", 40_000 + id / 20 % 20_000),
        connection_id: ConnectionId(id / 20),
        process: Some(ProcessInfo {
            pid: 4242,
            name: "firefox".into(),
            path: None,
        }),
        interceptor: Some("firefox".into()),
        via: ProxyMode::HttpProxy,
        tls: None,
    };
    let eid = ExchangeId(id);
    [
        ProxyEvent::RequestStarted {
            id: eid,
            kind: ExchangeKind::Http,
            head: request,
            client,
            timing: Timing {
                start: UnixMicros(T0.0 + id as i64 * 1000),
                ..Timing::default()
            },
        },
        ProxyEvent::RequestCompleted {
            id: eid,
            body: CapturedBody::empty(),
            trailers: None,
            at: 0.4,
        },
        ProxyEvent::ResponseStarted {
            id: eid,
            head: ResponseHead {
                status: if id.is_multiple_of(97) { 404 } else { 200 },
                status_message: String::new(),
                http_version: HttpVersion::H2,
                headers: headers(&[
                    ("content-type", "application/json; charset=utf-8".into()),
                    ("cache-control", "no-store, max-age=0".into()),
                    ("date", "Wed, 11 Nov 2026 10:00:00 GMT".into()),
                    ("server", "nginx".into()),
                    ("x-trace-id", format!("{:032x}", id * 7)),
                    ("strict-transport-security", "max-age=31536000".into()),
                ]),
            },
            at: 12.0,
        },
        ProxyEvent::ResponseCompleted {
            id: eid,
            body: CapturedBody::complete(body.clone()),
            trailers: None,
            at: 14.0,
        },
    ]
}

fn json_body() -> Bytes {
    Bytes::from(
        br#"{"id":123,"name":"widget","tags":["a","b","c"],"price":9.99,"stock":17}"#.repeat(14),
    )
}

/// What one run measured.
#[derive(Debug)]
struct Report {
    exchanges: u64,
    kept: u64,
    ingest: Duration,
    rss_before: Option<u64>,
    rss_after: Option<u64>,
    peak: Option<u64>,
}

impl Report {
    fn rss_growth(&self) -> Option<u64> {
        Some(self.rss_after?.saturating_sub(self.rss_before?))
    }

    fn print(&self, name: &str) {
        println!(
            "{name}: ingested {} exchanges in {:.2?} ({:.0} exchanges/s, {:.0} events/s); kept {}",
            self.exchanges,
            self.ingest,
            self.exchanges as f64 / self.ingest.as_secs_f64(),
            self.exchanges as f64 * 4.0 / self.ingest.as_secs_f64(),
            self.kept,
        );
        if let (Some(before), Some(after), Some(peak)) =
            (self.rss_before, self.rss_after, self.peak)
        {
            println!(
                "{name}: RSS {:.0} MiB → {:.0} MiB (growth {:.0} MiB, {:.0} bytes per kept exchange), peak {:.0} MiB",
                mib(before),
                mib(after),
                mib(after.saturating_sub(before)),
                after.saturating_sub(before) as f64 / self.kept.max(1) as f64,
                mib(peak),
            );
        }
    }
}

fn ingest(store: &Arc<Store>, n: u64) -> Report {
    let body = json_body();
    let rss_before = rss().map(|r| r.0);
    let t = Instant::now();
    for id in 1..=n {
        for ev in exchange(id, &body) {
            store.ingest(ev);
        }
    }
    let ingest = t.elapsed();
    let now = rss();
    Report {
        exchanges: n,
        kept: store.stats().exchanges,
        ingest,
        rss_before,
        rss_after: now.map(|r| r.0),
        peak: now.map(|r| r.1),
    }
}

fn time<T>(label: &str, f: impl FnOnce() -> T) -> (T, Duration) {
    let t = Instant::now();
    let out = f();
    let took = t.elapsed();
    println!("  {label}: {took:.2?}");
    (out, took)
}

/// Query latencies on a populated store.
fn queries(store: &Arc<Store>) {
    let (all, _) = time("summaries() (full snapshot for a new UI)", || {
        store.summaries()
    });
    println!("    {} rows", all.len());
    let f = Filter::parse("host:api7.example.com").unwrap();
    let (hits, _) = time("query_ids(host:…)", || store.query_ids(&f));
    println!("    {} hits", hits.len());
    let f = Filter::parse("status:404 method:GET").unwrap();
    let (hits, _) = time("query_ids(status:404 method:GET)", || store.query_ids(&f));
    println!("    {} hits", hits.len());
    let f = Filter::parse("resbody:widget").unwrap();
    let (hits, _) = time("query(resbody:…, newest 500)", || {
        store.query(&f, Some(500))
    });
    println!("    {} hits", hits.len());
    let f = Filter::parse("resbody:\"no such text\"").unwrap();
    let (hits, _) = time("query_ids(resbody:…) full scan, no hits", || {
        store.query_ids(&f)
    });
    println!("    {} hits", hits.len());
    time("suggest_context()", || store.suggest_context());
    let ids: Vec<ExchangeId> = all.iter().rev().take(1000).map(|s| s.id).collect();
    time("write_har(1000 newest)", || {
        let mut out = std::io::sink();
        store.write_har(&mut out, Some(&ids)).unwrap()
    });
}

fn bounded(max_exchanges: u64, dir: &std::path::Path) -> Arc<Store> {
    with_budget(
        max_exchanges,
        CaptureSettings::default().memory_budget_bytes,
        dir,
    )
}

fn with_budget(max_exchanges: u64, memory_budget_bytes: u64, dir: &std::path::Path) -> Arc<Store> {
    Store::new(
        CaptureSettings {
            max_exchanges,
            memory_budget_bytes,
            ..CaptureSettings::default()
        },
        Some(dir),
    )
}

fn print_bodies(store: &Store) {
    let stats = store.stats();
    println!(
        "  evicted {}, bodies in memory {:.0} MiB, spilled {:.0} MiB (on disk {:.0} MiB)",
        stats.evicted,
        mib(stats.memory_body_bytes),
        mib(stats.spilled_body_bytes),
        mib(stats.spill_disk_bytes)
    );
}

/// CI-sized: 20k exchanges into a 15k cap. The store's own accounting must stay within the
/// memory budget and RSS growth under 10 KB per ingested exchange — about twice what is
/// measured (about 4 KB per kept exchange, 1 KB body included, in debug and release builds
/// alike; see `docs/PERFORMANCE.md`), so a leak or unbounded buffering fails it.
#[test]
fn bounded_memory() {
    const N: u64 = 20_000;
    let dir = tempfile::tempdir().unwrap();
    // A cap below N exercises eviction as well.
    let store = bounded(15_000, dir.path());
    let report = ingest(&store, N);
    report.print("bounded_memory");
    let stats = store.stats();
    assert_eq!(stats.exchanges, 15_000);
    assert_eq!(stats.evicted, N - 15_000);
    assert!(stats.memory_body_bytes <= CaptureSettings::default().memory_budget_bytes);
    if let Some(growth) = report.rss_growth() {
        assert!(
            growth < N * 10 * 1024,
            "RSS grew {:.0} MiB for {N} exchanges",
            mib(growth)
        );
    }
    // Queries stay correct on the evicted store.
    let f = Filter::parse("resbody:widget").unwrap();
    assert_eq!(store.query(&f, Some(10)).len(), 10);
}

#[test]
#[ignore = "benchmark; run in release mode (see the module docs)"]
fn scale_100k() {
    let dir = tempfile::tempdir().unwrap();
    // A 32 MiB body budget, so most bodies are spilled (compressed) to disk and body
    // filters read them back.
    let store = with_budget(0, 32 << 20, dir.path());
    let _rx = store.subscribe();
    let report = ingest(&store, 100_000);
    report.print("scale_100k");
    print_bodies(&store);
    queries(&store);
}

/// One million exchanges into the default 200k cap: memory stays where 200k puts it.
#[test]
#[ignore = "benchmark; run in release mode (see the module docs)"]
fn scale_1m_capped() {
    let dir = tempfile::tempdir().unwrap();
    let store = bounded(CaptureSettings::default().max_exchanges, dir.path());
    let _rx = store.subscribe();
    let report = ingest(&store, 1_000_000);
    report.print("scale_1m_capped");
    print_bodies(&store);
    queries(&store);
}

/// One million exchanges with no cap (what the cap protects against).
#[test]
#[ignore = "benchmark; run in release mode (see the module docs)"]
fn scale_1m_uncapped() {
    let dir = tempfile::tempdir().unwrap();
    let store = bounded(0, dir.path());
    let _rx = store.subscribe();
    let report = ingest(&store, 1_000_000);
    report.print("scale_1m_uncapped");
    print_bodies(&store);
    queries(&store);
}
