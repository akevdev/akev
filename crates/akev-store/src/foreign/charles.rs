//! Charles Proxy JSON sessions (`.chlsj`, *File → Export Session… → JSON Session File*).
//!
//! The file is a JSON array with one object per transaction:
//!
//! ```json
//! [{
//!   "status": "COMPLETE", "method": "GET", "protocolVersion": "HTTP/1.1",
//!   "scheme": "https", "host": "api.example.com", "actualPort": 443,
//!   "path": "/users", "query": "page=2", "tunnel": false, "webSocket": false,
//!   "remoteAddress": "api.example.com/93.184.216.34", "clientAddress": "/127.0.0.1",
//!   "clientPort": 52178,
//!   "times": { "start": "2024-03-04T10:21:36.512+01:00", "requestBegin": "…",
//!              "requestComplete": "…", "responseBegin": "…", "end": "…" },
//!   "durations": { "total": 191, "dns": 3, "connect": 17, "ssl": 29, "request": 0,
//!                  "response": 2, "latency": 138 },
//!   "ssl": { "protocol": "TLSv1.3", "cipherSuite": "TLS_AES_128_GCM_SHA256" },
//!   "alpn": { "protocol": "h2" },
//!   "request": { "sizes": { "headers": 179, "body": 0 }, "mimeType": null,
//!                "header": { "firstLine": "GET /users?page=2 HTTP/1.1",
//!                            "headers": [{ "name": "Host", "value": "api.example.com" }] } },
//!   "response": { "status": 200, "sizes": { "headers": 361, "body": 1397 },
//!                 "contentEncoding": "gzip", "header": { "firstLine": "HTTP/1.1 200 OK", … },
//!                 "body": { "text": "{…}", "charset": "utf-8", "decoded": true } }
//! }]
//! ```
//!
//! * Bodies are `{"text": …}` (decoded with the charset) or `{"encoded": "<base64>"}`;
//!   `"decoded": true` means Charles removed the content-coding (the `Content-Encoding`
//!   header is then dropped, see the module docs of `foreign`).
//! * `status` other than `COMPLETE` (`FAILED`, `RECEIVING_RESPONSE`...) imports as aborted,
//!   with `errorMessage` mapped to an error kind.
//! * `CONNECT` transactions (`"tunnel": true`, hosts without SSL proxying) become tunnel rows.
//! * WebSocket transactions (`"webSocket": true`) keep their `messages` when present.
//! * HTTP/2 pseudo-headers are dropped (the request line already carries them).
//! * `notes` / `comment` become the exchange note.
//!
//! The array is streamed: one transaction's JSON tree is in memory at a time. Invalid
//! transactions are skipped (the import fails only when none is usable).

use super::{Draft, body, build_url, default_port, error_kind_for, is_pseudo_header, offset_ms};
use crate::har::{b64_decode, parse_headers, parse_start, parse_version, resolve_encoding};
use crate::{MessageWithPayload, NewRecord, Result, Store, StoreError};
use akev_core::*;
use bytes::Bytes;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

type Obj = Map<String, Value>;

fn obj(v: Option<&Value>) -> Option<&Obj> {
    v.and_then(Value::as_object)
}

fn str_of(v: Option<&Value>) -> Option<&str> {
    v.and_then(Value::as_str)
}

/// A number, also accepting numeric strings.
fn num(v: Option<&Value>) -> Option<f64> {
    match v? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
    .filter(|n| n.is_finite())
}

/// Charles writes `-1` (or `null`) for durations that don't apply.
fn non_negative(v: Option<&Value>) -> Option<f64> {
    num(v).filter(|n| *n >= 0.0)
}

fn truthy(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Bool(true)))
}

/// One imported transaction with its sort key.
struct Item {
    start: i64,
    record: NewRecord,
}

#[derive(Default)]
struct Collected {
    items: Vec<Item>,
    total: usize,
}

struct TransactionsSeed<'a>(&'a mut Collected);

impl<'de> DeserializeSeed<'de> for TransactionsSeed<'_> {
    type Value = ();

    fn deserialize<D: serde::Deserializer<'de>>(self, d: D) -> std::result::Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for TransactionsSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an array of Charles transactions")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<(), A::Error> {
        while let Some(v) = seq.next_element::<Value>()? {
            self.0.total += 1;
            let parsed =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parse_transaction(&v)))
                    .unwrap_or_else(|_| {
                        tracing::warn!("Charles transaction parser panicked; transaction skipped");
                        None
                    });
            if let Some(item) = parsed {
                self.0.items.push(item);
            }
        }
        Ok(())
    }

    /// Some tools wrap the array: `{"transactions": […]}` / `{"entries": […]}`.
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> std::result::Result<(), A::Error> {
        while let Some(key) = map.next_key::<String>()? {
            if matches!(key.as_str(), "transactions" | "entries" | "items") {
                map.next_value_seed(TransactionsSeed(&mut *self.0))?;
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(())
    }
}

/// Parse a Charles JSON session into records ready for insertion (chronological order).
pub(crate) fn parse(input: &[u8]) -> Result<Vec<NewRecord>> {
    let input = input.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(input);
    let mut collected = Collected::default();
    let mut de = serde_json::Deserializer::from_slice(input);
    let result = TransactionsSeed(&mut collected)
        .deserialize(&mut de)
        .and_then(|()| de.end());
    match result {
        Ok(()) => {}
        Err(e) if e.classify() == serde_json::error::Category::Data => {
            return Err(StoreError::InvalidFile(
                "not a Charles JSON session: expected an array of transactions".into(),
            ));
        }
        Err(e) => {
            return Err(StoreError::InvalidFile(format!(
                "Charles session is not valid JSON: {e}"
            )));
        }
    }
    let Collected { mut items, total } = collected;
    let skipped = total - items.len();
    if skipped > 0 {
        tracing::warn!(skipped, total, "skipped invalid Charles transactions");
    }
    if items.is_empty() && total > 0 {
        return Err(StoreError::InvalidFile(format!(
            "none of the {total} Charles transactions could be imported"
        )));
    }
    if total == 0 && !looks_like_array(input) {
        return Err(StoreError::InvalidFile(
            "not a Charles JSON session: expected an array of transactions".into(),
        ));
    }
    items.sort_by_key(|i| i.start);
    Ok(items.into_iter().map(|i| i.record).collect())
}

fn looks_like_array(input: &[u8]) -> bool {
    input
        .iter()
        .find(|b| !b.is_ascii_whitespace())
        .is_some_and(|b| *b == b'[')
}

/// `header.firstLine` (`GET /x HTTP/1.1`, `HTTP/1.1 200 OK`).
fn first_line(message: Option<&Obj>) -> Option<&str> {
    message
        .and_then(|m| obj(m.get("header")))
        .and_then(|h| str_of(h.get("firstLine")))
}

/// Headers of a request/response (`header.headers`), pseudo-headers dropped.
fn message_headers(message: Option<&Obj>) -> Headers {
    let raw = message
        .and_then(|m| obj(m.get("header")))
        .and_then(|h| h.get("headers"));
    let mut headers = parse_headers(raw);
    headers.0.retain(|(name, _)| !is_pseudo_header(name));
    headers
}

/// Body bytes of a request/response and how to treat them: `(bytes, was_base64, decoded)`.
fn message_body(message: Option<&Obj>) -> Option<(Vec<u8>, bool, bool)> {
    let b = obj(message?.get("body"))?;
    let decoded = truthy(b.get("decoded"));
    if let Some(encoded) = str_of(b.get("encoded")) {
        return Some((b64_decode(encoded)?, true, decoded));
    }
    if let Some(text) = str_of(b.get("text")) {
        return Some((text.as_bytes().to_vec(), false, decoded));
    }
    None
}

/// Captured body (resolving the Content-Encoding rule) for one side.
fn side_body(message: Option<&Obj>, headers: &mut Headers) -> CapturedBody {
    let wire = message
        .and_then(|m| obj(m.get("sizes")))
        .and_then(|s| non_negative(s.get("body")))
        .map(|n| n as u64);
    match message_body(message) {
        Some((bytes, was_base64, decoded)) => {
            let explicit = decoded.then_some(false);
            let declared = !headers.content_encodings().is_empty();
            let still_encoded = resolve_encoding(headers, &bytes, was_base64, explicit);
            // The kept bytes are the wire bytes unless a declared coding was removed.
            let raw = still_encoded || !declared;
            body(bytes, wire, raw)
        }
        None => body(Vec::new(), wire, true),
    }
}

/// `"api.example.com/93.184.216.34"` / `"/127.0.0.1"` → the IP part.
fn address_ip(raw: &str) -> Option<String> {
    let ip = raw.rsplit('/').next()?.trim();
    let ip = ip.trim_start_matches('[').trim_end_matches(']');
    ip.parse::<std::net::IpAddr>().ok().map(|ip| ip.to_string())
}

fn parse_transaction(v: &Value) -> Option<Item> {
    let t = v.as_object()?;
    let host = str_of(t.get("host"))?.trim().to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    let method = str_of(t.get("method"))
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .unwrap_or("GET")
        .to_string();
    let tunnel = truthy(t.get("tunnel")) || method.eq_ignore_ascii_case("CONNECT");
    let websocket = truthy(t.get("webSocket"));
    let mut scheme = str_of(t.get("scheme"))
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http".into());
    let port = num(t.get("actualPort"))
        .or_else(|| num(t.get("port")))
        .filter(|p| *p >= 1.0 && *p <= 65535.0)
        .map(|p| p as u16)
        .unwrap_or_else(|| if tunnel { 443 } else { default_port(&scheme) });

    let request_obj = obj(t.get("request"));
    let response_obj = obj(t.get("response"));

    // Timing.
    let times = obj(t.get("times"));
    let time = |k: &str| times.and_then(|o| parse_start(o.get(k)));
    let durations = obj(t.get("durations"));
    let dur = |k: &str| durations.and_then(|d| non_negative(d.get(k)));
    let start = time("start")
        .or_else(|| time("requestBegin"))
        .unwrap_or_else(UnixMicros::now);
    let at = |k: &str| time(k).and_then(|x| offset_ms(start, x));

    let status_text = str_of(t.get("status")).unwrap_or("COMPLETE");
    let complete = status_text.eq_ignore_ascii_case("COMPLETE");
    let error_message = str_of(t.get("errorMessage"))
        .map(str::trim)
        .filter(|m| !m.is_empty());

    let note = str_of(t.get("notes"))
        .or_else(|| str_of(t.get("comment")))
        .map(str::to_string);

    // Client & upstream.
    let mut client = ClientInfo::default();
    if let Some(ip) = str_of(t.get("clientAddress")).and_then(address_ip) {
        let client_port = num(t.get("clientPort")).map_or(0, |p| p as u16);
        client.remote_addr = crate::util::socket_addr_string(&ip, client_port);
    }
    let tls_version = obj(t.get("ssl"))
        .and_then(|s| str_of(s.get("protocol")))
        .map(str::to_string);
    let cipher = obj(t.get("ssl"))
        .and_then(|s| str_of(s.get("cipherSuite")))
        .map(str::to_string);
    let alpn = obj(t.get("alpn"))
        .and_then(|a| str_of(a.get("protocol")))
        .map(str::to_string);
    if tls_version.is_some() || (scheme == "https" && !tunnel) {
        client.tls = Some(TlsInfo {
            sni: Some(host.clone()),
            alpn: alpn.clone(),
            version: tls_version.clone(),
            cipher: cipher.clone(),
            ..TlsInfo::default()
        });
    }
    let remote = str_of(t.get("remoteAddress")).and_then(address_ip);
    let upstream = (remote.is_some() || tls_version.is_some()).then(|| UpstreamInfo {
        remote_addr: remote.map(|ip| crate::util::socket_addr_string(&ip, port)),
        tls_version: tls_version.clone(),
        alpn: alpn.clone(),
        cipher: cipher.clone(),
        ..UpstreamInfo::default()
    });

    if tunnel {
        let scheme = if port == 443 { "https" } else { "tcp" };
        let request = RequestHead {
            method: "CONNECT".into(),
            url: format!("{scheme}://{}", crate::util::authority(scheme, &host, port)),
            scheme: scheme.into(),
            host,
            port,
            path: String::new(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let mut draft = Draft::new(ExchangeKind::Tunnel, request, start);
        let size = |m: Option<&Obj>| {
            m.and_then(|m| obj(m.get("sizes")))
                .and_then(|s| non_negative(s.get("body")))
                .map_or(0, |n| n as u64)
        };
        draft.tunnel = Some(TunnelStats {
            bytes_sent: size(request_obj),
            bytes_received: size(response_obj),
            reason: "not decrypted (Charles)".into(),
        });
        draft.client = client;
        draft.upstream = upstream;
        draft.note = note;
        if let Some(message) = error_message.filter(|_| !complete) {
            draft.error = Some(ExchangeError {
                kind: error_kind_for(message),
                message: message.to_string(),
                code: None,
            });
        }
        draft.timing.response_body = at("end").or_else(|| dur("total"));
        draft.close = draft
            .timing
            .response_body
            .map(|at| crate::record::StreamClose {
                code: None,
                reason: None,
                by: None,
                at,
            });
        return Some(Item {
            start: start.0,
            record: draft.into_record(),
        });
    }

    if websocket && (scheme == "http" || scheme == "https") {
        // Keep http/https: Akev records WebSocket upgrades with the HTTP scheme.
    } else if scheme == "ws" {
        scheme = "http".into();
    } else if scheme == "wss" {
        scheme = "https".into();
    }

    // Request.
    let mut path = str_of(t.get("path")).unwrap_or("/").to_string();
    if !path.starts_with('/') && path != "*" {
        path.insert(0, '/');
    }
    if let Some(query) = str_of(t.get("query")).filter(|q| !q.is_empty()) {
        path.push('?');
        path.push_str(query);
    }
    let mut request_headers = message_headers(request_obj);
    let version_from_line = |line: Option<&str>, last: bool| {
        line.and_then(|l| {
            let mut words = l.split_whitespace();
            if last {
                words.next_back()
            } else {
                words.next()
            }
        })
        .and_then(|w| parse_version(Some(w)))
    };
    let http_version = parse_version(str_of(t.get("protocolVersion")))
        .or_else(|| version_from_line(first_line(request_obj), true))
        .unwrap_or(HttpVersion::Http11);
    let request_body = side_body(request_obj, &mut request_headers);
    let request = RequestHead {
        method,
        url: build_url(&scheme, &host, port, &path),
        scheme: scheme.clone(),
        host,
        port,
        path,
        http_version,
        headers: request_headers,
    };
    let kind = if websocket {
        ExchangeKind::WebSocket
    } else {
        ExchangeKind::Http
    };
    let mut draft = Draft::new(kind, request, start);
    draft.request_body = request_body;
    draft.client = client;
    draft.upstream = upstream;
    draft.note = note;

    // Response.
    let status = response_obj
        .and_then(|r| num(r.get("status")))
        .filter(|s| (100.0..=999.0).contains(s))
        .map(|s| s as u16);
    if let (Some(r), Some(status)) = (response_obj, status) {
        let mut headers = message_headers(Some(r));
        let line = first_line(Some(r));
        let status_message = line
            .and_then(|l| l.trim().splitn(3, ' ').nth(2))
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| canonical_reason(status).to_string());
        let version = version_from_line(line, false).unwrap_or(http_version);
        draft.response_body = side_body(Some(r), &mut headers);
        draft.response = Some(ResponseHead {
            status,
            status_message,
            http_version: version,
            headers,
        });
    }

    // Timing: absolute `times` when present, otherwise reconstructed from `durations`.
    let setup =
        dur("dns").unwrap_or(0.0) + dur("connect").unwrap_or(0.0) + dur("ssl").unwrap_or(0.0);
    let request_sent = at("requestComplete").or_else(|| dur("request").map(|r| setup + r));
    let response_headers = at("responseBegin").or_else(|| {
        request_sent
            .zip(dur("latency"))
            .map(|(sent, latency)| sent + latency)
    });
    let end = at("end").or_else(|| dur("total"));
    let has_response = draft.response.is_some();
    draft.timing = Timing {
        start,
        request_headers: at("requestBegin"),
        request_body: request_sent,
        dns_duration: dur("dns"),
        connect_duration: dur("connect"),
        tls_duration: dur("ssl"),
        request_sent,
        response_headers: response_headers.filter(|_| has_response),
        response_body: end.filter(|_| has_response && complete),
        aborted: None,
        paused_duration: None,
    };
    if !complete || error_message.is_some() {
        let message = error_message.map(str::to_string).unwrap_or_else(|| {
            format!(
                "Charles recorded the transaction as {}",
                status_text.to_ascii_lowercase().replace('_', " ")
            )
        });
        draft.error = Some(ExchangeError {
            kind: error_message.map_or(ErrorKind::ClientAborted, error_kind_for),
            message,
            code: None,
        });
        draft.timing.aborted = end;
    }

    if websocket {
        draft.messages = parse_messages(t.get("messages"), start);
        if let Some(end) = end.filter(|_| complete) {
            draft.close = Some(crate::record::StreamClose {
                code: None,
                reason: None,
                by: None,
                at: end,
            });
        }
    }
    Some(Item {
        start: start.0,
        record: draft.into_record(),
    })
}

/// WebSocket messages of a transaction (tolerant: `type`/`opcode`, `direction`/`from`/
/// `outgoing`, `text`/`data`/`encoded`, ISO or epoch-ms `time`).
fn parse_messages(list: Option<&Value>, start: UnixMicros) -> Vec<MessageWithPayload> {
    let Some(list) = list.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(list.len());
    for m in list {
        let Some(m) = m.as_object() else { continue };
        let sent = if let Some(Value::Bool(b)) = m.get("outgoing") {
            *b
        } else {
            let dir = str_of(m.get("direction"))
                .or_else(|| str_of(m.get("from")))
                .unwrap_or("")
                .to_ascii_lowercase();
            matches!(
                dir.as_str(),
                "send" | "sent" | "outgoing" | "out" | "client" | "request" | "up"
            )
        };
        let kind_text = str_of(m.get("type")).unwrap_or("").to_ascii_lowercase();
        let opcode = num(m.get("opcode")).map(|o| o as i64);
        let kind = match (kind_text.as_str(), opcode) {
            ("binary", _) | (_, Some(2)) => MessageKind::Binary,
            ("close", _) | (_, Some(8)) => MessageKind::Close,
            ("ping", _) | (_, Some(9)) => MessageKind::Ping,
            ("pong", _) | (_, Some(10)) => MessageKind::Pong,
            _ => MessageKind::Text,
        };
        let payload =
            if let Some(encoded) = str_of(m.get("encoded")).or_else(|| str_of(m.get("binary"))) {
                b64_decode(encoded).unwrap_or_default()
            } else {
                str_of(m.get("text"))
                    .or_else(|| str_of(m.get("data")))
                    .unwrap_or("")
                    .as_bytes()
                    .to_vec()
            };
        let at = match m.get("time").or_else(|| m.get("timestamp")) {
            Some(Value::String(_)) => parse_start(m.get("time").or_else(|| m.get("timestamp"))),
            Some(v) => num(Some(v)).map(|ms| UnixMicros((ms * 1000.0).round() as i64)),
            None => None,
        }
        .unwrap_or(start);
        out.push(MessageWithPayload {
            message: StreamMessage {
                seq: u32::try_from(out.len()).unwrap_or(u32::MAX),
                direction: if sent {
                    Direction::Sent
                } else {
                    Direction::Received
                },
                kind,
                at,
                size: payload.len() as u64,
                event: None,
                event_id: None,
                modified: false,
            },
            payload: Bytes::from(payload),
        });
    }
    out
}

impl Store {
    /// Import a Charles Proxy JSON session (`.chlsj`). Returns the new ids.
    pub fn import_charles(&self, input: &[u8]) -> Result<Vec<ExchangeId>> {
        let records = parse(input)?;
        Ok(self.insert_records(records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses() {
        assert_eq!(
            address_ip("api.example.com/93.184.216.34").as_deref(),
            Some("93.184.216.34")
        );
        assert_eq!(address_ip("/127.0.0.1").as_deref(), Some("127.0.0.1"));
        assert_eq!(
            address_ip("localhost/0:0:0:0:0:0:0:1").as_deref(),
            Some("::1")
        );
        assert_eq!(address_ip("nonsense"), None);
    }

    #[test]
    fn rejects_non_sessions() {
        for bad in [&b"{\"log\":{}}"[..], b"42", b"\"x\"", b"[1,2", b"not json"] {
            assert!(parse(bad).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        assert!(parse(b"[]").unwrap().is_empty());
        assert!(parse(b"[{\"nohost\":1}]").is_err());
    }
}
