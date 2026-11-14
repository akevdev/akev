//! Exports with secrets redacted ([`ExportOptions::redact`]): each exchange snapshot is
//! cleaned with [`akev_core::redact`] before it is written, so the store itself is never
//! modified.

use crate::Snapshot;
use akev_core::redact;
use akev_core::{CapturedBody, Headers, MessageKind};
use bytes::Bytes;

/// Options for HAR and session exports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ExportOptions {
    /// Replace secrets with `[redacted]`: credential headers (`Authorization`, `Cookie`,
    /// `Set-Cookie`, API-key headers...) and secret query parameters always; in text bodies
    /// (uncompressed only) and WebSocket text messages, bearer tokens, JWTs, API keys and
    /// secret-named JSON / form fields.
    pub redact: bool,
}

impl ExportOptions {
    /// Redacted export.
    pub fn redacted() -> Self {
        ExportOptions { redact: true }
    }

    pub(crate) fn apply(&self, snapshot: Snapshot) -> Snapshot {
        if self.redact {
            redacted(snapshot)
        } else {
            snapshot
        }
    }
}

/// A body whose bytes are text on the wire (no content encoding) with secrets redacted.
fn redact_body(body: CapturedBody, headers: &Headers) -> CapturedBody {
    let encoded = headers
        .content_encodings()
        .iter()
        .any(|e| !e.eq_ignore_ascii_case("identity"));
    if encoded || body.bytes.is_empty() {
        return body;
    }
    match redact::body(&body.bytes, headers.get("content-type")) {
        Some(bytes) => CapturedBody {
            bytes: Bytes::from(bytes),
            ..body
        },
        None => body,
    }
}

fn redacted(mut s: Snapshot) -> Snapshot {
    let d = &mut s.detail;
    // Bodies first: they are judged by the headers as recorded (Content-Type/-Encoding are
    // never redacted, so the order only matters for clarity).
    s.request_body = redact_body(std::mem::take(&mut s.request_body), &d.request.headers);
    if let Some(head) = &d.response {
        s.response_body = redact_body(std::mem::take(&mut s.response_body), &head.headers);
    }
    if let Some(body) = s.original_request_body.take() {
        let head = d.original_request.as_ref().unwrap_or(&d.request);
        s.original_request_body = Some(redact_body(body, &head.headers));
    }
    if let Some(body) = s.original_response_body.take() {
        let head = d.original_response.as_ref().or(d.response.as_ref());
        s.original_response_body = Some(match head {
            Some(head) => redact_body(body, &head.headers),
            None => body,
        });
    }
    for m in &mut s.messages {
        if m.message.kind == MessageKind::Text
            && let Ok(text) = std::str::from_utf8(&m.payload)
            && let std::borrow::Cow::Owned(clean) = redact::text(text)
        {
            m.payload = Bytes::from(clean);
        }
    }

    redact::request_head(&mut d.request);
    if let Some(head) = d.response.as_mut() {
        redact::response_head(head);
    }
    if let Some(head) = d.original_request.as_mut() {
        redact::request_head(head);
    }
    if let Some(head) = d.original_response.as_mut() {
        redact::response_head(head);
    }
    for trailers in [&mut d.request_trailers, &mut d.response_trailers]
        .into_iter()
        .flatten()
    {
        redact::headers(trailers);
    }
    if let std::borrow::Cow::Owned(path) = redact::path(&d.summary.path) {
        d.summary.path = path;
    }
    s
}
