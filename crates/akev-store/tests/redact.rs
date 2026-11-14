//! Redacted HAR and session exports (`ExportOptions::redact`).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use akev_core::*;
use akev_store::{ExportOptions, Store};
use common::*;
use serde_json::Value;

const JWT: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";

/// An OAuth-ish exchange full of secrets: credential headers, a secret query parameter, a
/// form body with a password and a JSON response with tokens.
fn secret_exchange(store: &Store) -> ExchangeId {
    store.ingest(started(
        1,
        "POST",
        "https://api.example.com/oauth/token?client_id=app&api_key=AIzaSECRET",
        &[
            ("Host", "api.example.com"),
            ("Authorization", "Basic dXNlcjpodW50ZXIy"),
            ("Cookie", "sid=s3cr3t; theme=dark"),
            ("Content-Type", "application/x-www-form-urlencoded"),
            ("X-Request-Id", "abc-123"),
        ],
    ));
    store.ingest(req_done(
        1,
        b"grant_type=password&username=me&password=hunter2",
        1.0,
    ));
    store.ingest(resp_start(
        1,
        200,
        &[
            ("Content-Type", "application/json"),
            ("Set-Cookie", "sid=n3w; Path=/; HttpOnly"),
        ],
        5.0,
    ));
    let body = format!(
        r#"{{"access_token":"{JWT}","token_type":"bearer","refresh_token":"r3fr3sh","expires_in":3600}}"#
    );
    store.ingest(resp_done(1, body.as_bytes(), 6.0));
    ExchangeId(1)
}

fn har(store: &Store, options: ExportOptions) -> (String, Value) {
    let mut out = Vec::new();
    store.write_har_with(&mut out, None, options).unwrap();
    let text = String::from_utf8(out).unwrap();
    let value = serde_json::from_str(&text).unwrap();
    (text, value)
}

fn header<'a>(list: &'a Value, name: &str) -> &'a str {
    list.as_array()
        .unwrap()
        .iter()
        .find(|h| h["name"].as_str().unwrap().eq_ignore_ascii_case(name))
        .and_then(|h| h["value"].as_str())
        .unwrap_or_else(|| panic!("no {name} header in {list}"))
}

#[test]
fn har_export_redacts_secrets_only_when_asked() {
    let store = store();
    secret_exchange(&store);

    let (plain, _) = har(&store, ExportOptions::default());
    for secret in [
        "hunter2",
        "AIzaSECRET",
        "s3cr3t",
        "dXNlcjpodW50ZXIy",
        JWT,
        "r3fr3sh",
    ] {
        assert!(plain.contains(secret), "unredacted export keeps {secret}");
    }

    let (text, har) = har(&store, ExportOptions::redacted());
    for secret in [
        "hunter2",
        "AIzaSECRET",
        "s3cr3t",
        "n3w",
        "dXNlcjpodW50ZXIy",
        JWT,
        "r3fr3sh",
    ] {
        assert!(!text.contains(secret), "redacted export leaks {secret}");
    }
    let entry = &har["log"]["entries"][0];
    let request = &entry["request"];
    assert_eq!(
        request["url"],
        "https://api.example.com/oauth/token?client_id=app&api_key=[redacted]"
    );
    assert_eq!(
        header(&request["headers"], "authorization"),
        "Basic [redacted]"
    );
    assert_eq!(
        header(&request["headers"], "cookie"),
        "sid=[redacted]; theme=[redacted]"
    );
    assert_eq!(header(&request["headers"], "x-request-id"), "abc-123");
    assert_eq!(
        request["postData"]["text"],
        "grant_type=password&username=me&password=[redacted]"
    );
    let response = &entry["response"];
    assert_eq!(
        header(&response["headers"], "set-cookie"),
        "sid=[redacted]; Path=/; HttpOnly"
    );
    let content: Value =
        serde_json::from_str(response["content"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(content["access_token"], "[redacted]");
    assert_eq!(content["refresh_token"], "[redacted]");
    assert_eq!(content["token_type"], "bearer");
    assert_eq!(content["expires_in"], 3600);
    // The store itself is untouched.
    let detail = store.detail(ExchangeId(1)).unwrap();
    assert_eq!(
        detail.request.headers.get("authorization"),
        Some("Basic dXNlcjpodW50ZXIy")
    );
}

#[test]
fn compressed_bodies_are_left_intact_but_headers_are_redacted() {
    let store = store();
    let body = gzip(format!(r#"{{"token":"{JWT}"}}"#).as_bytes());
    store.ingest(started(
        2,
        "GET",
        "https://example.com/me",
        &[("Authorization", "Bearer abcdefgh12345")],
    ));
    store.ingest(req_done(2, b"", 1.0));
    store.ingest(resp_start(
        2,
        200,
        &[
            ("Content-Type", "application/json"),
            ("Content-Encoding", "gzip"),
        ],
        2.0,
    ));
    store.ingest(resp_done(2, &body, 3.0));
    let (text, har) = har(&store, ExportOptions::redacted());
    assert!(!text.contains("abcdefgh12345"));
    let entry = &har["log"]["entries"][0];
    assert_eq!(
        header(&entry["request"]["headers"], "authorization"),
        "Bearer [redacted]"
    );
    // The gzip body still decodes (best effort: compressed bodies aren't rewritten).
    assert!(
        entry["response"]["content"]["text"]
            .as_str()
            .unwrap()
            .contains("token")
    );
}

#[test]
fn session_export_redacts_when_asked_and_round_trips() {
    let source = store();
    secret_exchange(&source);
    source.ingest(ws_message(
        1,
        0,
        Direction::Sent,
        MessageKind::Text,
        format!(r#"{{"type":"auth","token":"{JWT}"}}"#).as_bytes(),
    ));

    let mut plain = Vec::new();
    source.save_session(&mut plain, None).unwrap();
    let mut redacted = Vec::new();
    source
        .save_session_with(&mut redacted, None, None, ExportOptions::redacted())
        .unwrap();
    assert_ne!(plain, redacted);

    let target = store();
    let ids = target
        .load_session(&mut std::io::Cursor::new(redacted))
        .unwrap();
    let detail = target.detail(ids[0]).unwrap();
    assert_eq!(
        detail.request.headers.get("authorization"),
        Some("Basic [redacted]")
    );
    assert_eq!(
        detail.request.url,
        "https://api.example.com/oauth/token?client_id=app&api_key=[redacted]"
    );
    let body = target.body(ids[0], Side::Request).unwrap();
    assert_eq!(
        &body.bytes[..],
        b"grant_type=password&username=me&password=[redacted]"
    );
    let messages = target.messages(ids[0]);
    assert_eq!(
        std::str::from_utf8(&messages[0].payload).unwrap(),
        r#"{"type":"auth","token":"[redacted]"}"#
    );
}
