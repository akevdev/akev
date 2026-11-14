//! Regression: HAR export (`Store::write_har`) and import stream one exchange at a time, so
//! their memory use doesn't grow with the size of the document (unlike building one JSON tree
//! of the whole file).
//!
//! This file has a single test: the allocation counter below is process-wide.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_store::Store;
use common::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Counts live heap bytes and their high-water mark.
struct Tracking;

static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);

unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: forwarded unchanged to the system allocator.
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        p
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged to the system allocator.
        unsafe { System.dealloc(ptr, layout) };
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
}

#[global_allocator]
static GLOBAL: Tracking = Tracking;

/// Peak heap growth (bytes) while running `f`.
fn peak_growth(f: impl FnOnce()) -> usize {
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    f();
    PEAK.load(Ordering::Relaxed).saturating_sub(base)
}

#[test]
fn har_export_and_import_stream() {
    write_har_memory_does_not_grow_with_the_export();
    har_import_memory_does_not_grow_with_the_file();
}

fn har_import_memory_does_not_grow_with_the_file() {
    // 100 000 junk items of small values (each would be ~1 KB of JSON tree), plus one entry.
    let mut har = String::from(r#"{"log":{"entries":["#);
    for _ in 0..100_000 {
        har.push_str(r#"{"x":[0,0,0,0,0,0,0,0,0,0]},"#);
    }
    har.push_str(r#"{"request":{"url":"https://a.example/"},"response":{"status":200}}]}}"#);
    let target = store();
    let mut ids = Vec::new();
    let peak = peak_growth(|| ids = target.import_har(har.as_bytes()).unwrap());
    assert_eq!(ids.len(), 1);
    // A tree of the whole document would need tens of megabytes.
    assert!(
        peak < 4 << 20,
        "import peaked at {peak} bytes for {} bytes",
        har.len()
    );
    let tree = peak_growth(|| {
        let v: serde_json::Value = serde_json::from_str(&har).unwrap();
        assert!(v.is_object());
    });
    assert!(tree > 10 * peak, "tree {tree} vs streamed import {peak}");
}

fn write_har_memory_does_not_grow_with_the_export() {
    const BODY: usize = 512 << 10;
    const EXCHANGES: u64 = 16;
    let store: std::sync::Arc<Store> = store();
    for id in 1..=EXCHANGES {
        // Binary bodies: exported as base64 (4/3 of the body per exchange).
        complete_exchange(
            &store,
            id,
            "https://big.example/blob",
            "application/octet-stream",
            &noise(BODY, id as u32),
        );
    }
    let mut written = 0usize;
    let streaming = peak_growth(|| {
        let mut sink = std::io::sink();
        written = store.write_har(&mut sink, None).unwrap();
    });
    assert_eq!(written, EXCHANGES as usize);
    let whole_tree = peak_growth(|| {
        let har = store.export_har(None);
        assert_eq!(
            har["log"]["entries"].as_array().unwrap().len(),
            EXCHANGES as usize
        );
    });
    let total_base64 = EXCHANGES as usize * BODY * 4 / 3;
    // Building the tree holds every body; streaming holds about one exchange.
    assert!(whole_tree > total_base64, "tree: {whole_tree} bytes");
    assert!(
        streaming < 8 * BODY,
        "streaming export peaked at {streaming} bytes (whole tree: {whole_tree})"
    );
}
