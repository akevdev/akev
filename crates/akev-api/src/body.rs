//! Body helpers shared by the REST API, the stream filter and MCP: content-decoding with
//! zip-bomb limits, the API `BodyView`, editable request/response builders.

use akev_codegen::SnippetRequest;
use akev_core::{
    CapturedBody, EditableRequest, EditableResponse, Headers, HttpVersion, RequestHead,
    ResponseHead,
};
use bytes::Bytes;
use serde::Serialize;

/// `BodyView` as specified in the HTTP API reference (akev.dev/docs/api): akev-content's view plus wire metadata.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiBodyView {
    #[serde(flatten)]
    pub view: akev_content::BodyView,
    pub wire_size: u64,
    pub encodings: Vec<String>,
    pub capture_truncated: bool,
}

/// Build the view of a captured body. CPU heavy for large bodies: call from a blocking task.
pub fn body_view(body: &CapturedBody, headers: &Headers, path: &str) -> ApiBodyView {
    body_view_schema(body, headers, path, None)
}

/// [`body_view`], decoding protobuf / gRPC content with a schema when `schema` is given (see
/// `ProtoRegistry::plan`).
pub fn body_view_schema(
    body: &CapturedBody,
    headers: &Headers,
    path: &str,
    schema: Option<&akev_content::schema::SchemaPlan>,
) -> ApiBodyView {
    // A capture cut at the body limit still shows what decompresses before the cut.
    let opts = akev_content::ViewOptions {
        capture_truncated: body.truncated,
        ..akev_content::ViewOptions::default()
    };
    let view = akev_content::view_body_schema(&body.bytes, headers, path, &opts, schema);
    ApiBodyView {
        view,
        wire_size: body.wire_size,
        encodings: headers.content_encodings(),
        capture_truncated: body.truncated,
    }
}

/// View of a body returned by `ProxyHandle::send`. The bytes are expected as received on the
/// wire; if decoding per `Content-Encoding` fails, they're retried as already-decoded content.
/// `schema` decodes protobuf / gRPC content with a schema (see [`body_view_schema`]).
pub fn send_body_view(
    body: &Bytes,
    headers: &Headers,
    path: &str,
    schema: Option<&akev_content::schema::SchemaPlan>,
) -> ApiBodyView {
    let captured = CapturedBody::complete(body.clone());
    let mut view = body_view_schema(&captured, headers, path, schema);
    if view.view.decode_error.is_some() && !view.encodings.is_empty() {
        let mut plain = headers.clone();
        plain.remove("content-encoding");
        let retry = body_view_schema(&captured, &plain, path, schema).view;
        if retry.decode_error.is_none() {
            view.view = retry;
        }
    }
    view
}

/// Remove the content-encoding of `bytes` according to `headers`.
pub fn decode(bytes: &Bytes, headers: &Headers) -> Result<Bytes, akev_content::ContentError> {
    let encodings = headers.content_encodings();
    if encodings.is_empty() {
        return Ok(bytes.clone());
    }
    akev_content::decode::decode_body(bytes, &encodings, akev_content::DEFAULT_MAX_DECODED)
}

/// Decoded body if possible, otherwise the raw bytes (flag = decoded successfully).
pub fn decode_or_raw(bytes: &Bytes, headers: &Headers) -> (Bytes, bool) {
    match decode(bytes, headers) {
        Ok(b) => (b, true),
        Err(e) => {
            tracing::debug!("body decode failed: {e}");
            (bytes.clone(), false)
        }
    }
}

/// An editable copy of a captured request (decoded body, as the Compose page and breakpoints
/// expect; the proxy re-encodes when sending).
pub fn editable_request(head: &RequestHead, body: Option<&CapturedBody>) -> EditableRequest {
    let body = body
        .map(|b| decode_or_raw(&b.bytes, &head.headers).0)
        .unwrap_or_default();
    EditableRequest {
        method: head.method.clone(),
        url: head.url.clone(),
        headers: head.headers.clone(),
        body,
    }
}

/// An editable copy of a captured response (decoded body).
pub fn editable_response(head: &ResponseHead, body: Option<&CapturedBody>) -> EditableResponse {
    let body = body
        .map(|b| decode_or_raw(&b.bytes, &head.headers).0)
        .unwrap_or_default();
    EditableResponse {
        status: head.status,
        status_message: head.status_message.clone(),
        headers: head.headers.clone(),
        body,
    }
}

/// Request for code generation. The body is decoded; when that succeeded the
/// `Content-Encoding` header is dropped so the generated code sends what it says.
pub fn snippet_request(
    method: &str,
    url: &str,
    headers: &Headers,
    body: &Bytes,
    body_is_decoded: bool,
) -> SnippetRequest {
    let mut headers = headers.clone();
    if body_is_decoded {
        headers.remove("content-encoding");
    }
    SnippetRequest {
        method: method.to_string(),
        url: url.to_string(),
        headers,
        body: (!body.is_empty()).then(|| body.clone()),
    }
}

/// Snippet request from an `EditableRequest` (already decoded).
pub fn snippet_from_editable(req: &EditableRequest) -> SnippetRequest {
    snippet_request(&req.method, &req.url, &req.headers, &req.body, true)
}

/// Build a `RequestHead` for an editable request (used for rule dry runs).
pub fn request_head_from_editable(req: &EditableRequest) -> Result<RequestHead, String> {
    let url =
        url::Url::parse(req.url.trim()).map_err(|e| format!("invalid URL {:?}: {e}", req.url))?;
    let scheme = url.scheme().to_ascii_lowercase();
    if !matches!(scheme.as_str(), "http" | "https" | "ws" | "wss") {
        return Err(format!("unsupported URL scheme {scheme:?}"));
    }
    let host = match url.host() {
        Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
        Some(url::Host::Ipv4(ip)) => ip.to_string(),
        Some(url::Host::Ipv6(ip)) => ip.to_string(),
        None => return Err("URL has no host".into()),
    };
    let port = url
        .port_or_known_default()
        .unwrap_or(match scheme.as_str() {
            "https" | "wss" => 443,
            _ => 80,
        });
    let mut path = url.path().to_string();
    if let Some(q) = url.query() {
        path.push('?');
        path.push_str(q);
    }
    Ok(RequestHead {
        method: req.method.trim().to_ascii_uppercase(),
        url: url.to_string(),
        scheme,
        host,
        port,
        path,
        http_version: HttpVersion::Http11,
        headers: req.headers.clone(),
    })
}

/// Validate a request about to be sent from Compose.
pub fn validate_send_request(req: &EditableRequest) -> Result<(), String> {
    request_head_from_editable(req)?;
    let is_token_char = |b: u8| b.is_ascii_alphanumeric() || b"-_.!#$%&'*+^`|~".contains(&b);
    if req.method.is_empty() || !req.method.bytes().all(is_token_char) {
        return Err(format!("invalid HTTP method {:?}", req.method));
    }
    Ok(())
}

/// Lossy text for display when the bytes are (mostly) text; `None` for binary content.
pub fn text_if_textual(bytes: &[u8], content_type: Option<&str>, path: &str) -> Option<String> {
    let kind = akev_content::detect::detect_kind(content_type, bytes, path);
    if akev_content::detect::is_textual(kind) || std::str::from_utf8(bytes).is_ok() {
        Some(akev_content::text::decode_text(bytes, content_type).0)
    } else {
        None
    }
}

/// A file name for downloading a body: last URL path segment (sanitized), with an extension
/// derived from the content type when missing.
pub fn download_filename(url: &str, host: &str, id: u64, content_type: Option<&str>) -> String {
    let path = url::Url::parse(url)
        .ok()
        .map(|u| u.path().to_string())
        .unwrap_or_default();
    let last = path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("");
    let decoded = percent_encoding::percent_decode_str(last).decode_utf8_lossy();
    let mut name: String = decoded
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect();
    name = name.trim_matches(|c| c == '.' || c == '_').to_string();
    if name.is_empty() {
        let safe_host: String = host
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || matches!(c, '.' | '-') {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        name = format!("{safe_host}-{id}");
    }
    if name.chars().count() > 120 {
        name = name.chars().take(120).collect();
    }
    let has_ext = name.rsplit_once('.').is_some_and(|(stem, ext)| {
        !stem.is_empty()
            && (1..=8).contains(&ext.len())
            && ext.bytes().all(|b| b.is_ascii_alphanumeric())
    });
    if !has_ext && let Some(ext) = content_type.and_then(extension_for) {
        name.push('.');
        name.push_str(ext);
    }
    name
}

fn extension_for(content_type: &str) -> Option<&'static str> {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    Some(match essence.as_str() {
        "application/json" | "application/problem+json" => "json",
        "text/html" => "html",
        "text/plain" => "txt",
        "text/css" => "css",
        "text/javascript" | "application/javascript" => "js",
        "application/xml" | "text/xml" => "xml",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        "application/pdf" => "pdf",
        "application/wasm" => "wasm",
        "text/csv" => "csv",
        "application/zip" => "zip",
        _ => {
            return mime_guess::get_mime_extensions_str(&essence)
                .and_then(|exts| exts.first().copied());
        }
    })
}

/// `Content-Disposition: attachment` header value with an RFC 5987 UTF-8 fallback.
pub fn content_disposition(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii_graphic() && c != '"' && c != '\\' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let encoded =
        percent_encoding::utf8_percent_encode(filename, percent_encoding::NON_ALPHANUMERIC);
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filenames() {
        assert_eq!(
            download_filename("https://ex.com/a/b/report.pdf?x=1", "ex.com", 1, None),
            "report.pdf"
        );
        assert_eq!(
            download_filename(
                "https://ex.com/api/users",
                "ex.com",
                7,
                Some("application/json")
            ),
            "users.json"
        );
        assert_eq!(
            download_filename(
                "https://ex.com/",
                "ex.com",
                7,
                Some("text/html; charset=utf-8")
            ),
            "ex.com-7.html"
        );
        assert_eq!(
            download_filename("https://ex.com/..%2F..%2Fetc%2Fpasswd", "ex.com", 3, None),
            "etc_passwd"
        );
        assert_eq!(
            download_filename("not a url", "h:o/st", 2, None),
            "h_o_st-2"
        );
        assert_eq!(
            download_filename("https://ex.com/caf%C3%A9", "ex.com", 1, Some("text/plain")),
            "café.txt"
        );
        let long = format!("https://ex.com/{}", "a".repeat(500));
        assert!(download_filename(&long, "ex.com", 1, None).len() <= 120);
    }

    #[test]
    fn disposition_escapes() {
        assert_eq!(
            content_disposition("café \"x\".txt"),
            "attachment; filename=\"caf___x_.txt\"; filename*=UTF-8''caf%C3%A9%20%22x%22%2Etxt"
        );
    }

    #[test]
    fn head_from_editable() {
        let req = EditableRequest {
            method: "post".into(),
            url: "HTTPS://Example.COM:8443/a/b?c=d".into(),
            headers: Headers::new(),
            body: Bytes::new(),
        };
        let head = request_head_from_editable(&req).unwrap();
        assert_eq!(head.method, "POST");
        assert_eq!(head.scheme, "https");
        assert_eq!(head.host, "example.com");
        assert_eq!(head.port, 8443);
        assert_eq!(head.path, "/a/b?c=d");
        let v6 = EditableRequest {
            url: "http://[::1]/x".into(),
            ..req.clone()
        };
        let head = request_head_from_editable(&v6).unwrap();
        assert_eq!(head.host, "::1");
        assert_eq!(head.port, 80);
        assert!(
            request_head_from_editable(&EditableRequest {
                url: "ftp://x/".into(),
                ..req.clone()
            })
            .is_err()
        );
        assert!(
            request_head_from_editable(&EditableRequest {
                url: "nope".into(),
                ..req.clone()
            })
            .is_err()
        );
        assert!(
            validate_send_request(&EditableRequest {
                method: "GE T".into(),
                ..req.clone()
            })
            .is_err()
        );
        assert!(validate_send_request(&req).is_ok());
    }

    #[test]
    fn snippet_drops_content_encoding_only_when_decoded() {
        let mut h = Headers::new();
        h.append("Content-Encoding", "gzip");
        h.append("X-A", "1");
        let s = snippet_request("POST", "http://x/", &h, &Bytes::from_static(b"{}"), true);
        assert!(!s.headers.contains("content-encoding"));
        assert_eq!(s.body.as_deref(), Some(&b"{}"[..]));
        let s = snippet_request("GET", "http://x/", &h, &Bytes::new(), false);
        assert!(s.headers.contains("content-encoding"));
        assert_eq!(s.body, None);
    }
}
