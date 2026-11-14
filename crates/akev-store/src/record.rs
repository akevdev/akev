//! Exchange records: compact per-exchange storage, event application and summary derivation.
//!
//! A [`Record`] keeps everything [`ExchangeDetail`] exposes, but laid out for memory:
//! request-line fields live only in the summary (the request head is rebuilt on demand),
//! client info is shared per connection (`Arc`), and rarely used data (modification
//! originals, tunnel stats, stream close, notes, spilled message blobs) sits behind one
//! optional box. Bodies are [`StoredBody`]s whose bytes are either in memory (`Bytes`) or
//! spilled to disk ([`SpillHandle`]); every byte held in memory is accounted in a
//! [`Ledger`] so the store can enforce its memory budget.

use crate::MessageWithPayload;
use crate::spill::{Spill, SpillHandle};
use crate::util;
use akev_core::*;
use bytes::Bytes;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::{Arc, OnceLock};

/// Most notices kept per exchange.
const MAX_NOTICES: usize = 8;
const DISK_DROPPED_REQUEST: &str = "Request body deleted to stay within the spill disk limit";
const DISK_DROPPED_RESPONSE: &str = "Response body deleted to stay within the spill disk limit";
const DISK_DROPPED_ORIGINAL: &str = "Original body deleted to stay within the spill disk limit";
const DISK_DROPPED_MESSAGES: &str = "Message payloads deleted to stay within the spill disk limit";

/// Bytes of the decoded body used for content sniffing.
const SNIFF_BYTES: usize = 4096;
/// Encoded bodies up to this size are fully decoded for sniffing (cheap); larger ones only
/// get their first [`SNIFF_BYTES`] decoded, which succeeds only for decoders that tolerate a
/// truncated stream — otherwise detection falls back to the Content-Type.
const SNIFF_FULL_DECODE_MAX: usize = 16 * 1024;
/// Cap on decoded output while sniffing.
const SNIFF_MAX_OUTPUT: usize = 1024 * 1024;
/// Largest (decoded) request body inspected for the LLM tag; bigger prompts are tagged from
/// their URL alone (well-known providers) without a model.
const LLM_TAG_MAX_BODY: usize = 32 << 20;
/// Encoded bodies larger than this don't get their decoded size computed for `BodyMeta`.
const DECODED_SIZE_MAX_INPUT: u64 = 4 * 1024 * 1024;
/// Decompression cap when computing `BodyMeta::decoded_size` (zip-bomb guard).
const DECODED_SIZE_MAX_OUTPUT: usize = 128 * 1024 * 1024;

/// Which body of an exchange a ledger entry refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Slot {
    Request,
    Response,
    OriginalRequest,
    OriginalResponse,
    /// All in-memory stream message payloads of the exchange, spilled as one blob.
    Messages,
}

/// Where a body's bytes currently live.
#[derive(Clone, Debug)]
pub(crate) enum BodyData {
    Memory(Bytes),
    Spilled(SpillHandle),
}

/// A captured body plus its metadata.
#[derive(Debug)]
pub(crate) struct StoredBody {
    pub data: BodyData,
    pub wire_size: u64,
    pub truncated: bool,
    /// The bytes were discarded to respect a limit (spill disk cap, overloaded capture).
    pub dropped: bool,
    /// Lazily computed decoded size (`None` inside = unknown / too expensive).
    pub decoded_size: OnceLock<Option<u64>>,
}

impl Default for StoredBody {
    fn default() -> Self {
        StoredBody::new(Bytes::new(), 0, false)
    }
}

/// Decoded size of an encoded body (bounded against decompression bombs).
pub(crate) fn decoded_size_of(body: &BodyRef, encodings: &[String], spill: &Spill) -> Option<u64> {
    let bytes = body.load(spill).ok()?;
    akev_content::decode::decode_body(&bytes, encodings, DECODED_SIZE_MAX_OUTPUT)
        .ok()
        .map(|d| d.len() as u64)
}

/// Copy captured bytes into their own exact-size allocation.
///
/// The proxy hands over zero-copy slices of its socket read buffers (32 KiB+ each). Keeping
/// such a slice alive pins the whole underlying buffer, so storing 100k small bodies as slices
/// would retain gigabytes while the ledger only counts the slice lengths. One memcpy at ingest
/// (off the proxy's hot path) keeps memory accounting honest and RSS proportional to content.
pub(crate) fn compact(bytes: Bytes) -> Bytes {
    if bytes.is_empty() {
        Bytes::new()
    } else {
        Bytes::copy_from_slice(&bytes)
    }
}

impl BodyData {
    fn len(&self) -> u64 {
        match self {
            BodyData::Memory(b) => b.len() as u64,
            BodyData::Spilled(h) => h.raw_len,
        }
    }

    fn load(&self, spill: &Spill) -> std::io::Result<Bytes> {
        match self {
            BodyData::Memory(b) => Ok(b.clone()),
            BodyData::Spilled(h) => spill.read(h),
        }
    }
}

fn captured(data: &BodyData, wire_size: u64, truncated: bool, spill: &Spill) -> CapturedBody {
    match data.load(spill) {
        Ok(bytes) => CapturedBody {
            bytes,
            wire_size,
            truncated,
        },
        Err(e) => {
            tracing::warn!(error = %e, "failed to read spilled body");
            CapturedBody {
                bytes: Bytes::new(),
                wire_size,
                truncated: true,
            }
        }
    }
}

/// Where a body's bytes are, plus its sizes: copied out of a record under the store lock
/// (cheap: a `Bytes` reference or a spill handle) and loaded after the lock is released.
#[derive(Clone, Debug)]
pub(crate) struct BodyRef {
    pub data: BodyData,
    pub wire_size: u64,
    pub truncated: bool,
}

impl BodyRef {
    pub(crate) fn captured_len(&self) -> u64 {
        self.data.len()
    }

    pub(crate) fn load(&self, spill: &Spill) -> std::io::Result<Bytes> {
        self.data.load(spill)
    }

    /// As a [`CapturedBody`]; read failures (the blob was freed meanwhile, I/O errors) yield
    /// an empty, truncated body.
    pub(crate) fn to_captured(&self, spill: &Spill) -> CapturedBody {
        captured(&self.data, self.wire_size, self.truncated, spill)
    }
}

impl StoredBody {
    pub(crate) fn to_ref(&self) -> BodyRef {
        BodyRef {
            data: self.data.clone(),
            wire_size: self.wire_size,
            truncated: self.truncated,
        }
    }

    pub(crate) fn new(bytes: Bytes, wire_size: u64, truncated: bool) -> Self {
        let wire_size = wire_size.max(bytes.len() as u64);
        StoredBody {
            data: BodyData::Memory(bytes),
            wire_size,
            truncated,
            dropped: false,
            decoded_size: OnceLock::new(),
        }
    }

    /// Build from a proxy capture, enforcing `max_body_bytes` (0 = unlimited). Truncation
    /// copies the kept prefix so the remainder of a large shared buffer can be freed.
    pub(crate) fn from_captured(body: CapturedBody, max_body: u64) -> Self {
        let CapturedBody {
            mut bytes,
            wire_size,
            mut truncated,
        } = body;
        if max_body > 0 && bytes.len() as u64 > max_body {
            let keep = usize::try_from(max_body)
                .unwrap_or(usize::MAX)
                .min(bytes.len());
            bytes = Bytes::copy_from_slice(&bytes[..keep]);
            truncated = true;
        } else {
            bytes = compact(bytes);
        }
        StoredBody::new(bytes, wire_size, truncated)
    }

    /// Bytes captured (in memory or spilled).
    pub(crate) fn captured_len(&self) -> u64 {
        self.data.len()
    }

    /// Load the captured bytes (reading the spill file if needed).
    pub(crate) fn load(&self, spill: &Spill) -> std::io::Result<Bytes> {
        self.data.load(spill)
    }

    /// Decoded size: free for identity bodies, otherwise computed once (bounded) and cached.
    /// With `compute == false` only an already-known value is returned.
    fn decoded_size(&self, encodings: &[String], spill: &Spill, compute: bool) -> Option<u64> {
        if self.wire_size == 0 && self.captured_len() == 0 {
            return Some(0);
        }
        if encodings.is_empty() {
            return Some(self.wire_size);
        }
        if !compute {
            return self.decoded_size.get().copied().flatten();
        }
        *self.decoded_size.get_or_init(|| {
            if self.truncated || self.captured_len() > DECODED_SIZE_MAX_INPUT {
                return None;
            }
            let bytes = self.load(spill).ok()?;
            akev_content::decode::decode_body(&bytes, encodings, DECODED_SIZE_MAX_OUTPUT)
                .ok()
                .map(|d| d.len() as u64)
        })
    }

    /// Whether `other` refers to the same bytes as this body.
    fn same_data(&self, other: &BodyRef) -> bool {
        match (&self.data, &other.data) {
            (BodyData::Memory(a), BodyData::Memory(b)) => {
                a.as_ptr() == b.as_ptr() && a.len() == b.len()
            }
            (BodyData::Spilled(a), BodyData::Spilled(b)) => a == b,
            _ => false,
        }
    }

    fn meta(&self, headers: Option<&Headers>, spill: &Spill, compute: bool) -> BodyMeta {
        let encodings = headers.map(Headers::content_encodings).unwrap_or_default();
        BodyMeta {
            wire_size: self.wire_size,
            captured_size: self.captured_len(),
            truncated: self.truncated,
            decoded_size: self.decoded_size(&encodings, spill, compute),
            encodings,
            content_type: headers.and_then(Headers::content_type_essence),
            dropped: self.dropped,
        }
    }

    /// The in-memory bytes, if any (non-empty).
    pub(crate) fn memory_bytes(&self) -> Option<&Bytes> {
        match &self.data {
            BodyData::Memory(b) if !b.is_empty() => Some(b),
            _ => None,
        }
    }
}

/// Where a stream message payload lives.
#[derive(Clone, Debug)]
pub(crate) enum Payload {
    Memory(Bytes),
    /// A slice of spilled message blob `blob` (index into `Extra::msg_blobs`).
    Spilled {
        blob: u32,
        offset: u64,
        len: u64,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct StoredMessage {
    pub message: StreamMessage,
    pub payload: Payload,
}

/// How a WebSocket/SSE stream or tunnel ended.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StreamClose {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub code: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<Direction>,
    /// ms since the exchange start.
    pub at: f64,
}

/// Rarely present data, boxed so common records stay small.
#[derive(Debug, Default)]
pub(crate) struct Extra {
    pub original_request: Option<RequestHead>,
    pub original_request_body: Option<StoredBody>,
    pub original_response: Option<ResponseHead>,
    pub original_response_body: Option<StoredBody>,
    pub tunnel: Option<TunnelStats>,
    pub close: Option<StreamClose>,
    pub note: Option<String>,
    pub msg_blobs: Vec<SpillHandle>,
    /// Why Akev handled the exchange differently (see `ExchangeDetail::notices`).
    pub notices: Vec<String>,
}

/// Lifecycle progress used to derive [`ExchangeState`].
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Progress {
    pub req_done: bool,
    pub resp_started: bool,
    pub resp_done: bool,
    pub stream_open: bool,
    pub stream_closed: bool,
    /// A modified request body arrived before the client's body finished.
    pub req_body_modified: bool,
    /// State/duration are fixed (inserted records: no events will ever arrive for them).
    pub frozen: bool,
}

/// Byte accounting for in-memory and spilled bodies.
#[derive(Debug, Default)]
pub(crate) struct Ledger {
    /// In-memory, non-empty bodies ordered oldest exchange first (spill order).
    pub mem_index: BTreeSet<(ExchangeId, Slot)>,
    /// Spilled bodies and message blobs, oldest exchange first (disk-cap drop order).
    pub spill_index: BTreeSet<(ExchangeId, Slot)>,
    pub memory_bytes: u64,
    /// Logical (uncompressed) size of spilled bodies.
    pub spilled_bytes: u64,
}

impl Ledger {
    fn add_body(&mut self, id: ExchangeId, slot: Slot, body: &StoredBody) {
        match &body.data {
            BodyData::Memory(b) if !b.is_empty() => {
                self.memory_bytes += b.len() as u64;
                self.mem_index.insert((id, slot));
            }
            BodyData::Memory(_) => {}
            BodyData::Spilled(h) => {
                self.spilled_bytes += h.raw_len;
                self.spill_index.insert((id, slot));
            }
        }
    }

    /// Remove accounting for a body that is moving slots (its spill blob stays alive).
    fn detach_body(&mut self, id: ExchangeId, slot: Slot, body: &StoredBody) {
        match &body.data {
            BodyData::Memory(b) if !b.is_empty() => {
                self.memory_bytes = self.memory_bytes.saturating_sub(b.len() as u64);
                self.mem_index.remove(&(id, slot));
            }
            BodyData::Memory(_) => {}
            BodyData::Spilled(h) => {
                self.spilled_bytes = self.spilled_bytes.saturating_sub(h.raw_len);
                self.spill_index.remove(&(id, slot));
            }
        }
    }

    /// Dispose of a body: remove accounting and release its spill blob.
    fn free_body(&mut self, id: ExchangeId, slot: Slot, body: &StoredBody, spill: &Spill) {
        self.detach_body(id, slot, body);
        if let BodyData::Spilled(h) = &body.data {
            spill.free(h);
        }
    }
}

/// Context needed while applying events.
pub(crate) struct ApplyCtx<'a> {
    pub ledger: &'a mut Ledger,
    pub spill: &'a Spill,
    /// `capture.max_body_bytes` (0 = unlimited).
    pub max_body: u64,
}

/// What applying an event did to a record.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    /// Nothing visible changed.
    Unchanged,
    /// The summary changed.
    Changed,
    /// A body completed (always announced, even if the summary looks the same).
    Completed,
    /// A stream message was appended (the summary's message count changed too).
    Message(StreamMessage),
}

/// One captured exchange.
#[derive(Debug)]
pub(crate) struct Record {
    /// Derived row model, kept current after every event.
    pub summary: ExchangeSummary,
    /// The request's absolute URL (other request-line fields live in `summary`).
    pub url: String,
    pub request_headers: Headers,
    pub request_body: StoredBody,
    pub request_trailers: Option<Headers>,
    pub response: Option<ResponseHead>,
    pub response_body: StoredBody,
    pub response_trailers: Option<Headers>,
    pub timing: Timing,
    pub client: Arc<ClientInfo>,
    pub upstream: Option<Box<UpstreamInfo>>,
    pub error: Option<ExchangeError>,
    pub matched_rules: Vec<RuleId>,
    pub messages: Vec<StoredMessage>,
    pub extra: Option<Box<Extra>>,
    pub progress: Progress,
}

/// Display name of a client: process name, else the interceptor id.
pub(crate) fn source_of(client: &ClientInfo) -> Option<String> {
    client
        .process
        .as_ref()
        .map(|p| p.name.clone())
        .filter(|n| !n.is_empty())
        .or_else(|| client.interceptor.clone().filter(|i| !i.is_empty()))
}

fn normalize_text(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

/// Classify a body for the summary icon. `body` is the captured (still encoded) body; only a
/// small decoded prefix is ever produced.
pub(crate) fn detect_content_kind(
    content_type: Option<&str>,
    encodings: &[String],
    body: &[u8],
    path: &str,
) -> ContentKind {
    use akev_content::detect;
    if encodings.is_empty() || body.is_empty() {
        let n = body.len().min(SNIFF_BYTES);
        return detect::detect_kind(content_type, &body[..n], path);
    }
    let header_kind = content_type.map(detect::kind_from_content_type);
    let generic = header_kind.is_none_or(|k| {
        matches!(
            k,
            ContentKind::Unknown | ContentKind::Binary | ContentKind::Text
        )
    });
    if !generic {
        // A specific Content-Type wins over sniffing: no need to decode anything.
        return detect::detect_kind(content_type, &[], path);
    }
    let decoded = if body.len() <= SNIFF_FULL_DECODE_MAX {
        akev_content::decode::decode_body(body, encodings, SNIFF_MAX_OUTPUT)
    } else {
        akev_content::decode::decode_body(&body[..SNIFF_BYTES], encodings, SNIFF_MAX_OUTPUT)
    };
    match decoded {
        Ok(d) => detect::detect_kind(content_type, &d[..d.len().min(SNIFF_BYTES)], path),
        Err(_) => match header_kind {
            Some(k) => k,
            None => detect::detect_kind(None, &[], path),
        },
    }
}

/// The LLM tag of a request: provider and model when `host`/`path` are an LLM API endpoint
/// (and, for hosts that aren't well-known providers, the body looks like that API's
/// requests). `body` is the captured request body, decoded here when content-encoded. Cheap
/// for everything else: the body is only touched when the URL matches.
pub(crate) fn llm_tag(
    host: &str,
    port: u16,
    path: &str,
    headers: &Headers,
    body: Option<&[u8]>,
) -> Option<Box<LlmTag>> {
    akev_content::llm::route(host, port, path)?;
    let decoded;
    let body = match body {
        Some(b) if b.len() <= LLM_TAG_MAX_BODY => {
            let encodings = headers.content_encodings();
            if encodings.is_empty() {
                Some(b)
            } else {
                decoded = akev_content::decode::decode_body(b, &encodings, LLM_TAG_MAX_BODY).ok();
                decoded.as_deref()
            }
        }
        _ => None,
    };
    let (provider, model) = akev_content::llm::tag(host, port, path, body)?;
    Some(Box::new(LlmTag {
        provider: provider.to_string(),
        model,
    }))
}

/// Best-effort request line from the first bytes of an unparseable request.
pub(crate) fn guess_request(raw: &[u8], tls: bool) -> (String, Option<util::UrlParts>, String) {
    let head = &raw[..raw.len().min(8192)];
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split('\n').map(|l| l.trim_end_matches('\r'));
    let first = lines.next().unwrap_or("");
    let mut tokens = first.split(' ').filter(|t| !t.is_empty());
    let method = tokens
        .next()
        .filter(|m| m.len() <= 24 && m.bytes().all(|b| b.is_ascii_uppercase() || b == b'-'))
        .map(str::to_string);
    let Some(method) = method else {
        return (String::new(), None, String::new());
    };
    let target = tokens.next().unwrap_or("");
    if target.starts_with("http://") || target.starts_with("https://") {
        return (method, util::split_url(target), target.to_string());
    }
    let host = lines
        .take_while(|l| !l.is_empty())
        .find_map(|l| {
            l.split_once(':')
                .filter(|(n, _)| n.trim().eq_ignore_ascii_case("host"))
        })
        .map(|(_, v)| v.trim().to_string());
    if let Some(host) = host.filter(|h| !h.is_empty() && target.starts_with('/')) {
        let scheme = if tls { "https" } else { "http" };
        let url = format!("{scheme}://{host}{target}");
        return (method, util::split_url(&url), url);
    }
    let parts = util::UrlParts {
        scheme: String::new(),
        host: String::new(),
        port: 0,
        path: target.to_string(),
    };
    (method, Some(parts), String::new())
}

pub(crate) fn skeleton_summary(
    id: ExchangeId,
    kind: ExchangeKind,
    started: UnixMicros,
) -> ExchangeSummary {
    ExchangeSummary {
        id,
        kind,
        state: ExchangeState::Requesting,
        started_at: started.as_millis_f64(),
        method: String::new(),
        scheme: String::new(),
        host: String::new(),
        port: 0,
        path: String::new(),
        http_version: HttpVersion::default(),
        status: None,
        content_kind: ContentKind::Unknown,
        content_type: None,
        request_size: 0,
        response_size: 0,
        duration: None,
        error: None,
        paused: None,
        source: None,
        rule: None,
        modified: false,
        mocked: false,
        pinned: false,
        color: None,
        message_count: 0,
        has_note: false,
        llm: None,
    }
}

impl Record {
    fn blank(summary: ExchangeSummary, client: Arc<ClientInfo>, timing: Timing) -> Record {
        Record {
            summary,
            url: String::new(),
            request_headers: Headers::new(),
            request_body: StoredBody::default(),
            request_trailers: None,
            response: None,
            response_body: StoredBody::default(),
            response_trailers: None,
            timing,
            client,
            upstream: None,
            error: None,
            matched_rules: Vec::new(),
            messages: Vec::new(),
            extra: None,
            progress: Progress::default(),
        }
    }

    /// A new HTTP exchange from `RequestStarted`.
    pub(crate) fn new_http(
        id: ExchangeId,
        kind: ExchangeKind,
        head: RequestHead,
        client: Arc<ClientInfo>,
        timing: Timing,
    ) -> Record {
        let mut summary = skeleton_summary(id, kind, timing.start);
        summary.source = source_of(&client);
        let mut rec = Record::blank(summary, client, timing);
        rec.set_request_head(head);
        rec.update_llm(None);
        rec.refresh();
        rec
    }

    /// A TLS handshake failure row.
    pub(crate) fn new_tls_failure(
        id: ExchangeId,
        client: Arc<ClientInfo>,
        host: String,
        port: u16,
        error: ExchangeError,
        at: UnixMicros,
    ) -> Record {
        let mut summary = skeleton_summary(id, ExchangeKind::TlsFailure, at);
        summary.source = source_of(&client);
        summary.scheme = "https".into();
        summary.port = port;
        let mut rec = Record::blank(
            summary,
            client,
            Timing {
                start: at,
                ..Timing::default()
            },
        );
        rec.url = format!("https://{}", util::authority("https", &host, port));
        rec.summary.host = host;
        rec.error = Some(error);
        rec.refresh();
        rec
    }

    /// A tunnelled (not intercepted) connection row.
    pub(crate) fn new_tunnel(
        id: ExchangeId,
        client: Arc<ClientInfo>,
        host: String,
        port: u16,
        reason: String,
        at: UnixMicros,
    ) -> Record {
        let mut summary = skeleton_summary(id, ExchangeKind::Tunnel, at);
        summary.source = source_of(&client);
        summary.scheme = if port == 443 { "https" } else { "tcp" }.into();
        summary.method = if client.via == ProxyMode::Connect {
            "CONNECT".into()
        } else {
            String::new()
        };
        summary.port = port;
        let mut rec = Record::blank(
            summary,
            client,
            Timing {
                start: at,
                ..Timing::default()
            },
        );
        rec.url = format!(
            "{}://{}",
            rec.summary.scheme,
            util::authority(&rec.summary.scheme, &host, port)
        );
        rec.summary.host = host;
        rec.extra_mut().tunnel = Some(TunnelStats {
            bytes_sent: 0,
            bytes_received: 0,
            reason,
        });
        rec.refresh();
        rec
    }

    /// A row for garbage received from a client; `raw_prefix` becomes the request body.
    pub(crate) fn new_client_error(
        id: ExchangeId,
        client: Arc<ClientInfo>,
        error: ExchangeError,
        raw_prefix: Bytes,
        at: UnixMicros,
        max_body: u64,
    ) -> Record {
        let mut summary = skeleton_summary(id, ExchangeKind::ClientError, at);
        summary.source = source_of(&client);
        let (method, parts, url) = guess_request(&raw_prefix, client.tls.is_some());
        summary.method = method;
        if let Some(p) = parts {
            summary.scheme = p.scheme;
            summary.host = p.host;
            summary.port = p.port;
            summary.path = p.path;
        }
        let mut rec = Record::blank(
            summary,
            client,
            Timing {
                start: at,
                ..Timing::default()
            },
        );
        rec.url = url;
        rec.error = Some(error);
        rec.request_body = StoredBody::from_captured(CapturedBody::complete(raw_prefix), max_body);
        rec.progress.req_done = true;
        rec.refresh();
        rec
    }

    pub(crate) fn id(&self) -> ExchangeId {
        self.summary.id
    }

    pub(crate) fn extra(&self) -> Option<&Extra> {
        self.extra.as_deref()
    }

    pub(crate) fn extra_mut(&mut self) -> &mut Extra {
        self.extra.get_or_insert_with(Box::default)
    }

    pub(crate) fn note(&self) -> Option<&str> {
        self.extra().and_then(|e| e.note.as_deref())
    }

    pub(crate) fn close(&self) -> Option<&StreamClose> {
        self.extra().and_then(|e| e.close.as_ref())
    }

    /// Rebuild the current (effective) request head.
    pub(crate) fn request_head(&self) -> RequestHead {
        RequestHead {
            method: self.summary.method.clone(),
            url: self.url.clone(),
            scheme: self.summary.scheme.clone(),
            host: self.summary.host.clone(),
            port: self.summary.port,
            path: self.summary.path.clone(),
            http_version: self.summary.http_version,
            headers: self.request_headers.clone(),
        }
    }

    /// Replace the effective request head. Returns whether any summary field changed.
    fn set_request_head(&mut self, head: RequestHead) -> bool {
        let RequestHead {
            method,
            url,
            scheme,
            host,
            port,
            path,
            http_version,
            headers,
        } = head;
        let s = &mut self.summary;
        let changed = s.method != method
            || s.scheme != scheme
            || s.host != host
            || s.port != port
            || s.path != path
            || s.http_version != http_version;
        s.method = method;
        s.scheme = scheme;
        s.host = host;
        s.port = port;
        s.path = path;
        s.http_version = http_version;
        self.url = url;
        self.request_headers = headers;
        changed
    }

    pub(crate) fn slot(&self, slot: Slot) -> Option<&StoredBody> {
        match slot {
            Slot::Request => Some(&self.request_body),
            Slot::Response => Some(&self.response_body),
            Slot::OriginalRequest => self.extra()?.original_request_body.as_ref(),
            Slot::OriginalResponse => self.extra()?.original_response_body.as_ref(),
            Slot::Messages => None,
        }
    }

    pub(crate) fn slot_mut(&mut self, slot: Slot) -> Option<&mut StoredBody> {
        match slot {
            Slot::Request => Some(&mut self.request_body),
            Slot::Response => Some(&mut self.response_body),
            Slot::OriginalRequest => self.extra.as_mut()?.original_request_body.as_mut(),
            Slot::OriginalResponse => self.extra.as_mut()?.original_response_body.as_mut(),
            Slot::Messages => None,
        }
    }

    /// Store `body` in `slot`, disposing of whatever was there.
    fn put_body(&mut self, slot: Slot, body: StoredBody, ledger: &mut Ledger, spill: &Spill) {
        let id = self.id();
        ledger.add_body(id, slot, &body);
        let old = match slot {
            Slot::Request => Some(std::mem::replace(&mut self.request_body, body)),
            Slot::Response => Some(std::mem::replace(&mut self.response_body, body)),
            Slot::OriginalRequest => self.extra_mut().original_request_body.replace(body),
            Slot::OriginalResponse => self.extra_mut().original_response_body.replace(body),
            Slot::Messages => None,
        };
        if let Some(old) = old {
            ledger.free_body(id, slot, &old, spill);
            // `free_body` dropped the index entry shared with the new body; restore it.
            ledger.add_index_only(id, slot, self.slot(slot));
        }
    }

    /// Register every body/message of a freshly built record with the ledger.
    pub(crate) fn account_all(&self, ledger: &mut Ledger) {
        let id = self.id();
        for slot in [
            Slot::Request,
            Slot::Response,
            Slot::OriginalRequest,
            Slot::OriginalResponse,
        ] {
            if let Some(body) = self.slot(slot) {
                ledger.add_body(id, slot, body);
            }
        }
        for m in &self.messages {
            match &m.payload {
                Payload::Memory(b) if !b.is_empty() => {
                    ledger.memory_bytes += b.len() as u64;
                    ledger.mem_index.insert((id, Slot::Messages));
                }
                Payload::Memory(_) => {}
                Payload::Spilled { len, .. } => ledger.spilled_bytes += len,
            }
        }
    }

    /// Release everything this record holds (memory accounting + spill blobs).
    pub(crate) fn release(&self, ledger: &mut Ledger, spill: &Spill) {
        let id = self.id();
        for slot in [
            Slot::Request,
            Slot::Response,
            Slot::OriginalRequest,
            Slot::OriginalResponse,
        ] {
            if let Some(body) = self.slot(slot) {
                ledger.free_body(id, slot, body, spill);
            }
        }
        for m in &self.messages {
            match &m.payload {
                Payload::Memory(b) => {
                    ledger.memory_bytes = ledger.memory_bytes.saturating_sub(b.len() as u64)
                }
                Payload::Spilled { len, .. } => {
                    ledger.spilled_bytes = ledger.spilled_bytes.saturating_sub(*len)
                }
            }
        }
        ledger.mem_index.remove(&(id, Slot::Messages));
        ledger.spill_index.remove(&(id, Slot::Messages));
        if let Some(extra) = self.extra() {
            for h in &extra.msg_blobs {
                spill.free(h);
            }
        }
    }

    /// Discard a spilled body (or every spilled message payload with [`Slot::Messages`]) to
    /// free disk space: the exchange keeps its sizes, the body is marked dropped and a
    /// notice explains why. Returns the logical bytes dropped (0 if nothing was spilled).
    pub(crate) fn drop_spilled(&mut self, slot: Slot, ledger: &mut Ledger, spill: &Spill) -> u64 {
        let id = self.id();
        ledger.spill_index.remove(&(id, slot));
        if slot == Slot::Messages {
            let mut dropped = 0u64;
            for m in &mut self.messages {
                if let Payload::Spilled { len, .. } = m.payload {
                    dropped += len;
                    m.payload = Payload::Memory(Bytes::new());
                }
            }
            let blobs = self
                .extra
                .as_mut()
                .map(|e| std::mem::take(&mut e.msg_blobs))
                .unwrap_or_default();
            for h in &blobs {
                spill.free(h);
            }
            if dropped > 0 {
                ledger.spilled_bytes = ledger.spilled_bytes.saturating_sub(dropped);
                self.add_notice(DISK_DROPPED_MESSAGES);
            }
            return dropped;
        }
        let Some(body) = self.slot_mut(slot) else {
            return 0;
        };
        let BodyData::Spilled(handle) = body.data else {
            return 0;
        };
        body.data = BodyData::Memory(Bytes::new());
        body.truncated = true;
        body.dropped = true;
        spill.free(&handle);
        ledger.spilled_bytes = ledger.spilled_bytes.saturating_sub(handle.raw_len);
        self.add_notice(match slot {
            Slot::Request => DISK_DROPPED_REQUEST,
            Slot::Response => DISK_DROPPED_RESPONSE,
            _ => DISK_DROPPED_ORIGINAL,
        });
        handle.raw_len
    }

    /// Record a notice (once per distinct message, at most [`MAX_NOTICES`]).
    pub(crate) fn add_notice(&mut self, message: &str) -> bool {
        let extra = self.extra_mut();
        if extra.notices.len() >= MAX_NOTICES || extra.notices.iter().any(|n| n == message) {
            return false;
        }
        extra.notices.push(message.to_string());
        true
    }

    /// Release spill blobs only (used when the whole ledger is reset at once).
    pub(crate) fn release_spill(&self, spill: &Spill) {
        for slot in [
            Slot::Request,
            Slot::Response,
            Slot::OriginalRequest,
            Slot::OriginalResponse,
        ] {
            if let Some(StoredBody {
                data: BodyData::Spilled(h),
                ..
            }) = self.slot(slot)
            {
                spill.free(h);
            }
        }
        if let Some(extra) = self.extra() {
            for h in &extra.msg_blobs {
                spill.free(h);
            }
        }
    }

    fn derive_state(&self) -> ExchangeState {
        use ExchangeState::*;
        if self.summary.paused.is_some() {
            return Paused;
        }
        if self.error.is_some() {
            return Aborted;
        }
        let p = self.progress;
        match self.summary.kind {
            ExchangeKind::TlsFailure | ExchangeKind::ClientError => return Aborted,
            ExchangeKind::Tunnel => return if p.stream_closed { Complete } else { Streaming },
            _ => {}
        }
        if p.stream_open {
            let done =
                p.stream_closed || (self.summary.kind != ExchangeKind::WebSocket && p.resp_done);
            return if done { Complete } else { Streaming };
        }
        if p.resp_done {
            Complete
        } else if p.resp_started {
            Responding
        } else if p.req_done {
            Waiting
        } else {
            Requesting
        }
    }

    fn derive_duration(&self) -> Option<f64> {
        let close_at = self.close().map(|c| c.at);
        match self.summary.kind {
            ExchangeKind::Tunnel => close_at,
            ExchangeKind::TlsFailure | ExchangeKind::ClientError => None,
            _ if self.progress.stream_open => close_at.or(self.timing.aborted).or_else(|| {
                (self.summary.kind != ExchangeKind::WebSocket && self.progress.resp_done)
                    .then_some(self.timing.response_body)
                    .flatten()
            }),
            _ => self.timing.total(),
        }
    }

    fn derive_sizes(&self) -> (u64, u64) {
        if self.summary.kind == ExchangeKind::Tunnel
            && let Some(t) = self.extra().and_then(|e| e.tunnel.as_ref())
        {
            return (t.bytes_sent, t.bytes_received);
        }
        (self.request_body.wire_size, self.response_body.wire_size)
    }

    /// Recompute derived scalar summary fields; returns whether anything changed.
    pub(crate) fn refresh(&mut self) -> bool {
        let mut changed = false;
        macro_rules! set {
            ($field:expr, $value:expr) => {{
                let v = $value;
                if $field != v {
                    $field = v;
                    changed = true;
                }
            }};
        }
        if !self.progress.frozen {
            set!(self.summary.state, self.derive_state());
            set!(self.summary.duration, self.derive_duration());
        }
        set!(
            self.summary.status,
            self.response.as_ref().map(|r| r.status)
        );
        let (req, resp) = self.derive_sizes();
        set!(self.summary.request_size, req);
        set!(self.summary.response_size, resp);
        set!(self.summary.error, self.error.as_ref().map(|e| e.kind));
        set!(
            self.summary.message_count,
            u32::try_from(self.messages.len()).unwrap_or(u32::MAX)
        );
        set!(self.summary.has_note, self.note().is_some());
        changed
    }

    /// Update content type/kind from the current response (header + optional body bytes).
    fn update_content(&mut self, body: Option<&[u8]>) -> bool {
        let Some(resp) = &self.response else {
            return false;
        };
        let ct_raw = resp.headers.get("content-type");
        let essence = resp.headers.content_type_essence();
        let kind = match body {
            Some(bytes) => {
                let path = self.summary.path.split('?').next().unwrap_or("");
                detect_content_kind(ct_raw, &resp.headers.content_encodings(), bytes, path)
            }
            None => ct_raw
                .map(akev_content::detect::kind_from_content_type)
                .unwrap_or_default(),
        };
        let changed = self.summary.content_type != essence || self.summary.content_kind != kind;
        self.summary.content_type = essence;
        self.summary.content_kind = kind;
        changed
    }

    /// Recompute `summary.llm` from the request line and (when given) the request body.
    /// Returns whether it changed.
    fn update_llm(&mut self, body: Option<&[u8]>) -> bool {
        let s = &self.summary;
        let tag = llm_tag(&s.host, s.port, &s.path, &self.request_headers, body);
        if self.summary.llm == tag {
            return false;
        }
        self.summary.llm = tag;
        true
    }

    /// [`Record::update_llm`] with the stored request body.
    fn update_llm_from_body(&mut self, spill: &Spill) -> bool {
        if akev_content::llm::route(&self.summary.host, self.summary.port, &self.summary.path)
            .is_none()
        {
            return self.summary.llm.take().is_some();
        }
        let body = self.request_body.load(spill).ok();
        self.update_llm(body.as_deref().filter(|b| !b.is_empty()))
    }

    /// Apply a non-creating event.
    pub(crate) fn apply(&mut self, event: ProxyEvent, ctx: &mut ApplyCtx<'_>) -> Outcome {
        let mut force = false;
        let mut changed = false;
        let mut message = None;
        match event {
            ProxyEvent::RequestStarted { .. }
            | ProxyEvent::TlsFailed { .. }
            | ProxyEvent::TunnelOpened { .. }
            | ProxyEvent::ClientError { .. } => return Outcome::Unchanged,
            ProxyEvent::RequestCompleted {
                body, trailers, at, ..
            } => {
                self.timing.request_body = Some(at);
                self.progress.req_done = true;
                let body = StoredBody::from_captured(body, ctx.max_body);
                let slot = if self.progress.req_body_modified {
                    // The modified body was already recorded; this is what the client sent.
                    Slot::OriginalRequest
                } else {
                    Slot::Request
                };
                self.put_body(slot, body, ctx.ledger, ctx.spill);
                if trailers.is_some() {
                    self.request_trailers = trailers;
                }
                changed |= self.update_llm_from_body(ctx.spill);
                force = true;
            }
            ProxyEvent::RequestModified { head, body, .. } => {
                let original = self.request_head();
                let extra = self.extra_mut();
                if extra.original_request.is_none() {
                    extra.original_request = Some(original);
                }
                changed |= self.set_request_head(head);
                // Encodings may have changed: forget any cached decoded size.
                self.request_body.decoded_size = OnceLock::new();
                if let Some(body) = body {
                    let body = StoredBody::from_captured(body, ctx.max_body);
                    let has_original = self
                        .extra()
                        .is_some_and(|e| e.original_request_body.is_some());
                    if self.progress.req_done && !has_original {
                        let id = self.id();
                        let current = std::mem::take(&mut self.request_body);
                        ctx.ledger.detach_body(id, Slot::Request, &current);
                        self.put_body(Slot::OriginalRequest, current, ctx.ledger, ctx.spill);
                    } else if !self.progress.req_done {
                        self.progress.req_body_modified = true;
                    }
                    self.put_body(Slot::Request, body, ctx.ledger, ctx.spill);
                    force = true;
                }
                if !self.summary.modified {
                    self.summary.modified = true;
                    changed = true;
                }
                changed |= self.update_llm_from_body(ctx.spill);
            }
            ProxyEvent::Notice {
                message, dropped, ..
            } => {
                if let Some(side) = dropped {
                    let body = match side {
                        Side::Request => &mut self.request_body,
                        Side::Response => &mut self.response_body,
                    };
                    if body.captured_len() == 0 && body.wire_size > 0 {
                        body.dropped = true;
                    }
                }
                force |= self.add_notice(&message);
            }
            ProxyEvent::Highlighted { color, note, .. } => {
                if self.summary.color.is_none() {
                    self.summary.color = Some(color);
                    changed = true;
                }
                if let Some(note) = note.filter(|n| !n.trim().is_empty())
                    && self.note().is_none()
                {
                    self.extra_mut().note = Some(note);
                    self.summary.has_note = true;
                    changed = true;
                }
            }
            ProxyEvent::RuleMatched { rule, mocked, .. } => {
                if !self.matched_rules.contains(&rule) {
                    self.matched_rules.push(rule.clone());
                }
                if self.summary.rule.as_ref() != Some(&rule) {
                    self.summary.rule = Some(rule);
                    changed = true;
                }
                if mocked && !self.summary.mocked {
                    self.summary.mocked = true;
                    changed = true;
                }
            }
            ProxyEvent::Upstream {
                info,
                dns_duration,
                connect_duration,
                tls_duration,
                request_sent,
                ..
            } => {
                self.upstream = Some(Box::new(info));
                let t = &mut self.timing;
                t.dns_duration = dns_duration.or(t.dns_duration);
                t.connect_duration = connect_duration.or(t.connect_duration);
                t.tls_duration = tls_duration.or(t.tls_duration);
                t.request_sent = request_sent.or(t.request_sent);
            }
            ProxyEvent::ResponseStarted { head, at, .. } => {
                // Requests issued by Akev itself (Compose / Repeat) have no client connection:
                // their protocol is the one spoken upstream (HTTP/2, HTTP/3...).
                if self.client.via == ProxyMode::Composer
                    && self.summary.http_version != head.http_version
                {
                    self.summary.http_version = head.http_version;
                    changed = true;
                }
                self.timing.response_headers = Some(at);
                self.progress.resp_started = true;
                self.response = Some(head);
                changed |= self.update_content(None);
            }
            ProxyEvent::ResponseCompleted {
                body, trailers, at, ..
            } => {
                self.timing.response_body = Some(at);
                self.progress.resp_done = true;
                changed |= self.update_content(Some(&body.bytes));
                let body = StoredBody::from_captured(body, ctx.max_body);
                self.put_body(Slot::Response, body, ctx.ledger, ctx.spill);
                if trailers.is_some() {
                    self.response_trailers = trailers;
                }
                force = true;
            }
            ProxyEvent::ResponseModified {
                original_head,
                original_body,
                ..
            } => {
                let extra = self.extra_mut();
                if extra.original_response.is_none() {
                    extra.original_response = Some(original_head);
                }
                let has_original = self
                    .extra()
                    .is_some_and(|e| e.original_response_body.is_some());
                if let Some(body) = original_body.filter(|_| !has_original) {
                    let body = StoredBody::from_captured(body, ctx.max_body);
                    self.put_body(Slot::OriginalResponse, body, ctx.ledger, ctx.spill);
                }
                if !self.summary.modified {
                    self.summary.modified = true;
                    changed = true;
                }
            }
            ProxyEvent::Paused { stage, .. } => {
                if self.summary.paused != Some(stage) {
                    self.summary.paused = Some(stage);
                    changed = true;
                }
            }
            ProxyEvent::Resumed { paused_ms, .. } => {
                let t = &mut self.timing;
                t.paused_duration = Some(t.paused_duration.unwrap_or(0.0) + paused_ms.max(0.0));
                if self.summary.paused.take().is_some() {
                    changed = true;
                }
            }
            ProxyEvent::Aborted { error, at, .. } => {
                self.timing.aborted = Some(at);
                self.error = Some(error);
                if self.summary.paused.take().is_some() {
                    changed = true;
                }
            }
            ProxyEvent::StreamOpened { kind, .. } => {
                self.progress.stream_open = true;
                if self.summary.kind != kind {
                    self.summary.kind = kind;
                    changed = true;
                }
            }
            ProxyEvent::StreamMessage {
                message: msg,
                payload,
                ..
            } => {
                let payload = if ctx.max_body > 0 && payload.len() as u64 > ctx.max_body {
                    let keep = usize::try_from(ctx.max_body)
                        .unwrap_or(usize::MAX)
                        .min(payload.len());
                    Bytes::copy_from_slice(&payload[..keep])
                } else {
                    compact(payload)
                };
                if !payload.is_empty() {
                    ctx.ledger.memory_bytes += payload.len() as u64;
                    ctx.ledger.mem_index.insert((self.id(), Slot::Messages));
                }
                if msg.modified && !self.summary.modified {
                    self.summary.modified = true;
                }
                self.messages.push(StoredMessage {
                    message: msg.clone(),
                    payload: Payload::Memory(payload),
                });
                message = Some(msg);
            }
            ProxyEvent::StreamClosed {
                code,
                reason,
                by,
                at,
                ..
            } => {
                self.progress.stream_closed = true;
                self.extra_mut().close = Some(StreamClose {
                    code,
                    reason,
                    by: Some(by),
                    at,
                });
            }
            ProxyEvent::TunnelClosed { mut stats, at, .. } => {
                self.progress.stream_closed = true;
                let extra = self.extra_mut();
                if stats.reason.is_empty()
                    && let Some(prev) = &extra.tunnel
                {
                    stats.reason = prev.reason.clone();
                }
                extra.tunnel = Some(stats);
                extra.close = Some(StreamClose {
                    code: None,
                    reason: None,
                    by: None,
                    at,
                });
            }
        }
        changed |= self.refresh();
        match message {
            Some(m) => Outcome::Message(m),
            None if force => Outcome::Completed,
            None if changed => Outcome::Changed,
            None => Outcome::Unchanged,
        }
    }

    /// Apply a user patch; returns whether the summary changed.
    pub(crate) fn apply_patch(&mut self, patch: &crate::ExchangePatch) -> bool {
        let mut changed = false;
        if let Some(pinned) = patch.pinned.filter(|p| *p != self.summary.pinned) {
            self.summary.pinned = pinned;
            changed = true;
        }
        if let Some(color) = &patch.color {
            let color = normalize_text(color.clone());
            if self.summary.color != color {
                self.summary.color = color;
                changed = true;
            }
        }
        if let Some(note) = &patch.note {
            let note = normalize_text(note.clone());
            if self.note() != note.as_deref() {
                if note.is_some() || self.extra.is_some() {
                    self.extra_mut().note = note;
                }
                changed = true;
            }
        }
        self.refresh() || changed
    }

    /// Encoded bodies whose decoded size isn't known yet but can be computed (bounded): the
    /// caller decodes them with [`decoded_size_of`] outside the store lock and caches the
    /// result with [`Record::set_decoded_size`].
    pub(crate) fn decoded_size_jobs(&self) -> Vec<(Side, BodyRef, Vec<String>)> {
        let response_headers = self.response.as_ref().map(|r| &r.headers);
        let mut jobs = Vec::new();
        for (side, body, headers) in [
            (
                Side::Request,
                &self.request_body,
                Some(&self.request_headers),
            ),
            (Side::Response, &self.response_body, response_headers),
        ] {
            let encodings = headers.map(Headers::content_encodings).unwrap_or_default();
            if encodings.is_empty()
                || body.decoded_size.get().is_some()
                || (body.wire_size == 0 && body.captured_len() == 0)
            {
                continue;
            }
            if body.truncated || body.captured_len() > DECODED_SIZE_MAX_INPUT {
                let _ = body.decoded_size.set(None);
                continue;
            }
            jobs.push((side, body.to_ref(), encodings));
        }
        jobs
    }

    /// Cache a decoded size computed outside the lock, if the body is still the same one.
    pub(crate) fn set_decoded_size(&self, side: Side, body: &BodyRef, size: Option<u64>) {
        let current = match side {
            Side::Request => &self.request_body,
            Side::Response => &self.response_body,
        };
        if current.same_data(body) {
            let _ = current.decoded_size.set(size);
        }
    }

    /// Full detail. With `compute_decoded`, encoded bodies get their decoded size computed
    /// (bounded, cached); otherwise only already-known values are reported.
    pub(crate) fn detail(&self, spill: &Spill, compute_decoded: bool) -> ExchangeDetail {
        let extra = self.extra();
        let resp_headers = self.response.as_ref().map(|r| &r.headers);
        ExchangeDetail {
            summary: self.summary.clone(),
            request: self.request_head(),
            request_body: self.request_body.meta(
                Some(&self.request_headers),
                spill,
                compute_decoded,
            ),
            request_trailers: self.request_trailers.clone(),
            response: self.response.clone(),
            response_body: self
                .response_body
                .meta(resp_headers, spill, compute_decoded),
            response_trailers: self.response_trailers.clone(),
            original_request: extra.and_then(|e| e.original_request.clone()),
            original_response: extra.and_then(|e| e.original_response.clone()),
            timing: self.timing.clone(),
            client: (*self.client).clone(),
            upstream: self.upstream.as_deref().cloned(),
            error: self.error.clone(),
            tunnel: extra.and_then(|e| e.tunnel.clone()),
            note: extra.and_then(|e| e.note.clone()),
            matched_rules: self.matched_rules.clone(),
            notices: extra.map(|e| e.notices.clone()).unwrap_or_default(),
        }
    }

    /// Up to `limit` of the messages accepted by `keep` (the oldest, or the newest with
    /// `tail`), in order, with references to their payloads, to load after the store lock
    /// is released.
    pub(crate) fn message_refs(
        &self,
        keep: impl Fn(&StreamMessage) -> bool,
        limit: usize,
        tail: bool,
    ) -> MessageRefs {
        let messages: Vec<StoredMessage> = if tail {
            let mut v: Vec<StoredMessage> = self
                .messages
                .iter()
                .rev()
                .filter(|m| keep(&m.message))
                .take(limit)
                .cloned()
                .collect();
            v.reverse();
            v
        } else {
            self.messages
                .iter()
                .filter(|m| keep(&m.message))
                .take(limit)
                .cloned()
                .collect()
        };
        MessageRefs {
            messages,
            blobs: self
                .extra()
                .map(|e| e.msg_blobs.clone())
                .unwrap_or_default(),
        }
    }
}

/// Stream messages copied out of a record with references to their payloads.
pub(crate) struct MessageRefs {
    messages: Vec<StoredMessage>,
    blobs: Vec<SpillHandle>,
}

impl MessageRefs {
    /// Load the payloads (each spilled blob is read at most once).
    pub(crate) fn load(self, spill: &Spill) -> Vec<MessageWithPayload> {
        let blobs = self.blobs.as_slice();
        let mut cache: Vec<Option<Bytes>> = vec![None; blobs.len()];
        let mut load = |blob: u32, offset: u64, len: u64| -> Bytes {
            let idx = blob as usize;
            let (Some(handle), Some(slot)) = (blobs.get(idx), cache.get_mut(idx)) else {
                return Bytes::new();
            };
            let data = slot.get_or_insert_with(|| {
                spill.read(handle).unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "failed to read spilled messages");
                    Bytes::new()
                })
            });
            let start = usize::try_from(offset).unwrap_or(usize::MAX);
            let end = start.saturating_add(usize::try_from(len).unwrap_or(usize::MAX));
            if end <= data.len() {
                data.slice(start..end)
            } else {
                Bytes::new()
            }
        };
        self.messages
            .into_iter()
            .map(|m| {
                let payload = match m.payload {
                    Payload::Memory(b) => b,
                    Payload::Spilled { blob, offset, len } => load(blob, offset, len),
                };
                MessageWithPayload {
                    message: m.message,
                    payload,
                }
            })
            .collect()
    }
}

impl Record {
    /// Build a frozen record from a fully specified exchange (imports, sessions, inserts).
    pub(crate) fn from_new(id: ExchangeId, n: crate::NewRecord) -> Record {
        let crate::NewRecord {
            detail,
            request_body,
            response_body,
            original_request_body,
            original_response_body,
            messages,
            close,
            exact,
        } = n;
        let ExchangeDetail {
            mut summary,
            request,
            request_body: request_meta,
            request_trailers,
            response,
            response_body: response_meta,
            response_trailers,
            original_request,
            original_response,
            timing,
            client,
            upstream,
            error,
            tunnel,
            note,
            matched_rules,
            notices,
        } = detail;
        summary.id = id;
        summary.started_at = timing.start.as_millis_f64();
        let client = Arc::new(client);
        if summary.source.is_none() {
            summary.source = source_of(&client);
        }
        summary.modified |= original_request.is_some() || original_response.is_some();

        let seed = |body: StoredBody, meta: &BodyMeta| {
            if let Some(size) = meta.decoded_size {
                let _ = body.decoded_size.set(Some(size));
            }
            body
        };
        let to_body = |c: CapturedBody| StoredBody::new(c.bytes, c.wire_size, c.truncated);
        let content_kind_unknown = summary.content_kind == ContentKind::Unknown;
        let response_bytes = response_body.bytes.clone();

        let mut rec = Record::blank(summary, client, timing);
        rec.set_request_head(request);
        rec.request_body = seed(to_body(request_body), &request_meta);
        rec.request_trailers = request_trailers;
        rec.response = response;
        rec.response_body = seed(to_body(response_body), &response_meta);
        rec.response_trailers = response_trailers;
        rec.upstream = upstream.map(Box::new);
        rec.error = error;
        rec.matched_rules = matched_rules;
        rec.messages = messages
            .into_iter()
            .map(|m| StoredMessage {
                message: m.message,
                payload: Payload::Memory(m.payload),
            })
            .collect();
        let note = normalize_text(note);
        let original_request_body = original_request_body.map(to_body);
        let original_response_body = original_response_body.map(to_body);
        if original_request.is_some()
            || original_response.is_some()
            || original_request_body.is_some()
            || original_response_body.is_some()
            || tunnel.is_some()
            || close.is_some()
            || note.is_some()
            || !notices.is_empty()
        {
            rec.extra = Some(Box::new(Extra {
                original_request,
                original_request_body,
                original_response,
                original_response_body,
                tunnel,
                close,
                note,
                msg_blobs: Vec::new(),
                notices,
            }));
        }
        rec.summary.content_type = rec
            .response
            .as_ref()
            .and_then(|r| r.headers.content_type_essence());
        if rec.response.is_some() && content_kind_unknown && !exact {
            rec.update_content(Some(&response_bytes));
        }
        if rec.summary.duration.is_none() && !exact {
            rec.summary.duration = rec.close().map(|c| c.at).or(rec.timing.total());
        }
        let request_body = rec.request_body.memory_bytes().cloned();
        rec.update_llm(request_body.as_deref());
        rec.progress.frozen = true;
        rec.refresh();
        rec
    }
}

impl Ledger {
    /// Re-add the index entry for `slot` for its (new) body.
    fn add_index_only(&mut self, id: ExchangeId, slot: Slot, body: Option<&StoredBody>) {
        if body.and_then(StoredBody::memory_bytes).is_some() {
            self.mem_index.insert((id, slot));
        }
        if body.is_some_and(|b| matches!(b.data, BodyData::Spilled(_))) {
            self.spill_index.insert((id, slot));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guess_request_line() {
        let (m, p, url) = guess_request(b"GET /x?y HTTP/1.1\r\nHost: a.com:8080\r\n\r\n", false);
        assert_eq!(m, "GET");
        let p = p.unwrap();
        assert_eq!(
            (p.host.as_str(), p.port, p.path.as_str()),
            ("a.com", 8080, "/x?y")
        );
        assert_eq!(url, "http://a.com:8080/x?y");
        let (m, p, _) = guess_request(b"POST http://b.com/z HTTP/1.1\r\n", false);
        assert_eq!((m.as_str(), p.unwrap().host.as_str()), ("POST", "b.com"));
        let (m, p, url) = guess_request(b"\x16\x03\x01garbage", false);
        assert!(m.is_empty() && p.is_none() && url.is_empty());
        let (m, p, _) = guess_request(b"BREW /pot", true);
        assert_eq!(m, "BREW");
        assert_eq!(p.unwrap().path, "/pot");
    }

    #[test]
    fn truncation_enforces_max_body() {
        let body =
            StoredBody::from_captured(CapturedBody::complete(Bytes::from_static(b"0123456789")), 4);
        assert_eq!(body.captured_len(), 4);
        assert_eq!(body.wire_size, 10);
        assert!(body.truncated);
        let body =
            StoredBody::from_captured(CapturedBody::complete(Bytes::from_static(b"0123")), 0);
        assert!(!body.truncated);
        // A capture that claims a smaller wire size than it holds is normalised.
        let odd = StoredBody::from_captured(
            CapturedBody {
                bytes: Bytes::from_static(b"abc"),
                wire_size: 1,
                truncated: false,
            },
            0,
        );
        assert_eq!(odd.wire_size, 3);
    }

    #[test]
    fn put_body_replacement_keeps_ledger_consistent() {
        let spill = Spill::new(None);
        let mut ledger = Ledger::default();
        let head = RequestHead {
            method: "GET".into(),
            url: "http://a/".into(),
            scheme: "http".into(),
            host: "a".into(),
            port: 80,
            path: "/".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let mut rec = Record::new_http(
            ExchangeId(1),
            ExchangeKind::Http,
            head,
            Arc::new(ClientInfo::default()),
            Timing::default(),
        );
        rec.put_body(
            Slot::Request,
            StoredBody::new(Bytes::from_static(b"12345"), 5, false),
            &mut ledger,
            &spill,
        );
        assert_eq!(ledger.memory_bytes, 5);
        rec.put_body(
            Slot::Request,
            StoredBody::new(Bytes::from_static(b"12"), 2, false),
            &mut ledger,
            &spill,
        );
        assert_eq!(ledger.memory_bytes, 2);
        assert!(ledger.mem_index.contains(&(ExchangeId(1), Slot::Request)));
        rec.put_body(Slot::Request, StoredBody::default(), &mut ledger, &spill);
        assert_eq!(ledger.memory_bytes, 0);
        assert!(ledger.mem_index.is_empty());
        rec.release(&mut ledger, &spill);
        assert_eq!(ledger.memory_bytes, 0);
    }
}
