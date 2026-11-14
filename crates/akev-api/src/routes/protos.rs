//! `/api/protos*` — the protobuf schema registry (see `crate::protos`).

use super::common::{Params, blocking};
use crate::app::App;
use crate::error::{ApiError, ApiResult, parse_json};
use crate::protos::{NewFile, ProtoFileKind, ProtoMapping, ProtoRegistryInfo, RegistryError};
use akev_core::license::Feature;
use axum::Json;
use axum::extract::State;
use axum::response::Response;
use base64::Engine as _;
use bytes::Bytes;
use http::HeaderMap;
use http::header::{self, HeaderValue};
use serde::Deserialize;
use std::sync::Arc;

impl From<RegistryError> for ApiError {
    fn from(e: RegistryError) -> Self {
        match e {
            RegistryError::Invalid(m) => ApiError::bad_request(m),
            RegistryError::NotFound(_) => ApiError::not_found(e.to_string()),
            RegistryError::Io(_) => ApiError::internal(e.to_string()),
        }
    }
}

/// `GET /api/protos`
pub async fn list(State(app): State<Arc<App>>) -> Json<ProtoRegistryInfo> {
    Json(app.protos.info())
}

/// One file of an upload: `.proto` text in `content`, or any file (descriptor sets) as
/// `base64`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UploadFile {
    pub name: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub base64: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadBody {
    pub files: Vec<UploadFile>,
}

/// The files of an upload: JSON `{ files: [...] }`, or the raw bytes of one file named by
/// `?name=`.
fn upload_files(params: &Params, headers: &HeaderMap, body: Bytes) -> ApiResult<Vec<NewFile>> {
    if let Some(name) = params.non_empty("name") {
        return Ok(vec![NewFile {
            name: name.to_string(),
            bytes: body,
        }]);
    }
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_none_or(|ct| ct.to_ascii_lowercase().contains("json"));
    if !json {
        return Err(ApiError::bad_request(
            "send JSON `{ files: [{ name, content | base64 }] }`, or the file's bytes with `?name=`",
        ));
    }
    let upload: UploadBody = parse_json(&body)?;
    upload
        .files
        .into_iter()
        .map(|f| {
            let bytes = match (f.content, f.base64) {
                (Some(text), None) => Bytes::from(text),
                (None, Some(b64)) => Bytes::from(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64.trim())
                        .map_err(|e| {
                            ApiError::bad_request(format!("{}: invalid base64: {e}", f.name))
                        })?,
                ),
                _ => {
                    return Err(ApiError::bad_request(format!(
                        "{}: give exactly one of `content` (text) or `base64`",
                        f.name
                    )));
                }
            };
            Ok(NewFile {
                name: f.name,
                bytes,
            })
        })
        .collect()
}

/// `POST /api/protos`
pub async fn upload(
    State(app): State<Arc<App>>,
    params: Params,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<ProtoRegistryInfo>> {
    app.license.require(Feature::ProtoSchemas)?;
    let files = upload_files(&params, &headers, body)?;
    let info = blocking(move || app.protos.add(files)).await??;
    Ok(Json(info))
}

/// `DELETE /api/protos?name=<name>` (one file) or `?all=1` (every file).
pub async fn remove(
    State(app): State<Arc<App>>,
    params: Params,
) -> ApiResult<Json<ProtoRegistryInfo>> {
    let name = params.non_empty("name").map(str::to_string);
    if name.is_none() && !params.flag("all") {
        return Err(ApiError::bad_request(
            "name the file to delete with `?name=`, or delete every file with `?all=1`",
        ));
    }
    let info = blocking(move || app.protos.remove(name.as_deref())).await??;
    Ok(Json(info))
}

/// `GET /api/protos/file?name=<name>` — the file as uploaded.
pub async fn file(State(app): State<Arc<App>>, params: Params) -> ApiResult<Response> {
    let name = params
        .non_empty("name")
        .ok_or_else(|| ApiError::bad_request("`?name=` is required"))?;
    let (kind, bytes) = app
        .protos
        .file(name)
        .ok_or_else(|| ApiError::not_found(format!("no schema file named {name:?}")))?;
    let mut resp = Response::new(axum::body::Body::from(bytes));
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static(match kind {
            ProtoFileKind::Proto => "text/plain; charset=utf-8",
            ProtoFileKind::DescriptorSet => "application/octet-stream",
        }),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(resp)
}

/// `GET /api/protos/mappings`
pub async fn get_mappings(State(app): State<Arc<App>>) -> Json<Vec<ProtoMapping>> {
    Json(app.protos.mappings())
}

/// `PUT /api/protos/mappings`
pub async fn put_mappings(
    State(app): State<Arc<App>>,
    body: Bytes,
) -> ApiResult<Json<Vec<ProtoMapping>>> {
    app.license.require(Feature::ProtoSchemas)?;
    let mappings: Vec<ProtoMapping> = parse_json(&body)?;
    let saved = blocking(move || app.protos.set_mappings(mappings)).await??;
    Ok(Json(saved))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
        h
    }

    #[test]
    fn upload_forms() {
        let files = upload_files(
            &Params::parse(None),
            &json_headers(),
            Bytes::from_static(
                br#"{"files":[{"name":"a.proto","content":"syntax = \"proto3\";"},{"name":"b.desc","base64":"CgA="}]}"#,
            ),
        )
        .unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0].bytes, Bytes::from_static(b"syntax = \"proto3\";"));
        assert_eq!(files[1].bytes, Bytes::from_static(&[0x0a, 0x00]));
        // Raw bytes with ?name=.
        let files = upload_files(
            &Params::parse(Some("name=x.desc")),
            &HeaderMap::new(),
            Bytes::from_static(b"\x0a\x00"),
        )
        .unwrap();
        assert_eq!(files[0].name, "x.desc");
        // Errors.
        for body in [
            r#"{"files":[{"name":"a.proto"}]}"#,
            r#"{"files":[{"name":"a.proto","content":"x","base64":"eA=="}]}"#,
            r#"{"files":[{"name":"a.desc","base64":"!!"}]}"#,
            r#"{"files":[{"name":"a.proto","text":"x"}]}"#,
            r#"{"nope":1}"#,
        ] {
            let err = upload_files(
                &Params::parse(None),
                &json_headers(),
                Bytes::from(body.to_string()),
            );
            assert!(err.is_err(), "{body}");
        }
        let mut octets = HeaderMap::new();
        octets.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/octet-stream"),
        );
        assert!(upload_files(&Params::parse(None), &octets, Bytes::from_static(b"x")).is_err());
    }

    #[test]
    fn registry_errors_map_to_statuses() {
        assert_eq!(
            ApiError::from(RegistryError::Invalid("x".into())).status,
            http::StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ApiError::from(RegistryError::NotFound("x".into())).status,
            http::StatusCode::NOT_FOUND
        );
        assert_eq!(
            ApiError::from(RegistryError::Io("x".into())).status,
            http::StatusCode::INTERNAL_SERVER_ERROR
        );
    }
}
