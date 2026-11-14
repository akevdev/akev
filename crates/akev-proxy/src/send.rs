//! Requests issued by Akev itself (Compose / Repeat). They run through the same pipeline as
//! proxied traffic (optionally applying rules), are recorded with `via: composer`, and can
//! follow redirects (each hop recorded as its own exchange).

use crate::body::ClientBody;
use crate::pipeline::{self, ExchangeEnv, ExchangeRequest, encode_for_headers, fix_content_length};
use crate::responder::{CaptureResponder, Responder};
use crate::shared::Shared;
use crate::target::{authority_for, build_url, parse_absolute_url};
use crate::upstream::RequestOptions;
use crate::{SendOptions, SendResult};
use akev_core::*;
use bytes::Bytes;
use std::sync::Arc;

/// Largest response body returned in [`SendResult::body`].
const MAX_SEND_BODY: usize = 512 * 1024 * 1024;
/// Redirect hop limit.
const MAX_REDIRECTS: usize = 20;

fn method_has_body(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "POST" | "PUT" | "PATCH"
    )
}

/// Send `request` (body decoded; re-encoded per its `Content-Encoding`).
pub(crate) async fn send(
    shared: &Arc<Shared>,
    request: EditableRequest,
    options: SendOptions,
) -> SendResult {
    let mut request = request;
    let mut hops = 0;
    loop {
        let result = send_one(shared, request.clone(), &options).await;
        if options.follow_redirects
            && hops < MAX_REDIRECTS
            && result.error.is_none()
            && let Some(next) = redirect(&request, &result)
        {
            request = next;
            hops += 1;
            continue;
        }
        return result;
    }
}

fn invalid_request(message: String) -> SendResult {
    SendResult {
        id: ExchangeId::next(),
        response: None,
        body: Bytes::new(),
        error: Some(ExchangeError {
            kind: ErrorKind::ClientProtocolError,
            message,
            code: Some("ERR_INVALID_URL".into()),
        }),
        timing: Timing {
            start: UnixMicros::now(),
            ..Default::default()
        },
    }
}

async fn send_one(
    shared: &Arc<Shared>,
    request: EditableRequest,
    options: &SendOptions,
) -> SendResult {
    let sw = Stopwatch::start();
    let Some(url) = parse_absolute_url(request.url.trim()) else {
        return invalid_request(format!(
            "invalid URL {:?} (expected http(s)://host/path)",
            request.url
        ));
    };
    let method = request.method.trim().to_string();
    if method.is_empty() || !method.bytes().all(|b| b.is_ascii_graphic()) {
        return invalid_request(format!("invalid method {:?}", request.method));
    }
    let mut headers = request.headers;
    if !headers.contains("host") {
        headers.0.insert(
            0,
            (
                "Host".to_string(),
                authority_for(&url.scheme, &url.host, url.port),
            ),
        );
    }
    let wire = encode_for_headers(&request.body, &mut headers, true);
    if !wire.is_empty() || method_has_body(&method) || headers.contains("content-length") {
        fix_content_length(&mut headers, wire.len());
    }
    let head = RequestHead {
        url: build_url(&url.scheme, &url.host, url.port, &url.path),
        method,
        scheme: url.scheme,
        host: url.host,
        port: url.port,
        path: url.path,
        http_version: HttpVersion::Http11,
        headers,
    };
    let client = ClientInfo {
        remote_addr: "127.0.0.1:0".into(),
        connection_id: ConnectionId::next(),
        process: None,
        interceptor: None,
        via: ProxyMode::Composer,
        tls: None,
    };
    let env = ExchangeEnv {
        client,
        source: None,
        request_headers_ms: sw.elapsed_ms(),
        sw,
        apply_rules: options.apply_rules,
        options: RequestOptions {
            insecure: options.insecure,
            timeout_ms: options.timeout_ms,
            http_version: options.http_version,
            // Requests issued by Akev itself have no client connection: connections they
            // authenticate with NTLM / Negotiate are never reused.
            owner: None,
            fingerprint: Default::default(),
            // The user's own requests (Compose, Repeat, agents) may reach localhost.
            restrict_loopback: false,
        },
        local: None,
    };
    let body = if wire.is_empty() {
        ClientBody::Empty
    } else {
        ClientBody::Full(wire)
    };
    let req = ExchangeRequest {
        head: head.clone(),
        forward: head,
        body,
        upgrade: None,
    };
    let mut capture = CaptureResponder::new(MAX_SEND_BODY);
    let (events, _) = pipeline::run(shared, env, req, &mut Responder::Capture(&mut capture)).await;
    let error = events.error();
    // On failure the pipeline answers with an error page; only keep a response that really
    // came from upstream/rules (a head sent before the failure).
    let keep_response = error.is_none() || (capture.aborted && capture.head.is_some());
    let (response, body) = if keep_response {
        (capture.head.take(), capture.body.split().freeze())
    } else {
        (None, Bytes::new())
    };
    SendResult {
        id: events.id,
        response,
        body,
        error,
        timing: events.timing(),
    }
}

/// Compute the next request for a redirect response, browser-style.
fn redirect(req: &EditableRequest, res: &SendResult) -> Option<EditableRequest> {
    let head = res.response.as_ref()?;
    if !matches!(head.status, 301 | 302 | 303 | 307 | 308) {
        return None;
    }
    let location = head.headers.get("location")?.trim();
    let base = url::Url::parse(&req.url).ok()?;
    let next = base.join(location).ok()?;
    if !matches!(next.scheme(), "http" | "https") {
        return None;
    }
    let mut out = req.clone();
    out.url = next.to_string();
    let method = req.method.to_ascii_uppercase();
    let to_get = (head.status == 303 && method != "HEAD")
        || (matches!(head.status, 301 | 302) && method == "POST");
    if to_get {
        out.method = "GET".into();
        out.body = Bytes::new();
        for h in [
            "content-length",
            "content-type",
            "content-encoding",
            "transfer-encoding",
        ] {
            out.headers.remove(h);
        }
    }
    if base.origin() != next.origin() {
        for h in ["authorization", "cookie", "proxy-authorization"] {
            out.headers.remove(h);
        }
    }
    out.headers.remove("host");
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(method: &str, url: &str) -> EditableRequest {
        EditableRequest {
            method: method.into(),
            url: url.into(),
            headers: Headers(vec![
                ("Host".into(), "a.test".into()),
                ("Authorization".into(), "Bearer x".into()),
                ("Content-Type".into(), "text/plain".into()),
            ]),
            body: Bytes::from_static(b"payload"),
        }
    }

    fn res(status: u16, location: &str) -> SendResult {
        SendResult {
            id: ExchangeId(1),
            response: Some(ResponseHead {
                status,
                status_message: String::new(),
                http_version: HttpVersion::Http11,
                headers: Headers(vec![("Location".into(), location.into())]),
            }),
            body: Bytes::new(),
            error: None,
            timing: Timing::default(),
        }
    }

    #[test]
    fn redirects_follow_browser_semantics() {
        let next = redirect(&req("POST", "http://a.test/x"), &res(303, "/y?z=1")).unwrap();
        assert_eq!(
            (next.method.as_str(), next.url.as_str()),
            ("GET", "http://a.test/y?z=1")
        );
        assert!(
            next.body.is_empty()
                && !next.headers.contains("content-type")
                && !next.headers.contains("host")
        );
        assert!(next.headers.contains("authorization"));

        let next = redirect(&req("PUT", "http://a.test/x"), &res(307, "https://b.test/")).unwrap();
        assert_eq!(next.method, "PUT");
        assert_eq!(&next.body[..], b"payload");
        assert!(
            !next.headers.contains("authorization"),
            "credentials dropped cross-origin"
        );

        assert!(redirect(&req("GET", "http://a.test/"), &res(200, "/x")).is_none());
        assert!(redirect(&req("GET", "http://a.test/"), &res(302, "ftp://x/")).is_none());
    }
}
