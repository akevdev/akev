use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// A body as captured from the wire: the bytes are still content-encoded (gzip/br/...) exactly
/// as transmitted (transfer-encoding/chunking already removed). Capture may be truncated when a
/// body exceeds the configured capture limit; the proxy still streams the full body through.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapturedBody {
    pub bytes: Bytes,
    /// Total number of body bytes that went over the wire (>= bytes.len()).
    pub wire_size: u64,
    /// True when `bytes` holds only a prefix of the body.
    pub truncated: bool,
}

impl CapturedBody {
    pub fn empty() -> Self {
        CapturedBody::default()
    }

    pub fn complete(bytes: Bytes) -> Self {
        let wire_size = bytes.len() as u64;
        CapturedBody {
            bytes,
            wire_size,
            truncated: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.wire_size == 0
    }
}

/// Which side of an exchange a body/head belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Side {
    Request,
    Response,
}

/// Metadata about a stored body (sent to the UI instead of the body itself).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyMeta {
    /// Bytes on the wire (encoded).
    pub wire_size: u64,
    /// Bytes we actually stored (may be < wire_size when truncated).
    pub captured_size: u64,
    pub truncated: bool,
    /// Content-Encoding chain, e.g. `["gzip"]`.
    pub encodings: Vec<String>,
    /// Size after decoding, when known (computed lazily by the store).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decoded_size: Option<u64>,
    /// Content-Type essence from headers (lowercase, no params).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    /// The body's bytes were discarded to respect a limit (spill disk cap, overloaded
    /// capture); only its sizes remain.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub dropped: bool,
}

/// The kind of content a body holds, detected from headers + sniffing. Drives the default viewer
/// in the UI and formatting in `akev-content`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ContentKind {
    Json,
    /// JSON lines / NDJSON / JSON-seq
    JsonLines,
    Xml,
    Html,
    Javascript,
    Css,
    Markdown,
    Yaml,
    Csv,
    Text,
    FormUrlEncoded,
    Multipart,
    Image,
    Audio,
    Video,
    Font,
    Pdf,
    Wasm,
    Protobuf,
    Grpc,
    GrpcWeb,
    MsgPack,
    Cbor,
    EventStream,
    GraphQl,
    Binary,
    #[default]
    Unknown,
}
