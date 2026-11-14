//! Typed protobuf / gRPC decoding with user schemas: a sample schema (imports, nested types,
//! enums, oneofs, maps, repeated & packed fields, well-known types) is compiled with protox,
//! messages are encoded with prost-reflect's `DynamicMessage` and decoded through
//! `view_body_schema`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use akev_content::schema::{
    self, DescriptorPool, SchemaPlan, SchemaSource, decode_typed, estimate_values,
    plan_for_content_type, plan_for_grpc_path,
};
use akev_content::{ViewOptions, view_body_schema};
use akev_core::{ContentKind, Headers, Side};
use prost_reflect::prost::Message as _;
use prost_reflect::{DynamicMessage, MessageDescriptor, Value as PValue};
use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};
use serde_json::{Value, json};

const MONEY: &str = r#"
syntax = "proto3";
package shop.common;

// An amount of money.
message Money {
  string currency = 1;
  int64 units = 2;
  int32 nanos = 3;
}
"#;

const CATALOG: &str = r#"
syntax = "proto3";
package shop.v1;

import "common/money.proto";
import "google/protobuf/any.proto";
import "google/protobuf/duration.proto";
import "google/protobuf/struct.proto";
import "google/protobuf/timestamp.proto";
import "google/protobuf/wrappers.proto";

message Item {
  enum Status {
    STATUS_UNSPECIFIED = 0;
    IN_STOCK = 1;
    SOLD_OUT = 2;
  }
  message Variant {
    string sku = 1;
    repeated string tags = 2;
  }
  string id = 1;
  string name = 2;
  shop.common.Money price = 3;
  Status status = 4;
  repeated Variant variants = 5;
  map<string, int32> stock = 6;
  google.protobuf.Timestamp created_at = 7;
  oneof discount {
    int32 percent = 8;
    shop.common.Money amount = 9;
  }
  repeated int32 ratings = 10;
  bytes thumbnail = 11;
  double weight = 12;
  float score = 13;
  int64 big = 14;
  google.protobuf.StringValue note = 15;
  google.protobuf.Duration ttl = 16;
  google.protobuf.Struct attributes = 17;
  google.protobuf.Any extra = 18;
  sint64 delta = 19;
  map<int32, Status> history = 20;
}

message GetItemRequest {
  string id = 1;
  bool include_variants = 2;
}

message ListItemsRequest {
  int32 page_size = 1;
}

service Catalog {
  rpc GetItem(GetItemRequest) returns (Item);
  rpc ListItems(ListItemsRequest) returns (stream Item);
}
"#;

/// The same `Item` a release later: one more field the older schema doesn't know.
const CATALOG_V2: &str = r#"
syntax = "proto3";
package shop.v2;
message Item {
  string id = 1;
  string name = 2;
  string color = 21;
  repeated Item related = 22;
}
"#;

struct Mem(Vec<(&'static str, &'static str)>);

impl FileResolver for Mem {
    fn open_file(&self, name: &str) -> Result<File, protox::Error> {
        match self.0.iter().find(|(n, _)| *n == name) {
            Some((n, src)) => File::from_source(n, src),
            None => Err(protox::Error::file_not_found(name)),
        }
    }
}

fn pool() -> DescriptorPool {
    let files = vec![
        ("common/money.proto", MONEY),
        ("shop/v1/catalog.proto", CATALOG),
        ("shop/v2/catalog.proto", CATALOG_V2),
    ];
    let names: Vec<&str> = files.iter().map(|(n, _)| *n).collect();
    let mut chain = ChainFileResolver::new();
    chain.add(Mem(files));
    chain.add(GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);
    compiler.open_files(names).unwrap();
    compiler.descriptor_pool()
}

fn message(pool: &DescriptorPool, name: &str) -> MessageDescriptor {
    pool.get_message_by_name(name).unwrap()
}

/// Build a message from its proto3 JSON form and encode it.
fn encode_json(desc: &MessageDescriptor, json: &str) -> Vec<u8> {
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(desc.clone(), &mut de).unwrap();
    de.end().unwrap();
    msg.encode_to_vec()
}

fn sample_item(pool: &DescriptorPool) -> Vec<u8> {
    encode_json(
        &message(pool, "shop.v1.Item"),
        r#"{
          "id": "sku-1",
          "name": "Teapot",
          "price": {"currency": "EUR", "units": "12", "nanos": 500000000},
          "status": "IN_STOCK",
          "variants": [{"sku": "a", "tags": ["red", "small"]}, {"sku": "b"}],
          "stock": {"berlin": 3, "amsterdam": 0, "paris": 7},
          "createdAt": "2026-11-09T10:15:30.250Z",
          "amount": {"currency": "EUR", "units": "2"},
          "ratings": [5, 4, 300],
          "thumbnail": "AAEC",
          "weight": 0.75,
          "score": 1.1,
          "big": "9007199254740993",
          "note": "fragile",
          "ttl": "90.500s",
          "attributes": {"material": "porcelain", "dishwasher": false, "sizes": [1, 2]},
          "extra": {"@type": "type.googleapis.com/shop.common.Money", "currency": "USD", "units": "1"},
          "delta": "-42",
          "history": {"2": "SOLD_OUT", "1": "IN_STOCK"}
        }"#,
    )
}

fn headers(pairs: &[(&str, &str)]) -> Headers {
    let mut h = Headers::new();
    for (k, v) in pairs {
        h.append(*k, *v);
    }
    h
}

fn frame(flags: u8, payload: &[u8]) -> Vec<u8> {
    let mut v = vec![flags];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

#[test]
fn decodes_every_kind_of_field_with_names() {
    let pool = pool();
    let item = message(&pool, "shop.v1.Item");
    let typed = decode_typed(&item, &sample_item(&pool)).unwrap();
    assert!(
        typed.unknown_fields.is_empty(),
        "{:?}",
        typed.unknown_fields
    );
    assert_eq!(
        typed.value,
        json!({
            "id": "sku-1",
            "name": "Teapot",
            "price": {"currency": "EUR", "units": 12, "nanos": 500000000},
            "status": "IN_STOCK",
            "variants": [{"sku": "a", "tags": ["red", "small"]}, {"sku": "b"}],
            "stock": {"amsterdam": 0, "berlin": 3, "paris": 7},
            "created_at": "2026-11-09T10:15:30.250Z",
            "amount": {"currency": "EUR", "units": 2},
            "ratings": [5, 4, 300],
            "thumbnail": "AAEC",
            "weight": 0.75,
            "score": 1.1,
            "big": 9007199254740993u64,
            "note": "fragile",
            "ttl": "90.500s",
            "attributes": {"material": "porcelain", "dishwasher": false, "sizes": [1.0, 2.0]},
            "extra": {"@type": "type.googleapis.com/shop.common.Money", "currency": "USD", "units": 1},
            "delta": -42,
            "history": {"1": "IN_STOCK", "2": "SOLD_OUT"}
        })
    );
    // Field order follows the field numbers; map keys are sorted.
    let keys: Vec<&str> = typed
        .value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(&keys[..4], ["id", "name", "price", "status"]);
}

#[test]
fn messages_built_field_by_field_decode_too() {
    let pool = pool();
    let item = message(&pool, "shop.v1.Item");
    let mut msg = DynamicMessage::new(item.clone());
    msg.set_field_by_name("id", PValue::String("x".into()));
    msg.set_field_by_name("percent", PValue::I32(15));
    // An enum number the schema doesn't define is kept as a number.
    msg.set_field_by_name("status", PValue::EnumNumber(9));
    msg.set_field_by_name("ratings", PValue::List(vec![PValue::I32(-1)]));
    let typed = decode_typed(&item, &msg.encode_to_vec()).unwrap();
    assert_eq!(
        typed.value,
        json!({"id": "x", "status": 9, "percent": 15, "ratings": [-1]})
    );
}

#[test]
fn unknown_fields_are_kept_and_reported() {
    let pool = pool();
    let v2 = message(&pool, "shop.v2.Item");
    let bytes = encode_json(
        &v2,
        r#"{"id": "1", "color": "blue", "related": [{"id": "2", "color": "red"}]}"#,
    );
    // Decoded with the v1 schema: 21 and 22 are unknown.
    let typed = decode_typed(&message(&pool, "shop.v1.Item"), &bytes).unwrap();
    assert_eq!(typed.value["id"], "1");
    assert_eq!(typed.value["21"], "blue");
    assert_eq!(typed.value["22"], json!({"1": "2", "21": "red"}));
    assert_eq!(typed.unknown_fields, vec!["21", "22"]);
    // Nested unknown fields carry their path.
    let outer = message(&pool, "shop.v2.Item");
    let mut inner = DynamicMessage::new(message(&pool, "shop.v1.Item"));
    inner.set_field_by_name("id", PValue::String("in".into()));
    let mut wrapper_bytes = Vec::new();
    // `related` (22) holding a v1 Item with an extra field 99.
    let mut nested = inner.encode_to_vec();
    nested.extend_from_slice(&[0x98, 0x06, 0x01]); // field 99, varint 1
    prost_reflect::prost::encoding::bytes::encode(22, &nested, &mut wrapper_bytes);
    let typed = decode_typed(&outer, &wrapper_bytes).unwrap();
    assert_eq!(typed.value, json!({"related": [{"id": "in", "99": 1}]}));
    assert_eq!(typed.unknown_fields, vec!["related.99"]);
}

#[test]
fn bytes_that_dont_fit_the_schema_fail_with_a_reason() {
    let pool = pool();
    // Field 1 of GetItemRequest is a string; send a varint there.
    let err = decode_typed(&message(&pool, "shop.v1.GetItemRequest"), &[0x08, 0x01]).unwrap_err();
    assert!(err.contains("wire type"), "{err}");
    // Invalid UTF-8 in a string field.
    let err = decode_typed(
        &message(&pool, "shop.v1.GetItemRequest"),
        &[0x0a, 0x02, 0xff, 0xfe],
    )
    .unwrap_err();
    assert!(!err.is_empty());
    // Not protobuf at all.
    assert!(decode_typed(&message(&pool, "shop.v1.Item"), &[0x0a, 0x05, 1]).is_err());
}

#[test]
fn grpc_paths_resolve_to_method_types() {
    let pool = pool();
    let plan = plan_for_grpc_path(&pool, "/shop.v1.Catalog/GetItem", Side::Request).unwrap();
    assert_eq!(plan.message.full_name(), "shop.v1.GetItemRequest");
    assert_eq!(plan.source, SchemaSource::Grpc);
    let method = plan.method.unwrap();
    assert_eq!(method.service, "shop.v1.Catalog");
    assert_eq!(method.method, "GetItem");
    assert_eq!(method.output_type, "shop.v1.Item");
    assert!(!method.server_streaming);
    let plan = plan_for_grpc_path(&pool, "/web/shop.v1.Catalog/ListItems", Side::Response).unwrap();
    assert_eq!(plan.message.full_name(), "shop.v1.Item");
    assert!(plan.method.unwrap().server_streaming);
    assert!(plan_for_grpc_path(&pool, "/shop.v1.Catalog/Nope", Side::Request).is_none());
    assert!(plan_for_grpc_path(&pool, "/shop.v1.Nope/GetItem", Side::Request).is_none());
}

#[test]
fn grpc_bodies_get_typed_messages() {
    let pool = pool();
    let item = sample_item(&pool);
    let mut body = frame(0, &item);
    body.extend(frame(0, &item));
    let gz = akev_content::decode::encode_body(&item, &["gzip".to_string()]).unwrap();
    body.extend(frame(1, &gz));
    body.extend(frame(0x80, b"grpc-status: 0\r\n"));
    let h = headers(&[
        ("content-type", "application/grpc-web+proto"),
        ("grpc-encoding", "gzip"),
    ]);
    let plan = plan_for_grpc_path(&pool, "/shop.v1.Catalog/ListItems", Side::Response).unwrap();
    let v = view_body_schema(
        &body,
        &h,
        "/shop.v1.Catalog/ListItems",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.kind, ContentKind::GrpcWeb);
    let messages = v.structured.as_ref().unwrap()["messages"]
        .as_array()
        .unwrap();
    assert_eq!(messages.len(), 4);
    for m in &messages[..3] {
        assert_eq!(m["typed"]["name"], "Teapot");
        assert_eq!(m["typed"]["status"], "IN_STOCK");
        // The schemaless decode is still there.
        assert_eq!(m["decoded"]["2"], "Teapot");
    }
    assert!(messages[2]["compressed"].as_bool().unwrap());
    assert!(messages[3].get("typed").is_none() && messages[3].get("typedError").is_none());
    let schema = v.schema.unwrap();
    assert_eq!(schema.message_type, "shop.v1.Item");
    assert_eq!(schema.source, SchemaSource::Grpc);
    assert_eq!(schema.method.unwrap().method, "ListItems");
    assert_eq!(schema.error, None);
    assert_eq!(schema.typed, None);
    // Serialized shape (API contract).
    let s = serde_json::to_value(
        view_body_schema(&body, &h, "/x", &ViewOptions::default(), Some(&plan)).schema,
    )
    .unwrap();
    assert_eq!(s["messageType"], "shop.v1.Item");
    assert_eq!(s["source"], "grpc");
    assert_eq!(s["method"]["service"], "shop.v1.Catalog");
    assert_eq!(s["method"]["serverStreaming"], true);
    assert!(s.get("typed").is_none() && s.get("error").is_none());
}

#[test]
fn grpc_messages_that_dont_fit_keep_the_schemaless_decode() {
    let pool = pool();
    // A request decoded as the response type: field 2 (bool) arrives where Item has a string.
    let req = encode_json(
        &message(&pool, "shop.v1.GetItemRequest"),
        r#"{"id": "1", "includeVariants": true}"#,
    );
    let plan = plan_for_grpc_path(&pool, "/shop.v1.Catalog/GetItem", Side::Response).unwrap();
    let v = view_body_schema(
        &frame(0, &req),
        &headers(&[("content-type", "application/grpc")]),
        "/shop.v1.Catalog/GetItem",
        &ViewOptions::default(),
        Some(&plan),
    );
    let m = &v.structured.as_ref().unwrap()["messages"][0];
    assert!(m.get("typed").is_none());
    assert!(m["typedError"].as_str().unwrap().contains("wire type"));
    assert_eq!(m["decoded"]["1"], "1");
    let error = v.schema.unwrap().error.unwrap();
    assert!(error.contains("wire type"), "{error}");
}

#[test]
fn json_coded_grpc_is_not_decoded_with_a_schema() {
    let pool = pool();
    let plan = plan_for_grpc_path(&pool, "/shop.v1.Catalog/GetItem", Side::Request).unwrap();
    let v = view_body_schema(
        &frame(0, br#"{"id":"1"}"#),
        &headers(&[("content-type", "application/grpc+json")]),
        "/shop.v1.Catalog/GetItem",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.schema, None);
    assert_eq!(
        v.structured.unwrap()["messages"][0]["decoded"],
        json!({"id": "1"})
    );
}

#[test]
fn protobuf_bodies_with_a_content_type_parameter() {
    let pool = pool();
    let ct = "application/x-protobuf; messageType=\"shop.v1.Item\"";
    let plan = plan_for_content_type(&pool, ct).unwrap();
    assert_eq!(plan.source, SchemaSource::ContentType);
    let v = view_body_schema(
        &sample_item(&pool),
        &headers(&[("content-type", ct)]),
        "/items/1",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.kind, ContentKind::Protobuf);
    // Schemaless in `structured`, typed in `schema.typed`.
    assert_eq!(v.structured.as_ref().unwrap()["2"], "Teapot");
    let schema = v.schema.unwrap();
    assert_eq!(schema.message_type, "shop.v1.Item");
    assert_eq!(schema.typed.as_ref().unwrap()["name"], "Teapot");
    assert_eq!(
        schema.typed.as_ref().unwrap()["created_at"],
        "2026-11-09T10:15:30.250Z"
    );
    // A type the schemas don't define: no plan.
    assert!(plan_for_content_type(&pool, "application/x-protobuf; proto=nope.Missing").is_none());
}

#[test]
fn delimited_streams_decode_every_message() {
    let pool = pool();
    let desc = message(&pool, "shop.v1.GetItemRequest");
    let mut body = Vec::new();
    for id in ["a", "b", "c"] {
        let m = encode_json(&desc, &format!(r#"{{"id": "{id}"}}"#));
        prost_reflect::prost::encoding::encode_varint(m.len() as u64, &mut body);
        body.extend(m);
    }
    let ct = "application/vnd.google.protobuf; proto=shop.v1.GetItemRequest; encoding=delimited";
    let plan = plan_for_content_type(&pool, ct).unwrap();
    assert!(plan.delimited);
    let v = view_body_schema(
        &body,
        &headers(&[("content-type", ct)]),
        "/metrics",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(
        v.schema.unwrap().typed,
        Some(json!([{"id": "a"}, {"id": "b"}, {"id": "c"}]))
    );
    // A cut stream reports where.
    let err = schema::decode_typed_delimited(&desc, &body[..body.len() - 1]).unwrap_err();
    assert!(err.contains("message #2"), "{err}");
}

#[test]
fn mapped_binary_bodies_become_protobuf_views() {
    let pool = pool();
    let mut plan = SchemaPlan::new(message(&pool, "shop.v1.Item"), SchemaSource::Mapping);
    plan.mapping = Some("api.example.com/items/*".into());
    let h = headers(&[("content-type", "application/octet-stream")]);
    let v = view_body_schema(
        &sample_item(&pool),
        &h,
        "/items/1",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.kind, ContentKind::Protobuf);
    assert!(v.structured.is_some(), "schemaless decode added");
    let schema = v.schema.as_ref().unwrap();
    assert_eq!(schema.mapping.as_deref(), Some("api.example.com/items/*"));
    assert_eq!(schema.typed.as_ref().unwrap()["id"], "sku-1");
    let s = serde_json::to_value(&v).unwrap();
    assert_eq!(s["schema"]["source"], "mapping");
    assert_eq!(s["schema"]["mapping"], "api.example.com/items/*");

    // Garbage stays binary, with the reason.
    // A message starting with a high field number isn't sniffed as protobuf: the mapping
    // turns the binary view into a protobuf one.
    let late = encode_json(
        &message(&pool, "shop.v1.Item"),
        r#"{"weight": 0.75, "big": "5"}"#,
    );
    let plain = view_body_schema(&late, &h, "/items/2", &ViewOptions::default(), None);
    assert_eq!(plain.kind, ContentKind::Binary);
    assert_eq!(plain.structured, None);
    let v = view_body_schema(&late, &h, "/items/2", &ViewOptions::default(), Some(&plan));
    assert_eq!(v.kind, ContentKind::Protobuf);
    assert_eq!(v.structured.as_ref().unwrap()["14"], 5);
    assert_eq!(
        v.schema.unwrap().typed,
        Some(json!({"weight": 0.75, "big": 5}))
    );
    // Empty bodies aren't decoded.
    assert_eq!(
        view_body_schema(b"", &h, "/", &ViewOptions::default(), Some(&plan)).schema,
        None
    );

    let garbage = [0x00, 0xff, 0x80, 0x00, 0x07];
    let v = view_body_schema(
        &garbage,
        &h,
        "/items/1",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.kind, ContentKind::Binary);
    assert!(v.schema.unwrap().error.is_some());

    // JSON content is never decoded with a schema.
    let v = view_body_schema(
        b"{}",
        &headers(&[("content-type", "application/json")]),
        "/items/1",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(v.kind, ContentKind::Json);
    assert_eq!(v.schema, None);

    // gRPC plans only apply to gRPC content.
    let grpc = plan_for_grpc_path(&pool, "/shop.v1.Catalog/GetItem", Side::Response).unwrap();
    let plain = view_body_schema(&sample_item(&pool), &h, "/x", &ViewOptions::default(), None);
    let v = view_body_schema(
        &sample_item(&pool),
        &h,
        "/x",
        &ViewOptions::default(),
        Some(&grpc),
    );
    assert_eq!(v.kind, plain.kind);
    assert_eq!(v.schema, None);
}

#[test]
fn typed_decoding_is_bounded_by_the_output_budget() {
    let pool = pool();
    let item = message(&pool, "shop.v1.Item");
    // 100k packed ratings: ~100 KB on the wire, 100k values.
    let mut msg = DynamicMessage::new(item.clone());
    msg.set_field_by_name("ratings", PValue::List(vec![PValue::I32(1); 100_000]));
    let bytes = msg.encode_to_vec();
    assert!(estimate_values(&item, &bytes, usize::MAX) >= 100_000);
    // Past the limit is all the caller needs to know.
    assert!(estimate_values(&item, &bytes, 10) > 10);
    let opts = ViewOptions {
        max_structured_nodes: 10_000,
        ..ViewOptions::default()
    };
    let plan = SchemaPlan::new(item.clone(), SchemaSource::Mapping);
    let v = view_body_schema(
        &bytes,
        &headers(&[("content-type", "application/x-protobuf")]),
        "/",
        &opts,
        Some(&plan),
    );
    let error = v.schema.unwrap().error.unwrap();
    assert!(error.contains("too large"), "{error}");
    // With the default budget it decodes.
    let v = view_body_schema(
        &bytes,
        &headers(&[("content-type", "application/x-protobuf")]),
        "/",
        &ViewOptions::default(),
        Some(&plan),
    );
    assert_eq!(
        v.schema.unwrap().typed.unwrap()["ratings"]
            .as_array()
            .unwrap()
            .len(),
        100_000
    );
    // Bodies above `max_structured` aren't decoded at all.
    let v = view_body_schema(
        &bytes,
        &headers(&[("content-type", "application/x-protobuf")]),
        "/",
        &ViewOptions {
            max_structured: 1000,
            ..ViewOptions::default()
        },
        Some(&plan),
    );
    assert_eq!(v.schema, None);
}

#[test]
fn estimates_bound_the_decoded_values() {
    let pool = pool();
    let item = message(&pool, "shop.v1.Item");
    let bytes = sample_item(&pool);
    let typed = decode_typed(&item, &bytes).unwrap();
    fn count(v: &Value) -> usize {
        1 + match v {
            Value::Array(a) => a.iter().map(count).sum(),
            Value::Object(o) => o.values().map(|v| 1 + count(v)).sum(),
            _ => 0,
        }
    }
    // Well-known types expand (Struct) a little beyond their wire fields; the estimate still
    // tracks the decode closely enough to bound memory.
    let estimate = estimate_values(&item, &bytes, usize::MAX);
    assert!(
        estimate * 2 >= count(&typed.value),
        "{estimate} vs {}",
        count(&typed.value)
    );
}

#[test]
fn deep_nesting_is_refused_not_overflowing() {
    let pool = pool();
    let v2 = message(&pool, "shop.v2.Item");
    // `related` (22) nested 5000 levels deep.
    let mut bytes: Vec<u8> = Vec::new();
    for _ in 0..5000 {
        let mut outer = Vec::new();
        prost_reflect::prost::encoding::bytes::encode(22, &bytes, &mut outer);
        bytes = outer;
    }
    assert!(estimate_values(&v2, &bytes, usize::MAX) > 0);
    let err = decode_typed(&v2, &bytes).unwrap_err();
    assert!(err.contains("recursion"), "{err}");
    // 50 levels are fine.
    let mut ok: Vec<u8> = Vec::new();
    for _ in 0..50 {
        let mut outer = Vec::new();
        prost_reflect::prost::encoding::bytes::encode(22, &ok, &mut outer);
        ok = outer;
    }
    assert!(decode_typed(&v2, &ok).is_ok());
}

#[test]
fn random_bytes_never_panic() {
    let pool = pool();
    let item = message(&pool, "shop.v1.Item");
    let plan = SchemaPlan::new(item.clone(), SchemaSource::Mapping);
    let grpc = plan_for_grpc_path(&pool, "/shop.v1.Catalog/GetItem", Side::Response).unwrap();
    let sample = sample_item(&pool);
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for round in 0..3000 {
        let len = (next() % 200) as usize;
        let bytes: Vec<u8> = if round % 2 == 0 {
            (0..len).map(|_| next() as u8).collect()
        } else {
            // Mutations of a valid message find the interesting edges.
            let mut b = sample.clone();
            for _ in 0..1 + next() % 4 {
                let i = (next() as usize) % b.len();
                b[i] = next() as u8;
            }
            b.truncate(b.len() - (next() as usize % 4).min(b.len()));
            b
        };
        let _ = decode_typed(&item, &bytes);
        let _ = estimate_values(&item, &bytes, 10_000);
        let _ = view_body_schema(
            &bytes,
            &headers(&[("content-type", "application/x-protobuf")]),
            "/",
            &ViewOptions::default(),
            Some(&plan),
        );
        let _ = view_body_schema(
            &frame(0, &bytes),
            &headers(&[("content-type", "application/grpc")]),
            "/shop.v1.Catalog/GetItem",
            &ViewOptions::default(),
            Some(&grpc),
        );
    }
}
