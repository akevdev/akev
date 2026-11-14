//! Regression tests: a structured decode is bounded by its *output*, not only by the input size.
//!
//! * gRPC frames share one decompression budget (`max_structured`) instead of 64 MiB each.
//! * Every structured decoder stops at `max_structured_nodes` values / `max_structured_text`
//!   string bytes, keeps what it decoded and sets `structured_truncated`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::{
    DEFAULT_MAX_STRUCTURED_NODES, ViewOptions, decode, grpc, view_body, view_body_with,
};
use akev_core::Headers;
use serde_json::Value;

fn headers(pairs: &[(&str, &str)]) -> Headers {
    let mut h = Headers::new();
    for (k, v) in pairs {
        h.append(*k, *v);
    }
    h
}

fn frame(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![flags];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

fn small_budget(nodes: usize) -> ViewOptions {
    ViewOptions {
        max_structured_nodes: nodes,
        ..ViewOptions::default()
    }
}

/// Number of JSON values in a tree (object keys not counted).
fn count_nodes(v: &Value) -> usize {
    1 + match v {
        Value::Array(a) => a.iter().map(count_nodes).sum(),
        Value::Object(o) => o.values().map(count_nodes).sum(),
        _ => 0,
    }
}

#[test]
fn grpc_frames_share_one_decompression_budget() {
    // Ten zstd frames of 4 MiB of zeros each (a few hundred bytes on the wire) under a 16 MiB
    // budget: at most four messages are decoded, then the rest is refused.
    let zeros = vec![0u8; 4 << 20];
    let compressed = decode::encode_body(&zeros, &["zstd".to_string()]).unwrap();
    assert!(compressed.len() < 4096);
    let mut body = Vec::new();
    for _ in 0..10 {
        body.extend(frame(1, &compressed));
    }
    let opts = ViewOptions {
        max_structured: 16 << 20,
        ..ViewOptions::default()
    };
    let h = headers(&[
        ("content-type", "application/grpc"),
        ("grpc-encoding", "zstd"),
    ]);
    let v = view_body_with(&body, &h, "/svc/Method", &opts);
    let s = v.structured.expect("structured gRPC view");
    let messages = s["messages"].as_array().unwrap();
    let total: u64 = messages.iter().map(|m| m["size"].as_u64().unwrap()).sum();
    assert_eq!(messages.len(), 4);
    assert!(total <= 16 << 20, "decoded {total} bytes");
    assert!(
        s["error"]
            .as_str()
            .unwrap()
            .contains("decoded messages exceed 16777216 bytes in total"),
        "{s}"
    );
    assert!(v.structured_truncated);

    // The public parser applies a total cap too (default 256 MiB, or an explicit one).
    let (msgs, err) = grpc::parse_lenient_limited(&body, Some("zstd"), false, 9 << 20);
    assert_eq!(msgs.len(), 2);
    assert!(err.unwrap().to_string().contains("in total"));
    let (msgs, err) = grpc::parse_lenient(&body, Some("zstd"), false);
    assert_eq!((msgs.len(), err), (10, None));
}

#[test]
fn grpc_uncompressed_frames_count_against_the_budget_too() {
    let mut body = Vec::new();
    for _ in 0..4 {
        body.extend(frame(0, &vec![0x08; 1000])); // not valid protobuf: no decode
    }
    let (msgs, err) = grpc::parse_lenient_limited(&body, None, false, 2500);
    assert_eq!(msgs.len(), 2);
    assert!(err.is_some());
}

#[test]
fn grpc_message_count_is_bounded_by_the_output_budget() {
    // 100 000 empty frames: each would be a message object.
    let body = frame(0, &[]).repeat(100_000);
    let v = view_body_with(
        &body,
        &headers(&[("content-type", "application/grpc")]),
        "/",
        &small_budget(1000),
    );
    let s = v.structured.unwrap();
    let n = s["messages"].as_array().unwrap().len();
    assert!(n > 0 && n <= 200, "{n} messages");
    assert!(v.structured_truncated);
    assert!(s["error"].as_str().unwrap().contains("output limit"));
}

#[test]
fn grpc_json_payload_is_size_checked_before_parsing() {
    let json = format!("[{}0]", "0,".repeat(50_000));
    let body = frame(0, json.as_bytes());
    let h = headers(&[("content-type", "application/grpc+json")]);
    let v = view_body_with(&body, &h, "/", &small_budget(10_000));
    assert_eq!(v.structured.unwrap()["messages"][0]["decoded"], Value::Null);
    assert!(v.structured_truncated);
    let v = view_body(&body, &h, "/", 8 << 20);
    assert_eq!(
        v.structured.unwrap()["messages"][0]["decoded"]
            .as_array()
            .unwrap()
            .len(),
        50_001
    );
    assert!(!v.structured_truncated);
}

#[test]
fn urlencoded_form_is_bounded_with_default_options() {
    // 700 000 pairs from 1.4 MB of `a&` (a few KB gzipped): each pair is three JSON values.
    let body = "a&".repeat(700_000);
    let wire = decode::encode_body(body.as_bytes(), &["gzip".to_string()]).unwrap();
    let v = view_body(
        &wire,
        &headers(&[
            ("content-type", "application/x-www-form-urlencoded"),
            ("content-encoding", "gzip"),
        ]),
        "/",
        8 << 20,
    );
    let pairs = v.structured.unwrap();
    let n = pairs.as_array().unwrap().len();
    assert!(n > 0 && n <= DEFAULT_MAX_STRUCTURED_NODES / 3, "{n} pairs");
    assert!(v.structured_truncated);
    // A normal form is untouched.
    let v = view_body(
        b"a=1&b=2",
        &headers(&[("content-type", "application/x-www-form-urlencoded")]),
        "/",
        8 << 20,
    );
    assert_eq!(
        v.structured,
        Some(serde_json::json!([["a", "1"], ["b", "2"]]))
    );
    assert!(!v.structured_truncated);
}

#[test]
fn msgpack_and_cbor_arrays_of_nulls_are_bounded() {
    // array32 header + one million 1-byte nils / nulls.
    let mut msgpack = vec![0xdd];
    msgpack.extend_from_slice(&1_000_000u32.to_be_bytes());
    msgpack.extend(std::iter::repeat_n(0xc0, 1_000_000));
    let mut cbor = vec![0x9a];
    cbor.extend_from_slice(&1_000_000u32.to_be_bytes());
    cbor.extend(std::iter::repeat_n(0xf6, 1_000_000));
    for (ct, body) in [
        ("application/msgpack", &msgpack),
        ("application/cbor", &cbor),
    ] {
        let v = view_body(body, &headers(&[("content-type", ct)]), "/", 8 << 20);
        let items = v.structured.expect(ct);
        let n = items.as_array().unwrap().len();
        assert!(
            n > 0 && n < DEFAULT_MAX_STRUCTURED_NODES,
            "{ct}: {n} values"
        );
        assert!(v.structured_truncated, "{ct}");
    }
}

#[test]
fn nested_partial_trees_stay_within_the_budget() {
    // msgpack: [{"k": [1, 1, 1, ...]}] with 10 000 ones.
    let mut msgpack = vec![0x91, 0x81, 0xa1, b'k', 0xdc];
    msgpack.extend_from_slice(&10_000u16.to_be_bytes());
    msgpack.extend(std::iter::repeat_n(0x01, 10_000));
    let v = view_body_with(
        &msgpack,
        &headers(&[("content-type", "application/msgpack")]),
        "/",
        &small_budget(100),
    );
    let tree = v.structured.unwrap();
    assert!(count_nodes(&tree) <= 100);
    assert!(tree[0]["k"].as_array().unwrap().len() > 50);
    assert!(v.structured_truncated);
}

#[test]
fn protobuf_fields_are_bounded() {
    // `08 00` = field 1, varint 0; repeated 600 000 times.
    let body = [0x08u8, 0x00].repeat(600_000);
    let v = view_body(
        &body,
        &headers(&[("content-type", "application/x-protobuf")]),
        "/",
        8 << 20,
    );
    let fields = v.structured.unwrap();
    let n = fields["1"].as_array().unwrap().len();
    assert!(n > 0 && n <= DEFAULT_MAX_STRUCTURED_NODES, "{n} fields");
    assert!(v.structured_truncated);
}

#[test]
fn sse_events_and_multipart_parts_are_bounded() {
    let sse = "data:\n\n".repeat(50_000);
    let v = view_body_with(
        sse.as_bytes(),
        &headers(&[("content-type", "text/event-stream")]),
        "/",
        &small_budget(1000),
    );
    let n = v.structured.unwrap().as_array().unwrap().len();
    assert!(n > 0 && n <= 200, "{n} events");
    assert!(v.structured_truncated);

    let mut multipart = "--b\r\nX: y\r\n\r\nv\r\n".repeat(20_000);
    multipart.push_str("--b--");
    let v = view_body_with(
        multipart.as_bytes(),
        &headers(&[("content-type", "multipart/form-data; boundary=b")]),
        "/",
        &small_budget(1000),
    );
    let n = v.structured.unwrap().as_array().unwrap().len();
    assert!(n > 0 && n <= 100, "{n} parts");
    assert!(v.structured_truncated);

    // One part with a huge header block stops inside it.
    let mut many_headers = String::from("--b\r\n");
    many_headers.push_str(&"a: b\r\n".repeat(50_000));
    many_headers.push_str("\r\nv\r\n--b--");
    let v = view_body_with(
        many_headers.as_bytes(),
        &headers(&[("content-type", "multipart/form-data; boundary=b")]),
        "/",
        &small_budget(1000),
    );
    assert!(v.structured_truncated);
    assert!(count_nodes(&v.structured.unwrap()) <= 1000);
}

#[test]
fn graphql_documents_are_size_checked() {
    let body = format!(
        r#"{{"query":"{{ a }}","variables":{{"list":[{}0]}}}}"#,
        "0,".repeat(20_000)
    );
    let h = headers(&[("content-type", "application/json")]);
    let v = view_body_with(body.as_bytes(), &h, "/graphql", &small_budget(1000));
    assert_eq!(v.structured, None);
    assert!(v.structured_truncated);
    let v = view_body(body.as_bytes(), &h, "/graphql", 8 << 20);
    assert!(v.structured.is_some());
    assert!(!v.structured_truncated);
}

#[test]
fn string_bytes_are_bounded() {
    // A msgpack array of 100 strings of 1000 bytes under a 10 000-byte text budget.
    let mut body = vec![0xdc_u8];
    body.extend_from_slice(&100u16.to_be_bytes());
    for _ in 0..100 {
        body.push(0xda);
        body.extend_from_slice(&1000u16.to_be_bytes());
        body.extend(std::iter::repeat_n(b'x', 1000));
    }
    let opts = ViewOptions {
        max_structured_text: 10_000,
        ..ViewOptions::default()
    };
    let v = view_body_with(
        &body,
        &headers(&[("content-type", "application/msgpack")]),
        "/",
        &opts,
    );
    assert_eq!(v.structured.unwrap().as_array().unwrap().len(), 10);
    assert!(v.structured_truncated);
    // The flag is only serialized when set.
    let plain = view_body(
        b"{}",
        &headers(&[("content-type", "application/json")]),
        "/",
        99,
    );
    assert!(
        serde_json::to_value(&plain)
            .unwrap()
            .get("structuredTruncated")
            .is_none()
    );
}
