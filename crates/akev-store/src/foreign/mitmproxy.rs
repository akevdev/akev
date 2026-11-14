//! mitmproxy flow files (`mitmdump -w flows`, *File → Save* in mitmweb / the console).
//!
//! A flow file is a sequence of [tnetstrings](https://tnetstrings.info) (`<len>:<data><type>`;
//! mitmproxy adds `;` for UTF-8 strings), one dict per flow — what `Flow.get_state()`
//! returns:
//!
//! ```text
//! { "version": 21, "type": "http", "id": "…", "comment": "", "marked": ":red_circle:",
//!   "error": null | { "msg": "…", "timestamp": 1.7e9 },
//!   "client_conn": { "peername": ["127.0.0.1", 46858], "tls": true, "sni": "…",
//!                    "alpn": b"h2", "alpn_offers": [b"h2"], "tls_version": "TLSv1.3",
//!                    "cipher": "…", "timestamp_start": 1.7e9, … },
//!   "server_conn": { "id": "…", "peername": ["93.184.216.34", 443], "tls_version": "…",
//!                    "timestamp_start": …, "timestamp_tcp_setup": …,
//!                    "timestamp_tls_setup": …, … },
//!   "request": { "method": b"GET", "scheme": b"https", "host": "example.com", "port": 443,
//!                "authority": b"", "path": b"/x?y=1", "http_version": b"HTTP/1.1",
//!                "headers": [[b"Host", b"example.com"], …], "content": b"…",
//!                "trailers": null, "timestamp_start": …, "timestamp_end": … },
//!   "response": null | { "status_code": 200, "reason": b"OK", "http_version": b"HTTP/1.1",
//!                        "headers": […], "content": b"…", "trailers": null, … },
//!   "websocket": null | { "messages": [[1, true, b"hello", 1.7e9, false, false], …],
//!                         "closed_by_client": true, "close_code": 1000,
//!                         "close_reason": "", "timestamp_end": … } }
//! ```
//!
//! * `content` is the raw message body (still content-encoded), so it is stored as is.
//! * Strings may be bytes or UTF-8 strings (older versions used bytes everywhere); missing
//!   fields of older formats are tolerated (`address` for `peername`, `tls_established`,
//!   boolean `marked`).
//! * `http` flows are imported; `tcp`, `udp` and `dns` flows are skipped.
//! * `comment` becomes the note, `marked` a colour tag (`:red_circle:` → red, other marks →
//!   yellow).
//!
//! Parsing is bounded: nesting depth is limited and every length is checked against the
//! remaining input, so malformed files fail cleanly.

use super::{Draft, build_url, default_port, error_kind_for, is_pseudo_header, offset_ms};
use crate::record::StreamClose;
use crate::{MessageWithPayload, NewRecord, Result, Store, StoreError};
use akev_core::*;
use bytes::Bytes;
use std::collections::HashSet;

/// Deepest nesting accepted (real flows nest about 5 levels).
const MAX_DEPTH: usize = 64;
/// Longest length prefix (digits) accepted.
const MAX_LEN_DIGITS: usize = 12;

/// A parsed tnetstring value borrowing from the input.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TValue<'a> {
    Bytes(&'a [u8]),
    Str(&'a str),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    List(Vec<TValue<'a>>),
    Dict(Vec<(TValue<'a>, TValue<'a>)>),
}

impl<'a> TValue<'a> {
    /// Dict lookup by (bytes or string) key.
    fn get(&self, key: &str) -> Option<&TValue<'a>> {
        let TValue::Dict(items) = self else {
            return None;
        };
        items
            .iter()
            .find(|(k, _)| k.as_bytes() == Some(key.as_bytes()))
            .map(|(_, v)| v)
            .filter(|v| !matches!(v, TValue::Null))
    }

    fn as_bytes(&self) -> Option<&'a [u8]> {
        match self {
            TValue::Bytes(b) => Some(b),
            TValue::Str(s) => Some(s.as_bytes()),
            _ => None,
        }
    }

    /// Text of a bytes or string value (lossy for invalid UTF-8).
    fn text(&self) -> Option<String> {
        self.as_bytes()
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            TValue::Int(i) => Some(*i as f64),
            TValue::Float(f) if f.is_finite() => Some(*f),
            _ => None,
        }
    }

    fn as_i64(&self) -> Option<i64> {
        match self {
            TValue::Int(i) => Some(*i),
            TValue::Float(f) if f.is_finite() => Some(*f as i64),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            TValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    fn as_list(&self) -> &[TValue<'a>] {
        match self {
            TValue::List(items) => items,
            _ => &[],
        }
    }
}

fn invalid(msg: impl Into<String>) -> StoreError {
    StoreError::InvalidFile(msg.into())
}

/// Parse one tnetstring at the start of `input`; returns it and the rest.
pub(crate) fn parse_value(input: &[u8], depth: usize) -> Result<(TValue<'_>, &[u8])> {
    if depth > MAX_DEPTH {
        return Err(invalid("mitmproxy flow nests too deeply"));
    }
    let colon = input
        .iter()
        .take(MAX_LEN_DIGITS + 1)
        .position(|b| *b == b':')
        .ok_or_else(|| invalid("not a mitmproxy flow file (missing length prefix)"))?;
    let digits = &input[..colon];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return Err(invalid("not a mitmproxy flow file (bad length prefix)"));
    }
    let len: usize = std::str::from_utf8(digits)
        .ok()
        .and_then(|d| d.parse().ok())
        .ok_or_else(|| invalid("bad tnetstring length"))?;
    let rest = &input[colon + 1..];
    if rest.len() < len + 1 {
        return Err(invalid("truncated mitmproxy flow file"));
    }
    let (data, rest) = rest.split_at(len);
    let (&tag, rest) = rest
        .split_first()
        .ok_or_else(|| invalid("truncated mitmproxy flow file"))?;
    let text = || std::str::from_utf8(data).map_err(|_| invalid("invalid UTF-8 in tnetstring"));
    let value = match tag {
        b',' => TValue::Bytes(data),
        b';' => TValue::Str(text()?),
        b'#' => TValue::Int(
            text()?
                .parse()
                .map_err(|_| invalid("bad integer in tnetstring"))?,
        ),
        b'^' => TValue::Float(
            text()?
                .parse()
                .map_err(|_| invalid("bad float in tnetstring"))?,
        ),
        b'!' => match data {
            b"true" => TValue::Bool(true),
            b"false" => TValue::Bool(false),
            _ => return Err(invalid("bad boolean in tnetstring")),
        },
        b'~' => {
            if !data.is_empty() {
                return Err(invalid("bad null in tnetstring"));
            }
            TValue::Null
        }
        b']' => {
            let mut items = Vec::new();
            let mut body = data;
            while !body.is_empty() {
                let (item, next) = parse_value(body, depth + 1)?;
                items.push(item);
                body = next;
            }
            TValue::List(items)
        }
        b'}' => {
            let mut items = Vec::new();
            let mut body = data;
            while !body.is_empty() {
                let (key, next) = parse_value(body, depth + 1)?;
                if next.is_empty() {
                    return Err(invalid("tnetstring dict key without value"));
                }
                let (value, next) = parse_value(next, depth + 1)?;
                items.push((key, value));
                body = next;
            }
            TValue::Dict(items)
        }
        other => {
            return Err(invalid(format!(
                "not a mitmproxy flow file (unknown tnetstring type {:?})",
                other as char
            )));
        }
    };
    Ok((value, rest))
}

/// Whether `input` looks like a mitmproxy flow file (a tnetstring dict).
pub(crate) fn sniff(input: &[u8]) -> bool {
    let Some(colon) = input
        .iter()
        .take(MAX_LEN_DIGITS + 1)
        .position(|b| *b == b':')
    else {
        return false;
    };
    let digits = &input[..colon];
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let len: Option<usize> = std::str::from_utf8(digits)
        .ok()
        .and_then(|d| d.parse().ok());
    len.and_then(|len| input.get(colon + 1 + len))
        .is_some_and(|tag| *tag == b'}')
}

fn headers_of(v: Option<&TValue<'_>>) -> Headers {
    let mut out = Headers::new();
    for pair in v.map(TValue::as_list).unwrap_or_default() {
        let [name, value] = pair.as_list() else {
            continue;
        };
        let (Some(name), Some(value)) = (name.text(), value.text()) else {
            continue;
        };
        if name.is_empty() || name.chars().any(char::is_control) || is_pseudo_header(&name) {
            continue;
        }
        out.append(name, value);
    }
    out
}

fn seconds_to_micros(s: f64) -> UnixMicros {
    UnixMicros((s * 1_000_000.0).round() as i64)
}

fn time_of(v: Option<&TValue<'_>>) -> Option<UnixMicros> {
    v.and_then(TValue::as_f64)
        .filter(|s| *s > 0.0)
        .map(seconds_to_micros)
}

/// `["127.0.0.1", 46858]` → `127.0.0.1:46858`.
fn address_of(v: Option<&TValue<'_>>) -> Option<(String, u16)> {
    let [host, port] = v?.as_list() else {
        return None;
    };
    let host = host.text()?;
    let port = u16::try_from(port.as_i64()?).ok()?;
    Some((host, port))
}

fn tls_of(conn: Option<&TValue<'_>>) -> bool {
    let Some(conn) = conn else { return false };
    conn.get("tls")
        .or_else(|| conn.get("tls_established"))
        .and_then(TValue::as_bool)
        .unwrap_or(false)
}

fn colour_of(marked: Option<&TValue<'_>>) -> Option<String> {
    let mark = match marked? {
        TValue::Bool(true) => ":default:".to_string(),
        v => v.text().filter(|m| !m.is_empty())?,
    };
    let lower = mark.to_ascii_lowercase();
    let colour = ["red", "orange", "yellow", "green", "blue", "purple"]
        .into_iter()
        .find(|c| lower.contains(c))
        .unwrap_or(if lower.contains("white") || lower.contains("black") {
            "grey"
        } else {
            "yellow"
        });
    Some(colour.to_string())
}

/// Convert one flow. `seen_conns` remembers server connections already used by an earlier
/// flow (their setup timings belong to that flow only).
fn convert_flow(flow: &TValue<'_>, seen_conns: &mut HashSet<String>) -> Option<(i64, NewRecord)> {
    if flow.get("type").and_then(TValue::text).as_deref() != Some("http") {
        return None;
    }
    let req = flow.get("request")?;
    let method = req.get("method").and_then(TValue::text)?.trim().to_string();
    if method.is_empty() {
        return None;
    }
    let scheme = req
        .get("scheme")
        .and_then(TValue::text)
        .map(|s| s.to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "http".into());
    let host = req
        .get("host")
        .and_then(TValue::text)
        .map(|h| {
            h.trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_ascii_lowercase()
        })
        .filter(|h| !h.is_empty())?;
    let port = req
        .get("port")
        .and_then(TValue::as_i64)
        .and_then(|p| u16::try_from(p).ok())
        .filter(|p| *p != 0)
        .unwrap_or_else(|| default_port(&scheme));
    let mut path = req.get("path").and_then(TValue::text).unwrap_or_default();
    if path.is_empty() {
        path = "/".into();
    } else if !path.starts_with('/') && path != "*" {
        path.insert(0, '/');
    }
    let http_version = req
        .get("http_version")
        .and_then(TValue::text)
        .and_then(|v| crate::har::parse_version(Some(&v)))
        .unwrap_or(HttpVersion::Http11);
    let start = time_of(req.get("timestamp_start"))
        .or_else(|| time_of(flow.get("timestamp_created")))
        .unwrap_or_else(UnixMicros::now);
    let at = |v: Option<&TValue<'_>>| time_of(v).and_then(|t| offset_ms(start, t));

    let websocket = flow.get("websocket");
    let kind = if websocket.is_some() {
        ExchangeKind::WebSocket
    } else {
        ExchangeKind::Http
    };
    let request = RequestHead {
        method,
        url: build_url(&scheme, &host, port, &path),
        scheme,
        host: host.clone(),
        port,
        path,
        http_version,
        headers: headers_of(req.get("headers")),
    };
    let mut draft = Draft::new(kind, request, start);
    let req_content = req
        .get("content")
        .and_then(TValue::as_bytes)
        .unwrap_or_default();
    draft.request_body = CapturedBody::complete(Bytes::copy_from_slice(req_content));
    draft.request_trailers = req
        .get("trailers")
        .map(|t| headers_of(Some(t)))
        .filter(|t| !t.is_empty());

    if let Some(resp) = flow.get("response")
        && let Some(status) = resp
            .get("status_code")
            .and_then(TValue::as_i64)
            .and_then(|s| u16::try_from(s).ok())
            .filter(|s| (100..=999).contains(s))
    {
        let content = resp
            .get("content")
            .and_then(TValue::as_bytes)
            .unwrap_or_default();
        draft.response_body = CapturedBody::complete(Bytes::copy_from_slice(content));
        draft.response_trailers = resp
            .get("trailers")
            .map(|t| headers_of(Some(t)))
            .filter(|t| !t.is_empty());
        draft.response = Some(ResponseHead {
            status,
            status_message: resp
                .get("reason")
                .and_then(TValue::text)
                .filter(|r| !r.is_empty())
                .unwrap_or_else(|| canonical_reason(status).to_string()),
            http_version: resp
                .get("http_version")
                .and_then(TValue::text)
                .and_then(|v| crate::har::parse_version(Some(&v)))
                .unwrap_or(http_version),
            headers: headers_of(resp.get("headers")),
        });
        draft.timing.response_headers = at(resp.get("timestamp_start"));
        draft.timing.response_body = at(resp.get("timestamp_end"));
    }
    draft.timing.request_body = at(req.get("timestamp_end"));
    draft.timing.request_sent = draft.timing.request_body;

    // Connections.
    let client_conn = flow.get("client_conn");
    if let Some(conn) = client_conn {
        if let Some((ip, port)) =
            address_of(conn.get("peername")).or_else(|| address_of(conn.get("address")))
        {
            draft.client.remote_addr = crate::util::socket_addr_string(&ip, port);
        }
        if tls_of(Some(conn)) {
            draft.client.tls = Some(TlsInfo {
                sni: conn.get("sni").and_then(TValue::text),
                alpn_offered: conn
                    .get("alpn_offers")
                    .map(TValue::as_list)
                    .unwrap_or_default()
                    .iter()
                    .filter_map(TValue::text)
                    .collect(),
                alpn: conn
                    .get("alpn")
                    .and_then(TValue::text)
                    .filter(|a| !a.is_empty()),
                version: conn.get("tls_version").and_then(TValue::text),
                cipher: conn.get("cipher").and_then(TValue::text),
                ..TlsInfo::default()
            });
        }
    }
    if let Some(conn) = flow.get("server_conn") {
        let peer = address_of(conn.get("peername")).or_else(|| address_of(conn.get("ip_address")));
        let tls = tls_of(Some(conn));
        if peer.is_some() || tls {
            draft.upstream = Some(UpstreamInfo {
                remote_addr: peer.map(|(ip, port)| crate::util::socket_addr_string(&ip, port)),
                tls_version: conn.get("tls_version").and_then(TValue::text),
                alpn: conn
                    .get("alpn")
                    .and_then(TValue::text)
                    .filter(|a| !a.is_empty()),
                cipher: conn.get("cipher").and_then(TValue::text),
                ..UpstreamInfo::default()
            });
        }
        // Connection setup timings belong to the first flow that used the connection.
        let conn_id = conn.get("id").and_then(TValue::text);
        let fresh = conn_id.is_none_or(|id| seen_conns.insert(id));
        let conn_start = time_of(conn.get("timestamp_start"));
        let tcp = time_of(conn.get("timestamp_tcp_setup"));
        let tls_done = time_of(conn.get("timestamp_tls_setup"));
        if fresh {
            if let (Some(s), Some(t)) = (conn_start, tcp) {
                draft.timing.connect_duration = offset_ms(s, t);
            }
            if let (Some(t), Some(done)) = (tcp, tls_done) {
                draft.timing.tls_duration = offset_ms(t, done);
            }
        } else {
            draft
                .upstream
                .get_or_insert_with(UpstreamInfo::default)
                .reused = true;
        }
    }

    // WebSocket messages.
    if let Some(ws) = websocket {
        for m in ws.get("messages").map(TValue::as_list).unwrap_or_default() {
            let fields = m.as_list();
            let (Some(opcode), Some(from_client), Some(content)) = (
                fields.first().and_then(TValue::as_i64),
                fields.get(1).and_then(TValue::as_bool),
                fields.get(2).and_then(TValue::as_bytes),
            ) else {
                continue;
            };
            let kind = match opcode {
                2 => MessageKind::Binary,
                8 => MessageKind::Close,
                9 => MessageKind::Ping,
                10 => MessageKind::Pong,
                _ => MessageKind::Text,
            };
            let at = fields
                .get(3)
                .and_then(TValue::as_f64)
                .map(seconds_to_micros)
                .unwrap_or(start);
            draft.messages.push(MessageWithPayload {
                message: StreamMessage {
                    seq: u32::try_from(draft.messages.len()).unwrap_or(u32::MAX),
                    direction: if from_client {
                        Direction::Sent
                    } else {
                        Direction::Received
                    },
                    kind,
                    at,
                    size: content.len() as u64,
                    event: None,
                    event_id: None,
                    modified: fields.get(5).and_then(TValue::as_bool).unwrap_or(false),
                },
                payload: Bytes::copy_from_slice(content),
            });
        }
        let end = at(ws.get("timestamp_end"));
        if let Some(end) = end {
            draft.close = Some(StreamClose {
                code: ws
                    .get("close_code")
                    .and_then(TValue::as_i64)
                    .and_then(|c| u16::try_from(c).ok()),
                reason: ws
                    .get("close_reason")
                    .and_then(TValue::text)
                    .filter(|r| !r.is_empty()),
                by: ws
                    .get("closed_by_client")
                    .and_then(TValue::as_bool)
                    .map(|c| {
                        if c {
                            Direction::Sent
                        } else {
                            Direction::Received
                        }
                    }),
                at: end,
            });
            draft.timing.response_body = Some(end);
        }
    }

    if let Some(err) = flow.get("error") {
        let message = err
            .get("msg")
            .and_then(TValue::text)
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| "The request failed".into());
        draft.timing.aborted = at(err.get("timestamp"));
        draft.error = Some(ExchangeError {
            kind: error_kind_for(&message),
            message,
            code: None,
        });
    }
    draft.note = flow.get("comment").and_then(TValue::text);
    draft.color = colour_of(flow.get("marked"));
    Some((start.0, draft.into_record()))
}

/// Parse a mitmproxy flow file into records ready for insertion (chronological order).
pub(crate) fn parse(input: &[u8]) -> Result<Vec<NewRecord>> {
    if !sniff(input) {
        return Err(invalid("not a mitmproxy flow file"));
    }
    let mut rest = input;
    let mut items = Vec::new();
    let mut seen_conns = HashSet::new();
    let mut flows = 0usize;
    while !rest.iter().all(u8::is_ascii_whitespace) {
        let (flow, next) = parse_value(rest, 0)?;
        rest = next;
        flows += 1;
        if let Some(item) = convert_flow(&flow, &mut seen_conns) {
            items.push(item);
        }
    }
    if items.is_empty() {
        return Err(invalid(format!(
            "none of the {flows} mitmproxy flows is an HTTP flow"
        )));
    }
    if items.len() < flows {
        tracing::info!(
            skipped = flows - items.len(),
            flows,
            "skipped non-HTTP mitmproxy flows"
        );
    }
    items.sort_by_key(|(start, _)| *start);
    Ok(items.into_iter().map(|(_, record)| record).collect())
}

impl Store {
    /// Import a mitmproxy flow file (HTTP and WebSocket flows). Returns the new ids.
    pub fn import_mitmproxy(&self, input: &[u8]) -> Result<Vec<ExchangeId>> {
        let records = parse(input)?;
        Ok(self.insert_records(records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tnetstring_values() {
        let (v, rest) = parse_value(b"5:hello,rest", 0).unwrap();
        assert_eq!(v, TValue::Bytes(b"hello"));
        assert_eq!(rest, b"rest");
        assert_eq!(parse_value(b"2:42#", 0).unwrap().0, TValue::Int(42));
        assert_eq!(parse_value(b"3:1.5^", 0).unwrap().0, TValue::Float(1.5));
        assert_eq!(parse_value(b"4:true!", 0).unwrap().0, TValue::Bool(true));
        assert_eq!(parse_value(b"0:~", 0).unwrap().0, TValue::Null);
        assert_eq!(
            parse_value(b"3:\xc3\xa9x;", 0).unwrap().0,
            TValue::Str("éx")
        );
        let (list, _) = parse_value(b"8:1:a,1:1#]", 0).unwrap();
        assert_eq!(
            list,
            TValue::List(vec![TValue::Bytes(b"a"), TValue::Int(1)])
        );
        let (dict, _) = parse_value(b"8:1:k;1:v,}", 0).unwrap();
        assert_eq!(dict.get("k"), Some(&TValue::Bytes(b"v")));
    }

    #[test]
    fn malformed_tnetstrings_fail_cleanly() {
        for bad in [
            &b""[..],
            b"x:abc,",
            b"10:abc,",
            b"3:abc?",
            b"5:maybe!",
            b"1:x~",
            b"2:ab#",
            b"4:1:k,}",
            b"99999999999999999:x,",
        ] {
            assert!(
                parse_value(bad, 0).is_err(),
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
        // Deep nesting is refused, not a stack overflow.
        let mut deep = b"0:]".to_vec();
        for _ in 0..200 {
            deep = [format!("{}:", deep.len()).into_bytes(), deep, b"]".to_vec()].concat();
        }
        assert!(parse_value(&deep, 0).is_err());
    }

    #[test]
    fn sniffing() {
        assert!(sniff(b"3:0:~}"));
        assert!(!sniff(b"3:abc,"));
        assert!(!sniff(b"{\"log\":{}}"));
        assert!(!sniff(b"[]"));
        assert!(parse(b"[]").is_err());
        // A dict that isn't an HTTP flow.
        let tcp = b"13:4:type;3:tcp;}";
        assert!(sniff(tcp));
        let err = parse(tcp).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(err.contains("HTTP flow"), "{err}");
    }

    #[test]
    fn marks_become_colours() {
        assert_eq!(
            colour_of(Some(&TValue::Str(":red_circle:"))).as_deref(),
            Some("red")
        );
        assert_eq!(
            colour_of(Some(&TValue::Str(":large_blue_circle:"))).as_deref(),
            Some("blue")
        );
        assert_eq!(
            colour_of(Some(&TValue::Str(":default:"))).as_deref(),
            Some("yellow")
        );
        assert_eq!(
            colour_of(Some(&TValue::Bool(true))).as_deref(),
            Some("yellow")
        );
        assert_eq!(colour_of(Some(&TValue::Str(""))), None);
        assert_eq!(colour_of(Some(&TValue::Bool(false))), None);
        assert_eq!(colour_of(None), None);
    }
}
