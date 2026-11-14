//! End-to-end tests of the protobuf schema registry: schemas are uploaded through
//! `/api/protos`, protobuf / gRPC traffic goes through the proxy, and the body views decode it
//! with field names (gRPC path → method, Content-Type parameter, URL mappings).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/pro.rs"]
mod pro;

use akev_api::{AkevHandle, AkevOptions, start};
use axum::Router;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use base64::Engine as _;
use protox::prost_reflect::prost::Message as _;
use protox::prost_reflect::{DescriptorPool, DynamicMessage};
use serde_json::{Value, json};
use std::net::SocketAddr;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(10);

const MONEY: &str = r#"syntax = "proto3";
package shop.common;
message Money {
  string currency = 1;
  int64 units = 2;
}
"#;

const CATALOG: &str = r#"syntax = "proto3";
package shop.v1;
import "common/money.proto";
import "google/protobuf/timestamp.proto";

message Item {
  enum Status { STATUS_UNSPECIFIED = 0; IN_STOCK = 1; SOLD_OUT = 2; }
  string id = 1;
  string name = 2;
  shop.common.Money price = 3;
  Status status = 4;
  map<string, int32> stock = 5;
  repeated string tags = 6;
  google.protobuf.Timestamp updated_at = 7;
  oneof discount { int32 percent = 8; shop.common.Money amount = 9; }
}
message GetItemRequest { string id = 1; }
service Catalog {
  rpc GetItem(GetItemRequest) returns (Item);
}
"#;

const BROKEN: &str =
    "syntax = \"proto3\";\npackage broken;\nmessage Oops {\n  strng name = 1;\n}\n";

/// The test schemas compiled locally (to encode messages).
fn pool() -> DescriptorPool {
    use protox::file::{ChainFileResolver, File, FileResolver, GoogleFileResolver};
    struct Mem;
    impl FileResolver for Mem {
        fn open_file(&self, name: &str) -> Result<File, protox::Error> {
            match name {
                "common/money.proto" => File::from_source(name, MONEY),
                "shop/v1/catalog.proto" => File::from_source(name, CATALOG),
                _ => Err(protox::Error::file_not_found(name)),
            }
        }
    }
    let mut chain = ChainFileResolver::new();
    chain.add(Mem);
    chain.add(GoogleFileResolver::new());
    let mut compiler = protox::Compiler::with_file_resolver(chain);
    compiler.open_file("shop/v1/catalog.proto").unwrap();
    compiler.descriptor_pool()
}

fn encode(pool: &DescriptorPool, message: &str, json: &str) -> Vec<u8> {
    let desc = pool.get_message_by_name(message).unwrap();
    let mut de = serde_json::Deserializer::from_str(json);
    let msg = DynamicMessage::deserialize(desc, &mut de).unwrap();
    msg.encode_to_vec()
}

fn item_bytes(pool: &DescriptorPool) -> Vec<u8> {
    encode(
        pool,
        "shop.v1.Item",
        r#"{"id": "sku-1", "name": "Teapot", "price": {"currency": "EUR", "units": "12"},
            "status": "SOLD_OUT", "stock": {"b": 2, "a": 1}, "tags": ["new"],
            "updatedAt": "2026-11-09T08:00:00Z", "percent": 15}"#,
    )
}

fn frame(payload: &[u8]) -> Vec<u8> {
    let mut v = vec![0];
    v.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    v.extend_from_slice(payload);
    v
}

async fn upstream_server() -> SocketAddr {
    let item = item_bytes(&pool());
    let (grpc_item, raw_item, typed_item) = (item.clone(), item.clone(), item);
    let app = Router::new()
        .route(
            "/shop.v1.Catalog/GetItem",
            post(move || {
                let body = frame(&grpc_item);
                async move {
                    (
                        [("content-type", "application/grpc"), ("grpc-status", "0")],
                        body,
                    )
                        .into_response()
                }
            }),
        )
        .route(
            "/items/1",
            get(move || {
                let body = raw_item.clone();
                async move { ([("content-type", "application/octet-stream")], body).into_response() }
            }),
        )
        .route(
            "/typed",
            get(move || {
                let body = typed_item.clone();
                async move {
                    (
                        [("content-type", "application/x-protobuf; messageType=shop.v1.Item")],
                        body,
                    )
                        .into_response()
                }
            }),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

struct Env {
    handle: AkevHandle,
    api: reqwest::Client,
    proxied: reqwest::Client,
    upstream: SocketAddr,
}

async fn setup_with(options: AkevOptions) -> Env {
    let handle = start(options).await.expect("akev starts");
    let api = reqwest::Client::builder().no_proxy().build().unwrap();
    let proxied = reqwest::Client::builder()
        .proxy(reqwest::Proxy::all(format!("http://{}", handle.proxy_addr())).unwrap())
        .build()
        .unwrap();
    Env {
        handle,
        api,
        proxied,
        upstream: upstream_server().await,
    }
}

async fn setup() -> Env {
    setup_with(AkevOptions {
        ephemeral: true,
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    })
    .await
}

impl Env {
    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.handle.api_url())
    }

    fn upstream_url(&self, path: &str) -> String {
        format!("http://{}{path}", self.upstream)
    }

    async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut req = self
            .api
            .request(method, self.url(path))
            .bearer_auth(self.handle.token());
        if let Some(body) = body {
            req = req.json(&body);
        }
        let resp = req.send().await.unwrap();
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap();
        (
            status,
            serde_json::from_str(&text).unwrap_or(Value::String(text)),
        )
    }

    async fn get_json(&self, path: &str) -> Value {
        let (status, body) = self.request(reqwest::Method::GET, path, None).await;
        assert_eq!(status, 200, "GET {path}: {body}");
        body
    }

    async fn wait_for(&self, path: &str) -> u64 {
        let deadline = tokio::time::Instant::now() + TIMEOUT;
        loop {
            let list = self.get_json("/api/exchanges").await;
            if let Some(found) = list
                .as_array()
                .unwrap()
                .iter()
                .find(|s| s["path"] == path && s["state"] == "complete")
            {
                return found["id"].as_u64().unwrap();
            }
            assert!(tokio::time::Instant::now() < deadline, "no {path}: {list}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn upload_body() -> Value {
    json!({ "files": [
        { "name": "common/money.proto", "content": MONEY },
        { "name": "shop/v1/catalog.proto", "content": CATALOG },
        { "name": "broken.proto", "content": BROKEN },
    ]})
}

#[tokio::test]
async fn schemas_decode_grpc_protobuf_and_mapped_bodies() {
    let env = setup().await;
    let pool = pool();

    let empty = env.get_json("/api/protos").await;
    assert_eq!(
        empty,
        json!({"files": [], "mappings": [], "messageTypes": []})
    );

    // Upload: the broken file is kept with its error, the others compile.
    let (status, info) = env
        .request(reqwest::Method::POST, "/api/protos", Some(upload_body()))
        .await;
    assert_eq!(status, 200, "{info}");
    let files = info["files"].as_array().unwrap();
    assert_eq!(files.len(), 3);
    let broken = files.iter().find(|f| f["name"] == "broken.proto").unwrap();
    assert_eq!(broken["errors"][0]["line"], 4, "{broken}");
    assert_eq!(broken["errors"][0]["column"], 3);
    assert_eq!(broken["errors"][0]["file"], "broken.proto");
    let catalog = files
        .iter()
        .find(|f| f["name"] == "shop/v1/catalog.proto")
        .unwrap();
    assert_eq!(catalog["errors"], json!([]));
    assert_eq!(catalog["kind"], "proto");
    assert_eq!(catalog["package"], "shop.v1");
    assert_eq!(catalog["services"][0]["name"], "shop.v1.Catalog");
    assert_eq!(
        catalog["services"][0]["methods"][0],
        json!({"name": "GetItem", "path": "/shop.v1.Catalog/GetItem", "inputType": "shop.v1.GetItemRequest",
               "outputType": "shop.v1.Item", "clientStreaming": false, "serverStreaming": false})
    );
    assert_eq!(
        catalog["messages"],
        json!(["shop.v1.GetItemRequest", "shop.v1.Item"])
    );
    assert_eq!(catalog["enums"], json!(["shop.v1.Item.Status"]));
    assert_eq!(
        info["messageTypes"],
        json!([
            "shop.common.Money",
            "shop.v1.GetItemRequest",
            "shop.v1.Item"
        ])
    );

    // gRPC: the path names the method.
    let request = frame(&encode(
        &pool,
        "shop.v1.GetItemRequest",
        r#"{"id": "sku-1"}"#,
    ));
    let resp = env
        .proxied
        .post(env.upstream_url("/shop.v1.Catalog/GetItem"))
        .header("content-type", "application/grpc")
        .body(request)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let id = env.wait_for("/shop.v1.Catalog/GetItem").await;
    let req = env
        .get_json(&format!("/api/exchanges/{id}/body/request"))
        .await;
    assert_eq!(req["schema"]["messageType"], "shop.v1.GetItemRequest");
    assert_eq!(req["schema"]["source"], "grpc");
    assert_eq!(req["schema"]["method"]["service"], "shop.v1.Catalog");
    assert_eq!(
        req["structured"]["messages"][0]["typed"],
        json!({"id": "sku-1"})
    );
    assert_eq!(
        req["structured"]["messages"][0]["decoded"],
        json!({"1": "sku-1"})
    );
    let res = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert_eq!(res["schema"]["messageType"], "shop.v1.Item");
    assert_eq!(
        res["structured"]["messages"][0]["typed"],
        json!({"id": "sku-1", "name": "Teapot", "price": {"currency": "EUR", "units": 12},
               "status": "SOLD_OUT", "stock": {"a": 1, "b": 2}, "tags": ["new"],
               "updated_at": "2026-11-09T08:00:00Z", "percent": 15})
    );

    // Content-Type parameter.
    env.proxied
        .get(env.upstream_url("/typed"))
        .send()
        .await
        .unwrap();
    let id = env.wait_for("/typed").await;
    let typed = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert_eq!(typed["kind"], "protobuf");
    assert_eq!(typed["schema"]["source"], "content-type");
    assert_eq!(typed["schema"]["typed"]["name"], "Teapot");
    assert_eq!(typed["structured"]["2"], "Teapot", "schemaless decode kept");

    // Mapping for a body that doesn't say what it is.
    env.proxied
        .get(env.upstream_url("/items/1"))
        .send()
        .await
        .unwrap();
    let id = env.wait_for("/items/1").await;
    let before = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert!(before.get("schema").is_none(), "{before}");
    let (status, saved) = env
        .request(
            reqwest::Method::PUT,
            "/api/protos/mappings",
            Some(json!([{ "pattern": "/items/*", "side": "response", "messageType": ".shop.v1.Item" }])),
        )
        .await;
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved[0]["messageType"], "shop.v1.Item");
    assert_eq!(saved[0]["enabled"], true);
    assert!(saved[0]["id"].as_str().is_some_and(|s| !s.is_empty()));
    assert_eq!(env.get_json("/api/protos/mappings").await, saved);
    let after = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert_eq!(after["schema"]["source"], "mapping");
    assert_eq!(after["schema"]["mapping"], "/items/*");
    assert_eq!(after["schema"]["typed"]["status"], "SOLD_OUT");

    // MCP agents get the typed decode too.
    let mcp: Value = env
        .api
        .post(env.url("/mcp"))
        .bearer_auth(env.handle.token())
        .header("accept", "application/json, text/event-stream")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"akev_get_exchange","arguments":{"id": id, "includeBodies": true}}}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let text = mcp["result"]["content"][0]["text"].as_str().unwrap();
    let detail: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        detail["response"]["body"]["schema"]["typed"]["name"], "Teapot",
        "{detail}"
    );

    // Source download.
    let src = env
        .api
        .get(env.url("/api/protos/file?name=common/money.proto"))
        .bearer_auth(env.handle.token())
        .send()
        .await
        .unwrap();
    assert_eq!(src.status(), 200);
    assert!(
        src.headers()["content-type"]
            .to_str()
            .unwrap()
            .starts_with("text/plain")
    );
    assert_eq!(src.text().await.unwrap(), MONEY);

    // Deleting the schema turns typed decoding off again.
    let (status, info) = env
        .request(
            reqwest::Method::DELETE,
            "/api/protos?name=shop/v1/catalog.proto",
            None,
        )
        .await;
    assert_eq!(status, 200, "{info}");
    assert_eq!(info["files"].as_array().unwrap().len(), 2);
    let after = env
        .get_json(&format!("/api/exchanges/{id}/body/response"))
        .await;
    assert!(after.get("schema").is_none(), "{after}");
    let (status, info) = env
        .request(reqwest::Method::DELETE, "/api/protos?all=1", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(info["files"], json!([]));
    assert_eq!(
        info["mappings"].as_array().unwrap().len(),
        1,
        "mappings are kept"
    );
    env.handle.shutdown().await;
}

#[tokio::test]
async fn upload_forms_and_errors() {
    let env = setup().await;
    // A descriptor set uploaded as raw bytes.
    let set = pool().encode_to_vec();
    let resp = env
        .api
        .post(env.url("/api/protos?name=sets/catalog.binpb"))
        .bearer_auth(env.handle.token())
        .header("content-type", "application/octet-stream")
        .body(set.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let info: Value = resp.json().await.unwrap();
    let f = &info["files"][0];
    assert_eq!(f["kind"], "descriptor-set");
    assert_eq!(f["errors"], json!([]));
    assert_eq!(
        f["contains"],
        json!(["common/money.proto", "shop/v1/catalog.proto"])
    );
    assert_eq!(
        f["services"][0]["methods"][0]["path"],
        "/shop.v1.Catalog/GetItem"
    );

    // ...and as base64 in JSON, next to a .proto importing one of its files.
    let wallet = "syntax = \"proto3\";\npackage wallet;\nimport \"common/money.proto\";\nmessage Wallet { shop.common.Money balance = 1; }\n";
    let (status, info) = env
        .request(
            reqwest::Method::POST,
            "/api/protos",
            Some(json!({ "files": [
                { "name": "other.desc", "base64": base64::engine::general_purpose::STANDARD.encode(&set) },
                { "name": "wallet.proto", "content": wallet },
            ]})),
        )
        .await;
    assert_eq!(status, 200, "{info}");
    let wallet_info = info["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|f| f["name"] == "wallet.proto")
        .unwrap();
    assert_eq!(wallet_info["errors"], json!([]), "{wallet_info}");
    assert_eq!(wallet_info["messages"], json!(["wallet.Wallet"]));

    for (method, path, body, expected) in [
        (
            reqwest::Method::POST,
            "/api/protos",
            Some(json!({"files": [{"name": "../x.proto", "content": ""}]})),
            400,
        ),
        (
            reqwest::Method::POST,
            "/api/protos",
            Some(json!({"files": []})),
            400,
        ),
        (
            reqwest::Method::POST,
            "/api/protos",
            Some(json!({"files": [{"name": "a.desc", "base64": "%%%"}]})),
            400,
        ),
        (reqwest::Method::DELETE, "/api/protos", None, 400),
        (
            reqwest::Method::DELETE,
            "/api/protos?name=nope.proto",
            None,
            404,
        ),
        (
            reqwest::Method::GET,
            "/api/protos/file?name=nope.proto",
            None,
            404,
        ),
        (reqwest::Method::GET, "/api/protos/file", None, 400),
        (
            reqwest::Method::PUT,
            "/api/protos/mappings",
            Some(json!([{"pattern": "", "messageType": "a.B"}])),
            400,
        ),
        (
            reqwest::Method::PUT,
            "/api/protos/mappings",
            Some(json!([{"pattern": "/x", "messageType": "not a type"}])),
            400,
        ),
        (
            reqwest::Method::PUT,
            "/api/protos/mappings",
            Some(json!({"pattern": "/x"})),
            400,
        ),
    ] {
        let (status, body) = env.request(method.clone(), path, body).await;
        assert_eq!(status, expected, "{method} {path}: {body}");
        assert!(body["error"]["message"].is_string(), "{body}");
    }
    // Unauthenticated access is refused.
    let r = env.api.get(env.url("/api/protos")).send().await.unwrap();
    assert_eq!(r.status(), 401);
    env.handle.shutdown().await;
}

#[tokio::test]
async fn schemas_and_mappings_persist_across_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let options = || AkevOptions {
        data_dir: Some(dir.path().to_path_buf()),
        proxy_port: Some(0),
        api_port: 0,
        ..pro::options()
    };
    let env = setup_with(options()).await;
    let (status, _) = env
        .request(reqwest::Method::POST, "/api/protos", Some(upload_body()))
        .await;
    assert_eq!(status, 200);
    let (status, _) = env
        .request(
            reqwest::Method::PUT,
            "/api/protos/mappings",
            Some(json!([{ "id": "m", "pattern": "api.test/*", "messageType": "shop.v1.Item", "enabled": false }])),
        )
        .await;
    assert_eq!(status, 200);
    env.handle.shutdown().await;
    drop(env);
    assert!(
        dir.path()
            .join("protos/files/shop/v1/catalog.proto")
            .is_file()
    );

    let env = setup_with(options()).await;
    let info = env.get_json("/api/protos").await;
    assert_eq!(info["files"].as_array().unwrap().len(), 3);
    assert_eq!(
        info["mappings"],
        json!([{ "id": "m", "enabled": false, "pattern": "api.test/*", "side": "both", "messageType": "shop.v1.Item" }])
    );
    assert!(
        info["messageTypes"]
            .as_array()
            .unwrap()
            .contains(&json!("shop.v1.Item"))
    );
    env.handle.shutdown().await;
}
