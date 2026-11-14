//! Embedded web UI (`ui/dist`, built by the UI project and embedded with rust-embed).
//!
//! * `GET /?token=...` with the valid token serves the UI as is (`Cache-Control: no-store`):
//!   the UI reads the token, keeps it in (port-isolated) web storage, removes it from the
//!   address bar and sends it as `Authorization: Bearer`. The token is never put in a cookie —
//!   cookies aren't isolated by port, so it would reach every other service on the host. On
//!   a network-exposed API the response also sets a UI-only session cookie so the static
//!   assets load (it never authorizes `/api`). An invalid token redirects to `/` without it.
//! * Hashed assets (`/assets/*`) are cached for a year (immutable); `index.html` and other
//!   files are `no-cache` with an ETag.
//! * Unknown non-API paths fall back to `index.html` (SPA routing).
//! * HTML documents carry a strict Content-Security-Policy ([`ui_csp`]).

use crate::app::App;
use crate::auth::{constant_time_eq, is_protected_path, query_token, ui_session_cookie};
use crate::error::ApiError;
use axum::extract::{Request, State};
use axum::response::{IntoResponse, Response};
use http::header::{self, HeaderValue};
use http::{Method, StatusCode};
use std::borrow::Cow;
use std::sync::Arc;

#[cfg(feature = "embed-ui")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../../ui/dist"]
struct Assets;

/// Shown when the UI wasn't built into this binary.
const PLACEHOLDER: &str = r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Akev</title><style>:root{color-scheme:light dark}body{font:15px/1.5 system-ui,sans-serif;max-width:40rem;margin:15vh auto;padding:0 1.5rem}code{font:13px ui-monospace,monospace}</style></head><body><h1>Akev is running</h1><p>The web UI is not included in this build. Build it with <code>cd ui &amp;&amp; bun run build</code> and rebuild Akev. The HTTP API, live stream and MCP endpoint are available.</p></body></html>"#;

/// Allow framing only by ourselves.
const FRAME_POLICY: &str = "frame-ancestors 'self'";

/// Content-Security-Policy for the UI's HTML documents.
///
/// The UI holds the API token and displays captured traffic, which anyone on the network can
/// shape. So only the UI's own bundled scripts may run (no inline scripts, `eval`, plugins or
/// `<base>` tricks) and it may only talk back to this daemon, which keeps an injection from
/// running or from sending anything elsewhere. Inline *styles* are allowed: CodeMirror, Radix and
/// the toast library inject `<style>` elements. Captured HTML is only rendered in a `sandbox=""`
/// srcdoc frame (no scripts, opaque origin) and raw bodies carry their own sandbox policy.
fn ui_csp(app: &App, host: Option<&str>) -> String {
    csp(app.desktop, host)
}

fn csp(desktop: bool, host: Option<&str>) -> String {
    let mut connect = String::from("'self'");
    // CSP3 has 'self' cover same-origin WebSockets; older engines need the origin spelled out.
    if let Some(host) = host.map(str::trim).filter(|h| is_csp_host(h)) {
        connect.push_str(&format!(" ws://{host} wss://{host}"));
    }
    if desktop {
        // Tauri's IPC transport (it falls back to postMessage when this is blocked, noisily).
        connect.push_str(" ipc: http://ipc.localhost");
    }
    format!(
        "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; media-src 'self' data: blob:; font-src 'self' data:; \
         connect-src {connect}; worker-src 'self'; frame-src 'self'; object-src 'none'; \
         base-uri 'none'; form-action 'none'; {FRAME_POLICY}"
    )
}

/// A `host[:port]` that is a valid CSP host-source and can't smuggle in other directives
/// (IPv6 literals aren't valid host-sources: those rely on 'self').
fn is_csp_host(h: &str) -> bool {
    !h.is_empty()
        && h.len() <= 260
        && h.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b':')
}

struct Asset {
    data: Cow<'static, [u8]>,
    mime: String,
    etag: String,
}

#[cfg(feature = "embed-ui")]
fn lookup(path: &str) -> Option<Asset> {
    use rust_embed::RustEmbed;
    let file = <Assets as RustEmbed>::get(path)?;
    let hash = file.metadata.sha256_hash();
    Some(Asset {
        mime: file.metadata.mimetype().to_string(),
        etag: format!("\"{}\"", hex::encode(&hash[..16])),
        data: file.data,
    })
}

#[cfg(not(feature = "embed-ui"))]
fn lookup(_path: &str) -> Option<Asset> {
    None
}

fn placeholder() -> Asset {
    Asset {
        data: Cow::Borrowed(PLACEHOLDER.as_bytes()),
        mime: "text/html; charset=utf-8".into(),
        etag: "\"placeholder\"".into(),
    }
}

fn index() -> Asset {
    lookup("index.html").unwrap_or_else(placeholder)
}

/// Sanitize a request path into an embedded file path (None for traversal attempts).
fn asset_path(path: &str) -> Option<String> {
    let decoded = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()?;
    let rel = decoded.trim_start_matches('/');
    if rel
        .split('/')
        .any(|seg| seg == ".." || seg == "." || seg.contains('\\'))
    {
        return None;
    }
    Some(if rel.is_empty() {
        "index.html".to_string()
    } else {
        rel.to_string()
    })
}

/// Fallback handler: UI assets, SPA fallback, and JSON 404 for unknown API paths.
pub async fn serve(State(app): State<Arc<App>>, req: Request) -> Response {
    let path = req.uri().path().to_string();
    if is_protected_path(&path) {
        return ApiError::not_found(format!("no such endpoint: {} {path}", req.method()))
            .into_response();
    }
    if req.method() != Method::GET && req.method() != Method::HEAD {
        return ApiError::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method-not-allowed",
            "method not allowed",
        )
        .into_response();
    }
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    let csp = ui_csp(&app, host);
    if path == "/"
        && let Some(token) = req.uri().query().and_then(query_token)
    {
        // Another machine can only have come over HTTPS (`tls.rs`): its cookie is `Secure`.
        let remote = app.api_tls.is_some()
            && req
                .extensions()
                .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
                .is_some_and(|c| !crate::tls::is_this_host(c.0.ip()));
        return login(
            &app,
            &token,
            req.uri().query().unwrap_or(""),
            csp,
            req.method() == Method::HEAD,
            remote,
        );
    }
    let Some(rel) = asset_path(&path) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let (asset, immutable, is_html) = match lookup(&rel) {
        Some(a) => {
            let html = a.mime.starts_with("text/html");
            (a, rel.starts_with("assets/"), html)
        }
        None if rel == "index.html" => (index(), false, true),
        // A missing hashed asset must not be answered with HTML (the browser expects JS/CSS).
        None if rel.starts_with("assets/") => {
            return (StatusCode::NOT_FOUND, "not found").into_response();
        }
        None => (index(), false, true),
    };
    let if_none_match = req
        .headers()
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok());
    respond(
        asset,
        immutable,
        is_html.then_some(csp),
        if_none_match,
        req.method() == Method::HEAD,
    )
}

/// `csp` is the Content-Security-Policy for HTML documents (`None` for other assets).
fn respond(
    asset: Asset,
    immutable: bool,
    csp: Option<String>,
    if_none_match: Option<&str>,
    head: bool,
) -> Response {
    let cache = if immutable {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    let not_modified = if_none_match.is_some_and(|v| {
        v.split(',')
            .any(|t| t.trim() == asset.etag || t.trim() == "*")
    });
    let mut resp = if not_modified {
        StatusCode::NOT_MODIFIED.into_response()
    } else if head {
        let mut r = StatusCode::OK.into_response();
        r.headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(asset.data.len()));
        r
    } else {
        let body = match asset.data {
            Cow::Borrowed(b) => axum::body::Body::from(b),
            Cow::Owned(v) => axum::body::Body::from(v),
        };
        Response::new(body)
    };
    let h = resp.headers_mut();
    if !not_modified && let Ok(v) = HeaderValue::from_str(&asset.mime) {
        h.insert(header::CONTENT_TYPE, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    if let Ok(v) = HeaderValue::from_str(&asset.etag) {
        h.insert(header::ETAG, v);
    }
    if let Some(v) = csp.and_then(|p| HeaderValue::from_str(&p).ok()) {
        h.insert(header::CONTENT_SECURITY_POLICY, v);
    }
    resp
}

/// `GET /?token=…`. A valid token gets the UI itself (the UI takes the token from the URL);
/// an invalid one is redirected to `/` without it. Neither ever sets a cookie holding the
/// token.
fn login(app: &App, token: &str, query: &str, csp: String, head: bool, secure: bool) -> Response {
    if !constant_time_eq(token.as_bytes(), app.token.as_bytes()) {
        return strip_token_redirect(query);
    }
    let mut resp = respond(index(), false, Some(csp), None, head);
    let h = resp.headers_mut();
    // The URL carries the token: never cache this response or leak it in a Referer.
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    h.remove(header::ETAG);
    if let Some(secret) = &app.ui_session
        && let Ok(v) =
            HeaderValue::from_str(&ui_session_cookie(app.api_addr.port(), secret, secure))
    {
        h.append(header::SET_COOKIE, v);
    }
    resp
}

/// Redirect to `/`, keeping any query parameters except `token`.
fn strip_token_redirect(query: &str) -> Response {
    let rest: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .filter(|(k, _)| k != "token")
        .collect();
    let location = if rest.is_empty() {
        "/".to_string()
    } else {
        format!(
            "/?{}",
            url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(rest)
                .finish()
        )
    };
    let mut resp = StatusCode::SEE_OTHER.into_response();
    let h = resp.headers_mut();
    if let Ok(v) = HeaderValue::from_str(&location) {
        h.insert(header::LOCATION, v);
    }
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_sanitized() {
        assert_eq!(asset_path("/").as_deref(), Some("index.html"));
        assert_eq!(
            asset_path("/assets/app-1a2b.js").as_deref(),
            Some("assets/app-1a2b.js")
        );
        assert_eq!(asset_path("/../Cargo.toml"), None);
        assert_eq!(asset_path("/assets/%2e%2e/secret"), None);
        assert_eq!(asset_path("/a\\..\\b"), None);
        assert_eq!(asset_path("/traffic/42").as_deref(), Some("traffic/42"));
    }

    #[test]
    fn etag_revalidation() {
        let a = placeholder();
        let etag = a.etag.clone();
        let r = respond(a, false, Some(FRAME_POLICY.into()), Some(&etag), false);
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);
        let r = respond(placeholder(), true, None, Some("\"other\""), false);
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get(header::CACHE_CONTROL).unwrap(),
            "public, max-age=31536000, immutable"
        );
        let policy = csp(false, Some("127.0.0.1:45456"));
        let r = respond(placeholder(), false, Some(policy.clone()), None, false);
        assert_eq!(r.headers().get(header::CACHE_CONTROL).unwrap(), "no-cache");
        assert_eq!(
            r.headers().get(header::CONTENT_SECURITY_POLICY).unwrap(),
            policy.as_str()
        );
    }

    /// The directives of a policy, by name.
    fn directives(policy: &str) -> std::collections::HashMap<&str, Vec<&str>> {
        policy
            .split(';')
            .filter_map(|d| {
                let mut words = d.split_whitespace();
                Some((words.next()?, words.collect()))
            })
            .collect()
    }

    #[test]
    fn csp_only_runs_bundled_scripts_and_only_talks_to_the_daemon() {
        let p = csp(false, Some("127.0.0.1:45456"));
        let d = directives(&p);
        assert_eq!(d["script-src"], ["'self'"]);
        assert_eq!(d["object-src"], ["'none'"]);
        assert_eq!(d["base-uri"], ["'none'"]);
        assert_eq!(d["form-action"], ["'none'"]);
        assert_eq!(d["frame-ancestors"], ["'self'"]);
        assert_eq!(
            d["connect-src"],
            ["'self'", "ws://127.0.0.1:45456", "wss://127.0.0.1:45456"]
        );
        assert!(!p.contains("unsafe-eval"));
        assert!(!d["script-src"].contains(&"'unsafe-inline'"));
        // Every fetch directive is explicit or falls back to 'self'.
        assert_eq!(d["default-src"], ["'self'"]);
    }

    #[test]
    fn csp_host_cannot_inject_directives() {
        for host in [
            "evil.test; script-src *",
            "a b",
            "x'",
            "[::1]:45456",
            "h\tx",
            "",
        ] {
            let p = csp(false, Some(host));
            let d = directives(&p);
            assert_eq!(d["connect-src"], ["'self'"], "{host}");
            assert_eq!(d["script-src"], ["'self'"], "{host}");
        }
        let p = csp(false, None);
        assert_eq!(directives(&p)["connect-src"], ["'self'"]);
    }

    #[test]
    fn csp_lets_the_desktop_shell_use_tauri_ipc() {
        let p = csp(true, Some("localhost:1234"));
        let d = directives(&p);
        assert!(d["connect-src"].contains(&"ipc:"));
        assert!(d["connect-src"].contains(&"http://ipc.localhost"));
        assert_eq!(d["frame-ancestors"], ["'self'"]);
        assert!(!csp(false, None).contains("ipc"));
    }

    #[test]
    fn invalid_login_token_is_stripped() {
        let r = strip_token_redirect("token=nope&mock");
        assert_eq!(r.status(), StatusCode::SEE_OTHER);
        assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/?mock=");
        assert!(r.headers().get(header::SET_COOKIE).is_none());
        let r = strip_token_redirect("token=nope");
        assert_eq!(r.headers().get(header::LOCATION).unwrap(), "/");
    }

    #[test]
    fn index_is_available() {
        // Either the built UI or the placeholder written by build.rs / compiled in.
        let idx = index();
        assert!(idx.mime.starts_with("text/html"));
        assert!(!idx.data.is_empty());
    }
}
