//! Importers for other tools' capture files:
//!
//! * [`charles`] — Charles Proxy JSON sessions (`.chlsj`).
//! * [`mitmproxy`] — mitmproxy flow files (`mitmdump -w`, the UI's "Save"; tnetstring based).
//! * [`fiddler`] — Fiddler session archives (`.saz`).
//!
//! Both build the same [`Draft`] per exchange and go through [`crate::Store`]'s regular
//! insertion (fresh ids, derived summaries, memory budget and spilling). Bodies are stored as
//! the tool kept them; when a message declares a `Content-Encoding` the stored bytes aren't
//! encoded with (Charles keeps decoded bodies), that header is removed so stored headers
//! describe stored bytes (the HAR import rule, see `har.rs`).

pub(crate) mod charles;
pub(crate) mod fiddler;
pub(crate) mod mitmproxy;

use crate::record::{StreamClose, skeleton_summary};
use crate::{MessageWithPayload, NewRecord};
use akev_core::*;

/// Everything an importer knows about one exchange.
pub(crate) struct Draft {
    pub kind: ExchangeKind,
    pub request: RequestHead,
    pub request_body: CapturedBody,
    pub request_trailers: Option<Headers>,
    pub response: Option<ResponseHead>,
    pub response_body: CapturedBody,
    pub response_trailers: Option<Headers>,
    pub timing: Timing,
    pub client: ClientInfo,
    pub upstream: Option<UpstreamInfo>,
    pub error: Option<ExchangeError>,
    pub tunnel: Option<TunnelStats>,
    pub note: Option<String>,
    pub pinned: bool,
    /// Colour tag (`red`, `orange`...).
    pub color: Option<String>,
    pub messages: Vec<MessageWithPayload>,
    pub close: Option<StreamClose>,
}

impl Draft {
    /// A draft for `request` starting at `start`, without response.
    pub(crate) fn new(kind: ExchangeKind, request: RequestHead, start: UnixMicros) -> Self {
        Draft {
            kind,
            request,
            request_body: CapturedBody::empty(),
            request_trailers: None,
            response: None,
            response_body: CapturedBody::empty(),
            response_trailers: None,
            timing: Timing {
                start,
                ..Timing::default()
            },
            client: ClientInfo::default(),
            upstream: None,
            error: None,
            tunnel: None,
            note: None,
            pinned: false,
            color: None,
            messages: Vec::new(),
            close: None,
        }
    }

    /// The record to insert. Exchanges without a response (and not tunnels) get an error
    /// when they have none, so they never look like they're still in flight.
    pub(crate) fn into_record(mut self) -> NewRecord {
        if self.response.is_none() && self.kind != ExchangeKind::Tunnel && self.error.is_none() {
            self.error = Some(ExchangeError {
                kind: ErrorKind::Internal,
                message: "The request did not complete".into(),
                code: None,
            });
        }
        let state = if self.error.is_some() {
            ExchangeState::Aborted
        } else {
            ExchangeState::Complete
        };
        if state == ExchangeState::Aborted && self.timing.aborted.is_none() {
            self.timing.aborted = Some(
                self.timing
                    .response_body
                    .or(self.timing.response_headers)
                    .or(self.timing.request_sent)
                    .unwrap_or(0.0),
            );
            self.timing.response_body = None;
        }
        let mut summary = skeleton_summary(ExchangeId(0), self.kind, self.timing.start);
        summary.state = state;
        summary.pinned = self.pinned;
        summary.color = self.color;
        let note = self.note.filter(|n| !n.trim().is_empty());
        let detail = ExchangeDetail {
            summary,
            request: self.request,
            request_body: BodyMeta::default(),
            request_trailers: self.request_trailers,
            response: self.response,
            response_body: BodyMeta::default(),
            response_trailers: self.response_trailers,
            original_request: None,
            original_response: None,
            timing: self.timing,
            client: self.client,
            upstream: self.upstream,
            error: self.error,
            tunnel: self.tunnel,
            note,
            matched_rules: Vec::new(),
            notices: Vec::new(),
        };
        NewRecord {
            detail,
            request_body: self.request_body,
            response_body: self.response_body,
            original_request_body: None,
            original_response_body: None,
            messages: self.messages,
            close: self.close,
            exact: false,
        }
    }
}

/// Error kind for a tool's free-text error message ("Connection refused",
/// "UnknownHostException", "Read timed out", ...).
pub(crate) fn error_kind_for(message: &str) -> ErrorKind {
    let upper = message.to_ascii_uppercase().replace([' ', '-'], "_");
    if upper.contains("UNKNOWN_HOST")
        || upper.contains("UNKNOWNHOST")
        || upper.contains("NAME_OR_SERVICE")
        || upper.contains("NODENAME")
        || upper.contains("GETADDRINFO")
        || upper.contains("NO_ADDRESS")
    {
        return ErrorKind::DnsFailed;
    }
    // `Connection refused`, Python's `[Errno 111] Connect call failed` (ECONNREFUSED is 111
    // on Linux, 61 on macOS, 10061 on Windows).
    if upper.contains("REFUSED")
        || upper.contains("CONNECT_CALL_FAILED")
        || ["ERRNO_111]", "ERRNO_61]", "ERRNO_10061]"]
            .iter()
            .any(|e| upper.contains(e))
    {
        return ErrorKind::ConnectionRefused;
    }
    crate::har::error_kind_from_text(&upper)
}

/// `scheme://host[:port]path`, with the scheme's default port omitted.
pub(crate) fn build_url(scheme: &str, host: &str, port: u16, path: &str) -> String {
    format!(
        "{scheme}://{}{path}",
        crate::util::authority(scheme, host, port)
    )
}

/// Default port of a scheme.
pub(crate) fn default_port(scheme: &str) -> u16 {
    match scheme {
        "https" | "wss" => 443,
        _ => 80,
    }
}

/// Ms between two instants (`None` when `later` is before `start`).
pub(crate) fn offset_ms(start: UnixMicros, later: UnixMicros) -> Option<f64> {
    let d = later.0 - start.0;
    (d >= 0).then(|| d as f64 / 1000.0)
}

/// Whether a header name is an HTTP/2+ pseudo-header (`:method`, `:authority`...).
pub(crate) fn is_pseudo_header(name: &str) -> bool {
    name.starts_with(':')
}

/// A body whose wire size may exceed what was kept; `truncated` when the tool kept less than
/// the full (unencoded) body.
pub(crate) fn body(bytes: Vec<u8>, wire_size: Option<u64>, encoded_or_raw: bool) -> CapturedBody {
    let len = bytes.len() as u64;
    let wire = wire_size.unwrap_or(0);
    // Sizes only compare when the kept bytes are what went over the wire (still encoded, or
    // no content-coding at all); a body of decoded text has a different length.
    let truncated = wire > 0 && (len == 0 || (encoded_or_raw && wire > len));
    CapturedBody {
        wire_size: wire.max(len),
        truncated,
        bytes: bytes::Bytes::from(bytes),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_kinds_from_tool_messages() {
        assert_eq!(
            error_kind_for("Connection refused"),
            ErrorKind::ConnectionRefused
        );
        assert_eq!(
            error_kind_for("java.net.UnknownHostException: nope.invalid"),
            ErrorKind::DnsFailed
        );
        assert_eq!(error_kind_for("Read timed out"), ErrorKind::Timeout);
        assert_eq!(
            error_kind_for("Connection reset"),
            ErrorKind::ConnectionReset
        );
        assert_eq!(
            error_kind_for("[Errno -2] Name or service not known"),
            ErrorKind::DnsFailed
        );
        assert_eq!(
            error_kind_for("[Errno 111] Connect call failed ('127.0.0.1', 1)"),
            ErrorKind::ConnectionRefused
        );
        assert_eq!(error_kind_for("something odd"), ErrorKind::Internal);
    }

    #[test]
    fn urls_and_offsets() {
        assert_eq!(
            build_url("https", "a.example", 443, "/x?y=1"),
            "https://a.example/x?y=1"
        );
        assert_eq!(build_url("http", "::1", 8080, "/"), "http://[::1]:8080/");
        assert_eq!(offset_ms(UnixMicros(1_000), UnixMicros(3_500)), Some(2.5));
        assert_eq!(offset_ms(UnixMicros(3_500), UnixMicros(1_000)), None);
    }

    #[test]
    fn truncation_rule() {
        assert!(!body(b"abc".to_vec(), Some(3), true).truncated);
        assert!(body(b"ab".to_vec(), Some(3), true).truncated);
        // Decoded text longer or shorter than the encoded wire size: not truncated.
        assert!(!body(b"ab".to_vec(), Some(30), false).truncated);
        assert!(body(Vec::new(), Some(30), false).truncated);
        assert_eq!(body(b"abcd".to_vec(), Some(2), false).wire_size, 4);
    }
}
