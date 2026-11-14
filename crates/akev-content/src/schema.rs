//! Typed protobuf decoding with user-supplied schemas (`.proto` files / descriptor sets).
//!
//! The daemon compiles the user's schemas into a [`DescriptorPool`]. For each body the caller
//! picks the message type to decode it as — a [`SchemaPlan`] — and passes it to
//! [`crate::view_body_schema`]:
//!
//! * [`plan_for_content_type`]: a `messageType=` / `proto=` / `message=` parameter of the
//!   Content-Type (`application/x-protobuf; messageType="shop.v1.Item"`; Prometheus-style
//!   `proto=...; encoding=delimited` bodies are a sequence of length-prefixed messages);
//! * [`plan_for_grpc_path`]: gRPC / gRPC-Web requests map `/package.Service/Method` to the
//!   method's input (request) or output (response) type;
//! * user mappings (URL patterns → message type), resolved by the caller.
//!
//! Decoding ([`decode_typed`]) produces JSON following the proto3 JSON mapping, with these
//! display-oriented choices: field names as declared in the `.proto` (not lowerCamelCase), enum
//! values by name (their number when the schema doesn't know it), 64-bit integers as JSON
//! numbers (the UI reads them losslessly), map entries sorted by key, `bytes` as base64, and the
//! well-known types (`Timestamp`, `Duration`, wrappers, `Struct`, `Any`, ...) in their special
//! JSON forms. Fields on the wire that the schema doesn't define are kept, schemaless, under
//! their field number (`"7": ...`) and listed in [`TypedMessage::unknown_fields`] — usually a
//! sign that the schema is out of date.
//!
//! When the bytes don't fit the schema (wrong wire types, invalid UTF-8 in a `string`, ...),
//! decoding fails with a message and the view keeps only the schemaless decode.
//!
//! Decoding is bounded like every structured decode: before building anything, the wire format
//! is scanned against the schema to count the values the result would hold ([`estimate_values`]),
//! and decoding is refused when that doesn't fit the output budget.

use akev_core::Side;
use prost_reflect::{
    DynamicMessage, Kind, MapKey, MessageDescriptor, ReflectMessage, SerializeOptions,
    Value as PValue,
};
use serde::Serialize;
use serde_json::{Map, Number, Value};

use crate::budget::{Budget, base64_len};

pub use prost_reflect::{DescriptorPool, MethodDescriptor, ServiceDescriptor};

/// Maximum number of unknown-field paths reported per message (the count goes on).
pub const MAX_UNKNOWN_PATHS: usize = 32;

/// Nesting depth at which the value estimate stops descending (prost's decoder refuses
/// messages nested deeper than 100 levels anyway).
const MAX_ESTIMATE_DEPTH: usize = 100;

/// How the message type of a body was chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SchemaSource {
    /// gRPC / gRPC-Web: the method named by the request path.
    Grpc,
    /// A `messageType=` / `proto=` / `message=` parameter of the Content-Type.
    ContentType,
    /// A user mapping (URL pattern → message type).
    Mapping,
}

/// The gRPC method a request path names (as found in the schemas).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GrpcMethodInfo {
    /// Fully-qualified service name (`shop.v1.Catalog`).
    pub service: String,
    /// Method name (`GetItem`).
    pub method: String,
    /// Fully-qualified request message type.
    pub input_type: String,
    /// Fully-qualified response message type.
    pub output_type: String,
    /// The client sends a stream of messages.
    pub client_streaming: bool,
    /// The server sends a stream of messages.
    pub server_streaming: bool,
}

impl GrpcMethodInfo {
    /// Describe a method descriptor.
    pub fn new(method: &MethodDescriptor) -> Self {
        GrpcMethodInfo {
            service: method.parent_service().full_name().to_string(),
            method: method.name().to_string(),
            input_type: method.input().full_name().to_string(),
            output_type: method.output().full_name().to_string(),
            client_streaming: method.is_client_streaming(),
            server_streaming: method.is_server_streaming(),
        }
    }
}

/// The message type chosen for one body, and why.
#[derive(Clone, Debug)]
pub struct SchemaPlan {
    /// The message type the body (or each gRPC message) is decoded as.
    pub message: MessageDescriptor,
    /// How it was chosen.
    pub source: SchemaSource,
    /// The gRPC method ([`SchemaSource::Grpc`]).
    pub method: Option<GrpcMethodInfo>,
    /// The pattern of the user mapping that chose it ([`SchemaSource::Mapping`]).
    pub mapping: Option<String>,
    /// The body is a sequence of varint length-prefixed messages (`encoding=delimited`).
    pub delimited: bool,
}

impl SchemaPlan {
    /// A plan decoding bodies as `message`.
    pub fn new(message: MessageDescriptor, source: SchemaSource) -> Self {
        SchemaPlan {
            message,
            source,
            method: None,
            mapping: None,
            delimited: false,
        }
    }
}

/// `BodyView::schema`: which schema decoded the body and the result.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaView {
    /// Fully-qualified message type (`shop.v1.Item`).
    pub message_type: String,
    /// How the type was chosen.
    pub source: SchemaSource,
    /// The gRPC method, for [`SchemaSource::Grpc`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<GrpcMethodInfo>,
    /// The user mapping's pattern, for [`SchemaSource::Mapping`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping: Option<String>,
    /// Typed decode of a protobuf body (an array of messages for `encoding=delimited`).
    /// gRPC bodies carry theirs per message (`structured.messages[i].typed`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub typed: Option<Value>,
    /// Why typed decoding failed (the schemaless decode in `structured` is still there). For
    /// gRPC: set when no message decoded with the schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Paths of fields present on the wire but not in the schema (`7`, `items.9`), at most
    /// [`MAX_UNKNOWN_PATHS`].
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub unknown_fields: Vec<String>,
}

impl SchemaView {
    /// A view of `plan` without a decode result yet.
    pub fn for_plan(plan: &SchemaPlan) -> Self {
        SchemaView {
            message_type: plan.message.full_name().to_string(),
            source: plan.source,
            method: plan.method.clone(),
            mapping: plan.mapping.clone(),
            typed: None,
            error: None,
            unknown_fields: Vec::new(),
        }
    }
}

// =====================================================================================
// Choosing the message type
// =====================================================================================

/// The service and method named by a gRPC request path: `/shop.v1.Catalog/GetItem` →
/// `("shop.v1.Catalog", "GetItem")`. The last two segments are used, so gRPC-Web endpoints
/// mounted under a prefix (`/api/shop.v1.Catalog/GetItem`) work too; a query string is ignored.
pub fn grpc_path_parts(path: &str) -> Option<(&str, &str)> {
    let path = crate::util::path_only(path);
    let mut segments = path.rsplit('/');
    let method = segments.next()?;
    let service = segments.next()?;
    let ident = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'.')
            && !s.starts_with('.')
            && !s.ends_with('.')
    };
    (ident(service) && ident(method) && !method.contains('.')).then_some((service, method))
}

/// Look up the gRPC method a request path names.
pub fn find_grpc_method(pool: &DescriptorPool, path: &str) -> Option<MethodDescriptor> {
    let (service, method) = grpc_path_parts(path)?;
    let service = pool.get_service_by_name(service)?;
    service.methods().find(|m| m.name() == method)
}

/// Plan for a gRPC / gRPC-Web body: the input type of the method named by `path` for the
/// request, its output type for the response.
pub fn plan_for_grpc_path(pool: &DescriptorPool, path: &str, side: Side) -> Option<SchemaPlan> {
    let method = find_grpc_method(pool, path)?;
    let message = match side {
        Side::Request => method.input(),
        Side::Response => method.output(),
    };
    let mut plan = SchemaPlan::new(message, SchemaSource::Grpc);
    plan.method = Some(GrpcMethodInfo::new(&method));
    Some(plan)
}

/// The message type named by a Content-Type parameter (`messageType`, `proto` or `message`,
/// case-insensitive; a leading `.` is dropped), and whether the body is a sequence of
/// length-prefixed messages (`encoding=delimited`).
pub fn content_type_message(content_type: &str) -> Option<(String, bool)> {
    let params = crate::util::params(content_type);
    let name = params.iter().find_map(|(k, v)| {
        matches!(k.as_str(), "messagetype" | "proto" | "message")
            .then(|| v.trim().trim_start_matches('.'))
            .filter(|v| !v.is_empty())
    })?;
    let delimited = params
        .iter()
        .any(|(k, v)| k == "encoding" && v.trim().eq_ignore_ascii_case("delimited"));
    Some((name.to_string(), delimited))
}

/// Plan for a body whose Content-Type names its message type (see [`content_type_message`]),
/// when the schemas define that type.
pub fn plan_for_content_type(pool: &DescriptorPool, content_type: &str) -> Option<SchemaPlan> {
    let (name, delimited) = content_type_message(content_type)?;
    let message = pool.get_message_by_name(&name)?;
    let mut plan = SchemaPlan::new(message, SchemaSource::ContentType);
    plan.delimited = delimited;
    Some(plan)
}

// =====================================================================================
// Decoding
// =====================================================================================

/// The typed decode of one message.
#[derive(Clone, Debug, PartialEq)]
pub struct TypedMessage {
    /// The message as JSON (see the module docs for the mapping).
    pub value: Value,
    /// Paths of fields on the wire that the schema doesn't define (at most
    /// [`MAX_UNKNOWN_PATHS`]).
    pub unknown_fields: Vec<String>,
}

/// Decode `bytes` as one `message` (no output limit).
pub fn decode_typed(message: &MessageDescriptor, bytes: &[u8]) -> Result<TypedMessage, String> {
    decode_typed_budget(message, bytes, &mut Budget::unlimited())
}

/// Decode a sequence of varint length-prefixed messages (`encoding=delimited`), no output
/// limit. Returns the messages as a JSON array.
pub fn decode_typed_delimited(
    message: &MessageDescriptor,
    bytes: &[u8],
) -> Result<TypedMessage, String> {
    decode_delimited_budget(message, bytes, &mut Budget::unlimited())
}

/// [`decode_typed`] within an output budget. Fails (charging nothing) when the decode wouldn't
/// fit what is left of `budget`.
pub(crate) fn decode_typed_budget(
    message: &MessageDescriptor,
    bytes: &[u8],
    budget: &mut Budget,
) -> Result<TypedMessage, String> {
    let limit = budget.remaining_nodes();
    if estimate_values(message, bytes, limit) > limit {
        return Err(too_large());
    }
    let decoded = DynamicMessage::decode(message.clone(), bytes).map_err(|e| e.to_string())?;
    let mut conv = Converter {
        budget: *budget,
        unknown: Vec::new(),
    };
    let value = conv.message(&decoded, "")?;
    *budget = conv.budget;
    Ok(TypedMessage {
        value,
        unknown_fields: conv.unknown,
    })
}

fn decode_delimited_budget(
    message: &MessageDescriptor,
    bytes: &[u8],
    budget: &mut Budget,
) -> Result<TypedMessage, String> {
    let mut items = Vec::new();
    let mut unknown = Vec::new();
    let mut pos = 0;
    let mut index = 0usize;
    while pos < bytes.len() {
        let (len, n) = read_varint(&bytes[pos..])
            .ok_or_else(|| format!("message #{index} @{pos}: invalid length prefix"))?;
        let start = pos + n;
        let end = usize::try_from(len)
            .ok()
            .and_then(|len| start.checked_add(len))
            .filter(|&end| end <= bytes.len())
            .ok_or_else(|| format!("message #{index} @{pos}: length of message is corrupted"))?;
        if !budget.charge(1, 0) {
            return Err(too_large());
        }
        let typed = decode_typed_budget(message, &bytes[start..end], budget)
            .map_err(|e| format!("message #{index} @{pos}: {e}"))?;
        for path in typed.unknown_fields {
            push_unknown(&mut unknown, format!("[{index}].{path}"));
        }
        items.push(typed.value);
        pos = end;
        index += 1;
    }
    Ok(TypedMessage {
        value: Value::Array(items),
        unknown_fields: unknown,
    })
}

/// Decode the whole of a protobuf body with `plan` into a [`SchemaView`], within `budget`.
pub(crate) fn view_message(plan: &SchemaPlan, bytes: &[u8], budget: &mut Budget) -> SchemaView {
    let mut view = SchemaView::for_plan(plan);
    let result = if plan.delimited {
        decode_delimited_budget(&plan.message, bytes, budget)
    } else {
        decode_typed_budget(&plan.message, bytes, budget)
    };
    match result {
        Ok(t) => {
            view.typed = Some(t.value);
            view.unknown_fields = t.unknown_fields;
        }
        Err(e) => view.error = Some(e),
    }
    view
}

/// Add the typed decode of one gRPC message to its JSON object: `typed` (+ `unknownFields`),
/// or `typedError`. Returns whether it decoded.
pub(crate) fn annotate_grpc_message(
    object: &mut Map<String, Value>,
    plan: &SchemaPlan,
    data: &[u8],
    budget: &mut Budget,
) -> bool {
    match decode_typed_budget(&plan.message, data, budget) {
        Ok(t) => {
            object.insert("typed".into(), t.value);
            if !t.unknown_fields.is_empty() {
                object.insert(
                    "unknownFields".into(),
                    Value::Array(t.unknown_fields.into_iter().map(Value::String).collect()),
                );
            }
            true
        }
        Err(e) => {
            object.insert("typedError".into(), Value::String(e));
            false
        }
    }
}

fn too_large() -> String {
    "too large to decode with the schema (output limit reached)".to_string()
}

fn push_unknown(list: &mut Vec<String>, path: String) {
    if list.len() < MAX_UNKNOWN_PATHS && !list.contains(&path) {
        list.push(path);
    }
}

/// Well-known types whose JSON form prost-reflect's serializer produces.
fn is_well_known(full_name: &str) -> bool {
    matches!(
        full_name,
        "google.protobuf.Any"
            | "google.protobuf.Timestamp"
            | "google.protobuf.Duration"
            | "google.protobuf.Struct"
            | "google.protobuf.Value"
            | "google.protobuf.ListValue"
            | "google.protobuf.FieldMask"
            | "google.protobuf.Empty"
            | "google.protobuf.DoubleValue"
            | "google.protobuf.FloatValue"
            | "google.protobuf.Int64Value"
            | "google.protobuf.UInt64Value"
            | "google.protobuf.Int32Value"
            | "google.protobuf.UInt32Value"
            | "google.protobuf.BoolValue"
            | "google.protobuf.StringValue"
            | "google.protobuf.BytesValue"
    )
}

const WKT_OPTIONS: SerializeOptions = SerializeOptions::new()
    .use_proto_field_name(true)
    .stringify_64_bit_integers(false);

/// DynamicMessage → JSON, charging every value to the budget.
struct Converter {
    budget: Budget,
    unknown: Vec<String>,
}

impl Converter {
    fn charge(&mut self, nodes: usize, text: usize) -> Result<(), String> {
        if self.budget.charge(nodes, text) {
            Ok(())
        } else {
            Err(too_large())
        }
    }

    fn message(&mut self, msg: &DynamicMessage, path: &str) -> Result<Value, String> {
        let desc = msg.descriptor();
        if is_well_known(desc.full_name()) {
            // Out-of-range timestamps, `Any` of a type the schemas don't define, ... fall back
            // to the generic field-by-field form.
            if let Ok(v) = msg.serialize_with_options(serde_json::value::Serializer, &WKT_OPTIONS) {
                let (nodes, text) = json_size(&v);
                self.charge(nodes, text)?;
                return Ok(v);
            }
        }
        self.charge(1, 0)?;
        let mut map = Map::new();
        for (field, value) in msg.fields() {
            self.charge(1, field.name().len())?;
            let child = join_path(path, field.name());
            let v = self.field(&field.kind(), value, &child)?;
            map.insert(field.name().to_string(), v);
        }
        self.unknown_fields(msg, path, &mut map)?;
        Ok(Value::Object(map))
    }

    /// Keep fields the schema doesn't define, decoded schemalessly under their numbers.
    fn unknown_fields(
        &mut self,
        msg: &DynamicMessage,
        path: &str,
        map: &mut Map<String, Value>,
    ) -> Result<(), String> {
        let mut raw = Vec::new();
        for field in msg.unknown_fields() {
            push_unknown(
                &mut self.unknown,
                join_path(path, &field.number().to_string()),
            );
            field.encode(&mut raw);
        }
        if raw.is_empty() {
            return Ok(());
        }
        let fields = crate::protobuf::decode_raw_budget(&raw, &mut self.budget)
            .map_err(|e| e.to_string())?;
        if self.budget.exhausted() {
            return Err(too_large());
        }
        if let Value::Object(o) = crate::protobuf::to_json(&fields) {
            for (k, v) in o {
                map.entry(k).or_insert(v);
            }
        }
        Ok(())
    }

    fn field(&mut self, kind: &Kind, value: &PValue, path: &str) -> Result<Value, String> {
        match value {
            PValue::List(items) => {
                self.charge(1, 0)?;
                let mut out = Vec::with_capacity(items.len().min(self.budget.remaining_nodes()));
                for item in items {
                    out.push(self.single(kind, item, path)?);
                }
                Ok(Value::Array(out))
            }
            PValue::Map(entries) => {
                self.charge(1, 0)?;
                let value_kind = match kind {
                    Kind::Message(entry) => entry.map_entry_value_field().kind(),
                    other => other.clone(),
                };
                let mut sorted: Vec<(&MapKey, &PValue)> = entries.iter().collect();
                sorted.sort_by(|a, b| a.0.cmp(b.0));
                let mut map = Map::new();
                for (key, v) in sorted {
                    let key = map_key(key);
                    self.charge(1, key.len())?;
                    let child = join_path(path, &key);
                    let v = self.single(&value_kind, v, &child)?;
                    map.insert(key, v);
                }
                Ok(Value::Object(map))
            }
            single => self.single(kind, single, path),
        }
    }

    fn single(&mut self, kind: &Kind, value: &PValue, path: &str) -> Result<Value, String> {
        let out = match value {
            PValue::Message(m) => return self.message(m, path),
            PValue::List(_) | PValue::Map(_) => return self.field(kind, value, path),
            PValue::Bool(b) => Value::Bool(*b),
            PValue::I32(n) => Value::from(*n),
            PValue::I64(n) => Value::from(*n),
            PValue::U32(n) => Value::from(*n),
            PValue::U64(n) => Value::from(*n),
            PValue::F32(f) => float(f64::from(*f), Some(*f)),
            PValue::F64(f) => float(*f, None),
            PValue::String(s) => {
                self.charge(0, s.len())?;
                Value::String(s.clone())
            }
            PValue::Bytes(b) => {
                self.charge(0, base64_len(b.len()))?;
                Value::String(crate::util::b64(b))
            }
            PValue::EnumNumber(n) => match kind {
                Kind::Enum(e) => match e.get_value(*n) {
                    Some(v) => {
                        self.charge(0, v.name().len())?;
                        Value::String(v.name().to_string())
                    }
                    None => Value::from(*n),
                },
                _ => Value::from(*n),
            },
        };
        self.charge(1, 0)?;
        Ok(out)
    }
}

/// A float as JSON: NaN and the infinities as the strings the proto3 JSON mapping uses. A
/// `float` keeps its shortest decimal form (`1.1`, not `1.100000023841858`).
fn float(f: f64, single: Option<f32>) -> Value {
    if f.is_nan() {
        return Value::String("NaN".into());
    }
    if f.is_infinite() {
        return Value::String(if f > 0.0 { "Infinity" } else { "-Infinity" }.into());
    }
    let f = single
        .and_then(|s| s.to_string().parse::<f64>().ok())
        .unwrap_or(f);
    Number::from_f64(f).map_or(Value::Null, Value::Number)
}

fn map_key(key: &MapKey) -> String {
    match key {
        MapKey::Bool(b) => b.to_string(),
        MapKey::I32(n) => n.to_string(),
        MapKey::I64(n) => n.to_string(),
        MapKey::U32(n) => n.to_string(),
        MapKey::U64(n) => n.to_string(),
        MapKey::String(s) => s.clone(),
    }
}

fn join_path(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}.{name}")
    }
}

/// JSON values and string bytes in `v` (object keys count as values).
fn json_size(v: &Value) -> (usize, usize) {
    match v {
        Value::String(s) => (1, s.len()),
        Value::Array(a) => a.iter().fold((1, 0), |(n, t), v| {
            let (vn, vt) = json_size(v);
            (n + vn, t + vt)
        }),
        Value::Object(o) => o.iter().fold((1, 0), |(n, t), (k, v)| {
            let (vn, vt) = json_size(v);
            (n + vn + 1, t + vt + k.len())
        }),
        _ => (1, 0),
    }
}

// =====================================================================================
// Size estimate
// =====================================================================================

/// An upper bound on the JSON values (object keys included) the typed decode of `bytes` as
/// `message` produces, found by scanning the wire format against the schema without building
/// anything. Stops counting once the count exceeds `limit`. Bytes that aren't valid protobuf
/// end the scan early (decoding them fails anyway).
pub fn estimate_values(message: &MessageDescriptor, bytes: &[u8], limit: usize) -> usize {
    estimate(message, bytes, limit, 0)
}

fn estimate(message: &MessageDescriptor, bytes: &[u8], limit: usize, depth: usize) -> usize {
    // The message object itself.
    let mut count = 1usize;
    let mut pos = 0usize;
    while pos < bytes.len() && count <= limit {
        let Some((key, n)) = read_varint(&bytes[pos..]) else {
            break;
        };
        pos += n;
        let number = key >> 3;
        // A key and its value (or a list/map container; list elements are counted below).
        count = count.saturating_add(3);
        match key & 7 {
            0 => match read_varint(&bytes[pos..]) {
                Some((_, n)) => pos += n,
                None => break,
            },
            1 => pos += 8,
            5 => pos += 4,
            2 => {
                let Some((len, n)) = read_varint(&bytes[pos..]) else {
                    break;
                };
                pos += n;
                let Some(end) = usize::try_from(len)
                    .ok()
                    .and_then(|len| pos.checked_add(len))
                    .filter(|&end| end <= bytes.len())
                else {
                    break;
                };
                let payload = &bytes[pos..end];
                pos = end;
                let field = u32::try_from(number)
                    .ok()
                    .and_then(|number| message.get_field(number));
                let Some(field) = field else {
                    // Unknown field: its schemaless decode is charged when it is built.
                    continue;
                };
                match field.kind() {
                    Kind::Message(m) if depth < MAX_ESTIMATE_DEPTH => {
                        let remaining = limit.saturating_sub(count);
                        count = count.saturating_add(estimate(&m, payload, remaining, depth + 1));
                    }
                    kind => count = count.saturating_add(packed_elements(&kind, payload)),
                }
            }
            // Groups (proto2): every field inside is a tag of its own, counted as we go.
            3 | 4 => {}
            _ => break,
        }
    }
    count
}

/// Elements of a packed repeated scalar field's payload (1 for strings / bytes).
fn packed_elements(kind: &Kind, payload: &[u8]) -> usize {
    match kind {
        Kind::Double | Kind::Fixed64 | Kind::Sfixed64 => payload.len() / 8,
        Kind::Float | Kind::Fixed32 | Kind::Sfixed32 => payload.len() / 4,
        Kind::Int32
        | Kind::Int64
        | Kind::Uint32
        | Kind::Uint64
        | Kind::Sint32
        | Kind::Sint64
        | Kind::Bool
        | Kind::Enum(_) => payload.iter().filter(|&&b| b < 0x80).count(),
        _ => 1,
    }
}

/// A base-128 varint at the start of `bytes`: (value, bytes used).
fn read_varint(bytes: &[u8]) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for (i, &b) in bytes.iter().take(10).enumerate() {
        value |= u64::from(b & 0x7f) << (7 * i);
        if b < 0x80 {
            return Some((value, i + 1));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grpc_paths() {
        assert_eq!(
            grpc_path_parts("/shop.v1.Catalog/GetItem"),
            Some(("shop.v1.Catalog", "GetItem"))
        );
        assert_eq!(
            grpc_path_parts("/api/grpc/shop.v1.Catalog/GetItem?x=1"),
            Some(("shop.v1.Catalog", "GetItem"))
        );
        assert_eq!(grpc_path_parts("/Svc/M"), Some(("Svc", "M")));
        for bad in [
            "",
            "/",
            "/GetItem",
            "/shop.v1.Catalog/",
            "/shop v1/Get",
            "/.shop/Get",
            "/shop./Get",
            "/a/b.c",
        ] {
            assert_eq!(grpc_path_parts(bad), None, "{bad}");
        }
    }

    #[test]
    fn content_type_parameters() {
        assert_eq!(
            content_type_message("application/x-protobuf; messageType=\"shop.v1.Item\""),
            Some(("shop.v1.Item".into(), false))
        );
        assert_eq!(
            content_type_message(
                "application/vnd.google.protobuf; proto=io.prometheus.client.MetricFamily; encoding=delimited"
            ),
            Some(("io.prometheus.client.MetricFamily".into(), true))
        );
        assert_eq!(
            content_type_message("application/protobuf; MESSAGE=.a.B"),
            Some(("a.B".into(), false))
        );
        assert_eq!(content_type_message("application/x-protobuf"), None);
        assert_eq!(content_type_message("application/x-protobuf; proto="), None);
        assert_eq!(
            content_type_message("application/json; charset=utf-8"),
            None
        );
    }

    #[test]
    fn varints_and_packed_counts() {
        assert_eq!(read_varint(&[0x96, 0x01]), Some((150, 2)));
        assert_eq!(read_varint(&[0x80]), None);
        assert_eq!(read_varint(&[0xff; 11]), None);
        assert_eq!(packed_elements(&Kind::Int32, &[1, 0x96, 0x01, 3]), 3);
        assert_eq!(packed_elements(&Kind::Double, &[0; 24]), 3);
        assert_eq!(packed_elements(&Kind::Float, &[0; 8]), 2);
        assert_eq!(packed_elements(&Kind::String, b"hello"), 1);
    }

    #[test]
    fn floats_and_keys() {
        assert_eq!(
            float(f64::from(1.1f32), Some(1.1f32)),
            serde_json::json!(1.1)
        );
        assert_eq!(float(f64::NAN, None), serde_json::json!("NaN"));
        assert_eq!(float(f64::INFINITY, None), serde_json::json!("Infinity"));
        assert_eq!(
            float(f64::NEG_INFINITY, None),
            serde_json::json!("-Infinity")
        );
        assert_eq!(map_key(&MapKey::Bool(true)), "true");
        assert_eq!(map_key(&MapKey::I64(-3)), "-3");
        assert_eq!(map_key(&MapKey::String("k".into())), "k");
        assert_eq!(join_path("", "a"), "a");
        assert_eq!(join_path("a", "b"), "a.b");
        let (n, t) = json_size(&serde_json::json!({"ab": [1, "xyz"]}));
        assert_eq!((n, t), (5, 5));
    }
}
