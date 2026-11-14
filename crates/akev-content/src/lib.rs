//! # akev-content
//!
//! Everything about understanding body bytes:
//! * [`decode`] — undo `Content-Encoding` chains (gzip, x-gzip, deflate (zlib & raw), br, zstd,
//!   identity) and re-encode after edits.
//! * [`detect`] — classify content into [`akev_core::ContentKind`] from headers + sniffing.
//! * [`format`](mod@format) — pretty-print JSON / XML / HTML / CSS / JS / GraphQL / urlencoded forms.
//! * [`text`] — charset-aware text decoding.
//! * Protocol decoders: [`protobuf`] (schemaless), [`grpc`], [`msgpack`], [`cbor`], [`sse`],
//!   [`multipart`], [`form`], [`jwt`], [`image`] (dimensions + EXIF summary).
//! * [`schema`] — typed protobuf / gRPC decoding with user-supplied schemas
//!   ([`view_body_schema`]).
//! * [`validate`] — JSON / JSON-records / XML syntax errors with line & column, for editor
//!   error markers.
//! * [`llm`] — recognise LLM API calls (OpenAI, Anthropic, Gemini, Ollama, OpenAI-compatible
//!   servers) and normalise them into conversations, reassembling streamed responses.
//! * [`view_body`] — the one-stop "render this body for the UI" helper.
//!
//! Everything is synchronous, allocation-conscious and linear-time; nothing recurses on
//! attacker-controlled nesting without a bound.

use std::borrow::Cow;

use akev_core::ContentKind;
use serde::Serialize;

use crate::budget::Budget;

mod budget;
pub mod cbor;
pub mod decode;
pub mod detect;
pub mod form;
pub mod format;
pub mod grpc;
pub mod image;
pub mod jwt;
pub mod llm;
pub mod msgpack;
pub mod multipart;
pub mod protobuf;
pub mod schema;
mod snappy;
pub mod sse;
pub mod text;
mod util;
pub mod validate;

/// Errors from decoding / parsing body content.
#[derive(Debug, thiserror::Error, Clone, PartialEq)]
pub enum ContentError {
    /// A `Content-Encoding` we don't implement.
    #[error("unsupported encoding: {0}")]
    UnsupportedEncoding(String),
    /// Decompression failed (corrupt data, or the output exceeded the size cap).
    #[error("failed to decode {encoding} body: {message}")]
    Decode {
        /// The coding name as listed in the header (e.g. `gzip`).
        encoding: String,
        /// What went wrong (e.g. `decoded body exceeds 1048576 bytes`).
        message: String,
    },
    /// A structured format failed to parse.
    #[error("invalid {what}: {message}")]
    Parse {
        /// The format being parsed (`protobuf`, `gRPC`, `MessagePack`, ...).
        what: &'static str,
        /// What went wrong.
        message: String,
    },
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, ContentError>;

/// One-stop "render this body for the UI" helper used by the API.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BodyView {
    /// Detected content kind (drives the default viewer).
    pub kind: ContentKind,
    /// Decoded size in bytes.
    pub size: usize,
    /// Text (decoded with charset) when the content is textual, else None.
    pub text: Option<String>,
    /// Pretty-printed text when formattable.
    pub formatted: Option<String>,
    /// Structured decode for binary formats (protobuf/grpc/msgpack/cbor → JSON; multipart parts;
    /// SSE events; form fields; image info).
    pub structured: Option<serde_json::Value>,
    /// Set when decoding content-encoding failed (then `text` is None and raw bytes should be shown).
    pub decode_error: Option<String>,
    /// Whether the text was truncated for transport (very large bodies).
    pub truncated: bool,
    /// Whether `structured` stopped at a size limit ([`ViewOptions::max_structured_nodes`],
    /// [`ViewOptions::max_structured_text`], or — for gRPC — the decoded size of the messages
    /// exceeding [`ViewOptions::max_structured`]) and so holds only the first part of the
    /// content (open containers closed, the remaining items/messages left out). Serialized
    /// (as `structuredTruncated: true`) only when set.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub structured_truncated: bool,
    /// The protobuf schema that decoded this body, when one applied (see
    /// [`view_body_schema`]). Serialized only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<schema::SchemaView>,
}

/// Limits and options for [`view_body_with`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewOptions {
    /// Maximum bytes of `text` returned (cut on a character boundary).
    pub max_text: usize,
    /// Maximum decoded (decompressed) size; larger bodies fail with a decode error.
    pub max_decoded: usize,
    /// Texts larger than this are not pretty-printed.
    pub max_format: usize,
    /// Bodies larger than this get no `structured` decode. For gRPC it also caps the decoded
    /// (decompressed) size of all messages together.
    pub max_structured: usize,
    /// Maximum number of JSON values (object keys included) a `structured` decode may produce;
    /// decoding stops there and sets [`BodyView::structured_truncated`]. Small inputs can
    /// expand into very many values (a one-byte MessagePack `nil`, a two-byte urlencoded
    /// `a&` pair), so this, not the input size, bounds the tree's memory.
    pub max_structured_nodes: usize,
    /// Maximum total bytes of strings (text, base64 of binary data) in a `structured` decode;
    /// decoding stops there and sets [`BodyView::structured_truncated`].
    pub max_structured_text: usize,
    /// The capture holds only a prefix of the body: decode leniently, keeping whatever
    /// decompresses before the cut instead of reporting a decode error.
    pub capture_truncated: bool,
}

/// Default cap on decoded body size for [`view_body`] (256 MiB).
pub const DEFAULT_MAX_DECODED: usize = 256 << 20;
/// Default limit above which text isn't pretty-printed (8 MiB).
pub const DEFAULT_MAX_FORMAT: usize = 8 << 20;
/// Default limit above which no structured decode is produced (32 MiB).
pub const DEFAULT_MAX_STRUCTURED: usize = 32 << 20;
/// Default cap on the number of JSON values in a structured decode (500 000 values, roughly
/// 50–100 MB of JSON tree in the worst case).
pub const DEFAULT_MAX_STRUCTURED_NODES: usize = 500_000;
/// Default cap on the string bytes in a structured decode (64 MiB: room for the base64 of a
/// body of [`DEFAULT_MAX_STRUCTURED`] bytes).
pub const DEFAULT_MAX_STRUCTURED_TEXT: usize = 64 << 20;

impl Default for ViewOptions {
    fn default() -> Self {
        ViewOptions {
            max_text: 8 << 20,
            max_decoded: DEFAULT_MAX_DECODED,
            max_format: DEFAULT_MAX_FORMAT,
            max_structured: DEFAULT_MAX_STRUCTURED,
            max_structured_nodes: DEFAULT_MAX_STRUCTURED_NODES,
            max_structured_text: DEFAULT_MAX_STRUCTURED_TEXT,
            capture_truncated: false,
        }
    }
}

/// Build a [`BodyView`] from wire bytes + headers. `max_text` limits returned text size.
///
/// Equivalent to [`view_body_with`] with [`ViewOptions::default`] and the given `max_text`.
pub fn view_body(
    wire_bytes: &[u8],
    headers: &akev_core::Headers,
    path: &str,
    max_text: usize,
) -> BodyView {
    view_body_with(
        wire_bytes,
        headers,
        path,
        &ViewOptions {
            max_text,
            ..ViewOptions::default()
        },
    )
}

/// Build a [`BodyView`] with explicit limits.
///
/// 1. The `Content-Encoding` chain is decoded (capped at `max_decoded`). On failure the view
///    has `decode_error` set, no text/formatted/structured, the kind from the headers (or path
///    extension), and `size` = the wire size.
/// 2. The kind is detected from Content-Type + decoded bytes + path ([`detect::detect_kind`]).
/// 3. Textual kinds (and SVG images, and multipart bodies that are NUL-free UTF-8) get `text`,
///    charset-decoded and cut to `max_text` bytes (`truncated = true` when cut).
/// 4. `formatted` = [`format::pretty`] of the text when it is at most `max_format` bytes.
/// 5. `structured` (bodies up to `max_structured`, producing at most `max_structured_nodes`
///    JSON values and `max_structured_text` bytes of strings — beyond that the decode keeps
///    what it has and `structured_truncated` is set):
///    * protobuf → [`protobuf::to_json`] object;
///    * gRPC / gRPC-Web → `{"messages": [{compressed, decoded, trailers, size}], "error"?}`
///      (frames decoded before a parse error are kept; `error` describes it — also when the
///      messages decompress to more than `max_structured` bytes in total, or the output limit
///      left later messages out);
///    * MessagePack / CBOR → the decoded JSON value;
///    * multipart → array of parts (`{headers, name, filename, contentType, size, text}`);
///    * event stream → array of `{event, id, retry, data}`;
///    * urlencoded form → array of `[name, value]` pairs;
///    * image → `{format, width, height}`, plus `exif` ([`image::Exif`]) when the image carries
///      EXIF metadata;
///    * GraphQL JSON request → array of `{operationName, query (pretty-printed), variables}`;
///    * JSON and everything else → `None` (the UI parses JSON text itself).
pub fn view_body_with(
    wire_bytes: &[u8],
    headers: &akev_core::Headers,
    path: &str,
    opts: &ViewOptions,
) -> BodyView {
    view_body_schema(wire_bytes, headers, path, opts, None)
}

/// [`view_body_with`], decoding protobuf content with a schema when `schema` is given
/// (see [`schema`] for how the caller picks one):
///
/// * protobuf → [`BodyView::schema`] holds the typed decode (`typed`) next to the schemaless
///   one in `structured`;
/// * gRPC / gRPC-Web → every message in `structured.messages` gains `typed` (and
///   `unknownFields`) or `typedError`; [`BodyView::schema`] names the type / method;
/// * binary or unclassified content under a content-type or mapping plan → when the body
///   decodes with the schema, the view becomes a protobuf view (`kind` protobuf, schemaless
///   `structured`, `schema.typed`); otherwise `schema.error` says why.
///
/// A gRPC plan ([`schema::SchemaSource::Grpc`]) applies to gRPC content only, and JSON-coded
/// gRPC (`application/grpc+json`) is never decoded with a schema. Typed decoding has its own
/// output budget (the same limits as `structured`) and is skipped above
/// [`ViewOptions::max_structured`].
pub fn view_body_schema(
    wire_bytes: &[u8],
    headers: &akev_core::Headers,
    path: &str,
    opts: &ViewOptions,
    schema: Option<&schema::SchemaPlan>,
) -> BodyView {
    let content_type = headers.get("content-type");
    let encodings = headers.content_encodings();
    let decoded_owned;
    let decoded: &[u8] = if encodings.is_empty() || wire_bytes.is_empty() {
        wire_bytes
    } else {
        let result = if opts.capture_truncated {
            decode::decode_body_partial(wire_bytes, &encodings, opts.max_decoded).map(|(b, _)| b)
        } else {
            decode::decode_body(wire_bytes, &encodings, opts.max_decoded)
        };
        match result {
            Ok(b) => {
                decoded_owned = b;
                &decoded_owned
            }
            Err(e) => {
                return BodyView {
                    kind: detect::detect_kind(content_type, &[], path),
                    size: wire_bytes.len(),
                    text: None,
                    formatted: None,
                    structured: None,
                    decode_error: Some(e.to_string()),
                    truncated: false,
                    structured_truncated: false,
                    schema: None,
                };
            }
        }
    };

    let mut kind = detect::detect_kind(content_type, decoded, path);
    let plan = schema.filter(|p| {
        !decoded.is_empty()
            && decoded.len() <= opts.max_structured
            && schema_applies(p, kind, content_type)
    });
    // Content declared (by a mapping or the Content-Type) to be protobuf that wasn't recognised
    // as such: a successful typed decode makes it a protobuf view.
    let mut schema_view = None;
    if let Some(plan) = plan
        && !matches!(
            kind,
            ContentKind::Protobuf | ContentKind::Grpc | ContentKind::GrpcWeb
        )
    {
        let mut typed_budget = Budget::new(opts.max_structured_nodes, opts.max_structured_text);
        let view = schema::view_message(plan, decoded, &mut typed_budget);
        if view.typed.is_some() {
            kind = ContentKind::Protobuf;
        }
        schema_view = Some(view);
    }
    let svg = kind == ContentKind::Image && image::is_svg(decoded);
    let want_text = detect::is_textual(kind)
        || svg
        || (kind == ContentKind::Multipart && utf8_prefix_ok(decoded, opts.max_text));
    let (text, truncated) = if want_text {
        let (t, _, cut) = text::decode_text_prefix(decoded, content_type, opts.max_text);
        (Some(t), cut)
    } else {
        (None, false)
    };
    let format_kind = if svg { ContentKind::Xml } else { kind };
    let formatted = text
        .as_deref()
        .filter(|t| t.len() <= opts.max_format)
        .and_then(|t| format::pretty(format_kind, t));
    let (structured, structured_truncated) = if decoded.len() > opts.max_structured {
        (None, false)
    } else {
        structured(kind, decoded, headers, content_type, opts, plan)
    };
    if let Some(plan) = plan
        && schema_view.is_none()
    {
        schema_view = Some(match kind {
            ContentKind::Grpc | ContentKind::GrpcWeb => grpc_schema_view(plan, structured.as_ref()),
            _ => {
                let mut typed_budget =
                    Budget::new(opts.max_structured_nodes, opts.max_structured_text);
                schema::view_message(plan, decoded, &mut typed_budget)
            }
        });
    }
    BodyView {
        kind,
        size: decoded.len(),
        text,
        formatted,
        structured,
        decode_error: None,
        truncated,
        structured_truncated,
        schema: schema_view,
    }
}

/// Whether a schema plan applies to content of `kind`: gRPC plans to gRPC content, the others
/// to protobuf, gRPC and binary / unclassified content. JSON-coded gRPC never applies.
fn schema_applies(
    plan: &schema::SchemaPlan,
    kind: ContentKind,
    content_type: Option<&str>,
) -> bool {
    let json_grpc = content_type.is_some_and(|ct| {
        let e = util::essence(ct);
        e.starts_with("application/grpc") && e.ends_with("+json")
    });
    match kind {
        ContentKind::Grpc | ContentKind::GrpcWeb => !json_grpc,
        _ if plan.source == schema::SchemaSource::Grpc => false,
        ContentKind::Protobuf | ContentKind::Binary | ContentKind::Unknown => true,
        _ => false,
    }
}

/// The schema view of a gRPC body: its messages carry their typed decodes; `error` is set
/// when there were messages but none decoded with the schema.
fn grpc_schema_view(
    plan: &schema::SchemaPlan,
    structured: Option<&serde_json::Value>,
) -> schema::SchemaView {
    let mut view = schema::SchemaView::for_plan(plan);
    let messages = structured
        .and_then(|s| s.get("messages"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut errors = messages
        .iter()
        .filter_map(|m| m.get("typedError").and_then(serde_json::Value::as_str));
    let decoded_any = messages.iter().any(|m| m.get("typed").is_some());
    if !decoded_any && let Some(first) = errors.next() {
        view.error = Some(first.to_string());
    }
    view
}

/// Valid UTF-8 without NUL bytes within the first `max` bytes (a sequence cut at the limit is
/// fine) — used to decide whether a multipart body is worth showing as text.
fn utf8_prefix_ok(bytes: &[u8], max: usize) -> bool {
    let p = &bytes[..bytes.len().min(max)];
    if memchr::memchr(0, p).is_some() {
        return false;
    }
    match std::str::from_utf8(p) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && p.len() < bytes.len(),
    }
}

/// Bytes of an SSE body fed to the parser at a time, so the output budget is checked often.
const SSE_CHUNK: usize = 4096;

/// The structured decode of `decoded` within the output budget of `opts`, and whether it
/// stopped at a limit.
fn structured(
    kind: ContentKind,
    decoded: &[u8],
    headers: &akev_core::Headers,
    content_type: Option<&str>,
    opts: &ViewOptions,
    plan: Option<&schema::SchemaPlan>,
) -> (Option<serde_json::Value>, bool) {
    let mut budget = Budget::new(opts.max_structured_nodes, opts.max_structured_text);
    // Typed gRPC decodes have their own budget, so they never crowd out the schemaless ones.
    let mut typed = plan.map(|p| {
        (
            p,
            Budget::new(opts.max_structured_nodes, opts.max_structured_text),
        )
    });
    let (value, limited) = structured_with(
        kind,
        decoded,
        headers,
        content_type,
        opts.max_structured,
        &mut budget,
        typed.as_mut().map(|(p, b)| (*p, b)),
    );
    (value, limited || budget.exhausted())
}

fn structured_with(
    kind: ContentKind,
    decoded: &[u8],
    headers: &akev_core::Headers,
    content_type: Option<&str>,
    max_structured: usize,
    budget: &mut Budget,
    mut typed: Option<(&schema::SchemaPlan, &mut Budget)>,
) -> (Option<serde_json::Value>, bool) {
    use serde_json::{Value, json};
    let value = match kind {
        ContentKind::Protobuf => match protobuf::decode_raw_budget(decoded, budget) {
            Ok(fields) if fields.is_empty() && budget.exhausted() => None,
            Ok(fields) => Some(protobuf::to_json(&fields)),
            Err(_) => None,
        },
        ContentKind::Grpc | ContentKind::GrpcWeb => {
            let text_mode = content_type
                .is_some_and(|ct| util::essence(ct).starts_with("application/grpc-web-text"));
            let parsed = grpc::parse_budget(
                decoded,
                headers.get("grpc-encoding"),
                text_mode,
                max_structured,
                budget,
            );
            let count = parsed.messages.len();
            let messages: Vec<Value> = parsed
                .messages
                .into_iter()
                .map(|m| {
                    let trailer = m.trailers.is_some();
                    let mut o = serde_json::Map::new();
                    o.insert("compressed".into(), Value::Bool(m.compressed));
                    o.insert("decoded".into(), m.decoded.unwrap_or(Value::Null));
                    o.insert(
                        "trailers".into(),
                        m.trailers
                            .map_or(Value::Null, |t| serde_json::to_value(t).unwrap_or_default()),
                    );
                    o.insert("size".into(), Value::from(m.data.len()));
                    if let Some((plan, typed_budget)) = typed.as_mut()
                        && !trailer
                    {
                        schema::annotate_grpc_message(&mut o, plan, &m.data, typed_budget);
                    }
                    Value::Object(o)
                })
                .collect();
            let mut obj = serde_json::Map::new();
            obj.insert("messages".into(), Value::Array(messages));
            let error = match parsed.error {
                Some(e) => Some(e.to_string()),
                None if parsed.limited => Some(format!(
                    "output limit reached: only the first {count} message(s) are shown"
                )),
                None => None,
            };
            if let Some(e) = error {
                obj.insert("error".into(), Value::String(e));
            }
            return (Some(Value::Object(obj)), parsed.limited);
        }
        ContentKind::MsgPack => msgpack::to_json_budget(decoded, budget).ok().flatten(),
        ContentKind::Cbor => cbor::to_json_budget(decoded, budget).ok().flatten(),
        ContentKind::Multipart => content_type
            .and_then(|ct| multipart::parse_raw(decoded, ct, budget).ok())
            .map(|parts| {
                Value::Array(
                    parts
                        .iter()
                        .map(|p| serde_json::to_value(p.describe(decoded)).unwrap_or(Value::Null))
                        .collect(),
                )
            }),
        ContentKind::EventStream => {
            let mut parser = sse::SseParser::new();
            let mut events = Vec::new();
            let mut keep = |e: sse::SseEvent, budget: &mut Budget| {
                let text = e.data.len()
                    + e.event.as_ref().map_or(0, String::len)
                    + e.id.as_ref().map_or(0, String::len);
                // {event, id, retry, data}
                let fits = budget.charge(5, text);
                if fits {
                    events.push(e);
                }
                fits
            };
            'chunks: for chunk in decoded.chunks(SSE_CHUNK) {
                for e in parser.push(chunk) {
                    if !keep(e, budget) {
                        break 'chunks;
                    }
                }
            }
            if !budget.exhausted()
                && let Some(e) = parser.finish()
            {
                keep(e, budget);
            }
            serde_json::to_value(events).ok()
        }
        ContentKind::FormUrlEncoded => {
            let text: Cow<'_, str> = String::from_utf8_lossy(decoded);
            let pairs = form::urlencoded_pairs(text.trim())
                // [name, value]
                .take_while(|(k, v)| budget.charge(3, k.len() + v.len()))
                .map(|(k, v)| json!([k, v]))
                .collect();
            Some(Value::Array(pairs))
        }
        ContentKind::Image => {
            let image_value = || {
                let mut v = serde_json::to_value(image::info(decoded)?).ok()?;
                if let (Value::Object(map), Some(exif)) = (&mut v, image::exif(decoded))
                    && let Ok(e) = serde_json::to_value(exif)
                {
                    map.insert("exif".into(), e);
                }
                Some(v)
            };
            image_value()
        }
        ContentKind::GraphQl => graphql_structured(decoded, budget),
        _ => None,
    };
    (value, false)
}

/// GraphQL JSON request envelope(s) → `[{operationName, query, variables}]`. The whole JSON
/// document is parsed, so its size is checked against `budget` first.
fn graphql_structured(decoded: &[u8], budget: &mut Budget) -> Option<serde_json::Value> {
    use serde_json::Value;
    let t = util::skip_bom_ws(decoded);
    if !matches!(t.first(), Some(b'{' | b'[')) {
        return None;
    }
    if !budget.charge(budget::json_value_bound(t), t.len()) {
        return None;
    }
    let v: Value = serde_json::from_slice(t).ok()?;
    let ops = match v {
        Value::Array(a) => a,
        obj @ Value::Object(_) => vec![obj],
        _ => return None,
    };
    let out: Vec<Value> = ops
        .into_iter()
        .filter_map(|op| {
            let Value::Object(mut m) = op else {
                return None;
            };
            let query = m
                .remove("query")
                .and_then(|q| q.as_str().map(str::to_string))?;
            let pretty = format::graphql::pretty(&query).unwrap_or(query);
            let mut o = serde_json::Map::new();
            o.insert(
                "operationName".into(),
                m.remove("operationName").unwrap_or(Value::Null),
            );
            o.insert("query".into(), Value::String(pretty));
            o.insert(
                "variables".into(),
                m.remove("variables").unwrap_or(Value::Null),
            );
            Some(Value::Object(o))
        })
        .collect();
    (!out.is_empty()).then_some(Value::Array(out))
}
