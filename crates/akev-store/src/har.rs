//! HAR 1.2 export and tolerant import.
//!
//! # Export
//! HTTP, WebSocket and SSE exchanges become `log.entries` (Chrome-compatible
//! `_webSocketMessages` for WebSockets, HTTP Toolkit-compatible `_pinned`, `_trailers`,
//! `_webSocketClose`). Other row kinds use extension arrays so plain HAR consumers never see
//! half-valid entries: TLS failures go to `log._tlsErrors` and tunnels to `log._rawTunnels`
//! (both in HTTP Toolkit's format), client errors to `log._clientErrors`. Every item carries
//! an `_akev` object (id, kind, state, pinned, colour, note, error, source, rules, client &
//! TLS info, exact timings, ...) so Akev → HAR → Akev keeps everything but the raw
//! (still-compressed) body bytes. Bodies are exported decoded (`content.text`), as UTF-8
//! text when valid (and not mostly control characters, which JSON escapes six-fold),
//! otherwise base64 with `encoding: "base64"`. When decoding fails — or the body decodes to
//! more than [`EXPORT_MAX_DECODE`] bytes, or the export has already decoded
//! [`EXPORT_MAX_DECODE_TOTAL`] bytes (decompression bombs) — the raw, still-encoded bytes are
//! exported instead. Whenever a message declares a `Content-Encoding`,
//! `_akev.responseBodyEncoded` / `requestBodyEncoded` says which of the two it is.
//!
//! [`write`] streams the document entry by entry (the memory of one exchange at a time);
//! [`export`] builds it as one `serde_json::Value` (small exports, tests).
//!
//! # Import
//! Accepts Chrome, Firefox, Safari, Charles, Proxyman, Fiddler and HTTP Toolkit variants:
//! missing/`null` fields, numeric strings, negative or omitted timings, `h2`/`HTTP/2.0`/`h3`
//! versions, odd header casing, `comment` fields (→ note), HTTP Toolkit `_content`,
//! `_pinned`, `_trailers`, `_tlsErrors`, `_rawTunnels`, and `_webSocketMessages`. Invalid
//! entries are skipped (with a warning) instead of failing the whole file.
//!
//! **Content-Encoding rule.** HAR `text` is normally the *decoded* entity, so imported bodies
//! are stored as given and, when a message declares a `Content-Encoding` its stored text is
//! not actually encoded with, that header is removed so stored headers describe stored bytes.
//! The text counts as still encoded (header kept) only when `_akev.*BodyEncoded` says so or,
//! for HARs from other tools, when it is base64 and its bytes carry the declared outermost coding's
//! signature (gzip `1f 8b`, zstd `28 b5 2f fd`, a valid zlib header for deflate) or decode as
//! brotli. Bodies that weren't captured at all leave the headers untouched.

use crate::record::{StreamClose, guess_request, skeleton_summary};
use crate::util;
use crate::{MessageWithPayload, NewRecord, Result, Snapshot, StoreError};
use akev_core::*;
use base64::Engine;
use base64::engine::DecodePaddingMode;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig, STANDARD};
use bytes::Bytes;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value, json};
use std::io::Write;

/// Decompression cap for HAR import (brotli sniffing).
const MAX_DECODE: usize = 512 * 1024 * 1024;

/// Largest decoded size of one body in a HAR export; bigger bodies are exported still encoded.
pub(crate) const EXPORT_MAX_DECODE: usize = 64 * 1024 * 1024;

/// Decoded bytes one HAR export produces at most in total; once used up, the remaining encoded
/// bodies are exported still encoded (a decompression bomb can't inflate the file).
pub(crate) const EXPORT_MAX_DECODE_TOTAL: usize = 512 * 1024 * 1024;

/// Lenient base64 (padding optional) for imports.
const B64_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);
const B64_URL_LENIENT: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    let compact: String;
    let input = if text.bytes().any(|b| b.is_ascii_whitespace()) {
        compact = text.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        compact.as_str()
    } else {
        text
    };
    B64_LENIENT
        .decode(input)
        .or_else(|_| B64_URL_LENIENT.decode(input))
        .ok()
}

fn to_json<T: serde::Serialize>(v: &T) -> Value {
    serde_json::to_value(v).unwrap_or(Value::Null)
}

fn f64_json(v: f64) -> Value {
    serde_json::Number::from_f64(v)
        .map(Value::Number)
        .unwrap_or(Value::Null)
}

fn headers_json(headers: &Headers) -> Value {
    Value::Array(
        headers
            .iter()
            .map(|(n, v)| json!({ "name": n, "value": v }))
            .collect(),
    )
}

fn pairs_json<'a>(
    pairs: impl Iterator<Item = (std::borrow::Cow<'a, str>, std::borrow::Cow<'a, str>)>,
) -> Value {
    Value::Array(
        pairs
            .map(|(n, v)| json!({ "name": n, "value": v }))
            .collect(),
    )
}

// ---------------------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------------------

/// Decoding allowance shared by all bodies of one export.
pub(crate) struct ExportBudget {
    decoded_left: usize,
}

impl Default for ExportBudget {
    fn default() -> Self {
        ExportBudget {
            decoded_left: EXPORT_MAX_DECODE_TOTAL,
        }
    }
}

/// A body prepared for export: decoded when possible.
struct ExportBody {
    bytes: Bytes,
    /// Decoding failed (or wasn't allowed), so `bytes` are still content-encoded.
    still_encoded: bool,
}

fn decode_for_export(
    body: &CapturedBody,
    encodings: &[String],
    budget: &mut ExportBudget,
) -> ExportBody {
    if encodings.is_empty() || body.bytes.is_empty() {
        return ExportBody {
            bytes: body.bytes.clone(),
            still_encoded: false,
        };
    }
    let limit = EXPORT_MAX_DECODE.min(budget.decoded_left);
    match akev_content::decode::decode_body(&body.bytes, encodings, limit) {
        Ok(bytes) => {
            budget.decoded_left -= bytes.len();
            ExportBody {
                bytes,
                still_encoded: false,
            }
        }
        Err(_) => ExportBody {
            bytes: body.bytes.clone(),
            still_encoded: true,
        },
    }
}

/// The `_akev.*BodyEncoded` flag for a body: whether it's exported still encoded, stated
/// whenever the message declares a content-coding (so import never has to guess), `None`
/// otherwise.
fn encoded_flag(encodings: &[String], body: &CapturedBody, eb: &ExportBody) -> Option<bool> {
    (!encodings.is_empty() && !body.bytes.is_empty()).then_some(eb.still_encoded)
}

/// Whether JSON-escaping `text` would more than double it: control characters other than
/// `\t`, `\n`, `\r` become six-byte `\u00XX` escapes. Such text (NUL padding, binary that
/// happens to be valid UTF-8) is exported as base64 instead; readable text with the odd
/// control character (ANSI colours) stays text.
fn escapes_heavily(text: &str) -> bool {
    let controls = text
        .bytes()
        .filter(|&b| b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r'))
        .count();
    controls.saturating_mul(5) > text.len()
}

/// Insert `text` (UTF-8) or base64 + `encoding` into a HAR content/postData object. Takes the
/// bytes by value so a freshly decoded body becomes the JSON string without a copy.
fn put_text(obj: &mut Map<String, Value>, bytes: Bytes) {
    let text = match String::from_utf8(Vec::from(bytes)) {
        Ok(text) if !escapes_heavily(&text) => {
            obj.insert("text".into(), Value::String(text));
            return;
        }
        Ok(text) => text.into_bytes(),
        Err(e) => e.into_bytes(),
    };
    obj.insert("text".into(), Value::String(STANDARD.encode(&text)));
    obj.insert("encoding".into(), Value::String("base64".into()));
}

fn is_http1(v: HttpVersion) -> bool {
    matches!(
        v,
        HttpVersion::Http09 | HttpVersion::Http10 | HttpVersion::Http11
    )
}

/// HAR timings derived from Akev's [`Timing`] (`-1` = unknown), plus their total.
struct HarTimings {
    dns: f64,
    connect: f64,
    ssl: f64,
    send: f64,
    wait: f64,
    receive: f64,
}

fn har_timings(t: &Timing) -> (HarTimings, f64) {
    let known = |v: Option<f64>| v.filter(|x| x.is_finite() && *x >= 0.0);
    let dns = known(t.dns_duration);
    let ssl = known(t.tls_duration);
    // HAR: `connect` includes `ssl`.
    let connect = match known(t.connect_duration) {
        Some(c) => Some(c + ssl.unwrap_or(0.0)),
        None => ssl,
    };
    let setup = dns.unwrap_or(0.0) + connect.unwrap_or(0.0);
    let request_end = known(t.request_sent.or(t.request_body).or(t.request_headers)).unwrap_or(0.0);
    let send = (request_end - setup).max(0.0);
    let end = known(t.response_body.or(t.aborted));
    let (wait, receive) = match known(t.response_headers) {
        Some(rh) => (
            (rh - request_end).max(0.0),
            (end.unwrap_or(rh) - rh).max(0.0),
        ),
        None => ((end.unwrap_or(request_end) - request_end).max(0.0), 0.0),
    };
    let total = setup + send + wait + receive;
    let timings = HarTimings {
        dns: dns.unwrap_or(-1.0),
        connect: connect.unwrap_or(-1.0),
        ssl: ssl.unwrap_or(-1.0),
        send,
        wait,
        receive,
    };
    (timings, total)
}

fn request_json(
    d: &ExchangeDetail,
    body: &CapturedBody,
    budget: &mut ExportBudget,
) -> (Value, Option<bool>) {
    let req = &d.request;
    let mut r = Map::new();
    r.insert("method".into(), json!(req.method));
    r.insert("url".into(), json!(req.url));
    r.insert("httpVersion".into(), json!(req.http_version.as_str()));
    let cookies = req
        .headers
        .get_all("cookie")
        .flat_map(util::parse_cookie_header)
        .map(|(n, v)| json!({ "name": n, "value": v }))
        .collect::<Vec<_>>();
    r.insert("cookies".into(), Value::Array(cookies));
    r.insert("headers".into(), headers_json(&req.headers));
    let query = req
        .query()
        .map(|q| pairs_json(url::form_urlencoded::parse(q.as_bytes())));
    r.insert(
        "queryString".into(),
        query.unwrap_or_else(|| Value::Array(Vec::new())),
    );
    let headers_size = if is_http1(req.http_version) {
        let line = req.method.len() + req.path.len() + req.http_version.as_str().len() + 4;
        json!(line + req.headers.wire_size())
    } else {
        json!(-1)
    };
    r.insert("headersSize".into(), headers_size);
    r.insert("bodySize".into(), json!(body.wire_size));
    let mut encoded = None;
    if !body.bytes.is_empty() {
        let encodings = req.headers.content_encodings();
        let eb = decode_for_export(body, &encodings, budget);
        encoded = encoded_flag(&encodings, body, &eb);
        let still_encoded = eb.still_encoded;
        let mut post = Map::new();
        let mime = req
            .headers
            .get("content-type")
            .unwrap_or("application/octet-stream");
        post.insert("mimeType".into(), json!(mime));
        let urlencoded = req.headers.content_type_essence().as_deref()
            == Some("application/x-www-form-urlencoded");
        if urlencoded
            && !still_encoded
            && let Ok(text) = std::str::from_utf8(&eb.bytes)
        {
            // Chrome & Firefox export both params and text; so do we (with `; charset=`
            // parameters allowed, unlike HTTP Toolkit).
            post.insert(
                "params".into(),
                pairs_json(url::form_urlencoded::parse(text.as_bytes())),
            );
        }
        put_text(&mut post, eb.bytes);
        r.insert("postData".into(), Value::Object(post));
    }
    if let Some(t) = &d.request_trailers {
        r.insert("_trailers".into(), headers_json(t));
    }
    (Value::Object(r), encoded)
}

fn set_cookie_json(value: &str) -> Option<Value> {
    let c = util::parse_set_cookie(value)?;
    let mut m = Map::new();
    m.insert("name".into(), json!(c.name));
    m.insert("value".into(), json!(c.value));
    if let Some(p) = c.path {
        m.insert("path".into(), json!(p));
    }
    if let Some(d) = c.domain {
        m.insert("domain".into(), json!(d));
    }
    if let Some(e) = c.expires {
        m.insert(
            "expires".into(),
            json!(util::http_date_to_iso(e).unwrap_or_else(|| e.to_string())),
        );
    }
    m.insert("httpOnly".into(), json!(c.http_only));
    m.insert("secure".into(), json!(c.secure));
    if let Some(s) = c.same_site {
        m.insert("sameSite".into(), json!(s));
    }
    Some(Value::Object(m))
}

fn response_json(
    d: &ExchangeDetail,
    body: &CapturedBody,
    budget: &mut ExportBudget,
) -> (Value, Option<bool>) {
    let Some(resp) = &d.response else {
        let mut r = json!({
            "status": 0,
            "statusText": "",
            "httpVersion": "",
            "headers": [],
            "cookies": [],
            "content": { "size": 0, "mimeType": "x-unknown" },
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        });
        if let (Some(err), Some(obj)) = (&d.error, r.as_object_mut()) {
            obj.insert("_error".into(), json!(err.message));
        }
        return (r, None);
    };
    let encodings = resp.headers.content_encodings();
    let eb = decode_for_export(body, &encodings, budget);
    let encoded = encoded_flag(&encodings, body, &eb);
    let still_encoded = eb.still_encoded;
    let mut content = Map::new();
    let mut size = eb.bytes.len() as u64;
    if body.truncated && encodings.is_empty() {
        size = size.max(body.wire_size);
    }
    content.insert("size".into(), json!(size));
    if !encodings.is_empty() && !still_encoded && !body.truncated && size > body.wire_size {
        content.insert("compression".into(), json!(size - body.wire_size));
    }
    let default_mime = if body.bytes.is_empty() {
        "x-unknown"
    } else {
        "application/octet-stream"
    };
    content.insert(
        "mimeType".into(),
        json!(resp.headers.get("content-type").unwrap_or(default_mime)),
    );
    if !eb.bytes.is_empty() {
        put_text(&mut content, eb.bytes);
    }

    let mut r = Map::new();
    r.insert("status".into(), json!(resp.status));
    r.insert("statusText".into(), json!(resp.status_message));
    r.insert("httpVersion".into(), json!(resp.http_version.as_str()));
    let cookies = resp
        .headers
        .get_all("set-cookie")
        .filter_map(set_cookie_json)
        .collect::<Vec<_>>();
    r.insert("cookies".into(), Value::Array(cookies));
    r.insert("headers".into(), headers_json(&resp.headers));
    r.insert("content".into(), Value::Object(content));
    r.insert(
        "redirectURL".into(),
        json!(resp.headers.get("location").unwrap_or("")),
    );
    let headers_size = if is_http1(resp.http_version) {
        let line = resp.http_version.as_str().len()
            + resp.status.to_string().len()
            + resp.status_message.len()
            + 4;
        json!(line + resp.headers.wire_size())
    } else {
        json!(-1)
    };
    r.insert("headersSize".into(), headers_size);
    r.insert("bodySize".into(), json!(body.wire_size));
    if let Some(t) = &d.response_trailers {
        r.insert("_trailers".into(), headers_json(t));
    }
    if let Some(err) = &d.error {
        r.insert("_error".into(), json!(err.message));
    }
    (Value::Object(r), encoded)
}

/// Chrome's `_webSocketMessages` format (direction from the client's point of view), with
/// Akev extras only where needed to round-trip.
fn messages_json(messages: &[MessageWithPayload]) -> Value {
    Value::Array(
        messages
            .iter()
            .map(|m| {
                let msg = &m.message;
                let mut o = Map::new();
                let ty = match msg.direction {
                    Direction::Sent => "send",
                    Direction::Received => "receive",
                };
                o.insert("type".into(), json!(ty));
                o.insert("time".into(), f64_json(msg.at.0 as f64 / 1_000_000.0));
                let opcode = match msg.kind {
                    MessageKind::Text | MessageKind::Event => 1,
                    MessageKind::Binary => 2,
                    MessageKind::Close => 8,
                    MessageKind::Ping => 9,
                    MessageKind::Pong => 10,
                };
                o.insert("opcode".into(), json!(opcode));
                let text = match msg.kind {
                    MessageKind::Text | MessageKind::Event => std::str::from_utf8(&m.payload).ok(),
                    _ => None,
                };
                match text {
                    Some(t) => {
                        o.insert("data".into(), json!(t));
                    }
                    None => {
                        o.insert("data".into(), json!(STANDARD.encode(&m.payload)));
                        if opcode != 2 {
                            o.insert("encoding".into(), json!("base64"));
                        }
                    }
                }
                if msg.kind == MessageKind::Event {
                    o.insert("kind".into(), json!("event"));
                }
                if let Some(e) = &msg.event {
                    o.insert("event".into(), json!(e));
                }
                if let Some(e) = &msg.event_id {
                    o.insert("eventId".into(), json!(e));
                }
                if msg.modified {
                    o.insert("modified".into(), json!(true));
                }
                if msg.size != m.payload.len() as u64 {
                    o.insert("size".into(), json!(msg.size));
                }
                Value::Object(o)
            })
            .collect(),
    )
}

fn akev_ext(
    s: &Snapshot,
    req_encoded: Option<bool>,
    resp_encoded: Option<bool>,
) -> Map<String, Value> {
    let d = &s.detail;
    let sm = &d.summary;
    let mut m = Map::new();
    m.insert("id".into(), json!(sm.id.0));
    m.insert("kind".into(), to_json(&sm.kind));
    m.insert("state".into(), to_json(&sm.state));
    if sm.pinned {
        m.insert("pinned".into(), json!(true));
    }
    if let Some(c) = &sm.color {
        m.insert("color".into(), json!(c));
    }
    if let Some(n) = &d.note {
        m.insert("note".into(), json!(n));
    }
    if let Some(e) = &d.error {
        m.insert("error".into(), to_json(e));
    }
    if let Some(src) = &sm.source {
        m.insert("source".into(), json!(src));
    }
    if let Some(rule) = &sm.rule {
        m.insert("rule".into(), json!(rule));
    }
    if !d.matched_rules.is_empty() {
        m.insert("matchedRules".into(), to_json(&d.matched_rules));
    }
    if sm.mocked {
        m.insert("mocked".into(), json!(true));
    }
    if sm.modified {
        m.insert("modified".into(), json!(true));
    }
    if let Some(tls) = &d.client.tls {
        m.insert("tls".into(), to_json(tls));
    }
    m.insert("client".into(), to_json(&d.client));
    m.insert("timing".into(), to_json(&d.timing));
    if let Some(u) = &d.upstream {
        m.insert("upstream".into(), to_json(u));
    }
    if let Some(encoded) = req_encoded {
        m.insert("requestBodyEncoded".into(), json!(encoded));
    }
    if let Some(encoded) = resp_encoded {
        m.insert("responseBodyEncoded".into(), json!(encoded));
    }
    if s.request_body.truncated {
        m.insert("requestBodyTruncated".into(), json!(true));
    }
    if s.response_body.truncated {
        m.insert("responseBodyTruncated".into(), json!(true));
    }
    if let Some(c) = &s.close {
        m.insert("close".into(), to_json(c));
    }
    if let Some(t) = &d.tunnel {
        m.insert("tunnel".into(), to_json(t));
    }
    m
}

fn entry_json(s: &Snapshot, budget: &mut ExportBudget) -> Value {
    let d = &s.detail;
    let (t, total) = har_timings(&d.timing);
    let (request, req_encoded) = request_json(d, &s.request_body, budget);
    let (response, resp_encoded) = response_json(d, &s.response_body, budget);
    let mut e = Map::new();
    e.insert("startedDateTime".into(), json!(d.timing.start.to_rfc3339()));
    e.insert("time".into(), f64_json(total));
    e.insert("request".into(), request);
    e.insert("response".into(), response);
    e.insert("cache".into(), json!({}));
    e.insert(
        "timings".into(),
        json!({
            "blocked": -1,
            "dns": f64_json(t.dns),
            "connect": f64_json(t.connect),
            "ssl": f64_json(t.ssl),
            "send": f64_json(t.send),
            "wait": f64_json(t.wait),
            "receive": f64_json(t.receive),
        }),
    );
    if let Some(ip) = d
        .upstream
        .as_ref()
        .and_then(|u| u.remote_addr.as_deref())
        .and_then(util::ip_of)
    {
        e.insert("serverIPAddress".into(), json!(ip));
    }
    e.insert(
        "connection".into(),
        json!(d.client.connection_id.to_string()),
    );
    if let Some(note) = &d.note {
        e.insert("comment".into(), json!(note));
    }
    if d.summary.pinned {
        e.insert("_pinned".into(), json!(true));
    }
    let mut ext = akev_ext(s, req_encoded, resp_encoded);
    if d.summary.kind == ExchangeKind::WebSocket {
        e.insert("_resourceType".into(), json!("websocket"));
        e.insert("_webSocketMessages".into(), messages_json(&s.messages));
        if let Some(c) = &s.close {
            let mut close = Map::new();
            if let Some(code) = c.code {
                close.insert("code".into(), json!(code));
            }
            if let Some(reason) = &c.reason {
                close.insert("reason".into(), json!(reason));
            }
            let ts = (d.timing.start.as_millis_f64() + c.at) / 1000.0;
            close.insert("timestamp".into(), f64_json(ts));
            e.insert("_webSocketClose".into(), Value::Object(close));
        }
    } else if !s.messages.is_empty() {
        ext.insert("messages".into(), messages_json(&s.messages));
    }
    e.insert("_akev".into(), Value::Object(ext));
    Value::Object(e)
}

fn client_addr_fields(obj: &mut Map<String, Value>, client: &ClientInfo) {
    if let Some(ip) = util::ip_of(&client.remote_addr) {
        obj.insert("clientIPAddress".into(), json!(ip));
    }
    if let Some(port) = util::port_of(&client.remote_addr) {
        obj.insert("clientPort".into(), json!(port));
    }
}

fn tls_cause(error: Option<&ExchangeError>) -> &'static str {
    const KNOWN: [&str; 5] = [
        "closed",
        "reset",
        "cert-rejected",
        "no-shared-cipher",
        "handshake-timeout",
    ];
    let Some(e) = error else { return "unknown" };
    if let Some(code) = e.code.as_deref()
        && let Some(k) = KNOWN.iter().find(|k| **k == code)
    {
        return k;
    }
    match e.kind {
        ErrorKind::ClientTlsRejected => "cert-rejected",
        ErrorKind::ConnectionReset => "reset",
        ErrorKind::ClientAborted => "closed",
        ErrorKind::Timeout => "handshake-timeout",
        _ => "unknown",
    }
}

fn tls_error_json(s: &Snapshot) -> Value {
    let d = &s.detail;
    let mut o = Map::new();
    o.insert("startedDateTime".into(), json!(d.timing.start.to_rfc3339()));
    o.insert("time".into(), json!(0));
    o.insert("hostname".into(), json!(d.summary.host));
    o.insert("cause".into(), json!(tls_cause(d.error.as_ref())));
    client_addr_fields(&mut o, &d.client);
    if let Some(tls) = &d.client.tls {
        let mut meta = Map::new();
        if let Some(sni) = &tls.sni {
            meta.insert("sniHostname".into(), json!(sni));
        }
        if !tls.alpn_offered.is_empty() {
            meta.insert("clientAlpn".into(), to_json(&tls.alpn_offered));
        }
        if let Some(ja3) = &tls.ja3 {
            meta.insert("ja3Fingerprint".into(), json!(ja3));
        }
        if let Some(ja4) = &tls.ja4 {
            meta.insert("ja4Fingerprint".into(), json!(ja4));
        }
        o.insert("tlsMetadata".into(), Value::Object(meta));
    }
    let mut ext = akev_ext(s, None, None);
    ext.insert("port".into(), json!(d.summary.port));
    o.insert("_akev".into(), Value::Object(ext));
    Value::Object(o)
}

fn tunnel_json(s: &Snapshot) -> Value {
    let d = &s.detail;
    let mut o = Map::new();
    o.insert("startedDateTime".into(), json!(d.timing.start.to_rfc3339()));
    if let Some(c) = &s.close {
        o.insert("time".into(), f64_json(c.at));
    }
    client_addr_fields(&mut o, &d.client);
    o.insert("hostname".into(), json!(d.summary.host));
    o.insert("port".into(), json!(d.summary.port));
    o.insert("messages".into(), messages_json(&s.messages));
    if d.summary.pinned {
        o.insert("pinned".into(), json!(true));
    }
    let mut ext = akev_ext(s, None, None);
    ext.insert("method".into(), json!(d.summary.method));
    ext.insert("scheme".into(), json!(d.summary.scheme));
    o.insert("_akev".into(), Value::Object(ext));
    Value::Object(o)
}

fn client_error_json(s: &Snapshot) -> Value {
    let d = &s.detail;
    let mut o = Map::new();
    o.insert("startedDateTime".into(), json!(d.timing.start.to_rfc3339()));
    client_addr_fields(&mut o, &d.client);
    if let Some(e) = &d.error {
        o.insert("error".into(), to_json(e));
    }
    o.insert(
        "rawPrefix".into(),
        json!(STANDARD.encode(&s.request_body.bytes)),
    );
    o.insert("_akev".into(), Value::Object(akev_ext(s, None, None)));
    Value::Object(o)
}

/// The array of `log` an exchange is exported to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Group {
    /// `log.entries`: HTTP, WebSocket and event-stream exchanges.
    Entries,
    /// `log._tlsErrors`.
    TlsErrors,
    /// `log._rawTunnels`.
    RawTunnels,
    /// `log._clientErrors`.
    ClientErrors,
}

impl Group {
    /// Every group, in document order.
    pub(crate) const ALL: [Group; 4] = [
        Group::Entries,
        Group::TlsErrors,
        Group::RawTunnels,
        Group::ClientErrors,
    ];

    pub(crate) fn of(kind: ExchangeKind) -> Group {
        match kind {
            ExchangeKind::TlsFailure => Group::TlsErrors,
            ExchangeKind::Tunnel => Group::RawTunnels,
            ExchangeKind::ClientError => Group::ClientErrors,
            ExchangeKind::Http | ExchangeKind::WebSocket | ExchangeKind::EventStream => {
                Group::Entries
            }
        }
    }

    fn key(self) -> &'static str {
        match self {
            Group::Entries => "entries",
            Group::TlsErrors => "_tlsErrors",
            Group::RawTunnels => "_rawTunnels",
            Group::ClientErrors => "_clientErrors",
        }
    }
}

/// One item of `group` as JSON.
fn item_json(group: Group, snap: &Snapshot, budget: &mut ExportBudget) -> Value {
    match group {
        Group::Entries => entry_json(snap, budget),
        Group::TlsErrors => tls_error_json(snap),
        Group::RawTunnels => tunnel_json(snap),
        Group::ClientErrors => client_error_json(snap),
    }
}

fn creator_json() -> Value {
    json!({ "name": PRODUCT_NAME, "version": VERSION })
}

/// Build the HAR document for the given snapshots (already in the desired order) as one JSON
/// value. Holds every exported body at once: prefer [`write`] for anything large.
pub(crate) fn export(snapshots: impl Iterator<Item = Snapshot>) -> Value {
    let mut budget = ExportBudget::default();
    let mut groups: [Vec<Value>; 4] = Default::default();
    for snap in snapshots {
        let group = Group::of(snap.detail.summary.kind);
        let index = Group::ALL.iter().position(|g| *g == group).unwrap_or(0);
        groups[index].push(item_json(group, &snap, &mut budget));
    }
    let mut log = Map::new();
    log.insert("version".into(), json!("1.2"));
    log.insert("creator".into(), creator_json());
    for (group, items) in Group::ALL.into_iter().zip(groups) {
        // `entries` is mandatory; the extension arrays only appear when non-empty.
        if group == Group::Entries || !items.is_empty() {
            log.insert(group.key().into(), Value::Array(items));
        }
    }
    json!({ "log": log })
}

/// Indents everything written through it by `indent` after each line break. serde_json never
/// writes a raw newline inside a string (it escapes them), so every `\n` it emits is layout.
struct Indented<'a> {
    out: &'a mut dyn Write,
    indent: &'static [u8],
}

impl Write for Indented<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let mut rest = buf;
        while let Some(i) = rest.iter().position(|&b| b == b'\n') {
            self.out.write_all(&rest[..=i])?;
            self.out.write_all(self.indent)?;
            rest = &rest[i + 1..];
        }
        self.out.write_all(rest)?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

/// Pretty-print `value` at `indent` (the indentation of the line it starts on).
fn write_pretty(out: &mut dyn Write, indent: &'static [u8], value: &Value) -> std::io::Result<()> {
    let mut indented = Indented { out, indent };
    serde_json::to_writer_pretty(&mut indented, value).map_err(std::io::Error::from)
}

/// Stream the HAR document to `out`, one exchange at a time: `group` yields the snapshots of
/// each [`Group`] (asked in [`Group::ALL`] order), so only one exchange's bodies are in memory
/// at any time. The bytes are exactly `serde_json::to_vec_pretty` of [`export`] over the same
/// snapshots. Returns the number of exchanges written.
pub(crate) fn write<I: Iterator<Item = Snapshot>>(
    out: &mut dyn Write,
    mut group: impl FnMut(Group) -> I,
) -> std::io::Result<usize> {
    const LOG_INDENT: &[u8] = b"    ";
    const ITEM_INDENT: &[u8] = b"      ";
    let mut budget = ExportBudget::default();
    let mut written = 0usize;
    out.write_all(b"{\n  \"log\": {\n    \"version\": \"1.2\",\n    \"creator\": ")?;
    write_pretty(out, LOG_INDENT, &creator_json())?;
    for g in Group::ALL {
        let mut items = group(g).peekable();
        if g != Group::Entries && items.peek().is_none() {
            continue;
        }
        write!(out, ",\n    \"{}\": [", g.key())?;
        let mut first = true;
        for snap in items {
            out.write_all(if first { b"\n      " } else { b",\n      " })?;
            first = false;
            write_pretty(out, ITEM_INDENT, &item_json(g, &snap, &mut budget))?;
            written += 1;
        }
        if !first {
            out.write_all(b"\n    ")?;
        }
        out.write_all(b"]")?;
    }
    out.write_all(b"\n  }\n}")?;
    out.flush()?;
    Ok(written)
}

// ---------------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------------

type Obj = Map<String, Value>;

fn obj(v: Option<&Value>) -> Option<&Obj> {
    v.and_then(Value::as_object)
}

fn arr(v: Option<&Value>) -> &[Value] {
    v.and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

fn str_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

/// A number, also accepting numeric strings (some exporters quote numbers).
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

fn non_negative(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|n| *n >= 0.0)
}

fn truthy(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// Leniently deserialize one `_akev` field (a bad field never spoils the rest).
fn ext_field<T: DeserializeOwned>(ext: Option<&Obj>, key: &str) -> Option<T> {
    ext.and_then(|e| e.get(key))
        .and_then(|v| T::deserialize(v).ok())
}

/// Header list of a HAR request/response/trailers. Names containing control characters (a
/// line break, NUL, ...) are dropped: no HTTP message can carry them, and they would only
/// break whatever later renders the header (raw views, code snippets).
pub(crate) fn parse_headers(v: Option<&Value>) -> Headers {
    let mut out = Headers::new();
    for h in arr(v) {
        let Some(h) = h.as_object() else { continue };
        let Some(name) =
            str_of(h.get("name")).filter(|n| !n.is_empty() && !n.chars().any(char::is_control))
        else {
            continue;
        };
        let value = match h.get("value") {
            Some(Value::String(s)) => s.clone(),
            None | Some(Value::Null) => String::new(),
            Some(other) => other.to_string(),
        };
        out.append(name, value);
    }
    out
}

/// Parse a HAR `httpVersion` (`HTTP/1.1`, `http/2.0`, `h2`, `HTTP/3`, `h3-29`, `1.1`...).
pub(crate) fn parse_version(raw: Option<&str>) -> Option<HttpVersion> {
    let lower = raw?.trim().to_ascii_lowercase();
    let v = lower.strip_prefix("http/").unwrap_or(&lower);
    Some(match v {
        "0.9" => HttpVersion::Http09,
        "1" | "1.0" => HttpVersion::Http10,
        "1.1" => HttpVersion::Http11,
        "2" | "2.0" | "h2" | "h2c" | "spdy" => HttpVersion::H2,
        v if v == "3" || v == "3.0" || v.starts_with("h3") || v == "quic" => HttpVersion::H3,
        _ => return None,
    })
}

fn version_or_guess(raw: Option<&str>, headers: &Headers) -> HttpVersion {
    parse_version(raw).unwrap_or_else(|| {
        if headers.iter().any(|(n, _)| n.starts_with(':')) {
            HttpVersion::H2
        } else {
            HttpVersion::Http11
        }
    })
}

/// Parse `startedDateTime`: RFC 3339, plus missing-offset and `+HHMM` variants.
pub(crate) fn parse_start(v: Option<&Value>) -> Option<UnixMicros> {
    let s = str_of(v)?.trim();
    if let Some(t) = UnixMicros::from_rfc3339(s) {
        return Some(t);
    }
    // `+0100` → `+01:00`. The cut may fall inside a multi-byte character (then it isn't an
    // ASCII offset anyway): `get` instead of `split_at`, which would panic.
    if let Some(cut) = s.len().checked_sub(5).filter(|&c| c > 0)
        && let (Some(head), Some(tail)) = (s.get(..cut), s.get(cut..))
        && (tail.starts_with('+') || tail.starts_with('-'))
        && tail[1..].bytes().all(|b| b.is_ascii_digit())
        && let Some(t) = UnixMicros::from_rfc3339(&format!("{head}{}:{}", &tail[..3], &tail[3..]))
    {
        return Some(t);
    }
    // No offset at all → assume UTC.
    UnixMicros::from_rfc3339(&format!("{s}Z"))
}

/// Decode a HAR `text` + `encoding` pair. Returns the bytes and whether they were base64.
fn content_bytes(text: Option<&str>, encoding: Option<&str>) -> (Vec<u8>, bool) {
    let text = text.unwrap_or("");
    if encoding.is_some_and(|e| e.trim().eq_ignore_ascii_case("base64"))
        && let Some(bytes) = b64_decode(text)
    {
        return (bytes, true);
    }
    (text.as_bytes().to_vec(), false)
}

fn request_body_bytes(req: &Obj) -> (Vec<u8>, bool) {
    if let Some(c) = obj(req.get("_content")) {
        return content_bytes(str_of(c.get("text")), str_of(c.get("encoding")));
    }
    let Some(post) = obj(req.get("postData")) else {
        return (Vec::new(), false);
    };
    let text = str_of(post.get("text")).filter(|t| !t.is_empty());
    if text.is_some() {
        return content_bytes(text, str_of(post.get("encoding")));
    }
    let mime = str_of(post.get("mimeType"))
        .unwrap_or("")
        .to_ascii_lowercase();
    let params = arr(post.get("params"));
    if !params.is_empty()
        && (mime.is_empty() || mime.starts_with("application/x-www-form-urlencoded"))
    {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for p in params {
            if let Some(name) = str_of(p.get("name")) {
                ser.append_pair(name, str_of(p.get("value")).unwrap_or(""));
            }
        }
        return (ser.finish().into_bytes(), false);
    }
    (Vec::new(), false)
}

/// Whether bytes carry the signature of the outermost declared content-coding.
fn looks_encoded(encodings: &[String], bytes: &[u8]) -> bool {
    match encodings.last().map(String::as_str) {
        Some("gzip" | "x-gzip") => bytes.starts_with(&[0x1f, 0x8b]),
        Some("zstd") => bytes.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]),
        Some("deflate" | "x-deflate") => {
            bytes.len() >= 2
                && bytes[0] & 0x0f == 8
                && ((u16::from(bytes[0]) << 8) | u16::from(bytes[1])) % 31 == 0
        }
        Some("br") => akev_content::decode::decode_body(bytes, encodings, MAX_DECODE).is_ok(),
        _ => false,
    }
}

/// Apply the module-level Content-Encoding rule. Returns whether the bytes are (treated as)
/// still encoded.
pub(crate) fn resolve_encoding(
    headers: &mut Headers,
    bytes: &[u8],
    was_base64: bool,
    explicit: Option<bool>,
) -> bool {
    let encodings = headers.content_encodings();
    if encodings.is_empty() {
        return false;
    }
    if bytes.is_empty() {
        return true;
    }
    let still = explicit.unwrap_or_else(|| was_base64 && looks_encoded(&encodings, bytes));
    if !still {
        headers.remove("content-encoding");
    }
    still
}

pub(crate) fn error_kind_from_text(s: &str) -> ErrorKind {
    let u = s.to_ascii_uppercase();
    let has = |needle: &str| u.contains(needle);
    if has("NAME_NOT_RESOLVED") || has("DNS") {
        ErrorKind::DnsFailed
    } else if has("CONNECTION_REFUSED") || has("ECONNREFUSED") {
        ErrorKind::ConnectionRefused
    } else if has("CONNECTION_RESET")
        || has("ECONNRESET")
        || has("CONNECTION_CLOSED")
        || has("EMPTY_RESPONSE")
    {
        ErrorKind::ConnectionReset
    } else if has("TIMED_OUT") || has("TIMEOUT") {
        ErrorKind::Timeout
    } else if has("CERT") {
        ErrorKind::UpstreamCertInvalid
    } else if has("SSL") || has("TLS") {
        ErrorKind::UpstreamTlsFailed
    } else if has("UNREACHABLE") || has("INTERNET_DISCONNECTED") || has("NETWORK_CHANGED") {
        ErrorKind::HostUnreachable
    } else if has("PROXY") {
        ErrorKind::UpstreamProxyFailed
    } else if has("ABORT") || has("CANCEL") || has("BLOCKED") {
        ErrorKind::ClientAborted
    } else {
        ErrorKind::Internal
    }
}

/// Parse a list of HAR stream messages (`send`/`receive` only; Chrome `error` rows dropped).
fn parse_messages(list: &[Value], start: UnixMicros) -> Vec<MessageWithPayload> {
    let mut out = Vec::with_capacity(list.len());
    for m in list {
        let Some(m) = m.as_object() else { continue };
        let direction = match str_of(m.get("type")) {
            Some("send") => Direction::Sent,
            Some("receive") => Direction::Received,
            _ => continue,
        };
        let opcode = num(m.get("opcode")).unwrap_or(1.0) as i64;
        let kind = match (str_of(m.get("kind")), opcode) {
            (Some("event"), _) => MessageKind::Event,
            (_, 2) => MessageKind::Binary,
            (_, 8) => MessageKind::Close,
            (_, 9) => MessageKind::Ping,
            (_, 10) => MessageKind::Pong,
            _ => MessageKind::Text,
        };
        let data = str_of(m.get("data")).unwrap_or("");
        let base64 = opcode == 2
            || str_of(m.get("encoding")).is_some_and(|e| e.eq_ignore_ascii_case("base64"));
        let payload = if base64 {
            b64_decode(data).unwrap_or_else(|| data.as_bytes().to_vec())
        } else {
            data.as_bytes().to_vec()
        };
        let at = match num(m.get("time")) {
            // Epoch seconds (Chrome); tiny values are offsets from the entry start.
            Some(t) if t >= 1e8 => UnixMicros((t * 1_000_000.0).round() as i64),
            Some(t) if t >= 0.0 => UnixMicros(start.0 + (t * 1_000_000.0).round() as i64),
            _ => start,
        };
        let size = non_negative(m.get("size")).map_or(payload.len() as u64, |s| s as u64);
        let message = StreamMessage {
            seq: u32::try_from(out.len()).unwrap_or(u32::MAX),
            direction,
            kind,
            at,
            size,
            event: str_of(m.get("event")).map(str::to_string),
            event_id: str_of(m.get("eventId")).map(str::to_string),
            modified: truthy(m.get("modified")),
        };
        out.push(MessageWithPayload {
            message,
            payload: Bytes::from(payload),
        });
    }
    out
}

/// Timing reconstructed from HAR `timings` + `time` (negative/omitted values ignored).
fn har_timing(e: &Obj, start: UnixMicros, has_response: bool, aborted: bool) -> Timing {
    let t = obj(e.get("timings"));
    let get = |k: &str| t.and_then(|t| non_negative(t.get(k)));
    let blocked = get("blocked").unwrap_or(0.0);
    let dns = get("dns");
    let ssl = get("ssl");
    let connect = get("connect");
    let setup = dns.unwrap_or(0.0) + connect.unwrap_or(0.0);
    let request_sent = blocked + setup + get("send").unwrap_or(0.0);
    let response_headers = request_sent + get("wait").unwrap_or(0.0);
    let computed_end = response_headers + get("receive").unwrap_or(0.0);
    let total = non_negative(e.get("time")).unwrap_or(computed_end);
    Timing {
        start,
        request_headers: None,
        request_body: Some(request_sent),
        dns_duration: dns,
        connect_duration: connect.map(|c| (c - ssl.unwrap_or(0.0)).max(0.0)),
        tls_duration: ssl,
        request_sent: Some(request_sent),
        response_headers: has_response.then_some(response_headers),
        response_body: (has_response && !aborted).then_some(total.max(response_headers)),
        aborted: aborted.then_some(total),
        paused_duration: None,
    }
}

fn client_from(o: &Obj, ext: Option<&Obj>) -> ClientInfo {
    if let Some(c) = ext_field::<ClientInfo>(ext, "client") {
        return c;
    }
    let mut client = ClientInfo::default();
    if let Some(ip) = str_of(o.get("clientIPAddress")).filter(|s| !s.is_empty()) {
        let port = num(o.get("clientPort")).map_or(0, |p| p as u16);
        client.remote_addr = util::socket_addr_string(ip, port);
    }
    client.tls = ext_field(ext, "tls");
    client
}

fn upstream_from(e: &Obj, ext: Option<&Obj>, port: u16) -> Option<UpstreamInfo> {
    if let Some(u) = ext_field::<UpstreamInfo>(ext, "upstream") {
        return Some(u);
    }
    let raw = str_of(e.get("serverIPAddress"))?.trim();
    let ip = raw.trim_start_matches('[').trim_end_matches(']');
    let ip: std::net::IpAddr = ip.parse().ok()?;
    Some(UpstreamInfo {
        remote_addr: Some(util::socket_addr_string(&ip.to_string(), port)),
        ..UpstreamInfo::default()
    })
}

/// Parser for one item of a HAR array.
type ItemParser = fn(&Value) -> Option<Item>;

/// One importable item and its ordering keys.
struct Item {
    akev_id: Option<u64>,
    start: i64,
    record: NewRecord,
}

#[allow(clippy::too_many_arguments)]
fn detail_skeleton(
    kind: ExchangeKind,
    state: ExchangeState,
    request: RequestHead,
    timing: Timing,
    client: ClientInfo,
    ext: Option<&Obj>,
    error: Option<ExchangeError>,
    note: Option<String>,
) -> ExchangeDetail {
    let mut summary = skeleton_summary(ExchangeId(0), kind, timing.start);
    summary.state = state;
    summary.pinned = ext_field(ext, "pinned").unwrap_or(false);
    summary.color = ext_field(ext, "color");
    summary.source = ext_field(ext, "source");
    summary.rule = ext_field(ext, "rule");
    summary.mocked = ext_field(ext, "mocked").unwrap_or(false);
    summary.modified = ext_field(ext, "modified").unwrap_or(false);
    ExchangeDetail {
        summary,
        request,
        request_body: BodyMeta::default(),
        request_trailers: None,
        response: None,
        response_body: BodyMeta::default(),
        response_trailers: None,
        original_request: None,
        original_response: None,
        timing,
        client,
        upstream: None,
        error,
        tunnel: ext_field(ext, "tunnel"),
        note,
        matched_rules: ext_field(ext, "matchedRules").unwrap_or_default(),
        notices: Vec::new(),
    }
}

fn parse_entry(entry: &Value) -> Option<Item> {
    let e = entry.as_object()?;
    let req = obj(e.get("request"))?;
    let url = str_of(req.get("url"))?.trim();
    let parts = util::split_url(url)?;
    let ext = obj(e.get("_akev"));

    let method = str_of(req.get("method"))
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("GET");
    let mut req_headers = parse_headers(req.get("headers"));
    let http_version = version_or_guess(str_of(req.get("httpVersion")), &req_headers);
    let ext_timing: Option<Timing> = ext_field(ext, "timing");
    let start = parse_start(e.get("startedDateTime"))
        .or(ext_timing.as_ref().map(|t| t.start))
        .unwrap_or_else(UnixMicros::now);

    // Request body.
    let (req_bytes, req_b64) = request_body_bytes(req);
    resolve_encoding(
        &mut req_headers,
        &req_bytes,
        req_b64,
        ext_field(ext, "requestBodyEncoded"),
    );
    let req_truncated = ext_field(ext, "requestBodyTruncated").unwrap_or(false)
        || str_of(req.get("_requestBodyStatus"))
            .is_some_and(|s| s.starts_with("discarded:too-large"));
    let req_wire = non_negative(req.get("bodySize")).map_or(0, |n| n as u64);
    let request_body = CapturedBody {
        wire_size: req_wire.max(req_bytes.len() as u64),
        truncated: req_truncated,
        bytes: Bytes::from(req_bytes),
    };

    // Response.
    let resp = obj(e.get("response"));
    let status = resp
        .and_then(|r| num(r.get("status")))
        .map(|s| s as i64)
        .filter(|s| (1..=999).contains(s))
        .map(|s| s as u16);
    let mut response = None;
    let mut response_body = CapturedBody::empty();
    let mut response_trailers = None;
    if let (Some(r), Some(status)) = (resp, status) {
        let mut headers = parse_headers(r.get("headers"));
        let status_message = str_of(r.get("statusText"))
            .map(str::to_string)
            .unwrap_or_else(|| canonical_reason(status).to_string());
        let version = parse_version(str_of(r.get("httpVersion"))).unwrap_or(http_version);
        let content = obj(r.get("content"));
        let (bytes, b64) = content
            .map(|c| content_bytes(str_of(c.get("text")), str_of(c.get("encoding"))))
            .unwrap_or_default();
        let kept = resolve_encoding(
            &mut headers,
            &bytes,
            b64,
            ext_field(ext, "responseBodyEncoded"),
        );
        let len = bytes.len() as u64;
        let content_size = content
            .and_then(|c| non_negative(c.get("size")))
            .map(|n| n as u64);
        // 1xx/204/304 carry no body by definition (Firefox reports the cached size on 304s).
        let bodyless = status < 200 || status == 204 || status == 304;
        let truncated = ext_field(ext, "responseBodyTruncated").unwrap_or_else(|| {
            str_of(r.get("_responseBodyStatus")).is_some_and(|s| s.starts_with("discarded"))
                || (!bodyless && content_size.is_some_and(|size| size > len && (len == 0 || !kept)))
        });
        let mut wire = non_negative(r.get("bodySize"))
            .map_or(0, |n| n as u64)
            .max(len);
        if truncated && headers.content_encodings().is_empty() {
            wire = wire.max(content_size.unwrap_or(0));
        }
        response_body = CapturedBody {
            bytes: Bytes::from(bytes),
            wire_size: wire,
            truncated,
        };
        response_trailers = r.get("_trailers").map(|t| parse_headers(Some(t)));
        response = Some(ResponseHead {
            status,
            status_message,
            http_version: version,
            headers,
        });
    }
    let request_trailers = req.get("_trailers").map(|t| parse_headers(Some(t)));

    // WebSocket / stream messages.
    let resource_ws =
        str_of(e.get("_resourceType")).is_some_and(|t| t.eq_ignore_ascii_case("websocket"));
    let ws_list = arr(e.get("_webSocketMessages"));
    let upgrade = status == Some(101)
        && req_headers
            .get("upgrade")
            .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let kind = ext_field(ext, "kind").unwrap_or(if resource_ws || !ws_list.is_empty() || upgrade {
        ExchangeKind::WebSocket
    } else {
        ExchangeKind::Http
    });
    let list = if ws_list.is_empty() {
        arr(ext.and_then(|x| x.get("messages")))
    } else {
        ws_list
    };
    let messages = parse_messages(list, start);

    // Errors & timing.
    let resp_error = resp
        .and_then(|r| str_of(r.get("_error")))
        .filter(|s| !s.is_empty());
    let ws_close = e.get("_webSocketClose");
    let ws_aborted = str_of(ws_close).is_some_and(|s| s.eq_ignore_ascii_case("aborted"));
    let error: Option<ExchangeError> = ext_field(ext, "error").or_else(|| {
        if response.is_none() {
            let message = resp_error.unwrap_or("The request did not complete");
            Some(ExchangeError {
                kind: resp_error.map_or(ErrorKind::Internal, error_kind_from_text),
                message: message.to_string(),
                code: resp_error.map(str::to_string),
            })
        } else if ws_aborted {
            Some(ExchangeError {
                kind: ErrorKind::ConnectionReset,
                message: "WebSocket connection aborted".into(),
                code: None,
            })
        } else {
            resp_error.map(|m| ExchangeError {
                kind: error_kind_from_text(m),
                message: m.to_string(),
                code: Some(m.to_string()),
            })
        }
    });
    let aborted = error.is_some();
    let timing = ext_timing
        .map(|mut t| {
            t.start = start;
            t
        })
        .unwrap_or_else(|| har_timing(e, start, response.is_some(), aborted));
    let close: Option<StreamClose> = ext_field(ext, "close").or_else(|| {
        let c = obj(ws_close)?;
        let at = num(c.get("timestamp"))
            // Epoch seconds → ms since start, rounded to the microsecond.
            .map(|ts| ((ts * 1_000_000.0).round() as i64 - start.0) as f64 / 1000.0)
            .filter(|at| *at >= 0.0)
            .or(timing.total())
            .unwrap_or(0.0);
        Some(StreamClose {
            code: num(c.get("code")).map(|c| c as u16),
            reason: str_of(c.get("reason")).map(str::to_string),
            by: None,
            at,
        })
    });
    let state = ext_field(ext, "state").unwrap_or(if aborted || response.is_none() {
        ExchangeState::Aborted
    } else {
        ExchangeState::Complete
    });
    let note = ext_field::<String>(ext, "note")
        .or_else(|| str_of(e.get("comment")).map(str::to_string))
        .filter(|n| !n.trim().is_empty());

    let request = RequestHead {
        method: method.to_string(),
        url: url.to_string(),
        scheme: parts.scheme,
        host: parts.host,
        port: parts.port,
        path: parts.path,
        http_version,
        headers: req_headers,
    };
    let client = client_from(e, ext);
    let port = request.port;
    let mut detail = detail_skeleton(kind, state, request, timing, client, ext, error, note);
    detail.summary.pinned |= truthy(e.get("_pinned"));
    detail.request_trailers = request_trailers;
    detail.response = response;
    detail.response_trailers = response_trailers;
    detail.upstream = upstream_from(e, ext, port);
    Some(Item {
        akev_id: ext_field(ext, "id"),
        start: start.0,
        record: NewRecord {
            detail,
            request_body,
            response_body,
            original_request_body: None,
            original_response_body: None,
            messages,
            close,
            exact: false,
        },
    })
}

fn tls_error_kind(cause: &str) -> (ErrorKind, &'static str) {
    match cause {
        "cert-rejected" => (
            ErrorKind::ClientTlsRejected,
            "The client rejected the certificate",
        ),
        "no-shared-cipher" => (
            ErrorKind::ClientTlsRejected,
            "No cipher suite shared with the client",
        ),
        "handshake-timeout" => (ErrorKind::Timeout, "The TLS handshake timed out"),
        "reset" => (
            ErrorKind::ConnectionReset,
            "The client reset the connection during the TLS handshake",
        ),
        "closed" => (
            ErrorKind::ClientAborted,
            "The client closed the connection during the TLS handshake",
        ),
        _ => (ErrorKind::ClientTlsRejected, "The TLS handshake failed"),
    }
}

fn parse_tls_error(v: &Value) -> Option<Item> {
    let o = v.as_object()?;
    let ext = obj(o.get("_akev"));
    let start = parse_start(o.get("startedDateTime")).unwrap_or_else(UnixMicros::now);
    let meta = obj(o.get("tlsMetadata"));
    let host = str_of(o.get("hostname"))
        .or_else(|| meta.and_then(|m| str_of(m.get("sniHostname"))))
        .unwrap_or("")
        .to_ascii_lowercase();
    let port = ext_field::<u16>(ext, "port").unwrap_or(443);
    let mut client = client_from(o, ext);
    if client.tls.is_none()
        && let Some(m) = meta
    {
        let alpn_offered = match m.get("clientAlpn") {
            Some(Value::Array(a)) => a
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            Some(Value::String(s)) => s
                .split(',')
                .map(|p| p.trim().to_string())
                .filter(|p| !p.is_empty())
                .collect(),
            _ => Vec::new(),
        };
        client.tls = Some(TlsInfo {
            sni: str_of(m.get("sniHostname")).map(str::to_string),
            alpn_offered,
            ja3: str_of(m.get("ja3Fingerprint")).map(str::to_string),
            ja4: str_of(m.get("ja4Fingerprint")).map(str::to_string),
            ..TlsInfo::default()
        });
    }
    let cause = str_of(o.get("cause")).unwrap_or("unknown");
    let error = ext_field(ext, "error").unwrap_or_else(|| {
        let (kind, message) = tls_error_kind(cause);
        ExchangeError {
            kind,
            message: message.into(),
            code: Some(cause.to_string()),
        }
    });
    let request = RequestHead {
        method: String::new(),
        url: format!("https://{}", util::authority("https", &host, port)),
        scheme: "https".into(),
        host,
        port,
        path: String::new(),
        http_version: HttpVersion::default(),
        headers: Headers::new(),
    };
    let timing = ext_field::<Timing>(ext, "timing").unwrap_or(Timing {
        start,
        ..Timing::default()
    });
    let state = ext_field(ext, "state").unwrap_or(ExchangeState::Aborted);
    let note = ext_field(ext, "note");
    let detail = detail_skeleton(
        ExchangeKind::TlsFailure,
        state,
        request,
        timing,
        client,
        ext,
        Some(error),
        note,
    );
    Some(Item {
        akev_id: ext_field(ext, "id"),
        start: start.0,
        record: NewRecord::from_detail(detail),
    })
}

fn parse_tunnel(v: &Value) -> Option<Item> {
    let o = v.as_object()?;
    let ext = obj(o.get("_akev"));
    let start = parse_start(o.get("startedDateTime")).unwrap_or_else(UnixMicros::now);
    let host = str_of(o.get("hostname")).unwrap_or("").to_ascii_lowercase();
    let port = num(o.get("port")).map_or(0, |p| p as u16);
    let messages = parse_messages(arr(o.get("messages")), start);
    let scheme: String = ext_field(ext, "scheme")
        .unwrap_or_else(|| if port == 443 { "https" } else { "tcp" }.into());
    let mut detail_tunnel: Option<TunnelStats> = ext_field(ext, "tunnel");
    if detail_tunnel.is_none() {
        let sum = |d: Direction| {
            messages
                .iter()
                .filter(|m| m.message.direction == d)
                .map(|m| m.payload.len() as u64)
                .sum()
        };
        detail_tunnel = Some(TunnelStats {
            bytes_sent: sum(Direction::Sent),
            bytes_received: sum(Direction::Received),
            reason: "imported".into(),
        });
    }
    let close = ext_field::<StreamClose>(ext, "close").or_else(|| {
        num(o.get("time")).map(|t| StreamClose {
            code: None,
            reason: None,
            by: None,
            at: t.max(0.0),
        })
    });
    let request = RequestHead {
        method: ext_field(ext, "method").unwrap_or_default(),
        url: format!("{scheme}://{}", util::authority(&scheme, &host, port)),
        scheme,
        host,
        port,
        path: String::new(),
        http_version: HttpVersion::default(),
        headers: Headers::new(),
    };
    let timing = ext_field::<Timing>(ext, "timing").unwrap_or(Timing {
        start,
        ..Timing::default()
    });
    let state = ext_field(ext, "state").unwrap_or(ExchangeState::Complete);
    let client = client_from(o, ext);
    let note = ext_field(ext, "note");
    let mut detail = detail_skeleton(
        ExchangeKind::Tunnel,
        state,
        request,
        timing,
        client,
        ext,
        None,
        note,
    );
    detail.tunnel = detail_tunnel;
    detail.summary.pinned |= truthy(o.get("pinned"));
    let mut record = NewRecord::from_detail(detail);
    record.messages = messages;
    record.close = close;
    Some(Item {
        akev_id: ext_field(ext, "id"),
        start: start.0,
        record,
    })
}

fn parse_client_error(v: &Value) -> Option<Item> {
    let o = v.as_object()?;
    let ext = obj(o.get("_akev"));
    let start = parse_start(o.get("startedDateTime")).unwrap_or_else(UnixMicros::now);
    let raw = str_of(o.get("rawPrefix"))
        .and_then(b64_decode)
        .unwrap_or_default();
    let client = client_from(o, ext);
    let error = ext_field::<ExchangeError>(ext, "error")
        .or_else(|| {
            o.get("error")
                .and_then(|v| ExchangeError::deserialize(v).ok())
        })
        .unwrap_or(ExchangeError {
            kind: ErrorKind::ClientProtocolError,
            message: "The client sent an unparseable request".into(),
            code: None,
        });
    let (method, parts, url) = guess_request(&raw, client.tls.is_some());
    let parts = parts.unwrap_or(util::UrlParts {
        scheme: String::new(),
        host: String::new(),
        port: 0,
        path: String::new(),
    });
    let request = RequestHead {
        method,
        url,
        scheme: parts.scheme,
        host: parts.host,
        port: parts.port,
        path: parts.path,
        http_version: HttpVersion::default(),
        headers: Headers::new(),
    };
    let timing = ext_field::<Timing>(ext, "timing").unwrap_or(Timing {
        start,
        ..Timing::default()
    });
    let state = ext_field(ext, "state").unwrap_or(ExchangeState::Aborted);
    let note = ext_field(ext, "note");
    let detail = detail_skeleton(
        ExchangeKind::ClientError,
        state,
        request,
        timing,
        client,
        ext,
        Some(error),
        note,
    );
    let mut record = NewRecord::from_detail(detail);
    record.request_body = CapturedBody::complete(Bytes::from(raw));
    Some(Item {
        akev_id: ext_field(ext, "id"),
        start: start.0,
        record,
    })
}

/// The `log` arrays that hold importable items, with their parsers.
const ITEM_GROUPS: [(&str, ItemParser); 4] = [
    ("entries", parse_entry),
    ("_tlsErrors", parse_tls_error),
    ("_rawTunnels", parse_tunnel),
    ("_clientErrors", parse_client_error),
];

/// The items parsed from one `log` array.
#[derive(Default)]
struct GroupItems {
    items: Vec<Item>,
    total: usize,
    skipped: usize,
}

/// What the streaming parse of a HAR document collected.
#[derive(Default)]
struct Collected {
    /// Indexed like [`ITEM_GROUPS`].
    groups: [GroupItems; 4],
    /// A `log` object was found.
    has_log: bool,
}

/// Root object: finds `log` (the last one wins, like a parsed JSON object), skips the rest.
struct RootVisitor<'a>(&'a mut Collected);

impl<'de> serde::de::Visitor<'de> for RootVisitor<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a HAR document")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if key == "log" {
                *self.0 = Collected::default();
                map.next_value_seed(LogSeed(self.0))?;
            } else {
                map.next_value::<serde::de::IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

/// The `log` object: streams the item arrays (the last of a repeated key wins).
struct LogSeed<'a>(&'a mut Collected);

impl<'de> serde::de::DeserializeSeed<'de> for LogSeed<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> serde::de::Visitor<'de> for LogSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a `log` object")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<(), A::Error> {
        self.0.has_log = true;
        while let Some(key) = map.next_key::<String>()? {
            match ITEM_GROUPS.iter().position(|(k, _)| *k == key) {
                Some(i) => {
                    let group = &mut self.0.groups[i];
                    *group = GroupItems::default();
                    map.next_value_seed(ItemsSeed {
                        key: ITEM_GROUPS[i].0,
                        parse_one: ITEM_GROUPS[i].1,
                        group,
                    })?;
                }
                None => {
                    map.next_value::<serde::de::IgnoredAny>()?;
                }
            }
        }
        Ok(())
    }
}

/// One item array: each item is parsed into a JSON value, imported (or skipped) and dropped
/// before the next one is read, so only one item's tree is in memory at a time. Anything but
/// an array counts as no items.
struct ItemsSeed<'a> {
    key: &'static str,
    parse_one: ItemParser,
    group: &'a mut GroupItems,
}

impl<'de> serde::de::DeserializeSeed<'de> for ItemsSeed<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for ItemsSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an array of HAR items")
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            self.group.total += 1;
            // A bug in an item parser must cost that item, not the whole import (invalid
            // items are skipped, as documented).
            let parse_one = self.parse_one;
            let parsed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_one(&v)))
                .unwrap_or_else(|_| {
                    tracing::warn!(group = self.key, "HAR item parser panicked; item skipped");
                    None
                });
            match parsed {
                Some(item) => self.group.items.push(item),
                None => self.group.skipped += 1,
            }
        }
        Ok(())
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut map: A,
    ) -> std::result::Result<(), A::Error> {
        while map
            .next_entry::<serde::de::IgnoredAny, serde::de::IgnoredAny>()?
            .is_some()
        {}
        Ok(())
    }

    fn visit_bool<E>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }

    fn visit_i64<E>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }

    fn visit_u64<E>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }

    fn visit_f64<E>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }

    fn visit_str<E>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }

    fn visit_unit<E>(self) -> std::result::Result<(), E> {
        Ok(())
    }
}

struct RootSeed<'a>(&'a mut Collected);

impl<'de> serde::de::DeserializeSeed<'de> for RootSeed<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_map(RootVisitor(self.0))
    }
}

/// Parse a HAR document into records ready for insertion (in display order).
///
/// The document is streamed: items are parsed and imported one at a time, so memory use is
/// the input plus the imported records, not a JSON tree of the whole file.
pub(crate) fn parse(input: &[u8]) -> Result<Vec<NewRecord>> {
    use serde::de::DeserializeSeed;
    let input = input.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(input);
    let missing_log = || StoreError::InvalidFile("not a HAR file: missing `log` object".into());
    let mut collected = Collected::default();
    let mut de = serde_json::Deserializer::from_slice(input);
    let streamed = RootSeed(&mut collected)
        .deserialize(&mut de)
        .and_then(|()| de.end());
    match streamed {
        Ok(()) => {}
        // Valid JSON of the wrong shape (not an object, or `log` not an object).
        Err(e) if e.classify() == serde_json::error::Category::Data => return Err(missing_log()),
        Err(e) => {
            return Err(StoreError::InvalidFile(format!(
                "HAR is not valid JSON: {e}"
            )));
        }
    }
    if !collected.has_log {
        return Err(missing_log());
    }

    let total: usize = collected.groups.iter().map(|g| g.total).sum();
    let skipped: usize = collected.groups.iter().map(|g| g.skipped).sum();
    let mut items: Vec<Item> = collected.groups.into_iter().flat_map(|g| g.items).collect();
    if skipped > 0 {
        tracing::warn!(skipped, total, "skipped invalid HAR entries");
        if items.is_empty() {
            return Err(StoreError::InvalidFile(format!(
                "none of the {total} HAR entries could be imported"
            )));
        }
    }
    // Akev exports carry original ids: restore that exact order. Otherwise order items
    // chronologically (stable, so same-instant items keep file order).
    if items.iter().all(|i| i.akev_id.is_some()) {
        items.sort_by_key(|i| i.akev_id);
    } else {
        items.sort_by_key(|i| i.start);
    }
    Ok(items.into_iter().map(|i| i.record).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert_eq!(parse_version(Some("HTTP/1.1")), Some(HttpVersion::Http11));
        assert_eq!(parse_version(Some("http/2.0")), Some(HttpVersion::H2));
        assert_eq!(parse_version(Some("h2")), Some(HttpVersion::H2));
        assert_eq!(parse_version(Some("HTTP/2")), Some(HttpVersion::H2));
        assert_eq!(parse_version(Some("h3-29")), Some(HttpVersion::H3));
        assert_eq!(parse_version(Some("1.0")), Some(HttpVersion::Http10));
        assert_eq!(parse_version(Some("unknown")), None);
        let mut h = Headers::new();
        h.append(":authority", "x");
        assert_eq!(version_or_guess(None, &h), HttpVersion::H2);
        assert_eq!(
            version_or_guess(Some(""), &Headers::new()),
            HttpVersion::Http11
        );
    }

    #[test]
    fn dates() {
        let z = parse_start(Some(&json!("2024-01-02T03:04:05.123Z"))).unwrap();
        assert_eq!(
            parse_start(Some(&json!("2024-01-02T04:04:05.1230000+01:00"))),
            Some(z)
        );
        assert_eq!(
            parse_start(Some(&json!("2024-01-02T04:04:05.123+0100"))),
            Some(z)
        );
        assert_eq!(
            parse_start(Some(&json!("2024-01-02T03:04:05.123"))),
            Some(z)
        );
        assert_eq!(
            parse_start(Some(&json!("2024-01-02 03:04:05.123Z"))),
            Some(z)
        );
        assert_eq!(parse_start(Some(&json!("yesterday"))), None);
        assert_eq!(parse_start(Some(&json!(12))), None);
        // A multi-byte character where the `+HHMM` offset would be: no panic, just no date.
        assert_eq!(parse_start(Some(&json!("2026-01-01T00:00:00é1234"))), None);
        assert_eq!(parse_start(Some(&json!("ééé"))), None);
        assert_eq!(parse_start(Some(&json!("2026-01-01T00:00:00+é"))), None);
    }

    #[test]
    fn a_bad_start_date_skips_only_that_entry() {
        let har = json!({"log": {"entries": [
            {"startedDateTime": "2026-01-01T00:00:00é1234",
             "request": {"method": "GET", "url": "https://a.example/bad"},
             "response": {"status": 200}},
            {"startedDateTime": "2026-01-01T00:00:00Z",
             "request": {"method": "GET", "url": "https://a.example/good"},
             "response": {"status": 200}}
        ]}});
        let records = parse(&serde_json::to_vec(&har).unwrap()).unwrap();
        // The unusual date falls back to "now"; both entries survive.
        assert_eq!(records.len(), 2);
        assert!(
            records
                .iter()
                .any(|r| r.detail.request.url == "https://a.example/good")
        );
    }

    #[test]
    fn timings_roundtrip_shape() {
        let t = Timing {
            start: UnixMicros(1),
            request_headers: Some(1.0),
            request_body: Some(2.0),
            dns_duration: Some(3.0),
            connect_duration: Some(4.0),
            tls_duration: Some(5.0),
            request_sent: Some(20.0),
            response_headers: Some(50.0),
            response_body: Some(70.0),
            aborted: None,
            paused_duration: None,
        };
        let (h, total) = har_timings(&t);
        assert_eq!((h.dns, h.connect, h.ssl), (3.0, 9.0, 5.0));
        assert_eq!((h.send, h.wait, h.receive), (8.0, 30.0, 20.0));
        assert_eq!(total, 70.0);
        // Nothing known: no negatives in send/wait/receive.
        let (h, total) = har_timings(&Timing::default());
        assert_eq!(
            (h.dns, h.connect, h.ssl, h.send, h.wait, h.receive),
            (-1.0, -1.0, -1.0, 0.0, 0.0, 0.0)
        );
        assert_eq!(total, 0.0);
        // Aborted before any response.
        let t = Timing {
            request_body: Some(5.0),
            aborted: Some(12.0),
            ..Timing::default()
        };
        let (h, total) = har_timings(&t);
        assert_eq!((h.send, h.wait, h.receive, total), (5.0, 7.0, 0.0, 12.0));
    }

    #[test]
    fn encoding_rule() {
        let mut h = Headers::new();
        h.append("Content-Encoding", "gzip");
        // Decoded text with a gzip header: header dropped.
        assert!(!resolve_encoding(&mut h, b"{\"a\":1}", false, None));
        assert!(!h.contains("content-encoding"));
        // Base64 bytes with gzip magic: kept encoded.
        let mut h = Headers::new();
        h.append("content-encoding", "gzip");
        assert!(resolve_encoding(&mut h, &[0x1f, 0x8b, 8, 0], true, None));
        assert!(h.contains("content-encoding"));
        // Explicit marker wins.
        assert!(resolve_encoding(&mut h, b"plain", false, Some(true)));
        // Nothing captured: headers untouched.
        assert!(resolve_encoding(&mut h, b"", false, None));
        assert!(h.contains("content-encoding"));
        // zlib header check.
        assert!(looks_encoded(&["deflate".into()], &[0x78, 0x9c]));
        assert!(!looks_encoded(&["deflate".into()], b"{}"));
    }

    #[test]
    fn export_decoding_is_capped_per_export() {
        let gz = |n: usize| {
            let c =
                akev_content::decode::encode_body(&vec![b'x'; n], &["gzip".to_string()]).unwrap();
            CapturedBody::complete(c)
        };
        let gzip = vec!["gzip".to_string()];
        let mut budget = ExportBudget { decoded_left: 100 };
        // Fits: decoded, and the allowance shrinks.
        let small = decode_for_export(&gz(60), &gzip, &mut budget);
        assert!(!small.still_encoded);
        assert_eq!(small.bytes.len(), 60);
        assert_eq!(budget.decoded_left, 40);
        // Doesn't fit what's left: exported still encoded (the raw bytes).
        let body = gz(50);
        let big = decode_for_export(&body, &gzip, &mut budget);
        assert!(big.still_encoded);
        assert_eq!(big.bytes, body.bytes);
        assert_eq!(budget.decoded_left, 40);
        assert_eq!(encoded_flag(&gzip, &body, &big), Some(true));
        assert_eq!(encoded_flag(&gzip, &body, &small), Some(false));
        assert_eq!(encoded_flag(&[], &body, &small), None);
        assert_eq!(encoded_flag(&gzip, &CapturedBody::empty(), &small), None);
        // The defaults.
        assert_eq!(
            ExportBudget::default().decoded_left,
            EXPORT_MAX_DECODE_TOTAL
        );
        const { assert!(EXPORT_MAX_DECODE <= 256 * 1024 * 1024) };
    }

    #[test]
    fn heavy_escaping_detection() {
        assert!(!escapes_heavily("plain text\n\twith\r\nbreaks"));
        assert!(!escapes_heavily(""));
        assert!(escapes_heavily("\0\0\0\0"));
        assert!(!escapes_heavily(&format!("{}\u{1b}", "x".repeat(100))));
        assert!(!escapes_heavily(&"\u{1b}[31mred\u{1b}[0m ".repeat(20)));
        assert!(escapes_heavily(&"x\u{1}".repeat(50)));
    }

    #[test]
    fn header_names_with_control_characters_are_dropped() {
        let h = parse_headers(Some(&json!([
            {"name": "X-Ok", "value": "1"},
            {"name": "X-Bad\nfetch('https://evil.example')//", "value": "2"},
            {"name": "X-Nul\u{0}", "value": "3"},
            {"name": ":authority", "value": "a.example"},
            {"name": "Set-Cookie", "value": "a=1\nb=2"}
        ])));
        let names: Vec<&str> = h.iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["X-Ok", ":authority", "Set-Cookie"]);
        // Values keep their line breaks (Chrome joins repeated Set-Cookie values with `\n`).
        assert_eq!(h.get("set-cookie"), Some("a=1\nb=2"));
    }

    #[test]
    fn lenient_base64() {
        assert_eq!(b64_decode("aGVsbG8").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVs\nbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("-_8").unwrap(), vec![0xfb, 0xff]);
        assert!(b64_decode("!!!").is_none());
    }

    #[test]
    fn chrome_error_mapping() {
        assert_eq!(
            error_kind_from_text("net::ERR_NAME_NOT_RESOLVED"),
            ErrorKind::DnsFailed
        );
        assert_eq!(
            error_kind_from_text("net::ERR_CONNECTION_REFUSED"),
            ErrorKind::ConnectionRefused
        );
        assert_eq!(
            error_kind_from_text("net::ERR_CERT_DATE_INVALID"),
            ErrorKind::UpstreamCertInvalid
        );
        assert_eq!(
            error_kind_from_text("net::ERR_ABORTED"),
            ErrorKind::ClientAborted
        );
        assert_eq!(error_kind_from_text("weird"), ErrorKind::Internal);
    }

    #[test]
    fn rejects_non_har() {
        assert!(matches!(
            parse(b"not json"),
            Err(StoreError::InvalidFile(_))
        ));
        assert!(matches!(
            parse(b"{\"foo\":1}"),
            Err(StoreError::InvalidFile(_))
        ));
        assert!(parse(b"{\"log\":{}}").unwrap().is_empty());
        assert!(
            parse(b"\xEF\xBB\xBF{\"log\":{\"entries\":[]}}")
                .unwrap()
                .is_empty()
        );
        let all_bad = br#"{"log":{"entries":[{"request":{"url":"::bad"}}]}}"#;
        assert!(matches!(parse(all_bad), Err(StoreError::InvalidFile(_))));
    }

    #[test]
    fn streamed_parse_keeps_the_document_semantics() {
        let msg = |input: &[u8]| match parse(input) {
            Err(StoreError::InvalidFile(m)) => m,
            other => panic!("expected an error, got {:?}", other.map(|r| r.len())),
        };
        let missing = "not a HAR file: missing `log` object";
        assert_eq!(msg(b"[1, 2]"), missing);
        assert_eq!(msg(b"\"log\""), missing);
        assert_eq!(msg(br#"{"log": 5}"#), missing);
        assert_eq!(msg(br#"{"log": [{"entries": []}]}"#), missing);
        assert!(msg(br#"{"log": {}} trailing"#).starts_with("HAR is not valid JSON"));
        assert!(msg(br#"{"log": {"entries": [{"#).starts_with("HAR is not valid JSON"));
        // Non-array groups hold no items; unknown keys are skipped whatever they hold.
        assert!(
            parse(
                br#"{"x": [{"y": 1}], "log": {"entries": {"a": [1]}, "_tlsErrors": 3, "z": {}}}"#
            )
            .unwrap()
            .is_empty()
        );
        // Like a parsed JSON object, the last of a repeated key wins.
        let entry = r#"{"request": {"url": "https://a.example/"}, "response": {"status": 200}}"#;
        let twice = format!(r#"{{"log": {{"entries": [{entry}]}}, "log": {{"entries": []}}}}"#);
        assert!(parse(twice.as_bytes()).unwrap().is_empty());
        let groups = format!(r#"{{"log": {{"entries": [], "entries": [{entry}, {entry}]}}}}"#);
        assert_eq!(parse(groups.as_bytes()).unwrap().len(), 2);
    }
}
