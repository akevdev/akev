//! Error classification and the error responses Akev generates when it can't get a response
//! from upstream (502/504 with an explanatory body and an `x-akev-error` header).

use akev_core::*;
use bytes::Bytes;

/// kebab-case name of an [`ErrorKind`] (same as its serde form), e.g. `connection-refused`.
pub(crate) fn kind_slug(kind: ErrorKind) -> String {
    match serde_json::to_value(kind) {
        Ok(serde_json::Value::String(s)) => s,
        _ => "internal".to_string(),
    }
}

pub(crate) fn exchange_error(
    kind: ErrorKind,
    message: impl Into<String>,
    code: Option<&str>,
) -> ExchangeError {
    ExchangeError {
        kind,
        message: message.into(),
        code: code.map(str::to_string),
    }
}

pub(crate) fn client_aborted() -> ExchangeError {
    exchange_error(
        ErrorKind::ClientAborted,
        "The client closed the connection before the exchange completed",
        None,
    )
}

/// HTTP status for an error response.
pub(crate) fn status_for(kind: ErrorKind) -> u16 {
    match kind {
        ErrorKind::Timeout => 504,
        ErrorKind::ClientProtocolError => 400,
        ErrorKind::Internal => 500,
        _ => 502,
    }
}

/// Short human explanation for each failure class.
fn explanation(kind: ErrorKind) -> &'static str {
    match kind {
        ErrorKind::DnsFailed => "The upstream hostname could not be resolved (DNS lookup failed).",
        ErrorKind::ConnectionRefused => {
            "The upstream server refused the connection. Is it running and listening on this port?"
        }
        ErrorKind::ConnectionReset => "The upstream server reset the connection.",
        ErrorKind::HostUnreachable => "The upstream host is unreachable from this machine.",
        ErrorKind::Timeout => "Timed out connecting to, or waiting for, the upstream server.",
        ErrorKind::UpstreamCertInvalid => {
            "The upstream server's TLS certificate is not valid (untrusted, expired or for another host). \
             Add the host to \"ignore upstream certificate errors\" in Akev's settings if this is expected."
        }
        ErrorKind::UpstreamTlsFailed => "The TLS handshake with the upstream server failed.",
        ErrorKind::UpstreamProtocolError => "The upstream server sent an invalid HTTP response.",
        ErrorKind::UpstreamProxyFailed => {
            "The configured upstream proxy failed to connect to the server."
        }
        ErrorKind::ClientProtocolError => "The request could not be parsed.",
        _ => "Akev could not complete this request.",
    }
}

use akev_core::time::html_escape;

/// Build the response sent to the client when an exchange fails before a response head could
/// be forwarded. HTML when the client accepts it, plain text otherwise.
pub(crate) fn error_response(
    error: &ExchangeError,
    request: &RequestHead,
) -> (ResponseHead, Bytes) {
    let status = status_for(error.kind);
    let slug = kind_slug(error.kind);
    let wants_html = request
        .headers
        .get("accept")
        .is_some_and(|a| a.contains("text/html"));
    let target = request.url.as_str();
    let code = error
        .code
        .as_deref()
        .map(|c| format!(" ({c})"))
        .unwrap_or_default();
    let (ctype, body) = if wants_html {
        let body = format!(
            "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
             <title>{status} {reason} &middot; Akev</title><style>\
             body{{margin:0;font:15px/1.55 system-ui,-apple-system,Segoe UI,Roboto,sans-serif;background:#0d0d12;color:#e7e7ee;display:grid;place-items:center;min-height:100vh}}\
             main{{max-width:640px;padding:32px 24px}}h1{{font-size:22px;margin:0 0 4px}}.k{{color:#7c5cff;font:600 12px ui-monospace,monospace;letter-spacing:.06em;text-transform:uppercase}}\
             code{{font:13px ui-monospace,SFMono-Regular,Menlo,monospace;background:#1a1a24;padding:2px 6px;border-radius:6px;word-break:break-all}}p{{color:#b9b9c6}}\
             </style></head><body><main><div class=\"k\">Akev &middot; {slug}</div><h1>{status} {reason}</h1>\
             <p>{explain}</p><p><code>{target}</code></p><p>{message}{code}</p></main></body></html>",
            reason = canonical_reason(status),
            explain = html_escape(explanation(error.kind)),
            target = html_escape(target),
            message = html_escape(&error.message),
            code = html_escape(&code),
        );
        ("text/html; charset=utf-8", body)
    } else {
        let body = format!(
            "Akev: {status} {reason}\n{explain}\n\nRequest: {method} {target}\nError: {message}{code}\n",
            reason = canonical_reason(status),
            explain = explanation(error.kind),
            method = request.method,
            message = error.message,
        );
        ("text/plain; charset=utf-8", body)
    };
    let body = Bytes::from(body);
    let mut headers = Headers::with_capacity(4);
    headers.append("Content-Type", ctype);
    headers.append("Content-Length", body.len().to_string());
    headers.append("Cache-Control", "no-store");
    headers.append("x-akev-error", slug);
    let head = ResponseHead {
        status,
        status_message: canonical_reason(status).to_string(),
        http_version: HttpVersion::Http11,
        headers,
    };
    (head, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(accept: Option<&str>) -> RequestHead {
        let mut headers = Headers::new();
        if let Some(a) = accept {
            headers.append("Accept", a);
        }
        RequestHead {
            method: "GET".into(),
            url: "https://example.com/<x>".into(),
            scheme: "https".into(),
            host: "example.com".into(),
            port: 443,
            path: "/<x>".into(),
            http_version: HttpVersion::Http11,
            headers,
        }
    }

    #[test]
    fn slugs_match_serde() {
        assert_eq!(
            kind_slug(ErrorKind::ConnectionRefused),
            "connection-refused"
        );
        assert_eq!(
            kind_slug(ErrorKind::UpstreamCertInvalid),
            "upstream-cert-invalid"
        );
    }

    #[test]
    fn error_pages() {
        let err = exchange_error(
            ErrorKind::ConnectionRefused,
            "connect ECONNREFUSED 1.2.3.4:443",
            Some("ECONNREFUSED"),
        );
        let (head, body) = error_response(&err, &req(Some("text/html,*/*")));
        assert_eq!(head.status, 502);
        assert_eq!(head.headers.get("x-akev-error"), Some("connection-refused"));
        assert_eq!(head.headers.content_length(), Some(body.len() as u64));
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("&lt;x&gt;") && !text.contains("<x>"));
        let (head, body) = error_response(
            &exchange_error(ErrorKind::Timeout, "slow", None),
            &req(None),
        );
        assert_eq!(head.status, 504);
        assert!(
            head.headers
                .get("content-type")
                .unwrap()
                .starts_with("text/plain")
        );
        assert!(
            std::str::from_utf8(&body)
                .unwrap()
                .contains("GET https://example.com/<x>")
        );
    }
}
