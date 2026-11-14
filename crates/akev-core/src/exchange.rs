//! The captured-traffic model: what the store keeps and what the UI renders.

use crate::body::{BodyMeta, ContentKind};
use crate::http::{RequestHead, ResponseHead};
use crate::ids::{ConnectionId, ExchangeId, RuleId};
use crate::time::UnixMicros;
use serde::{Deserialize, Serialize};

/// What sort of thing an exchange row represents.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExchangeKind {
    /// A normal HTTP request/response.
    #[default]
    Http,
    /// An HTTP request that upgraded to a WebSocket (messages are attached to the exchange).
    WebSocket,
    /// A server-sent-events stream (text/event-stream response; events are parsed as messages).
    EventStream,
    /// A TLS connection the client aborted during the handshake (usually: client doesn't trust
    /// our CA, or certificate pinning).
    TlsFailure,
    /// A connection that was tunnelled without interception (TLS passthrough rule, non-HTTP
    /// protocol, or a host on the passthrough list).
    Tunnel,
    /// The client sent something unparseable.
    ClientError,
}

/// Lifecycle state of an exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExchangeState {
    /// Request head received, still receiving request body.
    #[default]
    Requesting,
    /// Request received, waiting for the upstream (or a rule) to respond.
    Waiting,
    /// Response head received, body streaming.
    Responding,
    /// Paused at a breakpoint, waiting for the user.
    Paused,
    /// Fully completed.
    Complete,
    /// Aborted/failed before completion (see `ExchangeDetail::error`).
    Aborted,
    /// WebSocket/SSE stream is open.
    Streaming,
}

/// Where a breakpoint paused the exchange.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BreakpointStage {
    Request,
    Response,
}

/// High-level classification of why an exchange failed. The UI uses this for icons, colours and
/// help text ("the upstream server's certificate has expired", etc).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ErrorKind {
    /// The client closed the connection before the exchange completed.
    ClientAborted,
    /// DNS lookup for the upstream host failed.
    DnsFailed,
    /// TCP connection to upstream refused.
    ConnectionRefused,
    /// Upstream connection reset mid-exchange.
    ConnectionReset,
    /// Upstream unreachable (network/host unreachable).
    HostUnreachable,
    /// Timed out connecting to / waiting for upstream.
    Timeout,
    /// Upstream TLS certificate was invalid/untrusted/expired/wrong host.
    UpstreamCertInvalid,
    /// Upstream TLS handshake failed for another reason (protocol/cipher mismatch, alert).
    UpstreamTlsFailed,
    /// Upstream sent a malformed HTTP response.
    UpstreamProtocolError,
    /// Client sent a malformed request.
    ClientProtocolError,
    /// The client rejected our TLS certificate (doesn't trust the Akev CA, or pinning).
    ClientTlsRejected,
    /// A rule deliberately closed the connection.
    RuleClosed,
    /// A rule deliberately reset the connection.
    RuleReset,
    /// A rule deliberately timed out (never responded).
    RuleTimeout,
    /// The user aborted the exchange at a breakpoint.
    BreakpointAborted,
    /// Upstream proxy failure.
    UpstreamProxyFailed,
    /// Any other internal error.
    Internal,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeError {
    pub kind: ErrorKind,
    /// Human readable message (from the underlying error).
    pub message: String,
    /// Raw error code if any (e.g. `ECONNRESET`, TLS alert name).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
}

/// Fine-grained timings. `start` is absolute; everything else is milliseconds since `start`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Timing {
    /// When the first byte of the request head was received.
    pub start: UnixMicros,
    /// Request head fully parsed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_headers: Option<f64>,
    /// Request body fully received from the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<f64>,
    /// Upstream DNS resolution duration (ms), when a fresh connection was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dns_duration: Option<f64>,
    /// Upstream TCP connect duration (ms), when a fresh connection was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connect_duration: Option<f64>,
    /// Upstream TLS handshake duration (ms), when a fresh connection was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_duration: Option<f64>,
    /// Request fully sent upstream.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_sent: Option<f64>,
    /// Response head received from upstream (or generated).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_headers: Option<f64>,
    /// Response body fully received.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_body: Option<f64>,
    /// Exchange aborted.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aborted: Option<f64>,
    /// Total time paused at breakpoints (ms) — excluded from "server time" in the UI.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_duration: Option<f64>,
}

impl Timing {
    /// Total duration if finished.
    pub fn total(&self) -> Option<f64> {
        self.response_body.or(self.aborted)
    }
}

/// TLS details of the client↔proxy connection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TlsInfo {
    /// SNI hostname sent by the client.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sni: Option<String>,
    /// ALPN protocols the client offered.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub alpn_offered: Vec<String>,
    /// ALPN protocol negotiated.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
    /// e.g. "TLSv1.3"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// e.g. "TLS13_AES_128_GCM_SHA256"
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    /// JA3 fingerprint string hash (md5 hex) of the ClientHello.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ja3: Option<String>,
    /// JA4 fingerprint of the ClientHello.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ja4: Option<String>,
}

/// Details of the upstream server connection (when proxied).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamInfo {
    /// Resolved remote address `ip:port`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote_addr: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cipher: Option<String>,
    /// The TLS ClientHello fingerprint Akev presented to the server (TLS connections only).
    /// Shared by every exchange of a pooled connection (`Arc`: one allocation per connection).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<std::sync::Arc<UpstreamFingerprint>>,
    /// Upstream leaf certificate summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate: Option<CertificateSummary>,
    /// The whole chain the server presented and what Akev's trust decision rested on (TLS
    /// connections only). Shared by every exchange of a pooled connection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chain: Option<std::sync::Arc<CertificateChain>>,
    /// Whether the connection was reused from the pool.
    pub reused: bool,
    /// Upstream proxy used, if any (`http://host:port`, `socks5://...`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via_proxy: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateSummary {
    pub subject: String,
    pub issuer: String,
    pub san: Vec<String>,
    pub not_before: String,
    pub not_after: String,
    pub serial: String,
    pub sha256_fingerprint: String,
}

/// The certificate chain an upstream server presented in its TLS handshake, and how Akev
/// trusted it ([`UpstreamInfo::chain`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CertificateChain {
    /// The certificates in the order the server sent them (the leaf first).
    pub certificates: Vec<CertificateSummary>,
    pub trust: ChainTrust,
}

/// What Akev's decision to trust an upstream certificate chain rested on.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChainTrust {
    pub status: ChainTrustStatus,
    /// Indices into [`CertificateChain::certificates`] of the path Akev verified, leaf first.
    /// Servers sometimes send certificates that aren't part of it (old cross-signs, duplicates).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<usize>,
    /// The trusted root the verified path ends at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<TrustAnchorInfo>,
}

/// Whether Akev verified an upstream certificate chain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChainTrustStatus {
    /// Verified up to a trusted root (`anchor`).
    Verified,
    /// Not verified: certificate errors are ignored for this host (settings) or this request
    /// (Compose "Ignore TLS errors").
    NotVerified,
    /// The handshake succeeded but the verified path couldn't be rebuilt to report it.
    #[default]
    Unknown,
}

/// The trusted root ("trust anchor") an upstream chain was verified against.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustAnchorInfo {
    /// The root's subject name.
    pub subject: String,
    /// Index into [`CertificateChain::certificates`] when the server sent the root itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<usize>,
    pub source: TrustAnchorSource,
}

/// Where a trusted root comes from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TrustAnchorSource {
    /// Mozilla's root store, built into Akev.
    BuiltIn,
    /// The operating system's trust store.
    #[default]
    System,
    /// A trusted CA file from Settings → Certificates.
    Custom,
}

/// The fingerprint of Akev's own TLS ClientHello (and HTTP/2 preface) to an upstream server,
/// so users can verify which browser an impersonation profile looked like on the wire.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamFingerprint {
    /// Profile actually used: `native`, `chrome`, `firefox`, `safari` or `edge`.
    pub profile: String,
    /// Profile that was asked for, when it differs from `profile` (`mirror-client` resolved
    /// to a browser, or a browser profile in a build without the impersonation backend).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    /// Why `requested` wasn't used as is (human readable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// JA3 hash (md5 hex) of the ClientHello sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ja3: Option<String>,
    /// Full JA3 string (`version,ciphers,extensions,groups,point_formats`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ja3_text: Option<String>,
    /// JA4 fingerprint of the ClientHello sent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ja4: Option<String>,
    /// Akamai-style HTTP/2 fingerprint of the connection preface
    /// (`SETTINGS|WINDOW_UPDATE|PRIORITY|pseudo-header order`), HTTP/2 connections only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http2: Option<String>,
}

/// Who made the request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientInfo {
    /// Client socket address `ip:port`.
    pub remote_addr: String,
    pub connection_id: ConnectionId,
    /// Local process that owns the client socket (best effort, local clients only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process: Option<ProcessInfo>,
    /// Interceptor that launched/configured this client, if known (e.g. `chrome`, `terminal`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interceptor: Option<String>,
    /// How the request reached the proxy.
    pub via: ProxyMode,
    /// TLS details if the client connection was TLS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls: Option<TlsInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// How a client connection reached the proxy.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ProxyMode {
    /// Absolute-form HTTP proxy request (`GET http://...`).
    #[default]
    HttpProxy,
    /// HTTP `CONNECT` tunnel.
    Connect,
    /// SOCKS4/4a/5.
    Socks,
    /// Request sent directly to the proxy port as if it were the server (transparent / reverse proxy).
    Direct,
    /// Request originated from Akev itself (Compose/Repeat).
    Composer,
    /// Connection from a device tunnelled through Akev's WireGuard server (the original
    /// destination is known, like a CONNECT target).
    Wireguard,
    /// Connection redirected to the transparent listener by the firewall (its original
    /// destination is the tunnel target).
    Transparent,
}

/// A WebSocket (or SSE) message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamMessage {
    /// Sequence number within the exchange, starting at 0.
    pub seq: u32,
    pub direction: Direction,
    pub kind: MessageKind,
    pub at: UnixMicros,
    /// Payload size in bytes.
    pub size: u64,
    /// For SSE: event name / id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Set when the message was modified/injected by a rule or the user.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modified: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Direction {
    /// client → server
    Sent,
    /// server → client
    Received,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MessageKind {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
    /// SSE event
    Event,
}

/// Compact row model for the traffic list. Kept small: the UI holds one per exchange.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeSummary {
    pub id: ExchangeId,
    pub kind: ExchangeKind,
    pub state: ExchangeState,
    /// Start time (unix ms, fractional).
    pub started_at: f64,
    pub method: String,
    pub scheme: String,
    pub host: String,
    pub port: u16,
    /// Path + query.
    pub path: String,
    pub http_version: crate::http::HttpVersion,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Response content kind, for the icon/colour.
    pub content_kind: ContentKind,
    /// Response content type essence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// Request body bytes on the wire.
    pub request_size: u64,
    /// Response body bytes on the wire.
    pub response_size: u64,
    /// Total duration in ms when finished.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused: Option<BreakpointStage>,
    /// Display name of the client source (process name or interceptor).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Rule that handled this exchange (if not plain passthrough).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<RuleId>,
    /// True if a rule or the user modified request or response.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub modified: bool,
    /// True if the response was generated by Akev (mock/map-local) instead of upstream.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub mocked: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pinned: bool,
    /// User colour tag (`red`, `orange`, `yellow`, `green`, `blue`, `purple`, `grey`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Number of WebSocket/SSE messages.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub message_count: u32,
    /// Has a user note.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_note: bool,
    /// Set when the exchange is a call to an LLM API (OpenAI, Anthropic, Gemini, Ollama, any
    /// OpenAI-compatible server…), recognised from its host, path and request body. Boxed: most
    /// rows aren't LLM calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<Box<LlmTag>>,
}

/// Provider and model of an LLM API call (`ExchangeSummary::llm`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmTag {
    /// Provider id: `openai`, `anthropic`, `google`, `vertex-ai`, `azure-openai`, `openrouter`,
    /// `ollama`, `lmstudio`, `openai-compatible`… (see `akev_content::llm`).
    pub provider: String,
    /// The model the request asked for, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Full detail for one exchange (everything except body bytes and stream message payloads).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangeDetail {
    pub summary: ExchangeSummary,
    pub request: RequestHead,
    pub request_body: BodyMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_trailers: Option<crate::http::Headers>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<ResponseHead>,
    pub response_body: BodyMeta,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_trailers: Option<crate::http::Headers>,
    /// The request as originally received, if a rule/breakpoint modified it before forwarding.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_request: Option<RequestHead>,
    /// The response as originally received from upstream, if modified before returning.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub original_response: Option<ResponseHead>,
    pub timing: Timing,
    pub client: ClientInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upstream: Option<UpstreamInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ExchangeError>,
    /// Tunnel stats for `ExchangeKind::Tunnel`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tunnel: Option<TunnelStats>,
    /// Free-form user note.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// Names of rules that matched (in evaluation order).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matched_rules: Vec<RuleId>,
    /// Why Akev handled this exchange differently from usual: a limit was reached (body
    /// too large for rules, too many paused exchanges), capture was overloaded, or a
    /// breakpoint released itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TunnelStats {
    pub bytes_sent: u64,
    pub bytes_received: u64,
    /// Why it was tunnelled rather than intercepted.
    pub reason: String,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn certificate_chain_wire_format() {
        let chain = CertificateChain {
            certificates: vec![CertificateSummary {
                subject: "CN=a".into(),
                ..CertificateSummary::default()
            }],
            trust: ChainTrust {
                status: ChainTrustStatus::NotVerified,
                path: vec![],
                anchor: Some(TrustAnchorInfo {
                    subject: "CN=Root".into(),
                    index: None,
                    source: TrustAnchorSource::BuiltIn,
                }),
            },
        };
        let info = UpstreamInfo {
            chain: Some(std::sync::Arc::new(chain)),
            ..UpstreamInfo::default()
        };
        let v = serde_json::to_value(&info).unwrap();
        assert_eq!(v["chain"]["certificates"][0]["subject"], "CN=a");
        assert_eq!(v["chain"]["trust"]["status"], "not-verified");
        assert_eq!(v["chain"]["trust"]["anchor"]["source"], "built-in");
        assert!(v["chain"]["trust"].get("path").is_none());
        assert!(v["chain"]["trust"]["anchor"].get("index").is_none());
        let back: UpstreamInfo = serde_json::from_value(v).unwrap();
        assert_eq!(back, info);
    }
}
