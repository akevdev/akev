//! Regression: viewing a body whose items are tiny (a two-byte urlencoded pair, a one-byte
//! MessagePack `nil`, a seven-byte SSE event) must not cost a large multiple of the body in
//! memory — neither the structured decode nor the pretty-printed text may build one heap
//! object per item for the whole body.
//!
//! This file has a single test: the allocation counter below is process-wide.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::{ViewOptions, view_body_with};
use akev_core::Headers;
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

/// Peak heap growth (bytes) while viewing `body`, including serializing the view as the API
/// does.
fn view_peak(body: &[u8], content_type: &str, opts: &ViewOptions) -> usize {
    let mut headers = Headers::new();
    headers.append("content-type", content_type);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let view = view_body_with(body, &headers, "/", opts);
    let json = serde_json::to_vec(&view).unwrap();
    assert!(!json.is_empty());
    drop(view);
    PEAK.load(Ordering::Relaxed).saturating_sub(base)
}

#[test]
fn viewing_many_tiny_items_stays_proportional() {
    // 4 MiB bodies; a tree of one object per item would need hundreds of megabytes.
    const SIZE: usize = 4 << 20;
    let opts = ViewOptions {
        max_structured_nodes: 100_000,
        ..ViewOptions::default()
    };
    let form = "a&".repeat(SIZE / 2);
    let sse = "data:\n\n".repeat(SIZE / 7);
    let mut msgpack = vec![0xdd];
    msgpack.extend_from_slice(&((SIZE - 5) as u32).to_be_bytes());
    msgpack.extend(std::iter::repeat_n(0xc0, SIZE - 5));
    let mut cbor = vec![0x9a];
    cbor.extend_from_slice(&((SIZE - 5) as u32).to_be_bytes());
    cbor.extend(std::iter::repeat_n(0xf6, SIZE - 5));
    let protobuf = [0x08u8, 0x00].repeat(SIZE / 2);
    for (name, body, content_type) in [
        ("form", form.as_bytes(), "application/x-www-form-urlencoded"),
        ("sse", sse.as_bytes(), "text/event-stream"),
        ("msgpack", &msgpack[..], "application/msgpack"),
        ("cbor", &cbor[..], "application/cbor"),
        ("protobuf", &protobuf[..], "application/x-protobuf"),
    ] {
        let peak = view_peak(body, content_type, &opts);
        assert!(
            peak < 32 * SIZE,
            "{name}: viewing {SIZE} bytes peaked at {peak} bytes"
        );
    }
}
