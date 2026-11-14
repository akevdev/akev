//! Fiddler session archives (`.saz`, *File → Save → All Sessions*), Fiddler Classic and
//! Fiddler Everywhere.
//!
//! A `.saz` is a ZIP archive with, per session `NN`:
//!
//! * `raw/NN_c.txt`: the request as sent (request line, headers, blank line, body);
//! * `raw/NN_s.txt`: the response as received;
//! * `raw/NN_m.xml`: metadata, notably `<SessionTimers ClientBeginRequest="…" …/>` and
//!   `<SessionFlag N="x-clientip" V="…"/>` flags (`x-hostip`, `ui-comments`, `ui-color`).
//!
//! Bodies are kept as Fiddler stored them: chunked transfer coding is removed (the stored
//! headers then describe the stored bytes), content codings stay. `CONNECT` sessions become
//! tunnel rows. Password-protected archives aren't supported. Sessions that can't be read are
//! skipped; the import fails only when none is usable.

use std::collections::BTreeMap;
use std::io::Read;

use super::{Draft, body, build_url, default_port, offset_ms};
use crate::har::parse_version;
use crate::{NewRecord, Result, Store, StoreError};
use akev_core::*;

/// Per-file and total limits, so a hostile archive can't exhaust memory.
const MAX_ENTRY: u64 = 256 << 20;
const MAX_TOTAL: u64 = 2 << 30;

#[derive(Default)]
struct Files {
    request: Option<Vec<u8>>,
    response: Option<Vec<u8>>,
    meta: Option<String>,
}

fn invalid(message: impl Into<String>) -> StoreError {
    StoreError::InvalidFile(message.into())
}

/// The sessions of a `.saz` archive, in session order.
pub(crate) fn parse(input: &[u8]) -> Result<Vec<NewRecord>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(input))
        .map_err(|e| invalid(format!("not a Fiddler .saz archive: {e}")))?;
    let mut sessions: BTreeMap<u64, Files> = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = match archive.by_index(i) {
            Ok(e) => e,
            Err(zip::result::ZipError::UnsupportedArchive(msg)) if msg.contains("assword") => {
                return Err(invalid(
                    "the archive is password-protected; save it again in Fiddler without a password",
                ));
            }
            Err(e) => return Err(invalid(format!("unreadable .saz entry: {e}"))),
        };
        if entry.encrypted() {
            return Err(invalid(
                "the archive is password-protected; save it again in Fiddler without a password",
            ));
        }
        let Ok(name) = entry.name().map(|n| n.replace('\\', "/")) else {
            continue;
        };
        let Some(file) = name.strip_prefix("raw/") else {
            continue;
        };
        let Some((id, kind)) = file.split_once('_') else {
            continue;
        };
        let Ok(id) = id.parse::<u64>() else { continue };
        if !matches!(kind, "c.txt" | "s.txt" | "m.xml") {
            continue;
        }
        let size = entry.size();
        if size > MAX_ENTRY {
            tracing::warn!(entry = %name, size, "skipped oversized .saz entry");
            continue;
        }
        total += size;
        if total > MAX_TOTAL {
            return Err(invalid("the archive is too large to import"));
        }
        let mut bytes = Vec::with_capacity(usize::try_from(size).unwrap_or(0));
        entry
            .by_ref()
            .take(MAX_ENTRY)
            .read_to_end(&mut bytes)
            .map_err(|e| invalid(format!("unreadable .saz entry {name}: {e}")))?;
        let files = sessions.entry(id).or_default();
        match kind {
            "c.txt" => files.request = Some(bytes),
            "s.txt" => files.response = Some(bytes),
            _ => files.meta = Some(String::from_utf8_lossy(&bytes).into_owned()),
        }
    }
    if sessions.is_empty() {
        return Err(invalid(
            "no Fiddler sessions in the archive (raw/NN_c.txt files)",
        ));
    }
    let mut skipped = 0usize;
    let mut records = Vec::new();
    for (_, files) in sessions {
        match session(files) {
            Some(r) => records.push(r),
            None => skipped += 1,
        }
    }
    if records.is_empty() {
        return Err(invalid("none of the Fiddler sessions could be read"));
    }
    if skipped > 0 {
        tracing::warn!(skipped, "skipped unreadable Fiddler sessions");
    }
    Ok(records)
}

/// A raw HTTP/1 message: the start line, headers and the bytes after the blank line.
struct Raw {
    start: String,
    headers: Headers,
    body: Vec<u8>,
}

fn parse_raw(bytes: &[u8]) -> Option<Raw> {
    let (head_len, sep) = memchr::memmem::find(bytes, b"\r\n\r\n")
        .map(|i| (i, 4))
        .or_else(|| memchr::memmem::find(bytes, b"\n\n").map(|i| (i, 2)))
        .unwrap_or((bytes.len(), 0));
    let head = String::from_utf8_lossy(&bytes[..head_len]);
    let mut lines = head.lines();
    let start = lines.next()?.trim().to_string();
    if start.is_empty() {
        return None;
    }
    let mut headers = Headers::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim();
            if !name.is_empty() && !name.chars().any(char::is_control) {
                headers.append(name, value.trim());
            }
        }
    }
    let body = bytes.get(head_len + sep..).unwrap_or_default().to_vec();
    Some(Raw {
        start,
        headers,
        body,
    })
}

/// Remove chunked transfer coding (then drop the header); a body that doesn't parse as chunks
/// is kept as is.
fn dechunk(headers: &mut Headers, body: Vec<u8>) -> Vec<u8> {
    let chunked = headers
        .get("transfer-encoding")
        .is_some_and(|te| te.to_ascii_lowercase().contains("chunked"));
    if !chunked {
        return body;
    }
    let mut out = Vec::with_capacity(body.len());
    let mut rest = &body[..];
    loop {
        let Some(eol) = memchr::memmem::find(rest, b"\r\n") else {
            return body;
        };
        let size_line = String::from_utf8_lossy(&rest[..eol]);
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            return body;
        };
        rest = &rest[eol + 2..];
        if size == 0 {
            break;
        }
        let Some(chunk) = rest.get(..size) else {
            // Cut short: keep what arrived.
            out.extend_from_slice(rest);
            break;
        };
        out.extend_from_slice(chunk);
        rest = rest.get(size + 2..).unwrap_or_default();
    }
    headers.remove("transfer-encoding");
    out
}

/// `Attr="value"` in Fiddler's metadata XML.
fn attr<'a>(xml: &'a str, name: &str) -> Option<&'a str> {
    let key = format!("{name}=\"");
    let start = xml.find(&key)? + key.len();
    let end = xml[start..].find('"')?;
    Some(&xml[start..start + end])
}

/// `<SessionFlag N="name" V="value"/>`.
fn flag(xml: &str, name: &str) -> Option<String> {
    let key = format!("N=\"{name}\"");
    let at = xml.find(&key)?;
    let tail = &xml[at..];
    let value = attr(tail.split('>').next()?, "V")?;
    Some(xml_unescape(value))
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Fiddler's timestamps (`2024-03-04T10:21:36.5120000+01:00`; `0001-01-01…` when unset).
fn time(xml: &str, name: &str) -> Option<UnixMicros> {
    let raw = attr(xml, name)?;
    if raw.starts_with("0001-") {
        return None;
    }
    crate::har::parse_start(Some(&serde_json::Value::String(raw.to_string())))
}

fn split_host_port(authority: &str, default: u16) -> (String, u16) {
    let authority = authority.trim();
    if let Some(rest) = authority.strip_prefix('[') {
        let (host, after) = rest.split_once(']').unwrap_or((rest, ""));
        let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
        return (host.to_ascii_lowercase(), port.unwrap_or(default));
    }
    match authority.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host.to_ascii_lowercase(), port.parse().unwrap_or(default))
        }
        _ => (authority.to_ascii_lowercase(), default),
    }
}

fn session(files: Files) -> Option<NewRecord> {
    let request = parse_raw(files.request.as_deref()?)?;
    let meta = files.meta.unwrap_or_default();
    let mut words = request.start.split_whitespace();
    let method = words.next()?.to_string();
    let target = words.next()?.to_string();
    let version = parse_version(words.next()).unwrap_or(HttpVersion::Http11);

    let start = time(&meta, "ClientBeginRequest")
        .or_else(|| time(&meta, "ClientConnected"))
        .unwrap_or_else(UnixMicros::now);
    let at = |name: &str| time(&meta, name).and_then(|t| offset_ms(start, t));

    let mut client = ClientInfo::default();
    if let Some(ip) = flag(&meta, "x-clientip") {
        let ip = ip.trim_start_matches("::ffff:").to_string();
        let port = flag(&meta, "x-clientport")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0);
        client.remote_addr = crate::util::socket_addr_string(&ip, port);
    }
    let note = flag(&meta, "ui-comments").filter(|n| !n.trim().is_empty());
    let color = flag(&meta, "ui-color").map(|c| c.to_ascii_lowercase());

    // CONNECT: a tunnel row.
    if method.eq_ignore_ascii_case("CONNECT") {
        let (host, port) = split_host_port(&target, 443);
        let scheme = if port == 443 { "https" } else { "tcp" };
        let head = RequestHead {
            method: "CONNECT".into(),
            url: format!("{scheme}://{}", crate::util::authority(scheme, &host, port)),
            scheme: scheme.into(),
            host,
            port,
            path: String::new(),
            http_version: version,
            headers: request.headers,
        };
        let mut draft = Draft::new(ExchangeKind::Tunnel, head, start);
        draft.client = client;
        draft.note = note;
        draft.color = color;
        draft.tunnel = Some(TunnelStats {
            bytes_sent: 0,
            bytes_received: 0,
            reason: "not decrypted (Fiddler)".into(),
        });
        draft.timing.response_body = at("ClientDoneResponse");
        draft.close = draft
            .timing
            .response_body
            .map(|at| crate::record::StreamClose {
                code: None,
                reason: None,
                by: None,
                at,
            });
        return Some(draft.into_record());
    }

    // Absolute-form (proxied) or origin-form with a Host header.
    let (scheme, authority, path) = if let Some((scheme, rest)) = target.split_once("://") {
        let (authority, path) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        (
            scheme.to_ascii_lowercase(),
            authority.to_string(),
            path.to_string(),
        )
    } else {
        let host = request.headers.get("host")?.to_string();
        let scheme = if host.ends_with(":443") || flag(&meta, "https-client-sni").is_some() {
            "https"
        } else {
            "http"
        };
        (scheme.to_string(), host, target.clone())
    };
    let (host, port) = split_host_port(&authority, default_port(&scheme));
    let mut request_headers = request.headers;
    let request_body = dechunk(&mut request_headers, request.body);
    let websocket = request_headers
        .get("upgrade")
        .is_some_and(|u| u.eq_ignore_ascii_case("websocket"));
    let head = RequestHead {
        method,
        url: build_url(&scheme, &host, port, &path),
        scheme: scheme.clone(),
        host,
        port,
        path,
        http_version: version,
        headers: request_headers,
    };
    let kind = if websocket {
        ExchangeKind::WebSocket
    } else {
        ExchangeKind::Http
    };
    let mut draft = Draft::new(kind, head, start);
    draft.request_body = body(request_body, None, true);
    draft.client = client;
    draft.note = note;
    draft.color = color;
    if let Some(ip) = flag(&meta, "x-hostip") {
        draft.upstream = Some(UpstreamInfo {
            remote_addr: Some(crate::util::socket_addr_string(&ip, port)),
            ..UpstreamInfo::default()
        });
    }

    if let Some(raw) = files.response.as_deref().and_then(parse_raw) {
        let mut words = raw.start.splitn(3, ' ');
        let version = parse_version(words.next()).unwrap_or(version);
        if let Some(status) = words.next().and_then(|s| s.parse::<u16>().ok()) {
            let status_message = words
                .next()
                .map(str::trim)
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| canonical_reason(status).to_string());
            let mut headers = raw.headers;
            let bytes = dechunk(&mut headers, raw.body);
            draft.response_body = body(bytes, None, true);
            draft.response = Some(ResponseHead {
                status,
                status_message,
                http_version: version,
                headers,
            });
        }
    }
    let has_response = draft.response.is_some();
    draft.timing = Timing {
        start,
        request_headers: at("FiddlerGotRequestHeaders"),
        request_body: at("ClientDoneRequest"),
        request_sent: at("ServerGotRequest").or_else(|| at("ClientDoneRequest")),
        response_headers: at("ServerBeginResponse").filter(|_| has_response),
        response_body: at("ClientDoneResponse")
            .or_else(|| at("ServerDoneResponse"))
            .filter(|_| has_response),
        ..Timing::default()
    };
    draft.timing.start = start;
    if !has_response {
        draft.error = Some(ExchangeError {
            kind: ErrorKind::Internal,
            message: "Fiddler saved no response for this session".into(),
            code: None,
        });
    }
    Some(draft.into_record())
}

impl Store {
    /// Import a Fiddler session archive (`.saz`). Returns the new ids.
    pub fn import_fiddler(&self, input: &[u8]) -> Result<Vec<ExchangeId>> {
        let records = parse(input)?;
        Ok(self.insert_records(records))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn saz(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut buf);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, data) in files {
                zip.start_file(*name, opts).unwrap();
                zip.write_all(data).unwrap();
            }
            zip.finish().unwrap();
        }
        buf.into_inner()
    }

    const META: &str = r#"<?xml version="1.0" encoding="utf-8"?><Session SID="1"><SessionTimers ClientConnected="2024-03-04T10:21:36.5000000+01:00" ClientBeginRequest="2024-03-04T10:21:36.5120000+01:00" ClientDoneRequest="2024-03-04T10:21:36.5130000+01:00" ServerGotRequest="2024-03-04T10:21:36.5200000+01:00" ServerBeginResponse="2024-03-04T10:21:36.6120000+01:00" ClientDoneResponse="2024-03-04T10:21:36.7120000+01:00" /><SessionFlags><SessionFlag N="x-clientip" V="::ffff:127.0.0.1" /><SessionFlag N="x-clientport" V="52178" /><SessionFlag N="x-hostip" V="93.184.216.34" /><SessionFlag N="ui-comments" V="login &amp; retry" /></SessionFlags></Session>"#;

    #[test]
    fn imports_sessions_with_chunked_bodies_and_timers() {
        let request: &[u8] = b"POST https://api.example.com/login?x=1 HTTP/1.1\r\nHost: api.example.com\r\nContent-Type: application/json\r\nContent-Length: 13\r\n\r\n{\"user\":\"a\"}\n";
        let response: &[u8] = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nHello\r\n6\r\n world\r\n0\r\n\r\n";
        let connect: &[u8] =
            b"CONNECT secure.example.com:443 HTTP/1.1\r\nHost: secure.example.com:443\r\n\r\n";
        let data = saz(&[
            ("[Content_Types].xml", b"<Types/>"),
            ("raw/01_c.txt", request),
            ("raw/01_s.txt", response),
            ("raw/01_m.xml", META.as_bytes()),
            ("raw/02_c.txt", connect),
        ]);
        let records = parse(&data).unwrap();
        assert_eq!(records.len(), 2);
        let r = &records[0].detail;
        assert_eq!(r.request.method, "POST");
        assert_eq!(r.request.url, "https://api.example.com/login?x=1");
        assert_eq!(r.request.port, 443);
        assert_eq!(&records[0].request_body.bytes[..], b"{\"user\":\"a\"}\n");
        let resp = r.response.as_ref().unwrap();
        assert_eq!(resp.status, 200);
        assert!(!resp.headers.contains("transfer-encoding"));
        assert_eq!(&records[0].response_body.bytes[..], b"Hello world");
        assert_eq!(r.client.remote_addr, "127.0.0.1:52178");
        assert_eq!(r.note.as_deref(), Some("login & retry"));
        assert_eq!(r.timing.response_headers, Some(100.0));
        assert_eq!(r.timing.response_body, Some(200.0));
        assert_eq!(
            r.upstream.as_ref().unwrap().remote_addr.as_deref(),
            Some("93.184.216.34:443")
        );
        let tunnel = &records[1].detail;
        assert_eq!(tunnel.summary.kind, ExchangeKind::Tunnel);
        assert_eq!(tunnel.request.host, "secure.example.com");
    }

    #[test]
    fn origin_form_requests_and_missing_responses() {
        let request: &[u8] = b"GET /status HTTP/1.1\r\nHost: localhost:8080\r\n\r\n";
        let data = saz(&[("raw/7_c.txt", request)]);
        let records = parse(&data).unwrap();
        let r = &records[0].detail;
        assert_eq!(r.request.url, "http://localhost:8080/status");
        assert!(r.response.is_none());
        assert!(r.error.is_some());
    }

    #[test]
    fn rejects_non_archives_and_empty_ones() {
        assert!(parse(b"not a zip").is_err());
        assert!(parse(&saz(&[("readme.txt", b"hi")])).is_err());
        assert!(parse(&saz(&[("raw/01_c.txt", b"")])).is_err());
    }

    #[test]
    fn dechunks_only_valid_chunked_bodies() {
        let mut h = Headers::new();
        h.append("Transfer-Encoding", "chunked");
        assert_eq!(dechunk(&mut h, b"3\r\nabc\r\n0\r\n\r\n".to_vec()), b"abc");
        let mut h = Headers::new();
        h.append("Transfer-Encoding", "chunked");
        assert_eq!(dechunk(&mut h, b"zz\r\nnope".to_vec()), b"zz\r\nnope");
        assert!(
            h.contains("transfer-encoding"),
            "kept when the body isn't chunked"
        );
    }
}
