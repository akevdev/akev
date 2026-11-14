//! Native `.akev` session files: zstd-compressed JSON Lines.
//!
//! ```text
//! {"format":"akev-session","version":1,"createdAt":"2026-01-01T00:00:00Z","app":"0.1.0"}
//! {"detail":{...ExchangeDetail...},"requestBody":{"data":"<base64>","wireSize":12},...}
//! ...one line per exchange...
//! ```
//!
//! Each exchange line holds the full [`ExchangeDetail`] (summary incl. pins/colours, note,
//! timings, client/upstream info, original heads), the captured bodies exactly as stored
//! (still content-encoded, base64), pre-modification bodies, stream messages with payloads
//! and stream-close info, so save → load is lossless apart from the freshly allocated ids.
//! Loading also accepts uncompressed JSON Lines.
//!
//! Loading is bounded by [`SessionLimits`] (zstd reaches ratios above 10 000:1, so a tiny file
//! could otherwise expand into gigabytes before any validation): the length of one line, the
//! total decompressed size, and how far the data may expand relative to the compressed bytes
//! read so far.

use crate::record::StreamClose;
use crate::{MessageWithPayload, NewRecord, Result, Snapshot, StoreError};
use akev_core::{CapturedBody, ExchangeDetail, StreamMessage, UnixMicros, VERSION};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::cell::Cell;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::rc::Rc;

/// Value of the header line's `format` field.
pub(crate) const FORMAT: &str = "akev-session";
/// Current (and highest understood) session format version.
pub(crate) const FORMAT_VERSION: u32 = 1;
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xb5, 0x2f, 0xfd];
const ZSTD_LEVEL: i32 = 3;

/// Size limits for loading a session file (see [`crate::Store::load_session_with_limits`]).
///
/// A file breaking one is rejected with [`StoreError::InvalidFile`] before anything is
/// inserted, as soon as the limit is crossed (so memory use stays near the limits).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SessionLimits {
    /// Longest accepted line — one exchange with its bodies (base64) and stream messages — in
    /// decompressed bytes. Default 512 MiB.
    pub max_line_bytes: usize,
    /// Most decompressed bytes read from one file. Default 16 GiB.
    pub max_total_bytes: u64,
    /// Most decompressed bytes per compressed byte read so far (zstd files only), once more
    /// than [`SessionLimits::expansion_floor_bytes`] were decompressed. Real sessions —
    /// mostly base64 of compressed bodies — stay far below the default of 256; decompression
    /// bombs are far above it.
    pub max_expansion: u64,
    /// Decompressed bytes always allowed regardless of [`SessionLimits::max_expansion`].
    /// Default 64 MiB.
    pub expansion_floor_bytes: u64,
}

impl Default for SessionLimits {
    fn default() -> Self {
        SessionLimits {
            max_line_bytes: 512 << 20,
            max_total_bytes: 16 << 30,
            max_expansion: 256,
            expansion_floor_bytes: 64 << 20,
        }
    }
}

/// The error a [`Limited`] reader fails with (recognised by [`read_error`]).
#[derive(Debug)]
struct LimitExceeded(String);

impl std::fmt::Display for LimitExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for LimitExceeded {}

/// Counts the bytes read through it (the compressed input).
struct Counted<R> {
    inner: R,
    count: Rc<Cell<u64>>,
}

impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.count.set(self.count.get().saturating_add(n as u64));
        Ok(n)
    }
}

/// Fails once more data was read through it than the limits allow.
struct Limited<R> {
    inner: R,
    read: u64,
    /// Compressed bytes consumed so far (zstd input), for the expansion limit.
    compressed: Option<Rc<Cell<u64>>>,
    limits: SessionLimits,
}

impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read = self.read.saturating_add(n as u64);
        let limits = &self.limits;
        if self.read > limits.max_total_bytes {
            return Err(std::io::Error::other(LimitExceeded(format!(
                "session data is larger than {} bytes",
                limits.max_total_bytes
            ))));
        }
        if let Some(compressed) = &self.compressed {
            let allowed = compressed
                .get()
                .saturating_mul(limits.max_expansion)
                .max(limits.expansion_floor_bytes);
            if self.read > allowed {
                return Err(std::io::Error::other(LimitExceeded(format!(
                    "session data expands to more than {} times its compressed size \
                     (possible decompression bomb)",
                    limits.max_expansion
                ))));
            }
        }
        Ok(n)
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeaderLine {
    format: String,
    version: u32,
    // Defaulted so a newer version's header still reaches the version check.
    #[serde(default)]
    created_at: String,
    #[serde(default)]
    app: String,
    /// Name of a saved session (session library); absent for plain exports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionBody {
    /// Captured bytes, base64.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    data: String,
    wire_size: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    truncated: bool,
}

impl SessionBody {
    fn from_captured(b: &CapturedBody) -> Self {
        SessionBody {
            data: STANDARD.encode(&b.bytes),
            wire_size: b.wire_size,
            truncated: b.truncated,
        }
    }

    fn is_empty(&self) -> bool {
        self.data.is_empty() && self.wire_size == 0 && !self.truncated
    }

    fn into_captured(self, line: usize) -> Result<CapturedBody> {
        let bytes = STANDARD
            .decode(self.data.as_bytes())
            .map_err(|e| StoreError::InvalidFile(format!("line {line}: invalid body data: {e}")))?;
        Ok(CapturedBody {
            wire_size: self.wire_size.max(bytes.len() as u64),
            truncated: self.truncated,
            bytes: Bytes::from(bytes),
        })
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SessionMessage {
    #[serde(flatten)]
    message: StreamMessage,
    payload: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExchangeLine {
    detail: ExchangeDetail,
    #[serde(default, skip_serializing_if = "SessionBody::is_empty")]
    request_body: SessionBody,
    #[serde(default, skip_serializing_if = "SessionBody::is_empty")]
    response_body: SessionBody,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_request_body: Option<SessionBody>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    original_response_body: Option<SessionBody>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    messages: Vec<SessionMessage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    close: Option<StreamClose>,
}

impl ExchangeLine {
    fn from_snapshot(s: Snapshot) -> Self {
        ExchangeLine {
            request_body: SessionBody::from_captured(&s.request_body),
            response_body: SessionBody::from_captured(&s.response_body),
            original_request_body: s
                .original_request_body
                .as_ref()
                .map(SessionBody::from_captured),
            original_response_body: s
                .original_response_body
                .as_ref()
                .map(SessionBody::from_captured),
            messages: s
                .messages
                .iter()
                .map(|m| SessionMessage {
                    message: m.message.clone(),
                    payload: STANDARD.encode(&m.payload),
                })
                .collect(),
            close: s.close,
            detail: s.detail,
        }
    }

    fn into_record(self, line: usize) -> Result<NewRecord> {
        let messages = self
            .messages
            .into_iter()
            .map(|m| {
                let payload = STANDARD.decode(m.payload.as_bytes()).map_err(|e| {
                    StoreError::InvalidFile(format!("line {line}: invalid message payload: {e}"))
                })?;
                Ok(MessageWithPayload {
                    message: m.message,
                    payload: Bytes::from(payload),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(NewRecord {
            detail: self.detail,
            request_body: self.request_body.into_captured(line)?,
            response_body: self.response_body.into_captured(line)?,
            original_request_body: self
                .original_request_body
                .map(|b| b.into_captured(line))
                .transpose()?,
            original_response_body: self
                .original_response_body
                .map(|b| b.into_captured(line))
                .transpose()?,
            messages,
            close: self.close,
            exact: true,
        })
    }
}

/// Write a session: header line (with the session `name`, if any), then one line per
/// snapshot. Returns the number of exchanges written.
pub(crate) fn save(
    writer: &mut dyn Write,
    snapshots: impl Iterator<Item = Snapshot>,
    name: Option<&str>,
) -> Result<usize> {
    let encoder = zstd::stream::write::Encoder::new(writer, ZSTD_LEVEL)?;
    let mut out = BufWriter::with_capacity(64 * 1024, encoder);
    let header = HeaderLine {
        format: FORMAT.into(),
        version: FORMAT_VERSION,
        created_at: UnixMicros::now().to_rfc3339(),
        app: VERSION.into(),
        name: name.map(str::to_string),
    };
    serde_json::to_writer(&mut out, &header).map_err(std::io::Error::from)?;
    out.write_all(b"\n")?;
    let mut written = 0;
    for snap in snapshots {
        serde_json::to_writer(&mut out, &ExchangeLine::from_snapshot(snap))
            .map_err(std::io::Error::from)?;
        out.write_all(b"\n")?;
        written += 1;
    }
    let encoder = out.into_inner().map_err(|e| e.into_error())?;
    encoder.finish()?.flush()?;
    Ok(written)
}

fn corrupt(e: std::io::Error) -> StoreError {
    StoreError::InvalidFile(format!("unreadable session data: {e}"))
}

/// A read error: a crossed [`SessionLimits`] limit, or corrupt data.
fn read_error(e: std::io::Error) -> StoreError {
    match e
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<LimitExceeded>())
    {
        Some(limit) => StoreError::InvalidFile(limit.to_string()),
        None => corrupt(e),
    }
}

/// Read the next non-blank line into `buf`; `false` at end of input. Lines longer than
/// `max_line` bytes (without the line break) are rejected without reading them whole.
fn next_line(
    lines: &mut dyn BufRead,
    buf: &mut Vec<u8>,
    line_no: &mut usize,
    max_line: usize,
) -> Result<bool> {
    // Room for the content plus a CRLF line break.
    let cap = (max_line as u64).saturating_add(2);
    loop {
        buf.clear();
        if lines.take(cap).read_until(b'\n', buf).map_err(read_error)? == 0 {
            return Ok(false);
        }
        *line_no += 1;
        let content = buf.strip_suffix(b"\n").unwrap_or(buf);
        let content = content.strip_suffix(b"\r").unwrap_or(content);
        if content.len() > max_line {
            return Err(StoreError::InvalidFile(format!(
                "line {line_no} is longer than {max_line} bytes"
            )));
        }
        if buf.iter().any(|b| !b.is_ascii_whitespace()) {
            return Ok(true);
        }
    }
}

/// What [`scan`] learns about a session file without parsing its exchanges.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ScanInfo {
    /// The session name stored in the header (saved sessions only).
    pub name: Option<String>,
    /// The header's creation time (RFC 3339), if any.
    pub created_at: Option<String>,
    /// Number of exchange lines.
    pub exchanges: u64,
}

/// Read a session's header and count its exchange lines (decompressing, but without parsing
/// the exchanges), under the same limits as [`load`].
pub(crate) fn scan(reader: &mut dyn Read, limits: SessionLimits) -> Result<ScanInfo> {
    let mut lines = open_lines(reader, limits)?;
    let mut buf = Vec::new();
    let mut line_no = 0usize;
    let header = read_header(&mut *lines, &mut buf, &mut line_no, limits.max_line_bytes)?;
    let mut exchanges = 0u64;
    while next_line(&mut *lines, &mut buf, &mut line_no, limits.max_line_bytes)? {
        exchanges += 1;
    }
    Ok(ScanInfo {
        name: header.name,
        created_at: Some(header.created_at).filter(|c| !c.is_empty()),
        exchanges,
    })
}

/// Parse and validate a whole session (nothing is returned unless every line is valid).
pub(crate) fn load(reader: &mut dyn Read, limits: SessionLimits) -> Result<Vec<NewRecord>> {
    let mut records = Vec::new();
    for_each(reader, limits, &mut |record| records.push(record))?;
    Ok(records)
}

/// Parse a session line by line, handing each exchange to `each` as soon as it's read (so a
/// caller can insert in batches with bounded memory). Stops at the first invalid line —
/// records handed out before stay handed out. Returns the number of exchanges.
pub(crate) fn for_each(
    reader: &mut dyn Read,
    limits: SessionLimits,
    each: &mut dyn FnMut(NewRecord),
) -> Result<usize> {
    let mut lines = open_lines(reader, limits)?;
    let max_line = limits.max_line_bytes;
    let mut buf = Vec::new();
    let mut line_no = 0usize;
    read_header(&mut *lines, &mut buf, &mut line_no, max_line)?;
    let mut count = 0;
    while next_line(&mut *lines, &mut buf, &mut line_no, max_line)? {
        let line: ExchangeLine = serde_json::from_slice(&buf).map_err(|e| {
            StoreError::InvalidFile(format!("invalid exchange on line {line_no}: {e}"))
        })?;
        each(line.into_record(line_no)?);
        count += 1;
    }
    Ok(count)
}

/// A line reader over a session file: zstd-decompressed when the data starts with the zstd
/// magic, plain JSON Lines otherwise; bounded by `limits`.
fn open_lines<'a>(
    reader: &'a mut dyn Read,
    limits: SessionLimits,
) -> Result<Box<dyn BufRead + 'a>> {
    // Sniff the zstd magic without requiring a seekable reader.
    let mut head = [0u8; 4];
    let mut filled = 0;
    while filled < head.len() {
        match reader.read(&mut head[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    let chained = std::io::Cursor::new(head).take(filled as u64).chain(reader);
    Ok(if filled == 4 && head == ZSTD_MAGIC {
        let compressed = Rc::new(Cell::new(0u64));
        let input = Counted {
            inner: chained,
            count: Rc::clone(&compressed),
        };
        let decoder = zstd::stream::read::Decoder::new(input).map_err(corrupt)?;
        let limited = Limited {
            inner: decoder,
            read: 0,
            compressed: Some(compressed),
            limits,
        };
        Box::new(BufReader::with_capacity(64 * 1024, limited))
    } else {
        let limited = Limited {
            inner: chained,
            read: 0,
            compressed: None,
            limits,
        };
        Box::new(BufReader::with_capacity(64 * 1024, limited))
    })
}

/// Read and validate the header line (`line_no` counts the lines read, blank ones included).
fn read_header(
    lines: &mut dyn BufRead,
    buf: &mut Vec<u8>,
    line_no: &mut usize,
    max_line: usize,
) -> Result<HeaderLine> {
    if !next_line(lines, buf, line_no, max_line)? {
        return Err(StoreError::InvalidFile("empty session file".into()));
    }
    let header: HeaderLine = serde_json::from_slice(buf)
        .map_err(|e| StoreError::InvalidFile(format!("not an Akev session (bad header: {e})")))?;
    if header.format != FORMAT {
        return Err(StoreError::InvalidFile(format!(
            "not an Akev session (format `{}`)",
            header.format
        )));
    }
    if header.version == 0 || header.version > FORMAT_VERSION {
        return Err(StoreError::InvalidFile(format!(
            "unsupported session version {} (this build reads up to {FORMAT_VERSION})",
            header.version
        )));
    }
    Ok(header)
}
