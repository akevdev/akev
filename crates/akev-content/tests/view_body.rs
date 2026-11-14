//! `view_body` integration tests: wire bytes + headers → what the UI receives.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::{BodyView, ViewOptions, decode, view_body, view_body_with};
use akev_core::{ContentKind, Headers};
use serde_json::json;

fn headers(pairs: &[(&str, &str)]) -> Headers {
    let mut h = Headers::new();
    for (k, v) in pairs {
        h.append(*k, *v);
    }
    h
}

fn enc(body: &[u8], chain: &[&str]) -> Vec<u8> {
    let chain: Vec<String> = chain.iter().map(|s| s.to_string()).collect();
    decode::encode_body(body, &chain).unwrap().to_vec()
}

const MAX: usize = 8 << 20;

#[test]
fn gzipped_json() {
    let body = r#"{"user":{"id":12345678901234567890,"name":"Zo\u00eb"}}"#.as_bytes();
    let wire = enc(body, &["gzip"]);
    let v = view_body(
        &wire,
        &headers(&[
            ("Content-Type", "application/json; charset=utf-8"),
            ("Content-Encoding", "gzip"),
        ]),
        "/api/me",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Json);
    assert_eq!(v.size, body.len());
    assert_eq!(v.text.as_deref(), Some(std::str::from_utf8(body).unwrap()));
    assert_eq!(
        v.formatted.as_deref(),
        Some(
            "{\n  \"user\": {\n    \"id\": 12345678901234567890,\n    \"name\": \"Zo\\u00eb\"\n  }\n}"
        )
    );
    assert_eq!(v.structured, None);
    assert_eq!(v.decode_error, None);
    assert!(!v.truncated);
}

#[test]
fn chained_encodings_and_identity() {
    let body = b"<html><body><p>hi</p></body></html>";
    let wire = enc(body, &["deflate", "br"]);
    let v = view_body(
        &wire,
        &headers(&[
            ("content-type", "text/html"),
            ("content-encoding", "deflate, identity"),
            ("Content-Encoding", "br"),
        ]),
        "/",
        MAX,
    );
    assert_eq!(v.decode_error, None);
    assert_eq!(v.kind, ContentKind::Html);
    assert_eq!(
        v.formatted.as_deref(),
        Some("<html>\n<body>\n  <p>hi</p>\n</body>\n</html>")
    );
}

#[test]
fn decode_failure() {
    let wire = b"definitely not gzip";
    let v = view_body(
        wire,
        &headers(&[
            ("content-type", "application/json"),
            ("content-encoding", "gzip"),
        ]),
        "/",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Json);
    assert_eq!(v.size, wire.len());
    assert_eq!(v.text, None);
    assert_eq!(v.formatted, None);
    assert_eq!(v.structured, None);
    assert!(
        v.decode_error
            .as_deref()
            .unwrap()
            .starts_with("failed to decode gzip body")
    );
    let unknown = view_body(
        b"x",
        &headers(&[("content-encoding", "compress")]),
        "/a.png",
        MAX,
    );
    assert_eq!(
        unknown.decode_error.as_deref(),
        Some("unsupported encoding: compress")
    );
    assert_eq!(unknown.kind, ContentKind::Image); // from the path extension
}

#[test]
fn zip_bomb_is_rejected() {
    let zeros = vec![0u8; 4 << 20];
    let wire = enc(&zeros, &["gzip"]);
    let opts = ViewOptions {
        max_decoded: 1 << 20,
        ..ViewOptions::default()
    };
    let v = view_body_with(&wire, &headers(&[("content-encoding", "gzip")]), "/", &opts);
    assert!(v.decode_error.unwrap().contains("exceeds 1048576 bytes"));
}

#[test]
fn truncated_capture_decodes_leniently() {
    let body: String = (0..50_000)
        .map(|i| format!("line {i} {}\n", i * 7919 % 10007))
        .collect();
    let wire = enc(body.as_bytes(), &["gzip"]);
    let cut = &wire[..wire.len() / 2];
    let h = headers(&[("content-type", "text/plain"), ("content-encoding", "gzip")]);
    assert!(view_body(cut, &h, "/", MAX).decode_error.is_some());
    let opts = ViewOptions {
        capture_truncated: true,
        ..ViewOptions::default()
    };
    let v = view_body_with(cut, &h, "/", &opts);
    assert_eq!(v.decode_error, None);
    let text = v.text.unwrap();
    assert!(!text.is_empty() && body.starts_with(&text));
}

#[test]
fn text_truncation_on_char_boundary() {
    let body = "é".repeat(100);
    let v = view_body(
        body.as_bytes(),
        &headers(&[("content-type", "text/plain; charset=utf-8")]),
        "/",
        51,
    );
    assert!(v.truncated);
    assert_eq!(v.text.as_deref(), Some("é".repeat(25).as_str()));
    assert_eq!(v.size, 200);
}

#[test]
fn charset_decoding() {
    let v = view_body(
        b"Gr\xfc\xdfe",
        &headers(&[("content-type", "text/plain; charset=iso-8859-1")]),
        "/",
        MAX,
    );
    assert_eq!(v.text.as_deref(), Some("Grüße"));
}

#[test]
fn formatting_is_skipped_for_huge_text() {
    let big = format!("[{}1]", "1,".repeat(600_000));
    let opts = ViewOptions {
        max_format: 1 << 20,
        ..ViewOptions::default()
    };
    let v = view_body_with(
        big.as_bytes(),
        &headers(&[("content-type", "application/json")]),
        "/",
        &opts,
    );
    assert!(v.text.is_some());
    assert_eq!(v.formatted, None);
}

#[test]
fn protobuf_and_grpc() {
    let pb = [0x0a, 0x02, b'h', b'i', 0x10, 0x96, 0x01];
    let v = view_body(
        &pb,
        &headers(&[("content-type", "application/x-protobuf")]),
        "/",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Protobuf);
    assert_eq!(v.text, None);
    assert_eq!(v.structured, Some(json!({"1": "hi", "2": 150})));

    let mut grpc = vec![0, 0, 0, 0, pb.len() as u8];
    grpc.extend_from_slice(&pb);
    let compressed = enc(&pb, &["gzip"]);
    grpc.push(1);
    grpc.extend_from_slice(&(compressed.len() as u32).to_be_bytes());
    grpc.extend_from_slice(&compressed);
    let v = view_body(
        &grpc,
        &headers(&[
            ("content-type", "application/grpc"),
            ("grpc-encoding", "gzip"),
        ]),
        "/pkg.Svc/Call",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Grpc);
    assert_eq!(
        v.structured,
        Some(json!({"messages": [
            {"compressed": false, "decoded": {"1": "hi", "2": 150}, "trailers": null, "size": 7},
            {"compressed": true, "decoded": {"1": "hi", "2": 150}, "trailers": null, "size": 7}
        ]}))
    );

    // grpc-web-text with a trailer frame; and a truncated stream keeps the decoded frames.
    use base64::Engine as _;
    let mut web = vec![0, 0, 0, 0, pb.len() as u8];
    web.extend_from_slice(&pb);
    let trailer = b"grpc-status: 0\r\n";
    web.push(0x80);
    web.extend_from_slice(&(trailer.len() as u32).to_be_bytes());
    web.extend_from_slice(trailer);
    let text = base64::engine::general_purpose::STANDARD.encode(&web);
    let v = view_body(
        text.as_bytes(),
        &headers(&[("content-type", "application/grpc-web-text+proto")]),
        "/",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::GrpcWeb);
    let s = v.structured.unwrap();
    assert_eq!(s["messages"][1]["trailers"], json!([["grpc-status", "0"]]));
    let v = view_body(
        &web[..web.len() - 3],
        &headers(&[("content-type", "application/grpc-web+proto")]),
        "/",
        MAX,
    );
    let s = v.structured.unwrap();
    assert_eq!(s["messages"].as_array().unwrap().len(), 1);
    assert!(s["error"].as_str().unwrap().contains("corrupted"));
}

#[test]
fn msgpack_and_cbor() {
    let v = view_body(
        b"\x81\xa1a\x01",
        &headers(&[("content-type", "application/msgpack")]),
        "/",
        MAX,
    );
    assert_eq!(
        (v.kind, v.structured),
        (ContentKind::MsgPack, Some(json!({"a": 1})))
    );
    let v = view_body(
        &[0xa1, 0x61, 0x61, 0x01],
        &headers(&[("content-type", "application/cbor")]),
        "/",
        MAX,
    );
    assert_eq!(
        (v.kind, v.structured),
        (ContentKind::Cbor, Some(json!({"a": 1})))
    );
}

#[test]
fn multipart_parts() {
    let ct = "multipart/form-data; boundary=XX";
    let body = b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nvalue\r\n--XX\r\nContent-Disposition: form-data; name=\"f\"; filename=\"x.bin\"\r\nContent-Type: application/octet-stream\r\n\r\n\x00\x01\x02\r\n--XX--\r\n";
    let v = view_body(body, &headers(&[("content-type", ct)]), "/upload", MAX);
    assert_eq!(v.kind, ContentKind::Multipart);
    assert!(
        v.text.is_none(),
        "a binary part means no text rendering of the whole body"
    );
    let parts = v.structured.unwrap();
    assert_eq!(parts[0]["name"], "a");
    assert_eq!(parts[0]["text"], "value");
    assert_eq!(parts[1]["filename"], "x.bin");
    assert_eq!(parts[1]["size"], 3);
    assert_eq!(parts[1]["text"], json!(null));
    assert!(parts[1].get("data").is_none());
    // an all-text multipart body is shown as text too
    let text_only =
        b"--XX\r\nContent-Disposition: form-data; name=\"a\"\r\n\r\nvalue\r\n--XX--\r\n";
    assert!(
        view_body(text_only, &headers(&[("content-type", ct)]), "/", MAX)
            .text
            .is_some()
    );
}

#[test]
fn event_stream_form_image_svg() {
    let v = view_body(
        b"event: tick\ndata: 1\n\ndata: 2\n\n",
        &headers(&[("content-type", "text/event-stream")]),
        "/",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::EventStream);
    assert_eq!(
        v.structured,
        Some(
            json!([{"event": "tick", "id": null, "retry": null, "data": "1"}, {"event": null, "id": null, "retry": null, "data": "2"}])
        )
    );
    assert!(v.text.is_some());
    assert_eq!(
        v.formatted.as_deref(),
        Some(
            "[\n  {\n    \"event\": \"tick\",\n    \"data\": 1\n  },\n  {\n    \"data\": 2\n  }\n]"
        )
    );

    let v = view_body(
        b"a=1&b=two+words",
        &headers(&[("content-type", "application/x-www-form-urlencoded")]),
        "/",
        MAX,
    );
    assert_eq!(v.structured, Some(json!([["a", "1"], ["b", "two words"]])));
    assert_eq!(v.formatted.as_deref(), Some("a=1\nb=two words"));

    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
    png.extend_from_slice(&[0, 0, 0, 16, 0, 0, 0, 9]);
    let v = view_body(
        &png,
        &headers(&[("content-type", "image/png")]),
        "/a.png",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Image);
    assert_eq!(v.text, None);
    assert_eq!(
        v.structured,
        Some(json!({"format": "png", "width": 16, "height": 9}))
    );

    let svg = b"<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"20\"><g><rect/></g></svg>";
    let v = view_body(
        svg,
        &headers(&[("content-type", "image/svg+xml")]),
        "/i.svg",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::Image);
    assert_eq!(v.text.as_deref(), Some(std::str::from_utf8(svg).unwrap()));
    assert_eq!(
        v.formatted.as_deref(),
        Some(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"10\" height=\"20\">\n  <g>\n    <rect/>\n  </g>\n</svg>"
        )
    );
    assert_eq!(
        v.structured,
        Some(json!({"format": "svg", "width": 10, "height": 20}))
    );
}

#[test]
fn jpeg_with_exif_metadata() {
    // Big-endian TIFF: IFD0 = {Make: "Sony" (inline), Model: "ILCE-7M3"} and no sub-IFDs.
    let mut tiff = b"MM\0*\0\0\0\x08".to_vec();
    tiff.extend_from_slice(&[0, 2]);
    tiff.extend_from_slice(&[0x01, 0x0F, 0, 2, 0, 0, 0, 4]);
    tiff.extend_from_slice(b"Sony");
    tiff.extend_from_slice(&[0x01, 0x10, 0, 2, 0, 0, 0, 9]);
    tiff.extend_from_slice(&(8u32 + 2 + 24 + 4).to_be_bytes());
    tiff.extend_from_slice(&[0, 0, 0, 0]);
    tiff.extend_from_slice(b"ILCE-7M3\0");
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    jpeg.extend_from_slice(&app1);
    // SOF0: 120 x 80
    jpeg.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x50, 0x00, 0x78, 0x01]);
    let v = view_body(&jpeg, &headers(&[("content-type", "image/jpeg")]), "/", MAX);
    let s = v.structured.unwrap();
    assert_eq!(
        (&s["format"], &s["width"], &s["height"]),
        (&json!("jpeg"), &json!(120), &json!(80))
    );
    assert_eq!(s["exif"]["make"], "Sony");
    assert_eq!(s["exif"]["model"], "ILCE-7M3");
    assert_eq!(s["exif"]["summary"], "Sony ILCE-7M3");
    assert_eq!(s["exif"]["tagCount"], 2);
}

#[test]
fn graphql_requests() {
    let body = br#"{"operationName":"Me","query":"query Me{me{id name}}","variables":{"x":1}}"#;
    let v = view_body(
        body,
        &headers(&[("content-type", "application/json")]),
        "/graphql",
        MAX,
    );
    assert_eq!(v.kind, ContentKind::GraphQl);
    assert_eq!(
        v.structured,
        Some(
            json!([{"operationName": "Me", "query": "query Me {\n  me {\n    id\n    name\n  }\n}", "variables": {"x": 1}}])
        )
    );
    assert!(
        v.formatted
            .unwrap()
            .starts_with("{\n  \"operationName\": \"Me\"")
    );
    let raw = view_body(
        b"{me{id}}",
        &headers(&[("content-type", "application/graphql")]),
        "/",
        MAX,
    );
    assert_eq!(raw.formatted.as_deref(), Some("{\n  me {\n    id\n  }\n}"));
}

#[test]
fn empty_and_unknown_bodies() {
    let v = view_body(b"", &Headers::new(), "/", MAX);
    assert_eq!(
        (v.kind, v.size, v.text.as_deref()),
        (ContentKind::Unknown, 0, None)
    );
    let v = view_body(
        b"",
        &headers(&[
            ("content-type", "application/json"),
            ("content-encoding", "gzip"),
        ]),
        "/",
        MAX,
    );
    assert_eq!(
        (v.kind, v.text.as_deref(), v.decode_error.as_deref()),
        (ContentKind::Json, Some(""), None)
    );
    let v = view_body(
        &[0xde, 0xad, 0xbe, 0xef, 0, 1, 2],
        &Headers::new(),
        "/",
        MAX,
    );
    assert_eq!((v.kind, v.text.as_deref()), (ContentKind::Binary, None));
}

#[test]
fn serializes_for_the_api() {
    let v = view_body(
        b"{\"a\":1}",
        &headers(&[("content-type", "application/json")]),
        "/",
        MAX,
    );
    let j = serde_json::to_value(&v).unwrap();
    assert_eq!(
        j,
        json!({"kind": "json", "size": 7, "text": "{\"a\":1}", "formatted": "{\n  \"a\": 1\n}", "structured": null, "decodeError": null, "truncated": false})
    );
    let _: &BodyView = &v;
}
