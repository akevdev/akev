//! Authentication, Origin/Host checks and CORS for the local API (see the HTTP API
//! reference, akev.dev/docs/api § Authentication).
//!
//! Threat model: the API can launch applications and read all captured traffic, so it must be
//! unreachable from web pages the user happens to visit (CSRF / drive-by / DNS rebinding) and
//! from other local users without the token.
//!
//! * `/api/*` and `/mcp` require the session token (`Authorization: Bearer` or `?token=`),
//!   compared in constant time. The token is never put in a cookie: cookies are not isolated
//!   by port, so a host cookie for `127.0.0.1` would be sent to every other service on
//!   loopback (dev servers, containers, other users' apps, Akev's own proxy port, where it
//!   would be recorded). The browser UI keeps the token in (port-isolated) web storage.
//! * Requests carrying a browser `Origin` are rejected unless the origin is the API's own
//!   origin or explicitly allowed (`--allow-origin`; the Tauri webview origins only when the
//!   embedder opts in). `Origin: null` is always rejected (sandboxed documents, e.g. raw
//!   captured bodies served with `CSP: sandbox`).
//! * When bound to loopback, the `Host` header must be a loopback name — this blocks DNS
//!   rebinding even for the public static UI.
//! * When bound to a network address, the static UI requires authentication too: the login
//!   URL sets a *UI-only* session cookie (a separate random secret that never authorizes
//!   `/api` or `/mcp`), and peers that keep failing authentication are throttled.
//! * CORS headers (with credentials) and Private Network Access preflights are answered only
//!   for allowed cross origins.

use axum::extract::{ConnectInfo, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http::header::{self, HeaderMap, HeaderValue};
use http::{Method, StatusCode};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::error::ApiError;

/// Prefix of the UI-only session cookie (`akev_ui_<api port>`) of network-exposed APIs.
pub const UI_SESSION_COOKIE_PREFIX: &str = "akev_ui_";

/// Window in which failed authentications from one network peer are counted.
const FAILURE_WINDOW: Duration = Duration::from_secs(60);
/// Failed authentications per window after which a network peer is refused (`429`).
const MAX_FAILURES: u32 = 20;
/// Upper bound of tracked peers (old windows are pruned first).
const MAX_TRACKED_PEERS: usize = 4096;

/// Security policy for one API server.
#[derive(Debug, Clone)]
pub struct Security {
    token: String,
    port: u16,
    loopback_only: bool,
    /// Normalized extra allowed origins (lowercase, no trailing slash).
    allowed_origins: Vec<String>,
    /// `host[:port]` authorities of the allowed origins (accepted as Host header values).
    allowed_hosts: Vec<String>,
    /// Secret of the UI-only session cookie (network-exposed APIs).
    ui_session: Option<String>,
    /// Served over HTTPS too (network-exposed APIs): `https://` origins are our own.
    tls: bool,
    /// Failed-authentication throttle for network peers.
    throttle: Arc<FailureThrottle>,
    /// One-time download tickets: browser downloads (`<a href download>`) carry a ticket for
    /// one URL instead of the token, so the token never lands in download history.
    tickets: Arc<Mutex<HashMap<String, Ticket>>>,
}

/// A download ticket: one `GET` of `path`, before `expires`.
#[derive(Debug)]
struct Ticket {
    path: String,
    expires: Instant,
}

/// How long a download ticket is valid.
pub const TICKET_TTL: Duration = Duration::from_secs(60);
/// Most tickets outstanding at once (oldest are dropped).
const MAX_TICKETS: usize = 256;

/// Counts failed authentication *guesses* per network peer (IPv4 address / IPv6 /64): only
/// distinct wrong credentials count, so a browser tab retrying one stale token after a
/// restart doesn't lock its user out, while a brute force (always new guesses) is stopped.
#[derive(Debug, Default)]
struct FailureThrottle {
    peers: Mutex<HashMap<IpAddr, PeerFailures>>,
    /// Keyed hasher for credential fingerprints (never stored in clear).
    hasher: std::collections::hash_map::RandomState,
}

#[derive(Debug)]
struct PeerFailures {
    window_start: Instant,
    guesses: u32,
    /// Fingerprints of the most recent distinct wrong credentials.
    recent: std::collections::VecDeque<u64>,
}

/// Distinct wrong credentials remembered per peer (repeats of these don't count).
const RECENT_GUESSES: usize = 8;

impl FailureThrottle {
    /// `Some(remaining)` when `peer` exceeded the guess budget of its current window.
    fn blocked(&self, peer: IpAddr, now: Instant) -> Option<Duration> {
        let peers = self.peers.lock();
        let p = peers.get(&throttle_key(peer))?;
        let elapsed = now.saturating_duration_since(p.window_start);
        (p.guesses >= MAX_FAILURES && elapsed < FAILURE_WINDOW).then(|| FAILURE_WINDOW - elapsed)
    }

    /// Fingerprint of the credentials a request presented (`None` = it presented none, which
    /// isn't a guess).
    fn fingerprint(&self, credentials: &[&str]) -> Option<u64> {
        use std::hash::BuildHasher;
        (!credentials.is_empty()).then(|| self.hasher.hash_one(credentials))
    }

    fn record_failure(&self, peer: IpAddr, credential: u64, now: Instant) {
        let mut peers = self.peers.lock();
        if peers.len() >= MAX_TRACKED_PEERS {
            peers.retain(|_, p| now.saturating_duration_since(p.window_start) < FAILURE_WINDOW);
            if peers.len() >= MAX_TRACKED_PEERS {
                // Still full of fresh entries: forget the oldest window.
                if let Some(oldest) = peers
                    .iter()
                    .min_by_key(|(_, p)| p.window_start)
                    .map(|(k, _)| *k)
                {
                    peers.remove(&oldest);
                }
            }
        }
        let entry = peers
            .entry(throttle_key(peer))
            .or_insert_with(|| PeerFailures {
                window_start: now,
                guesses: 0,
                recent: Default::default(),
            });
        if now.saturating_duration_since(entry.window_start) >= FAILURE_WINDOW {
            entry.window_start = now;
            entry.guesses = 0;
        }
        if entry.recent.contains(&credential) {
            return;
        }
        if entry.recent.len() >= RECENT_GUESSES {
            entry.recent.pop_front();
        }
        entry.recent.push_back(credential);
        entry.guesses = entry.guesses.saturating_add(1);
    }
}

/// IPv4 addresses (incl. IPv4-mapped IPv6) as is; IPv6 by /64, which one host controls.
fn throttle_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => {
                let s = v6.segments();
                IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
            }
        },
    }
}

/// How a request's `Origin` relates to the API.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OriginClass {
    /// The API's own origin (no CORS headers needed).
    SameOrigin,
    /// An allowed foreign origin (CORS headers are added).
    AllowedCross,
    /// Not allowed.
    Forbidden,
}

impl Security {
    /// `bind` is the API bind address, `port` the actual listening port.
    pub fn new(token: String, bind: &str, port: u16, allowed_origins: &[String]) -> Self {
        let loopback_only = bind_is_loopback(bind);
        let allowed_origins: Vec<String> = allowed_origins
            .iter()
            .map(|o| normalize_origin(o))
            .filter(|o| !o.is_empty() && o != "null")
            .collect();
        let allowed_hosts = allowed_origins
            .iter()
            .filter_map(|o| o.split_once("://").map(|(_, rest)| rest.to_string()))
            .collect();
        Security {
            token,
            port,
            loopback_only,
            allowed_origins,
            allowed_hosts,
            ui_session: None,
            tls: false,
            throttle: Arc::new(FailureThrottle::default()),
            tickets: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// The API is also served over HTTPS (network-exposed APIs): pages loaded from
    /// `https://<host>:<port>` are same-origin.
    pub fn with_tls(mut self, tls: bool) -> Self {
        self.tls = tls;
        self
    }

    /// Accept this secret in the UI-only session cookie (`akev_ui_<port>`) for the static UI.
    pub fn with_ui_session(mut self, secret: Option<String>) -> Self {
        self.ui_session = secret.filter(|s| !s.is_empty());
        self
    }

    /// Name of the UI-only session cookie (per API port, so instances don't clash).
    pub fn ui_session_cookie_name(&self) -> String {
        ui_session_cookie_name(self.port)
    }

    /// Whether a request carries the UI-only session cookie. It only ever authorizes the
    /// static UI, never `/api` or `/mcp`.
    pub fn ui_session_authorized(&self, headers: &HeaderMap) -> bool {
        let Some(secret) = &self.ui_session else {
            return false;
        };
        let name = self.ui_session_cookie_name();
        headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .any(|s| {
                cookie_values(s, &name).any(|v| constant_time_eq(v.as_bytes(), secret.as_bytes()))
            })
    }

    /// Whether a request to `path` must carry the token. On loopback binds only the API and
    /// MCP endpoints are protected (the static UI holds no data); when the API is reachable
    /// from the network, everything is.
    pub fn requires_token(&self, path: &str) -> bool {
        !self.loopback_only || is_protected_path(path)
    }

    /// Constant-time token comparison.
    pub fn token_matches(&self, candidate: &str) -> bool {
        constant_time_eq(candidate.as_bytes(), self.token.as_bytes())
    }

    /// Whether a `Host` header value is acceptable (DNS rebinding protection).
    pub fn host_allowed(&self, host: Option<&str>) -> bool {
        if !self.loopback_only {
            return true;
        }
        let Some(host) = host else { return true };
        let host = host.trim().to_ascii_lowercase();
        if self.allowed_hosts.contains(&host) {
            return true;
        }
        let (name, _port) = split_host_port(&host);
        is_loopback_name(name)
    }

    /// Classify an `Origin` header value. `host` is the request's Host header.
    pub fn classify_origin(&self, origin: &str, host: Option<&str>) -> OriginClass {
        let origin = normalize_origin(origin);
        if origin.is_empty() || origin == "null" {
            return OriginClass::Forbidden;
        }
        if self.allowed_origins.contains(&origin) {
            return OriginClass::AllowedCross;
        }
        let (authority, default_port) = match origin.strip_prefix("http://") {
            Some(a) => (a, 80),
            None => match origin.strip_prefix("https://") {
                Some(a) if self.tls => (a, 443),
                _ => return OriginClass::Forbidden,
            },
        };
        let (name, port) = split_host_port(authority);
        let port = port.unwrap_or(default_port);
        // Loopback origins on our own port are always us (127.0.0.1 vs localhost vs [::1]).
        if port == self.port && is_loopback_name(name) {
            return OriginClass::SameOrigin;
        }
        // Otherwise: same origin as the Host the request was sent to (LAN access when bound to
        // a non-loopback address). The Host itself was validated by `host_allowed`.
        if let Some(host) = host {
            let host = host.trim().to_ascii_lowercase();
            let (hname, hport) = split_host_port(&host);
            if hname == name
                && hport.unwrap_or(default_port) == port
                && self.host_allowed(Some(&host))
            {
                return OriginClass::SameOrigin;
            }
        }
        OriginClass::Forbidden
    }

    /// Issue a one-time ticket for a `GET` of `path` (an `/api/` path), valid for
    /// [`TICKET_TTL`]. `None` for paths that can't be downloaded with a ticket.
    pub fn issue_ticket(&self, path: &str) -> Option<String> {
        if !path.starts_with("/api/")
            || path.contains('?')
            || path.starts_with("/api/download-ticket")
        {
            return None;
        }
        let now = Instant::now();
        let ticket = {
            let mut bytes = [0u8; 24];
            rand::fill(&mut bytes[..]);
            hex::encode(bytes)
        };
        let mut tickets = self.tickets.lock();
        tickets.retain(|_, t| t.expires > now);
        if tickets.len() >= MAX_TICKETS
            && let Some(oldest) = tickets
                .iter()
                .min_by_key(|(_, t)| t.expires)
                .map(|(k, _)| k.clone())
        {
            tickets.remove(&oldest);
        }
        tickets.insert(
            ticket.clone(),
            Ticket {
                path: path.to_string(),
                expires: now + TICKET_TTL,
            },
        );
        Some(ticket)
    }

    /// Whether a `GET` of `path` carries a valid ticket for it (`?ticket=`); the ticket is
    /// used up either way once presented.
    fn redeem_ticket(&self, method: &Method, path: &str, query: Option<&str>) -> bool {
        if method != Method::GET {
            return false;
        }
        let Some(ticket) = query.and_then(|q| {
            url::form_urlencoded::parse(q.as_bytes())
                .find(|(k, _)| k == "ticket")
                .map(|(_, v)| v.into_owned())
        }) else {
            return false;
        };
        let found = self.tickets.lock().remove(&ticket);
        found.is_some_and(|t| t.path == path && t.expires > Instant::now())
    }

    /// Every credential a request presents (bearer token, `?token=`, UI session cookie).
    fn presented_credentials<'a>(
        &self,
        headers: &'a HeaderMap,
        query_token: Option<&'a str>,
    ) -> Vec<&'a str> {
        let mut out = Vec::new();
        if let Some((_, token)) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().split_once(' '))
        {
            out.push(token.trim());
        }
        out.extend(query_token);
        let name = self.ui_session_cookie_name();
        for s in headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
        {
            out.extend(cookie_values(s, &name));
        }
        out
    }

    /// Whether the request carries the API token (`Authorization: Bearer` or `?token=`).
    pub fn request_authorized(&self, headers: &HeaderMap, query: Option<&str>) -> bool {
        if let Some(value) = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
        {
            let value = value.trim();
            if let Some((scheme, token)) = value.split_once(' ')
                && scheme.eq_ignore_ascii_case("bearer")
                && self.token_matches(token.trim())
            {
                return true;
            }
        }
        query
            .and_then(query_token)
            .is_some_and(|token| self.token_matches(&token))
    }
}

/// Name of the UI-only session cookie for an API on `port`.
pub fn ui_session_cookie_name(port: u16) -> String {
    format!("{UI_SESSION_COOKIE_PREFIX}{port}")
}

/// The `Set-Cookie` value for the UI-only session cookie (`secure` when it was set over
/// HTTPS for another machine: the browser then never sends it over plain HTTP).
pub fn ui_session_cookie(port: u16, secret: &str, secure: bool) -> String {
    format!(
        "{}={secret}; Path=/; HttpOnly; SameSite=Strict{}",
        ui_session_cookie_name(port),
        if secure { "; Secure" } else { "" }
    )
}

/// `true` for paths that require the token.
pub fn is_protected_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/") || path == "/mcp" || path.starts_with("/mcp/")
}

/// Extract `token` from a query string (percent-decoded).
pub fn query_token(query: &str) -> Option<String> {
    url::form_urlencoded::parse(query.as_bytes())
        .find(|(k, _)| k == "token")
        .map(|(_, v)| v.into_owned())
}

fn cookie_values<'a>(header: &'a str, name: &str) -> impl Iterator<Item = &'a str> {
    header.split(';').filter_map(move |pair| {
        let (k, v) = pair.split_once('=')?;
        (k.trim() == name).then(|| v.trim().trim_matches('"'))
    })
}

/// Constant-time byte comparison (the length is not secret).
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    std::hint::black_box(diff) == 0
}

fn normalize_origin(origin: &str) -> String {
    origin.trim().trim_end_matches('/').to_ascii_lowercase()
}

/// Split `host:port` / `[v6]:port` / `host` into (host without brackets, port).
fn split_host_port(authority: &str) -> (&str, Option<u16>) {
    if let Some(rest) = authority.strip_prefix('[') {
        if let Some((h, after)) = rest.split_once(']') {
            let port = after.strip_prefix(':').and_then(|p| p.parse().ok());
            return (h, port);
        }
        return (rest, None);
    }
    match authority.rsplit_once(':') {
        // A bare IPv6 address without brackets has several colons: no port.
        Some((h, p)) if !h.contains(':') => (h, p.parse().ok()),
        _ => (authority, None),
    }
}

/// Loopback hostnames: `localhost`, `*.localhost` (RFC 6761), 127.0.0.0/8, ::1.
fn is_loopback_name(name: &str) -> bool {
    let name = name.trim_end_matches('.');
    if name == "localhost" || name.ends_with(".localhost") {
        return true;
    }
    name.parse::<IpAddr>()
        .map(|ip| ip.is_loopback())
        .unwrap_or(false)
}

/// Whether the API bind address only accepts loopback connections.
pub fn bind_is_loopback(bind: &str) -> bool {
    let b = bind.trim().trim_start_matches('[').trim_end_matches(']');
    b == "localhost"
        || b.parse::<IpAddr>()
            .map(|ip| ip.is_loopback())
            .unwrap_or(false)
}

fn request_host(req: &Request) -> Option<String> {
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .or_else(|| req.uri().authority().map(|a| a.as_str().to_string()))
}

fn apply_cors(headers: &mut HeaderMap, origin: &str) {
    if let Ok(v) = HeaderValue::from_str(origin) {
        headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, v);
    }
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_CREDENTIALS,
        HeaderValue::from_static("true"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static("content-disposition, content-length"),
    );
    headers.append(header::VARY, HeaderValue::from_static("Origin"));
}

/// `401` page for browsers opening the UI of a network-exposed API without the token.
fn login_required_page() -> Response {
    const PAGE: &str = "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>Akev</title><style>:root{color-scheme:light dark}body{font:15px/1.5 system-ui,sans-serif;max-width:36rem;margin:15vh auto;padding:0 1.5rem}code{font:13px ui-monospace,monospace}</style></head><body><h1>Akev</h1><p>This Akev instance is reachable from the network, so the web UI requires the access token. Open the full URL printed by <code>akev start</code> (it ends with <code>/?token=…</code>), or run <code>akev status</code> on the host to see it.</p></body></html>";
    let mut resp = (StatusCode::UNAUTHORIZED, PAGE).into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    // A static page: nothing may load, run or frame it.
    h.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'",
        ),
    );
    resp
}

/// The connecting peer (when the server was started with connect info) if it should be
/// throttled: only network peers of a network-exposed API (loopback clients share one
/// address, so throttling them would let one local user lock out another).
fn throttled_peer(security: &Security, req: &Request) -> Option<IpAddr> {
    if security.loopback_only {
        return None;
    }
    let ConnectInfo(addr) = req.extensions().get::<ConnectInfo<SocketAddr>>()?;
    let ip = addr.ip();
    let ip = match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(ip),
        v4 => v4,
    };
    (!ip.is_loopback()).then_some(ip)
}

fn too_many_attempts(retry_after: Duration) -> Response {
    let secs = retry_after.as_secs().max(1);
    let mut resp = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "too-many-attempts",
        format!("too many failed authentication attempts; try again in {secs} s"),
    )
    .into_response();
    resp.headers_mut()
        .insert(header::RETRY_AFTER, HeaderValue::from(secs));
    resp
}

/// Axum middleware enforcing the policy. Must wrap every route.
pub async fn guard(State(security): State<Arc<Security>>, req: Request, next: Next) -> Response {
    let security = &*security;
    let host = request_host(&req);
    if !security.host_allowed(host.as_deref()) {
        return ApiError::forbidden(
            "forbidden-host",
            "requests must address the API by a loopback hostname (DNS rebinding protection)",
        )
        .into_response();
    }

    let cross_origin = match req.headers().get(header::ORIGIN).map(|v| v.to_str()) {
        None => None,
        Some(Err(_)) => {
            return ApiError::forbidden("forbidden-origin", "invalid Origin header")
                .into_response();
        }
        Some(Ok(origin)) => {
            match security.classify_origin(origin, host.as_deref()) {
                OriginClass::SameOrigin => None,
                OriginClass::AllowedCross => Some(origin.to_string()),
                OriginClass::Forbidden => {
                    return ApiError::forbidden(
                    "forbidden-origin",
                    format!("origin {origin} is not allowed to use the Akev API (see --allow-origin)"),
                )
                .into_response();
                }
            }
        }
    };

    // CORS preflight (no credentials are sent on preflights).
    if req.method() == Method::OPTIONS
        && let Some(origin) = &cross_origin
        && req
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
    {
        let mut resp = StatusCode::NO_CONTENT.into_response();
        let h = resp.headers_mut();
        apply_cors(h, origin);
        h.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, PATCH, DELETE, OPTIONS"),
        );
        let allow_headers = req
            .headers()
            .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("authorization, content-type"));
        h.insert(header::ACCESS_CONTROL_ALLOW_HEADERS, allow_headers);
        h.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("86400"),
        );
        if req
            .headers()
            .contains_key("access-control-request-private-network")
        {
            h.insert(
                "access-control-allow-private-network",
                HeaderValue::from_static("true"),
            );
        }
        return resp;
    }

    let path = req.uri().path();
    if security.requires_token(path) {
        let peer = throttled_peer(security, &req);
        let now = Instant::now();
        let refused = match peer.and_then(|ip| security.throttle.blocked(ip, now)) {
            Some(retry_after) => Some(too_many_attempts(retry_after)),
            None => {
                let authorized = security.request_authorized(req.headers(), req.uri().query())
                    || (!is_protected_path(path) && security.ui_session_authorized(req.headers()))
                    || security.redeem_ticket(req.method(), path, req.uri().query());
                (!authorized).then(|| {
                    if let Some(ip) = peer {
                        let query = req.uri().query().and_then(query_token);
                        let presented =
                            security.presented_credentials(req.headers(), query.as_deref());
                        if let Some(guess) = security.throttle.fingerprint(&presented) {
                            security.throttle.record_failure(ip, guess, now);
                        }
                    }
                    if is_protected_path(path) {
                        ApiError::unauthorized().into_response()
                    } else {
                        login_required_page()
                    }
                })
            }
        };
        if let Some(mut resp) = refused {
            if let Some(origin) = &cross_origin {
                apply_cors(resp.headers_mut(), origin);
            }
            return resp;
        }
    }

    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    if let Some(origin) = &cross_origin {
        apply_cors(h, origin);
    }
    h.entry(header::X_CONTENT_TYPE_OPTIONS)
        .or_insert(HeaderValue::from_static("nosniff"));
    h.entry(header::REFERRER_POLICY)
        .or_insert(HeaderValue::from_static("no-referrer"));
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::routing::get;
    use tower::ServiceExt;

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn download_tickets_are_one_time_and_bound_to_a_path() {
        let s = Security::new(TOKEN.into(), "127.0.0.1", 45456, &[]);
        assert!(s.issue_ticket("/").is_none(), "only API paths");
        assert!(s.issue_ticket("/api/download-ticket").is_none());
        assert!(
            s.issue_ticket("/api/export/har?x=1").is_none(),
            "no query in the path"
        );
        let t = s.issue_ticket("/api/export/har").unwrap();
        let q = format!("filter=x&ticket={t}");
        assert!(
            !s.redeem_ticket(&Method::POST, "/api/export/har", Some(&q)),
            "GET only"
        );
        let t = s.issue_ticket("/api/export/har").unwrap();
        let q = format!("ticket={t}");
        assert!(
            !s.redeem_ticket(&Method::GET, "/api/exchanges", Some(&q)),
            "other path"
        );
        assert!(
            !s.redeem_ticket(&Method::GET, "/api/export/har", Some(&q)),
            "used up"
        );
        let t = s.issue_ticket("/api/export/har").unwrap();
        let q = format!("ticket={t}&filter=a");
        assert!(s.redeem_ticket(&Method::GET, "/api/export/har", Some(&q)));
        assert!(
            !s.redeem_ticket(&Method::GET, "/api/export/har", Some(&q)),
            "one time"
        );
        assert!(!s.redeem_ticket(&Method::GET, "/api/export/har", Some("ticket=nope")));
    }

    fn sec() -> Security {
        Security::new(
            TOKEN.into(),
            "127.0.0.1",
            45456,
            &["http://localhost:5173/".to_string()],
        )
    }

    #[test]
    fn constant_time_compare() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn host_validation_blocks_rebinding() {
        let s = sec();
        assert!(s.host_allowed(Some("127.0.0.1:45456")));
        assert!(s.host_allowed(Some("localhost:45456")));
        assert!(s.host_allowed(Some("LOCALHOST")));
        assert!(s.host_allowed(Some("[::1]:45456")));
        assert!(s.host_allowed(Some("127.1.2.3")));
        assert!(s.host_allowed(Some("tauri.localhost")));
        assert!(s.host_allowed(Some("localhost:5173"))); // from allowed origin
        assert!(s.host_allowed(None));
        assert!(!s.host_allowed(Some("evil.com:45456")));
        assert!(!s.host_allowed(Some("192.168.1.10:45456")));
        assert!(!s.host_allowed(Some("localhost.evil.com")));
        // Bound to all interfaces: any Host is fine (token required anyway).
        let open = Security::new(TOKEN.into(), "0.0.0.0", 45456, &[]);
        assert!(open.host_allowed(Some("192.168.1.10:45456")));
    }

    #[test]
    fn origin_classification() {
        let s = sec();
        let host = Some("127.0.0.1:45456");
        assert_eq!(
            s.classify_origin("http://127.0.0.1:45456", host),
            OriginClass::SameOrigin
        );
        assert_eq!(
            s.classify_origin("http://localhost:45456", host),
            OriginClass::SameOrigin
        );
        assert_eq!(
            s.classify_origin("http://[::1]:45456", host),
            OriginClass::SameOrigin
        );
        assert_eq!(
            s.classify_origin("http://localhost:5173", host),
            OriginClass::AllowedCross
        );
        assert_eq!(
            s.classify_origin("HTTP://LOCALHOST:5173/", host),
            OriginClass::AllowedCross
        );
        assert_eq!(
            s.classify_origin("http://localhost:3000", host),
            OriginClass::Forbidden
        );
        assert_eq!(
            s.classify_origin("https://evil.com", host),
            OriginClass::Forbidden
        );
        assert_eq!(s.classify_origin("null", host), OriginClass::Forbidden);
        assert_eq!(s.classify_origin("", host), OriginClass::Forbidden);
        assert_eq!(
            s.classify_origin("https://127.0.0.1:45456", host),
            OriginClass::Forbidden
        );
        // DNS rebinding: Origin equals the (attacker controlled) Host → still forbidden.
        assert_eq!(
            s.classify_origin("http://evil.com:45456", Some("evil.com:45456")),
            OriginClass::Forbidden
        );
        // LAN access when bound to 0.0.0.0: origin == Host is same-origin.
        let open = Security::new(TOKEN.into(), "0.0.0.0", 45456, &[]);
        assert_eq!(
            open.classify_origin("http://192.168.1.10:45456", Some("192.168.1.10:45456")),
            OriginClass::SameOrigin
        );
        assert_eq!(
            open.classify_origin("http://192.168.1.11:45456", Some("192.168.1.10:45456")),
            OriginClass::Forbidden
        );
        // Served over HTTPS too: `https://` pages of the same host and port are ours.
        assert_eq!(
            open.classify_origin("https://192.168.1.10:45456", Some("192.168.1.10:45456")),
            OriginClass::Forbidden,
            "only when TLS is on"
        );
        let tls = Security::new(TOKEN.into(), "0.0.0.0", 45456, &[]).with_tls(true);
        assert_eq!(
            tls.classify_origin("https://192.168.1.10:45456", Some("192.168.1.10:45456")),
            OriginClass::SameOrigin
        );
        assert_eq!(
            tls.classify_origin("https://127.0.0.1:45456", None),
            OriginClass::SameOrigin
        );
        assert_eq!(
            tls.classify_origin("https://evil.com:45456", Some("192.168.1.10:45456")),
            OriginClass::Forbidden
        );
        // Default ports follow the scheme.
        let tls443 = Security::new(TOKEN.into(), "0.0.0.0", 443, &[]).with_tls(true);
        assert_eq!(
            tls443.classify_origin("https://box.lan", Some("box.lan")),
            OriginClass::SameOrigin
        );
        assert_eq!(
            tls443.classify_origin("https://box.lan:8443", Some("box.lan")),
            OriginClass::Forbidden
        );
    }

    #[test]
    fn token_sources() {
        let s = sec();
        let mut h = HeaderMap::new();
        assert!(!s.request_authorized(&h, None));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {TOKEN}")).unwrap(),
        );
        assert!(s.request_authorized(&h, None));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("bearer   {TOKEN}")).unwrap(),
        );
        assert!(s.request_authorized(&h, None));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer wrong"),
        );
        assert!(!s.request_authorized(&h, None));
        h.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {TOKEN}")).unwrap(),
        );
        assert!(!s.request_authorized(&h, None));

        let h = HeaderMap::new();
        assert!(s.request_authorized(&h, Some(&format!("a=1&token={TOKEN}"))));
        assert!(!s.request_authorized(&h, Some("token=nope")));
        assert!(!s.request_authorized(&h, Some(&format!("xtoken={TOKEN}"))));

        // The token is never accepted from a cookie (cookies leak to every loopback port).
        let mut h = HeaderMap::new();
        h.insert(
            header::COOKIE,
            HeaderValue::from_str(&format!("theme=dark; akev_token={TOKEN}")).unwrap(),
        );
        assert!(!s.request_authorized(&h, None));
    }

    #[test]
    fn ui_session_cookie_is_separate_from_the_token() {
        let s = sec().with_ui_session(Some("ui-secret".into()));
        assert_eq!(s.ui_session_cookie_name(), "akev_ui_45456");
        let cookie = |v: &str| {
            let mut h = HeaderMap::new();
            h.insert(header::COOKIE, HeaderValue::from_str(v).unwrap());
            h
        };
        assert!(s.ui_session_authorized(&cookie("a=1; akev_ui_45456=ui-secret")));
        assert!(!s.ui_session_authorized(&cookie("akev_ui_45457=ui-secret")));
        assert!(!s.ui_session_authorized(&cookie("akev_ui_45456=nope")));
        assert!(!s.ui_session_authorized(&cookie(&format!("akev_ui_45456={TOKEN}"))));
        // The UI cookie is never an API credential.
        assert!(!s.request_authorized(&cookie("akev_ui_45456=ui-secret"), None));
        // Without a UI session (loopback binds) the cookie means nothing.
        assert!(!sec().ui_session_authorized(&cookie("akev_ui_45456=ui-secret")));
        assert_eq!(
            ui_session_cookie(45456, "x", false),
            "akev_ui_45456=x; Path=/; HttpOnly; SameSite=Strict"
        );
    }

    #[test]
    fn throttle_keys_group_ipv6_by_prefix() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(throttle_key(a), throttle_key(b));
        assert_ne!(throttle_key(a), throttle_key(c));
        let mapped: IpAddr = "::ffff:10.0.0.5".parse().unwrap();
        assert_eq!(throttle_key(mapped), "10.0.0.5".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn throttle_windows_expire() {
        let t = FailureThrottle::default();
        let ip: IpAddr = "10.0.0.5".parse().unwrap();
        let start = Instant::now();
        for guess in 0..u64::from(MAX_FAILURES) - 1 {
            t.record_failure(ip, guess, start);
        }
        assert!(t.blocked(ip, start).is_none());
        t.record_failure(ip, 1000, start);
        let remaining = t.blocked(ip, start + Duration::from_secs(10)).unwrap();
        assert_eq!(remaining, FAILURE_WINDOW - Duration::from_secs(10));
        assert!(t.blocked(ip, start + FAILURE_WINDOW).is_none());
        // A failure after the window starts a new one.
        t.record_failure(ip, 2000, start + FAILURE_WINDOW);
        assert!(t.blocked(ip, start + FAILURE_WINDOW).is_none());
    }

    #[test]
    fn repeating_one_stale_credential_is_not_guessing() {
        let t = FailureThrottle::default();
        let ip: IpAddr = "10.0.0.6".parse().unwrap();
        let now = Instant::now();
        let stale = t.fingerprint(&["old-token"]).unwrap();
        for _ in 0..MAX_FAILURES * 5 {
            t.record_failure(ip, stale, now);
        }
        assert!(t.blocked(ip, now).is_none());
        assert_eq!(t.fingerprint(&[]), None, "no credential, no guess");
        assert_ne!(t.fingerprint(&["a"]), t.fingerprint(&["b"]));
    }

    #[test]
    fn protected_paths() {
        assert!(is_protected_path("/api/status"));
        assert!(is_protected_path("/api"));
        assert!(is_protected_path("/mcp"));
        assert!(!is_protected_path("/"));
        assert!(!is_protected_path("/assets/index-abc.js"));
        assert!(!is_protected_path("/apix"));
        assert!(!is_protected_path("/mcpx"));
    }

    #[test]
    fn host_port_split() {
        assert_eq!(split_host_port("a.b:80"), ("a.b", Some(80)));
        assert_eq!(split_host_port("a.b"), ("a.b", None));
        assert_eq!(split_host_port("[::1]:8080"), ("::1", Some(8080)));
        assert_eq!(split_host_port("[::1]"), ("::1", None));
        assert_eq!(split_host_port("::1"), ("::1", None));
    }

    fn app() -> Router {
        let security = Arc::new(sec());
        Router::new()
            .route("/api/status", get(|| async { "ok" }))
            .route("/", get(|| async { "ui" }))
            .layer(axum::middleware::from_fn_with_state(security, guard))
    }

    async fn call(req: http::Request<Body>) -> Response {
        app().oneshot(req).await.unwrap()
    }

    fn req(method: &str, uri: &str) -> http::request::Builder {
        http::Request::builder()
            .method(method)
            .uri(uri)
            .header(header::HOST, "127.0.0.1:45456")
    }

    #[tokio::test]
    async fn middleware_requires_token_for_api() {
        let r = call(req("GET", "/api/status").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        let r = call(
            req("GET", "/api/status")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers().get(header::X_CONTENT_TYPE_OPTIONS).unwrap(),
            "nosniff"
        );
        let r = call(
            req("GET", &format!("/api/status?token={TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        // Static UI is public.
        let r = call(req("GET", "/").body(Body::empty()).unwrap()).await;
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn middleware_rejects_foreign_origin_even_with_token() {
        let r = call(
            req("GET", &format!("/api/status?token={TOKEN}"))
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        let body = axum::body::to_bytes(r.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "forbidden-origin");
        // Even the public UI can't be fetched cross-origin from a foreign page.
        let r = call(
            req("GET", "/")
                .header(header::ORIGIN, "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn network_bind_requires_token_everywhere() {
        let security = Arc::new(Security::new(TOKEN.into(), "0.0.0.0", 45456, &[]));
        let app = Router::new()
            .route("/", get(|| async { "ui" }))
            .route("/assets/app.js", get(|| async { "js" }))
            .layer(axum::middleware::from_fn_with_state(security, guard));
        let lan = |uri: &str| {
            http::Request::builder()
                .uri(uri)
                .header(header::HOST, "192.168.1.10:45456")
        };
        let r = app
            .clone()
            .oneshot(lan("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(
            r.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("text/html")
        );
        let r = app
            .clone()
            .oneshot(lan("/assets/app.js").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        // The login URL (query token) works.
        let r = app
            .clone()
            .oneshot(
                lan(&format!("/?token={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // A cookie holding the API token doesn't.
        let r = app
            .oneshot(
                lan("/assets/app.js")
                    .header(header::COOKIE, format!("akev_token={TOKEN}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        // Loopback binds keep the static UI public.
        assert!(!sec().requires_token("/"));
        assert!(sec().requires_token("/api/status"));
    }

    #[tokio::test]
    async fn ui_session_cookie_unlocks_only_the_static_ui() {
        let security = Arc::new(
            Security::new(TOKEN.into(), "0.0.0.0", 45456, &[])
                .with_ui_session(Some("ui-secret".into())),
        );
        let app = Router::new()
            .route("/assets/app.js", get(|| async { "js" }))
            .route("/api/status", get(|| async { "ok" }))
            .route("/mcp", get(|| async { "mcp" }))
            .layer(axum::middleware::from_fn_with_state(security, guard));
        let with_cookie = |uri: &str| {
            http::Request::builder()
                .uri(uri)
                .header(header::HOST, "192.168.1.10:45456")
                .header(header::COOKIE, "akev_ui_45456=ui-secret")
                .body(Body::empty())
                .unwrap()
        };
        let r = app
            .clone()
            .oneshot(with_cookie("/assets/app.js"))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        for path in ["/api/status", "/mcp"] {
            let r = app.clone().oneshot(with_cookie(path)).await.unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED, "{path}");
        }
    }

    fn from_peer(uri: &str, peer: &str) -> http::Request<Body> {
        let mut req = http::Request::builder()
            .uri(uri)
            .header(header::HOST, "192.168.1.10:45456")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(ConnectInfo(peer.parse::<SocketAddr>().unwrap()));
        req
    }

    #[tokio::test]
    async fn network_peers_that_keep_failing_are_throttled() {
        let security = Arc::new(Security::new(TOKEN.into(), "0.0.0.0", 45456, &[]));
        let app = Router::new()
            .route("/api/status", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(security, guard));
        // A stale tab retrying one old token, or requests without any credential, aren't
        // guesses.
        for _ in 0..MAX_FAILURES * 2 {
            for uri in ["/api/status?token=stale", "/api/status"] {
                let r = app
                    .clone()
                    .oneshot(from_peer(uri, "192.168.1.30:1"))
                    .await
                    .unwrap();
                assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
            }
        }
        let r = app
            .clone()
            .oneshot(from_peer(
                &format!("/api/status?token={TOKEN}"),
                "192.168.1.30:1",
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);

        let attacker = "192.168.1.66:50000";
        for i in 0..MAX_FAILURES {
            let r = app
                .clone()
                .oneshot(from_peer(&format!("/api/status?token=guess{i}"), attacker))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }
        // Further guesses aren't even evaluated — not even a right one.
        let r = app
            .clone()
            .oneshot(from_peer(&format!("/api/status?token={TOKEN}"), attacker))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(r.headers().contains_key(header::RETRY_AFTER));
        // Other peers are unaffected.
        let r = app
            .clone()
            .oneshot(from_peer(
                &format!("/api/status?token={TOKEN}"),
                "192.168.1.20:40000",
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
        // Loopback clients (the user's own tools, a local reverse proxy) are never throttled.
        for _ in 0..MAX_FAILURES + 5 {
            let r = app
                .clone()
                .oneshot(from_peer("/api/status?token=nope", "127.0.0.1:1234"))
                .await
                .unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }
        let r = app
            .clone()
            .oneshot(from_peer(
                &format!("/api/status?token={TOKEN}"),
                "127.0.0.1:1234",
            ))
            .await
            .unwrap();
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn loopback_binds_are_not_throttled() {
        let app = app();
        for _ in 0..MAX_FAILURES + 5 {
            let mut req = req("GET", "/api/status?token=nope")
                .body(Body::empty())
                .unwrap();
            req.extensions_mut()
                .insert(ConnectInfo("127.0.0.1:1".parse::<SocketAddr>().unwrap()));
            let r = app.clone().oneshot(req).await.unwrap();
            assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        }
    }

    #[tokio::test]
    async fn tauri_origin_preflight_is_refused() {
        let r = call(
            req("OPTIONS", "/api/status")
                .header(header::ORIGIN, "http://tauri.localhost")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "authorization")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
        assert!(
            !r.headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }

    #[tokio::test]
    async fn middleware_rejects_rebinding_host() {
        let r = call(
            http::Request::builder()
                .uri("/")
                .header(header::HOST, "attacker.example:45456")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn middleware_cors_for_allowed_origins() {
        // Preflight without credentials succeeds for an allowed origin.
        let r = call(
            req("OPTIONS", "/api/status")
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "PUT")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization, content-type",
                )
                .header("access-control-request-private-network", "true")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            r.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "http://localhost:5173"
        );
        assert_eq!(
            r.headers()
                .get(header::ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .unwrap(),
            "true"
        );
        assert_eq!(
            r.headers()
                .get("access-control-allow-private-network")
                .unwrap(),
            "true"
        );
        // Actual request gets CORS headers.
        let r = call(
            req("GET", "/api/status")
                .header(header::ORIGIN, "http://localhost:5173")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert_eq!(
            r.headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .unwrap(),
            "http://localhost:5173"
        );
        // Unauthorized cross-origin responses are still readable by the allowed origin.
        let r = call(
            req("GET", "/api/status")
                .header(header::ORIGIN, "http://localhost:5173")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
        assert!(
            r.headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
        // Same-origin requests carry no CORS headers.
        let r = call(
            req("GET", "/api/status")
                .header(header::ORIGIN, "http://127.0.0.1:45456")
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(
            !r.headers()
                .contains_key(header::ACCESS_CONTROL_ALLOW_ORIGIN)
        );
    }
}
