//! The per-exchange pipeline shared by HTTP/1, HTTP/2 and Compose:
//!
//! capture → rules (`akev-rules`) → delay → request transforms → forward rewrite → scripts →
//! request breakpoint → terminal action (upstream / mock / map-local / close / reset / timeout /
//! WebSocket echo or reject) → response transforms / scripts / breakpoint (buffered) or a
//! streaming pass-through with capture tee, SSE parsing and throttling → client.
//!
//! Events are emitted through [`ExchangeEvents`] in causal order.

use crate::body::{
    Buffered, ClientBody, Collected, MAX_BUFFERED_BODY, MAX_DECODED_BODY, PASSED_THROUGH_NOTICE,
    Reservation, TeeStream, collect_within, throttled_replay,
};
use crate::errors::{client_aborted, error_response, exchange_error};
use crate::events::{Capture, ExchangeEvents, capture_full};
use crate::http1::parse::header_tokens;
use crate::responder::{AbortKind, BodyKind, Responder};
use crate::shared::{CounterGuard, Shared};
use crate::target::{authority_for, build_url, parse_absolute_url};
use crate::throttle::TokenBucket;
use crate::upstream::{
    BodyChunk, BodyStream, ConnectTimings, RequestBody, RequestOptions, Upgraded, UpstreamError,
};
use akev_core::rules::{Action, BodySource};
use akev_core::*;
use akev_rules::{Decision, MapLocalOutcome, MatchContext};
use bytes::Bytes;
use futures::StreamExt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A request body that never went upstream (the connection failed) is still read for the
/// capture: at least this many bytes (or the capture limit, if larger)...
const UNSENT_BODY_DRAIN_BYTES: u64 = 4 * 1024 * 1024;
/// ...for at most this long.
const UNSENT_BODY_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);
/// While the proxy → store queue is congested, a new exchange waits this long at most for
/// the store to catch up before it goes ahead (its bodies may then be dropped from capture).
const BACKPRESSURE_WAIT: Duration = Duration::from_millis(250);

/// What kind of connection upgrade the request asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Upgrade {
    WebSocket,
    /// Any other `Upgrade:` protocol: relayed as raw bytes after a 101.
    Other,
}

/// The request entering the pipeline.
pub(crate) struct ExchangeRequest {
    /// The request as received (recorded in `RequestStarted`).
    pub head: RequestHead,
    /// The request as it will be forwarded by default (proxy-only headers stripped, protocol
    /// translation applied). Rule changes are applied on top of this.
    pub forward: RequestHead,
    pub body: ClientBody,
    pub upgrade: Option<Upgrade>,
}

/// Per-exchange environment.
pub(crate) struct ExchangeEnv {
    pub client: ClientInfo,
    /// Interceptor id / process name used by `source` matchers.
    pub source: Option<String>,
    /// Started when the first byte of the request head arrived.
    pub sw: Stopwatch,
    pub request_headers_ms: f64,
    pub apply_rules: bool,
    pub options: RequestOptions,
    /// Set when the request is addressed to the proxy itself / `akev.cert`: the address the
    /// client used to reach the proxy.
    pub local: Option<String>,
}

/// How the client connection should continue.
pub(crate) enum Outcome {
    Done { keep_alive: bool },
    Upgrade(UpgradeJob),
}

/// A connection upgrade the HTTP/1 frontend must carry out with the raw client stream.
pub(crate) struct UpgradeJob {
    pub events: Arc<ExchangeEvents>,
    pub mode: UpgradeMode,
    pub capture_limit: usize,
    /// `ws-breakpoint` rules matched by the upgrade request (relayed WebSockets only).
    pub ws_breakpoints: Vec<RuleId>,
}

pub(crate) enum UpgradeMode {
    /// Relay WebSocket frames to/from upstream, emitting message events.
    WebSocket(Upgraded),
    /// Relay raw bytes (non-WebSocket `Upgrade`).
    Raw(Upgraded),
    /// Accept locally and echo every message (`ws-echo` rule).
    Echo,
}

/// Request body state while the pipeline runs.
enum ReqBody {
    /// Not read yet.
    Pending(ClientBody),
    /// Fully read; `RequestCompleted` already emitted.
    Buffered(Buffered),
}

/// A response body to deliver.
enum RespBody {
    Full(Bytes, Option<Headers>),
    Stream(BodyStream),
}

/// A body being edited (rules / scripts / breakpoints work on the decoded form).
struct EditBody {
    original_decoded: Bytes,
    decoded: Bytes,
    decode_ok: bool,
}

impl EditBody {
    /// The decoded body to edit, if it fits the rule-body budget.
    fn new(
        wire: &Bytes,
        headers: &Headers,
        reservation: &mut Reservation<'_>,
        budget: u64,
    ) -> Option<Self> {
        let (decoded, decode_ok) = decode_within(wire, headers, reservation, budget)?;
        Some(EditBody {
            original_decoded: decoded.clone(),
            decoded,
            decode_ok,
        })
    }

    fn changed(&self) -> bool {
        self.decoded != self.original_decoded
    }

    /// Re-encode the edited body for `headers`' Content-Encoding chain.
    fn encode(&self, headers: &mut Headers) -> Bytes {
        encode_for_headers(&self.decoded, headers, self.decode_ok)
    }
}

/// Encode a decoded body with the Content-Encoding listed in `headers` (dropping the header if
/// the encoding isn't supported, so headers and body stay consistent).
pub(crate) fn encode_for_headers(decoded: &Bytes, headers: &mut Headers, enabled: bool) -> Bytes {
    let enc = headers.content_encodings();
    if enc.is_empty() || !enabled {
        return decoded.clone();
    }
    match akev_content::decode::encode_body(decoded, &enc) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "can't re-encode modified body; sending it unencoded");
            headers.remove("content-encoding");
            decoded.clone()
        }
    }
}

/// After a body change: keep Content-Length truthful (add it unless the message is chunked).
/// An existing header keeps its position and casing; a new one follows the message's casing
/// style (`Content-Length` next to canonically-cased headers, lowercase otherwise).
pub(crate) fn fix_content_length(headers: &mut Headers, len: usize) {
    if header_tokens(headers, "transfer-encoding")
        .iter()
        .any(|t| t == "chunked")
    {
        return;
    }
    if headers.contains("content-length") {
        headers.set("content-length", len.to_string());
    } else {
        let canonical = headers
            .iter()
            .any(|(k, _)| k.bytes().any(|b| b.is_ascii_uppercase()));
        headers.append(
            if canonical {
                "Content-Length"
            } else {
                "content-length"
            },
            len.to_string(),
        );
    }
}

/// Decode a body for matching / editing within what the rule-body budget has left. `None`
/// when the decoded copy doesn't fit (the caller passes the exchange through); a body that
/// can't be decoded otherwise comes back as its raw bytes with `false`.
fn decode_within(
    data: &Bytes,
    headers: &Headers,
    reservation: &mut Reservation<'_>,
    budget: u64,
) -> Option<(Bytes, bool)> {
    let enc = headers.content_encodings();
    if enc.is_empty() {
        return Some((data.clone(), true));
    }
    let available = usize::try_from(reservation.available(budget)).unwrap_or(usize::MAX);
    let limit = MAX_DECODED_BODY.min(available);
    match akev_content::decode::decode_body(data, &enc, limit) {
        Ok(d) if reservation.grow(d.len() as u64, budget) => Some((d, true)),
        Ok(_) => None,
        // Out of budget rather than undecodable: don't fall back to editing raw bytes.
        Err(_) if limit < MAX_DECODED_BODY => None,
        Err(e) => {
            tracing::debug!(error = %e, "body could not be decoded; using the raw bytes");
            Some((data.clone(), false))
        }
    }
}

fn no_body_status(status: u16) -> bool {
    (100..200).contains(&status) || status == 204 || status == 304
}

/// Whether a script can have an `onRequest` hook. Mirrors the lookup in
/// `akev_rules::script::run_on_request`, which skips scripts whose source doesn't mention the
/// hook: only scripts that may change the request make the pipeline buffer its body.
fn has_request_hook(source: &str) -> bool {
    source.contains("onRequest")
}

/// Whether a script can have an `onResponse` hook (see [`has_request_hook`]).
fn has_response_hook(source: &str) -> bool {
    source.contains("onResponse")
}

fn is_event_stream(head: &ResponseHead) -> bool {
    (200..300).contains(&head.status)
        && head.headers.content_type_essence().as_deref() == Some("text/event-stream")
}

/// Apply an edited request from a breakpoint onto the forwarded head.
fn apply_editable_request(fwd: &mut RequestHead, edit: &mut EditBody, request: EditableRequest) {
    let old_authority = fwd.authority();
    fwd.method = request.method;
    fwd.headers = request.headers;
    edit.decoded = request.body;
    if request.url != fwd.url {
        match parse_absolute_url(&request.url) {
            Some(u) => {
                fwd.scheme = u.scheme;
                fwd.host = u.host;
                fwd.port = u.port;
                fwd.path = u.path;
                fwd.url = build_url(&fwd.scheme, &fwd.host, fwd.port, &fwd.path);
                let new_authority = authority_for(&fwd.scheme, &fwd.host, fwd.port);
                // Host follows the URL unless the user edited it explicitly.
                if fwd
                    .headers
                    .get("host")
                    .is_some_and(|h| h.eq_ignore_ascii_case(&old_authority))
                {
                    fwd.headers.set("host", new_authority);
                }
            }
            None => tracing::warn!(url = %request.url, "ignoring invalid URL from breakpoint edit"),
        }
    }
}

/// Build a locally generated response from a decoded body.
fn local_response(
    status: u16,
    status_message: String,
    mut headers: Headers,
    decoded: Bytes,
) -> (ResponseHead, Bytes) {
    let wire = encode_for_headers(&decoded, &mut headers, true);
    fix_content_length(&mut headers, wire.len());
    let status_message = if status_message.is_empty() {
        canonical_reason(status).to_string()
    } else {
        status_message
    };
    (
        ResponseHead {
            status,
            status_message,
            http_version: HttpVersion::Http11,
            headers,
        },
        wire,
    )
}

/// Run blocking rule helpers (file IO, scripts) off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> akev_rules::Result<T> + Send + 'static,
) -> Result<T, String> {
    match tokio::task::spawn_blocking(f).await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(e.to_string()),
        Err(e) => Err(format!("rule task failed: {e}")),
    }
}

/// Like [`blocking`], but runs inline when `io` is false (pure CPU work such as building a
/// mock response from an inline body: not worth a thread hop on the hot path).
async fn maybe_blocking<T: Send + 'static>(
    io: bool,
    f: impl FnOnce() -> akev_rules::Result<T> + Send + 'static,
) -> Result<T, String> {
    if io {
        blocking(f).await
    } else {
        f().map_err(|e| e.to_string())
    }
}

/// Run `onResponse` hooks in order on a response head and its decoded body. A failing script
/// is skipped (logged) and the response continues unchanged.
async fn run_response_scripts(
    scripts: Vec<String>,
    mut head: ResponseHead,
    mut body: Bytes,
    sent: &RequestHead,
) -> (ResponseHead, Bytes) {
    for src in scripts {
        let request = sent.clone();
        let h = head.clone();
        let b = body.clone();
        match blocking(move || akev_rules::script::run_on_response(&src, &request, h, b)).await {
            Ok((h, b, log)) => {
                for line in log.lines {
                    tracing::debug!(target: "akev_proxy::script", "{line}");
                }
                head = h;
                body = b;
            }
            Err(e) => tracing::warn!(error = %e, "response script failed; skipped"),
        }
    }
    (head, body)
}

/// Whether resolving a body source touches the filesystem.
fn reads_file(body: &BodySource) -> bool {
    matches!(body, BodySource::File { .. })
}

/// Run one exchange to completion.
pub(crate) async fn run(
    shared: &Arc<Shared>,
    env: ExchangeEnv,
    req: ExchangeRequest,
    responder: &mut Responder<'_>,
) -> (Arc<ExchangeEvents>, Outcome) {
    let settings = shared.settings();
    let record = !settings.ignore.matches(&req.head.host, Some(req.head.port));
    // Backpressure: while the store is behind, hold new exchanges back for a moment rather
    // than dropping their bodies from the capture straight away.
    if record
        && let Some(queue) = shared.sink.backpressure()
        && queue.is_congested()
    {
        tokio::select! {
            _ = queue.wait_for_space(BACKPRESSURE_WAIT) => {}
            _ = shared.hard_stop.cancelled() => {}
        }
    }
    let ev = ExchangeEvents::new(shared.sink.clone(), record, env.sw, env.request_headers_ms);
    let _active = CounterGuard::new(&shared.stats.active_exchanges);
    let mut options = env.options;
    // For the `mirror-client` TLS profile: the client's browser family (JA4 / User-Agent).
    options.fingerprint.client = crate::upstream::fingerprint::detect_client(
        env.client.tls.as_ref().and_then(|t| t.ja4.as_deref()),
        req.head.headers.get("user-agent"),
    );
    ev.request_started(ExchangeKind::Http, req.head.clone(), env.client);

    let mut p = Pipeline {
        shared,
        capture_limit: settings.capture_limit(),
        ev: ev.clone(),
        source: env.source,
        options,
        decision: Decision::passthrough(),
        request: req.head,
        upgrade: req.upgrade,
        body: ReqBody::Pending(req.body),
        download: None,
        latency_ms: 0,
        terminal_reported: false,
        rule_budget: settings.proxy.rule_body_budget_bytes,
        reservation: Reservation::new(&shared.stats.rule_body_bytes),
        passed_through: false,
    };
    let outcome = match env.local {
        Some(proxy_addr) => p.serve_local(&proxy_addr, responder).await,
        None => p.process(req.forward, env.apply_rules, responder).await,
    };
    (ev, outcome)
}

struct Pipeline<'s> {
    shared: &'s Arc<Shared>,
    capture_limit: usize,
    ev: Arc<ExchangeEvents>,
    source: Option<String>,
    options: RequestOptions,
    decision: Decision,
    /// The request as received.
    request: RequestHead,
    upgrade: Option<Upgrade>,
    body: ReqBody,
    download: Option<TokenBucket>,
    latency_ms: u64,
    /// The terminal rule's `RuleMatched` was emitted (it waits until we know whether the
    /// response is generated locally).
    terminal_reported: bool,
    /// `proxy.ruleBodyBudgetBytes` when the exchange started.
    rule_budget: u64,
    /// This exchange's share of that budget (bodies held for rules, scripts, breakpoints).
    reservation: Reservation<'s>,
    /// A body was too large to hold: rules that need it are skipped and it streams through.
    passed_through: bool,
}

impl Pipeline<'_> {
    /// A body didn't fit the rule-body budget: say so (once) and count it.
    fn pass_through(&mut self) {
        if !std::mem::replace(&mut self.passed_through, true) {
            self.shared
                .stats
                .passed_through
                .fetch_add(1, Ordering::Relaxed);
            self.ev.notice(PASSED_THROUGH_NOTICE);
            tracing::debug!(id = %self.ev.id, "body too large for the rule body budget; passed through");
        }
    }

    /// A rule or breakpoint the user set up sent the request somewhere else (e.g. a `forward`
    /// rule to a dev server on `localhost`): that destination is the user's choice, so a
    /// client on another machine may reach it even if it is this machine's loopback.
    fn allow_user_chosen_target(&mut self, fwd: &RequestHead) {
        if self.options.restrict_loopback
            && (!fwd.host.eq_ignore_ascii_case(&self.request.host) || fwd.port != self.request.port)
        {
            self.options.restrict_loopback = false;
        }
    }

    /// Report the terminal rule (once), now that it's known whether Akev generated the
    /// response itself. A local response produced by a modifier (script short-circuit,
    /// breakpoint "respond") is attributed to the last matched rule.
    fn report_terminal(&mut self, mocked: bool) {
        if std::mem::replace(&mut self.terminal_reported, true) {
            return;
        }
        match &self.decision.terminal_rule {
            Some(rule) => self.ev.rule_matched(rule.clone(), mocked),
            None if mocked => {
                if let Some(last) = self.decision.matched.last() {
                    self.ev.rule_matched(last.clone(), true);
                }
            }
            None => {}
        }
    }

    fn done(&self, responder: &Responder<'_>) -> Outcome {
        Outcome::Done {
            keep_alive: responder.keep_alive(),
        }
    }

    /// Read the whole request body (sending `100 Continue` first when expected). `Ok(None)`
    /// when it doesn't fit the rule-body budget: the body is left to stream through
    /// untouched (what was read so far goes first) and the exchange is marked.
    async fn ensure_buffered(
        &mut self,
        responder: &mut Responder<'_>,
    ) -> Result<Option<Buffered>, ExchangeError> {
        if self.passed_through {
            return Ok(None);
        }
        let pending =
            match std::mem::replace(&mut self.body, ReqBody::Buffered(Buffered::default())) {
                ReqBody::Buffered(b) => {
                    self.body = ReqBody::Buffered(b.clone());
                    return Ok(Some(b));
                }
                ReqBody::Pending(body) => body,
            };
        if !pending.is_empty() {
            responder.send_continue().await;
        }
        let budget = self.rule_budget;
        let buffered = match pending {
            ClientBody::Empty => Buffered::default(),
            ClientBody::Full(data) => {
                if data.len() > MAX_BUFFERED_BODY
                    || !self.reservation.grow(data.len() as u64, budget)
                {
                    self.body = ReqBody::Pending(ClientBody::Full(data));
                    self.pass_through();
                    return Ok(None);
                }
                Buffered {
                    data,
                    trailers: None,
                }
            }
            ClientBody::Stream { stream, length } => {
                let shared = self.shared;
                let res = tokio::select! {
                    r = collect_within(stream, MAX_BUFFERED_BODY, &mut self.reservation, budget) => r,
                    _ = shared.hard_stop.cancelled() => Err(UpstreamError { kind: ErrorKind::ClientAborted, message: "proxy shutting down".into(), code: None }),
                };
                match res {
                    Ok(Collected::Full(b)) => b,
                    Ok(Collected::TooLarge(stream)) => {
                        self.body = ReqBody::Pending(ClientBody::Stream { stream, length });
                        self.pass_through();
                        return Ok(None);
                    }
                    Err(e) => {
                        let mut err = e.to_exchange_error();
                        err.kind = ErrorKind::ClientAborted;
                        return Err(err);
                    }
                }
            }
        };
        self.ev.request_completed(
            capture_full(&buffered.data, self.capture_limit),
            buffered.trailers.clone(),
        );
        self.body = ReqBody::Buffered(buffered.clone());
        Ok(Some(buffered))
    }

    /// Read (and discard beyond the capture limit) the request body so `RequestCompleted` is
    /// recorded before a locally generated response, and the connection stays reusable.
    async fn consume_body(&mut self, responder: &mut Responder<'_>) -> Result<(), ExchangeError> {
        let pending =
            match std::mem::replace(&mut self.body, ReqBody::Buffered(Buffered::default())) {
                ReqBody::Buffered(b) => {
                    self.body = ReqBody::Buffered(b);
                    return Ok(());
                }
                ReqBody::Pending(p) => p,
            };
        match pending {
            ClientBody::Empty => self.ev.request_completed(CapturedBody::empty(), None),
            ClientBody::Full(b) => self
                .ev
                .request_completed(capture_full(&b, self.capture_limit), None),
            ClientBody::Stream { stream, length } => {
                if length != Some(0) {
                    responder.send_continue().await;
                }
                let res = tokio::select! {
                    r = drain_with_capture(stream, self.capture_limit) => r,
                    _ = self.shared.hard_stop.cancelled() => Err(client_aborted()),
                };
                let (body, trailers) = res?;
                self.ev.request_completed(body, trailers);
            }
        }
        Ok(())
    }

    async fn abort(
        &mut self,
        error: ExchangeError,
        kind: AbortKind,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        self.report_terminal(false);
        self.ev.aborted(error);
        responder.abort(kind).await;
        Outcome::Done { keep_alive: false }
    }

    /// Fail the exchange; answer with an error page if nothing was sent yet.
    async fn fail(&mut self, error: ExchangeError, responder: &mut Responder<'_>) -> Outcome {
        self.report_terminal(false);
        self.ev.aborted(error.clone());
        if responder.head_sent() {
            responder.abort(AbortKind::Incomplete).await;
            return Outcome::Done { keep_alive: false };
        }
        let (head, body) = error_response(&error, &self.request);
        let res = async {
            responder
                .send_head(&head, BodyKind::Full(body.len() as u64))
                .await?;
            responder.send_data(body).await?;
            responder.finish(None).await
        }
        .await;
        if res.is_err() {
            return Outcome::Done { keep_alive: false };
        }
        self.done(responder)
    }

    /// Sleep unless the client goes away first. Returns false if it went away.
    async fn sleep_or_gone(&mut self, ms: u64, responder: &mut Responder<'_>) -> bool {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(ms)) => true,
            _ = responder.client_gone() => false,
            _ = self.shared.hard_stop.cancelled() => false,
        }
    }

    async fn serve_local(&mut self, proxy_addr: &str, responder: &mut Responder<'_>) -> Outcome {
        if let Err(e) = self.consume_body(responder).await {
            return self.abort(e, AbortKind::Close, responder).await;
        }
        let (head, body) = crate::local::respond(&self.shared.ca, &self.request, proxy_addr);
        self.respond(head, RespBody::Full(body, None), responder)
            .await
    }

    async fn process(
        &mut self,
        mut fwd: RequestHead,
        apply_rules: bool,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        // ---- rules ------------------------------------------------------------------------
        if apply_rules {
            // One snapshot per exchange: a concurrent `set_rules` must not swap the rule set
            // between deciding whether to buffer the body and evaluating (the body may take
            // a while to arrive), or the evaluation could run rules that needed a body
            // without one.
            let rules = self.shared.rules.snapshot();
            // A body too large for the rule-body budget streams through: rules that match on
            // it see none (so they don't match) and the exchange says why.
            let body = if rules.needs_body(&self.request, self.source.as_deref()) {
                match self.ensure_buffered(responder).await {
                    Ok(Some(b)) => {
                        let decoded = decode_within(
                            &b.data,
                            &self.request.headers,
                            &mut self.reservation,
                            self.rule_budget,
                        );
                        if decoded.is_none() {
                            self.pass_through();
                        }
                        decoded.map(|(d, _)| d)
                    }
                    Ok(None) => None,
                    Err(e) => return self.abort(e, AbortKind::Close, responder).await,
                }
            } else {
                None
            };
            self.decision = rules.evaluate(&MatchContext {
                head: &self.request,
                body: body.as_deref(),
                source: self.source.as_deref(),
            });
            self.options.fingerprint.rule = self.decision.tls_profile;
        }
        // Modifier matches are reported right away; the terminal rule once its outcome is known.
        for rule in &self.decision.matched {
            if self.decision.terminal_rule.as_ref() != Some(rule) {
                self.ev.rule_matched(rule.clone(), false);
            }
        }
        if let Some((color, note)) = self.decision.highlight.clone() {
            self.ev.highlighted(color, note);
        }
        if let Some(t) = &self.decision.throttle {
            self.download = TokenBucket::new(t.download_bps);
            self.latency_ms = t.latency_ms;
        }

        // ---- delay ------------------------------------------------------------------------
        if self.decision.delay_ms > 0
            && !self.sleep_or_gone(self.decision.delay_ms, responder).await
        {
            return self
                .abort(client_aborted(), AbortKind::Close, responder)
                .await;
        }

        // ---- request modifications --------------------------------------------------------
        let original_fwd = fwd.clone();
        let (req_body_transforms, _) = akev_rules::transforms_need_body(
            &self.decision.request_transforms,
            &self.decision.response_transforms,
        );
        // Only scripts with an `onRequest` hook see (and may change) the request body; an
        // `onResponse`-only script must not hold back a streaming upload.
        let mut request_scripts: Vec<String> = self
            .decision
            .scripts
            .iter()
            .filter(|s| has_request_hook(s))
            .cloned()
            .collect();
        let needs_body =
            req_body_transforms || !request_scripts.is_empty() || self.decision.break_on_request;
        let mut edit = if needs_body {
            match self.ensure_buffered(responder).await {
                Ok(Some(b)) => {
                    let edit = EditBody::new(
                        &b.data,
                        &fwd.headers,
                        &mut self.reservation,
                        self.rule_budget,
                    );
                    if edit.is_none() {
                        self.pass_through();
                    }
                    edit
                }
                Ok(None) => None,
                Err(e) => return self.abort(e, AbortKind::Close, responder).await,
            }
        } else {
            None
        };
        if self.passed_through {
            // The body streams through untouched: no script or breakpoint sees a body that
            // isn't there (header-only transforms below still apply).
            request_scripts.clear();
            self.decision.break_on_request = false;
        }

        let transforms = std::mem::take(&mut self.decision.request_transforms);
        for t in &transforms {
            if self.passed_through && t.body.is_some() {
                continue; // needs the body that streams through untouched
            }
            let mut body = edit.as_ref().map(|e| e.decoded.clone()).unwrap_or_default();
            match akev_rules::apply_request_transform(&mut fwd, &mut body, t) {
                Ok(()) => {
                    if let Some(e) = edit.as_mut() {
                        e.decoded = body;
                    }
                }
                Err(e) => tracing::warn!(error = %e, "request transform failed; skipped"),
            }
        }

        if let Action::Forward { .. } = &self.decision.action {
            match akev_rules::forward_request(&self.decision.action, &fwd) {
                Ok(h) => fwd = h,
                Err(e) => {
                    let err = exchange_error(
                        ErrorKind::Internal,
                        format!("invalid forward rule: {e}"),
                        None,
                    );
                    return self.fail(err, responder).await;
                }
            }
        }

        let mut local: Option<(ResponseHead, Bytes)> = None;
        for src in request_scripts {
            let body = edit.as_ref().map(|e| e.decoded.clone()).unwrap_or_default();
            let head = fwd.clone();
            match blocking(move || akev_rules::script::run_on_request(&src, head, body)).await {
                Ok((outcome, log)) => {
                    for line in log.lines {
                        tracing::debug!(target: "akev_proxy::script", "{line}");
                    }
                    match outcome {
                        akev_rules::script::ScriptRequestOutcome::Continue { head, body } => {
                            fwd = head;
                            if let Some(e) = edit.as_mut() {
                                e.decoded = body;
                            }
                        }
                        akev_rules::script::ScriptRequestOutcome::Respond { head, body } => {
                            local = Some(local_response(
                                head.status,
                                head.status_message,
                                head.headers,
                                body,
                            ));
                            break;
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "request script failed; skipped"),
            }
        }

        if self.decision.break_on_request && local.is_none() {
            let editable = EditableRequest {
                method: fwd.method.clone(),
                url: fwd.url.clone(),
                headers: fwd.headers.clone(),
                body: edit.as_ref().map(|e| e.decoded.clone()).unwrap_or_default(),
            };
            self.ev.paused(BreakpointStage::Request);
            let started = std::time::Instant::now();
            let breakpoints = self.shared.breakpoints.clone();
            let resume = tokio::select! {
                r = breakpoints.on_request(self.ev.id, &fwd, editable) => Some(r),
                _ = responder.client_gone() => None,
                _ = self.shared.hard_stop.cancelled() => None,
            };
            self.ev.resumed(started.elapsed().as_secs_f64() * 1000.0);
            match resume {
                None => {
                    return self
                        .abort(client_aborted(), AbortKind::Close, responder)
                        .await;
                }
                Some(RequestResume::Continue { request }) => {
                    if let Some(e) = edit.as_mut() {
                        apply_editable_request(&mut fwd, e, request);
                    }
                }
                Some(RequestResume::Respond { response }) => {
                    local = Some(local_response(
                        response.status,
                        response.status_message,
                        response.headers,
                        response.body,
                    ));
                }
                Some(RequestResume::Close) => {
                    let err = exchange_error(
                        ErrorKind::BreakpointAborted,
                        "Connection closed at a request breakpoint",
                        None,
                    );
                    return self.abort(err, AbortKind::Close, responder).await;
                }
                Some(RequestResume::Reset) => {
                    let err = exchange_error(
                        ErrorKind::BreakpointAborted,
                        "Connection reset at a request breakpoint",
                        None,
                    );
                    return self.abort(err, AbortKind::Reset, responder).await;
                }
            }
        }

        // Finalise the forwarded body.
        let mut body_changed = false;
        if let Some(e) = &edit
            && e.changed()
        {
            let wire = e.encode(&mut fwd.headers);
            fix_content_length(&mut fwd.headers, wire.len());
            let trailers = match &self.body {
                ReqBody::Buffered(b) => b.trailers.clone(),
                ReqBody::Pending(_) => None,
            };
            self.body = ReqBody::Buffered(Buffered {
                data: wire,
                trailers,
            });
            body_changed = true;
        }
        if body_changed || fwd != original_fwd {
            let body = match (&self.body, body_changed) {
                (ReqBody::Buffered(b), true) => Some(capture_full(&b.data, self.capture_limit)),
                _ => None,
            };
            self.ev.request_modified(fwd.clone(), body);
        }

        if let Some((head, body)) = local {
            return self.respond_local(head, body, responder).await;
        }

        // ---- terminal action --------------------------------------------------------------
        let action = self.decision.action.clone();
        match action {
            Action::Mock { ref body, .. } => {
                let io = reads_file(body);
                let request = fwd.clone();
                let a = action.clone();
                match maybe_blocking(io, move || akev_rules::mock_response(&a, &request)).await {
                    Ok((head, body)) => self.respond_local(head, body, responder).await,
                    Err(e) => {
                        self.fail(
                            exchange_error(
                                ErrorKind::Internal,
                                format!("mock rule failed: {e}"),
                                None,
                            ),
                            responder,
                        )
                        .await
                    }
                }
            }
            Action::MapLocal { .. } => {
                let request = fwd.clone();
                let a = action.clone();
                match blocking(move || akev_rules::map_local(&a, &request)).await {
                    Ok(MapLocalOutcome::Found(head, body))
                    | Ok(MapLocalOutcome::NotFound(head, body)) => {
                        self.respond_local(head, body, responder).await
                    }
                    Ok(MapLocalOutcome::FoundFile { head, file, len }) => {
                        let body = match file {
                            Some(file) => RespBody::Stream(file_stream(file, len)),
                            None => RespBody::Full(Bytes::new(), None),
                        };
                        self.report_terminal(true);
                        if let Err(e) = self.consume_body(responder).await {
                            return self.abort(e, AbortKind::Close, responder).await;
                        }
                        self.respond(head, body, responder).await
                    }
                    Ok(MapLocalOutcome::FallbackToUpstream) => self.forward(fwd, responder).await,
                    Err(e) => {
                        self.fail(
                            exchange_error(
                                ErrorKind::Internal,
                                format!("map-local rule failed: {e}"),
                                None,
                            ),
                            responder,
                        )
                        .await
                    }
                }
            }
            Action::Close => {
                let err = exchange_error(
                    ErrorKind::RuleClosed,
                    "Connection closed intentionally by a rule",
                    None,
                );
                self.abort(err, AbortKind::Close, responder).await
            }
            Action::Reset => {
                let err = exchange_error(
                    ErrorKind::RuleReset,
                    "Connection reset intentionally by a rule",
                    None,
                );
                self.abort(err, AbortKind::Reset, responder).await
            }
            Action::Timeout => self.hold_until_gone(responder).await,
            Action::WsEcho => {
                if self.upgrade == Some(Upgrade::WebSocket) && matches!(responder, Responder::H1(_))
                {
                    self.accept_echo(&fwd, responder).await
                } else {
                    let (head, body) = local_response(
                        400,
                        String::new(),
                        Headers(vec![("content-type".into(), "text/plain".into())]),
                        Bytes::from_static(
                            b"This endpoint only accepts WebSocket connections (ws-echo rule).\n",
                        ),
                    );
                    self.respond_local(head, body, responder).await
                }
            }
            Action::WsReject { status, body } => {
                let decoded = match maybe_blocking(reads_file(&body), move || {
                    akev_rules::resolve_body(&body)
                })
                .await
                {
                    Ok(b) => b,
                    Err(e) => {
                        tracing::warn!(error = %e, "ws-reject body could not be resolved");
                        Bytes::new()
                    }
                };
                let (head, body) = local_response(status, String::new(), Headers::new(), decoded);
                self.respond_local(head, body, responder).await
            }
            _ => self.forward(fwd, responder).await,
        }
    }

    async fn respond_local(
        &mut self,
        head: ResponseHead,
        body: Bytes,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        self.report_terminal(true);
        if let Err(e) = self.consume_body(responder).await {
            return self.abort(e, AbortKind::Close, responder).await;
        }
        self.respond(head, RespBody::Full(body, None), responder)
            .await
    }

    /// `timeout` rule: accept the request and never answer.
    async fn hold_until_gone(&mut self, responder: &mut Responder<'_>) -> Outcome {
        self.report_terminal(false);
        let started = std::time::Instant::now();
        // Keep reading the request body so a client closing the connection is noticed.
        let pending =
            match std::mem::replace(&mut self.body, ReqBody::Buffered(Buffered::default())) {
                ReqBody::Pending(p) => Some(p),
                ReqBody::Buffered(b) => {
                    self.body = ReqBody::Buffered(b);
                    None
                }
            };
        let ev = self.ev.clone();
        let limit = self.capture_limit;
        let drain = async move {
            if let Some(p) = pending {
                match p {
                    ClientBody::Empty => ev.request_completed(CapturedBody::empty(), None),
                    ClientBody::Full(b) => ev.request_completed(capture_full(&b, limit), None),
                    ClientBody::Stream { stream, .. } => {
                        if let Ok((body, trailers)) = drain_with_capture(stream, limit).await {
                            ev.request_completed(body, trailers);
                        }
                    }
                }
            }
            std::future::pending::<()>().await
        };
        tokio::select! {
            _ = drain => {}
            _ = responder.client_gone() => {}
            _ = self.shared.hard_stop.cancelled() => {}
        }
        let err = exchange_error(
            ErrorKind::RuleTimeout,
            format!(
                "A timeout rule held the request until the client gave up ({:.1}s)",
                started.elapsed().as_secs_f64()
            ),
            None,
        );
        self.ev.aborted(err);
        responder.abort(AbortKind::Close).await;
        Outcome::Done { keep_alive: false }
    }

    async fn accept_echo(&mut self, fwd: &RequestHead, responder: &mut Responder<'_>) -> Outcome {
        self.report_terminal(true);
        let Some(key) = fwd.headers.get("sec-websocket-key").map(str::to_string) else {
            let (head, body) = local_response(
                400,
                String::new(),
                Headers::new(),
                Bytes::from_static(b"Missing Sec-WebSocket-Key\n"),
            );
            return self.respond_local(head, body, responder).await;
        };
        if let Err(e) = self.consume_body(responder).await {
            return self.abort(e, AbortKind::Close, responder).await;
        }
        let mut headers = Headers::with_capacity(3);
        headers.append("Upgrade", "websocket");
        headers.append("Connection", "Upgrade");
        headers.append("Sec-WebSocket-Accept", crate::websocket::accept_key(&key));
        let head = ResponseHead {
            status: 101,
            status_message: "Switching Protocols".into(),
            http_version: HttpVersion::Http11,
            headers,
        };
        self.ev.response_started(head.clone());
        if responder.send_head(&head, BodyKind::None).await.is_err() {
            return self
                .abort(client_aborted(), AbortKind::Close, responder)
                .await;
        }
        self.ev.stream_opened(ExchangeKind::WebSocket);
        Outcome::Upgrade(UpgradeJob {
            events: self.ev.clone(),
            mode: UpgradeMode::Echo,
            capture_limit: self.capture_limit,
            ws_breakpoints: Vec::new(),
        })
    }

    /// Send the request upstream and relay the response.
    async fn forward(&mut self, fwd: RequestHead, responder: &mut Responder<'_>) -> Outcome {
        self.report_terminal(false);
        if let Some(kind) = self.upgrade
            && matches!(responder, Responder::H1(_))
        {
            return self.forward_upgrade(fwd, kind, responder).await;
        }
        let upload = self
            .decision
            .throttle
            .as_ref()
            .and_then(|t| TokenBucket::new(t.upload_bps));
        let client_failed = Arc::new(AtomicBool::new(false));
        // The client body streams upstream through a capture tee (recorded as it is read).
        let mut tee_body = false;
        let body = match std::mem::replace(&mut self.body, ReqBody::Buffered(Buffered::default())) {
            ReqBody::Buffered(b) => {
                let len = b.data.len() as u64;
                match upload {
                    Some(bucket) => RequestBody::Stream {
                        stream: throttled_replay(b.data, b.trailers, bucket),
                        length: Some(len),
                    },
                    None if b.trailers.is_some() => {
                        let mut chunks = Vec::with_capacity(2);
                        if !b.data.is_empty() {
                            chunks.push(Ok(BodyChunk::Data(b.data)));
                        }
                        if let Some(t) = b.trailers {
                            chunks.push(Ok(BodyChunk::Trailers(t)));
                        }
                        RequestBody::Stream {
                            stream: Box::pin(futures::stream::iter(chunks)),
                            length: Some(len),
                        }
                    }
                    None if b.data.is_empty() => RequestBody::Empty,
                    None => RequestBody::Full(b.data),
                }
            }
            ReqBody::Pending(ClientBody::Empty) => {
                self.ev.request_completed(CapturedBody::empty(), None);
                RequestBody::Empty
            }
            ReqBody::Pending(ClientBody::Full(b)) => {
                self.ev
                    .request_completed(capture_full(&b, self.capture_limit), None);
                match upload {
                    Some(bucket) => {
                        let len = b.len() as u64;
                        RequestBody::Stream {
                            stream: throttled_replay(b, None, bucket),
                            length: Some(len),
                        }
                    }
                    None if b.is_empty() => RequestBody::Empty,
                    None => RequestBody::Full(b),
                }
            }
            ReqBody::Pending(ClientBody::Stream { stream, length }) => {
                if length != Some(0) {
                    responder.send_continue().await;
                }
                let tee = TeeStream::new(
                    stream,
                    self.ev.clone(),
                    self.capture_limit,
                    upload,
                    client_failed.clone(),
                )
                .with_declared_length(length);
                tee_body = true;
                RequestBody::Stream {
                    stream: Box::pin(tee),
                    length,
                }
            }
        };

        let send_started = self.ev.now_ms();
        self.allow_user_chosen_target(&fwd);
        let upstream = &self.shared.upstream;
        let res = tokio::select! {
            r = upstream.send_returning_body(&fwd, body, &self.options) => Some(r),
            _ = responder.client_gone() => None,
            _ = self.shared.hard_stop.cancelled() => None,
        };
        let resp = match res {
            None => {
                return self
                    .abort(client_aborted(), AbortKind::Close, responder)
                    .await;
            }
            Some(Err(_)) if client_failed.load(Ordering::Relaxed) => {
                return self
                    .abort(client_aborted(), AbortKind::Close, responder)
                    .await;
            }
            Some(Err(failure)) => {
                // Failed before the body was sent (connect / DNS / TLS / proxy error): read it
                // anyway so the exchange records what the client sent, not an empty capture.
                if let (true, Some(RequestBody::Stream { stream, .. })) =
                    (tee_body, failure.unsent_body)
                {
                    self.drain_unsent_body(stream, responder).await;
                    if client_failed.load(Ordering::Relaxed) {
                        return self
                            .abort(client_aborted(), AbortKind::Close, responder)
                            .await;
                    }
                }
                return self
                    .fail(failure.error.to_exchange_error(), responder)
                    .await;
            }
            Some(Ok(resp)) => resp,
        };
        self.ev
            .upstream(resp.info, &resp.timings, resp.request_sent_ms, send_started);
        self.respond_with_request(resp.head, RespBody::Stream(resp.body), &fwd, responder)
            .await
    }

    /// Read a streamed request body that never went upstream, through its capture tee, so
    /// `RequestCompleted` records the body the client sent (and an HTTP/1 connection stays
    /// reusable). Bounded in size and time; whatever wasn't read is recorded as truncated
    /// when the tee is dropped.
    async fn drain_unsent_body(&mut self, mut stream: BodyStream, responder: &mut Responder<'_>) {
        let limit = (self.capture_limit as u64).max(UNSENT_BODY_DRAIN_BYTES);
        let drain = async {
            let mut read = 0u64;
            while let Some(item) = stream.next().await {
                match item {
                    Ok(BodyChunk::Data(d)) => {
                        read += d.len() as u64;
                        if read > limit {
                            break;
                        }
                    }
                    Ok(BodyChunk::Trailers(_)) => {}
                    Err(_) => break,
                }
            }
        };
        tokio::select! {
            _ = tokio::time::timeout(UNSENT_BODY_DRAIN_TIMEOUT, drain) => {}
            _ = responder.client_gone() => {}
            _ = self.shared.hard_stop.cancelled() => {}
        }
    }

    async fn forward_upgrade(
        &mut self,
        mut fwd: RequestHead,
        kind: Upgrade,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        if kind == Upgrade::WebSocket {
            // No permessage-deflate: frames stay readable for capture.
            fwd.headers.remove("sec-websocket-extensions");
        }
        if let Err(e) = self.consume_body(responder).await {
            return self.abort(e, AbortKind::Close, responder).await;
        }
        let send_started = self.ev.now_ms();
        self.allow_user_chosen_target(&fwd);
        let upstream = &self.shared.upstream;
        let res = tokio::select! {
            r = upstream.send_upgrade(&fwd, &self.options) => Some(r),
            _ = responder.client_gone() => None,
            _ = self.shared.hard_stop.cancelled() => None,
        };
        match res {
            None => {
                self.abort(client_aborted(), AbortKind::Close, responder)
                    .await
            }
            Some(Err(e)) => self.fail(e.to_exchange_error(), responder).await,
            Some(Ok((head, Some(upgraded), _, info))) if head.status == 101 => {
                self.ev
                    .upstream(info, &ConnectTimings::default(), None, send_started);
                self.ev.response_started(head.clone());
                if responder.send_head(&head, BodyKind::None).await.is_err() {
                    return self
                        .abort(client_aborted(), AbortKind::Close, responder)
                        .await;
                }
                let mode = match kind {
                    Upgrade::WebSocket => {
                        self.ev.stream_opened(ExchangeKind::WebSocket);
                        UpgradeMode::WebSocket(upgraded)
                    }
                    Upgrade::Other => UpgradeMode::Raw(upgraded),
                };
                Outcome::Upgrade(UpgradeJob {
                    events: self.ev.clone(),
                    mode,
                    capture_limit: self.capture_limit,
                    ws_breakpoints: self.decision.ws_breakpoints.clone(),
                })
            }
            Some(Ok((_, _, Some(resp), info))) => {
                self.ev
                    .upstream(info, &resp.timings, resp.request_sent_ms, send_started);
                self.respond_with_request(resp.head, RespBody::Stream(resp.body), &fwd, responder)
                    .await
            }
            Some(Ok((head, _, None, info))) => {
                self.ev
                    .upstream(info, &ConnectTimings::default(), None, send_started);
                self.respond_with_request(head, RespBody::Full(Bytes::new(), None), &fwd, responder)
                    .await
            }
        }
    }

    async fn respond(
        &mut self,
        head: ResponseHead,
        body: RespBody,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        let request = self.request.clone();
        self.respond_with_request(head, body, &request, responder)
            .await
    }

    /// Read the response body in full for transforms, scripts or a breakpoint. `Ok(Err(..))`
    /// when it doesn't fit the rule-body budget: the body to stream through untouched.
    async fn buffer_response(
        &mut self,
        body: RespBody,
        responder: &mut Responder<'_>,
    ) -> Result<Result<Buffered, RespBody>, Outcome> {
        let budget = self.rule_budget;
        match body {
            RespBody::Full(data, trailers) => {
                if data.len() > MAX_BUFFERED_BODY
                    || !self.reservation.grow(data.len() as u64, budget)
                {
                    return Ok(Err(RespBody::Full(data, trailers)));
                }
                Ok(Ok(Buffered { data, trailers }))
            }
            RespBody::Stream(stream) => {
                let shared = self.shared;
                let res = tokio::select! {
                    r = collect_within(stream, MAX_BUFFERED_BODY, &mut self.reservation, budget) => Some(r),
                    _ = responder.client_gone() => None,
                    _ = shared.hard_stop.cancelled() => None,
                };
                match res {
                    None => Err(self
                        .abort(client_aborted(), AbortKind::Close, responder)
                        .await),
                    Some(Err(e)) => Err(self.fail(e.to_exchange_error(), responder).await),
                    Some(Ok(Collected::Full(b))) => Ok(Ok(b)),
                    Some(Ok(Collected::TooLarge(stream))) => Ok(Err(RespBody::Stream(stream))),
                }
            }
        }
    }

    /// Response side. `sent` is the request as it was sent upstream (given to response hooks).
    async fn respond_with_request(
        &mut self,
        mut head: ResponseHead,
        mut body: RespBody,
        sent: &RequestHead,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        let head_request = self.request.method.eq_ignore_ascii_case("HEAD");
        let sse = is_event_stream(&head);
        let (_, res_body_transforms) = akev_rules::transforms_need_body(
            &self.decision.request_transforms,
            &self.decision.response_transforms,
        );
        let mut response_scripts = self.response_scripts();
        let modifiers =
            res_body_transforms || !response_scripts.is_empty() || self.decision.break_on_response;
        let needs_buffer = if sse {
            // A live event stream can't be buffered: only its head is modified (below).
            false
        } else if head_request || no_body_status(head.status) {
            // The body is known to be empty, so "buffering" it costs nothing. Every modifier
            // then sees the complete message, and a status change that gives the response a
            // body (204 → 200 + body) gets a body and correct framing.
            modifiers || !self.decision.response_transforms.is_empty()
        } else {
            modifiers
        };
        if needs_buffer {
            match self.buffer_response(body, responder).await {
                Ok(Ok(buffered)) => {
                    return self.respond_buffered(head, buffered, sent, responder).await;
                }
                Ok(Err(untouched)) => {
                    // Too large to hold: stream it as received, skipping everything that
                    // needs the body (header-only transforms still apply).
                    self.pass_through();
                    body = untouched;
                    response_scripts.clear();
                    self.decision.break_on_response = false;
                    self.decision
                        .response_transforms
                        .retain(|t| t.body.is_none());
                }
                Err(outcome) => return outcome,
            }
        }

        // Streaming path: header-level modifications only.
        let original_head = head.clone();
        for t in std::mem::take(&mut self.decision.response_transforms) {
            let mut placeholder = Bytes::new();
            let mut header_only = t.clone();
            header_only.body = None;
            if let Err(e) =
                akev_rules::apply_response_transform(&mut head, &mut placeholder, &header_only)
            {
                tracing::warn!(error = %e, "response transform failed; skipped");
            }
        }
        if !response_scripts.is_empty() {
            // Only event streams get here with scripts: they can change the status and headers,
            // but the body hasn't arrived yet (and never ends), so they see an empty one.
            let (h, b) = run_response_scripts(response_scripts, head, Bytes::new(), sent).await;
            head = h;
            if !b.is_empty() {
                tracing::warn!(
                    "an onResponse script set a body on a streamed (text/event-stream) response; \
                     only its status and header changes are applied"
                );
            }
        }
        if self.decision.break_on_response {
            match self
                .response_breakpoint(&head, Bytes::new(), sent, responder)
                .await
            {
                Ok(Some((h, _))) => head = h,
                Ok(None) => return Outcome::Done { keep_alive: false },
                Err(outcome) => return outcome,
            }
        }
        // Framing follows the head actually sent (a header-only transform may have changed the
        // status).
        let no_body = head_request || no_body_status(head.status);
        self.ev.response_started(head.clone());
        if head != original_head {
            self.ev.response_modified(original_head, None);
        }
        if sse {
            self.ev.stream_opened(ExchangeKind::EventStream);
        }
        if self.latency_ms > 0 && !self.sleep_or_gone(self.latency_ms, responder).await {
            return self
                .abort(client_aborted(), AbortKind::Close, responder)
                .await;
        }

        let (mut stream, kind): (BodyStream, BodyKind) = match body {
            RespBody::Full(b, t) => {
                let len = b.len() as u64;
                let mut chunks = Vec::with_capacity(2);
                if !b.is_empty() {
                    chunks.push(Ok(BodyChunk::Data(b)));
                }
                if let Some(t) = t {
                    chunks.push(Ok(BodyChunk::Trailers(t)));
                }
                (
                    Box::pin(futures::stream::iter(chunks)),
                    if no_body {
                        BodyKind::None
                    } else {
                        BodyKind::Full(len)
                    },
                )
            }
            RespBody::Stream(s) => (
                s,
                if no_body {
                    BodyKind::None
                } else {
                    BodyKind::Stream
                },
            ),
        };
        if responder.send_head(&head, kind).await.is_err() {
            return self
                .abort(client_aborted(), AbortKind::Close, responder)
                .await;
        }

        let mut capture = Capture::new(self.capture_limit);
        let mut trailers: Option<Headers> = None;
        let parse_events = sse && head.headers.content_encodings().is_empty();
        let mut parser = parse_events.then(akev_content::sse::SseParser::new);
        let hard_stop = self.shared.hard_stop.clone();
        enum Step {
            Next(Option<Result<BodyChunk, UpstreamError>>),
            Gone,
            Stop,
        }
        loop {
            let step = tokio::select! {
                n = stream.next() => Step::Next(n),
                _ = responder.client_gone() => Step::Gone,
                _ = hard_stop.cancelled() => Step::Stop,
            };
            let next = match step {
                Step::Next(n) => n,
                Step::Gone => {
                    return self
                        .client_left_mid_response(sse, &mut capture, trailers, responder)
                        .await;
                }
                Step::Stop => {
                    return self
                        .abort_mid_response(
                            client_aborted(),
                            &mut capture,
                            trailers,
                            AbortKind::Incomplete,
                            responder,
                        )
                        .await;
                }
            };
            match next {
                None => break,
                Some(Ok(BodyChunk::Trailers(t))) => trailers = Some(t),
                Some(Ok(BodyChunk::Data(data))) => {
                    if no_body {
                        continue;
                    }
                    capture.push(&data);
                    if let Some(p) = parser.as_mut() {
                        for e in p.push(&data) {
                            self.emit_sse(e);
                        }
                    }
                    if self.send_throttled(data, responder).await.is_err() {
                        return self
                            .client_left_mid_response(sse, &mut capture, trailers, responder)
                            .await;
                    }
                }
                Some(Err(e)) => {
                    if sse {
                        self.ev
                            .stream_closed(None, Some(e.message.clone()), Direction::Received);
                    }
                    // Keep what arrived before the failure: it's what the client got, and what
                    // the user needs to see to debug a truncated response.
                    self.report_terminal(false);
                    self.ev.aborted_mid_response(
                        e.to_exchange_error(),
                        capture.finish(),
                        trailers.take(),
                    );
                    responder.abort(AbortKind::Incomplete).await;
                    return Outcome::Done { keep_alive: false };
                }
            }
        }
        if responder.finish(trailers.as_ref()).await.is_err() {
            return self
                .client_left_mid_response(sse, &mut capture, trailers, responder)
                .await;
        }
        if let Some(mut p) = parser.take()
            && let Some(e) = p.finish()
        {
            self.emit_sse(e);
        }
        if sse {
            self.ev.stream_closed(None, None, Direction::Received);
        }
        self.ev.response_completed(capture.finish(), trailers);
        self.done(responder)
    }

    fn emit_sse(&self, e: akev_content::sse::SseEvent) {
        let payload = Bytes::from(e.data.into_bytes());
        let size = payload.len() as u64;
        self.ev.stream_message(
            Direction::Received,
            MessageKind::Event,
            payload,
            size,
            e.event,
            e.id,
        );
    }

    /// The client disconnected while the response was streaming. For event streams that's the
    /// normal way to stop listening; for everything else it's an abort.
    async fn client_left_mid_response(
        &mut self,
        sse: bool,
        capture: &mut Capture,
        trailers: Option<Headers>,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        if sse {
            self.ev.stream_closed(None, None, Direction::Sent);
            self.ev.response_completed(capture.finish(), None);
            responder.abort(AbortKind::Incomplete).await;
            return Outcome::Done { keep_alive: false };
        }
        self.abort_mid_response(
            client_aborted(),
            capture,
            trailers,
            AbortKind::Incomplete,
            responder,
        )
        .await
    }

    /// Abort an exchange whose response body was being relayed, recording the part of the
    /// body that was received so far.
    async fn abort_mid_response(
        &mut self,
        error: ExchangeError,
        capture: &mut Capture,
        trailers: Option<Headers>,
        kind: AbortKind,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        self.report_terminal(false);
        self.ev
            .aborted_mid_response(error, capture.finish(), trailers);
        responder.abort(kind).await;
        Outcome::Done { keep_alive: false }
    }

    /// The scripts with an `onResponse` hook (the others never change a response).
    fn response_scripts(&self) -> Vec<String> {
        self.decision
            .scripts
            .iter()
            .filter(|s| has_response_hook(s))
            .cloned()
            .collect()
    }

    async fn send_throttled(
        &mut self,
        mut data: Bytes,
        responder: &mut Responder<'_>,
    ) -> std::io::Result<()> {
        let Some(bucket) = self.download.as_mut() else {
            return responder.send_data(data).await;
        };
        let slice = bucket.slice_size();
        while !data.is_empty() {
            let piece = data.split_to(slice.min(data.len()));
            bucket.wait(piece.len()).await;
            responder.send_data(piece).await?;
        }
        Ok(())
    }

    /// Pause at a response breakpoint. `Ok(Some(..))` = continue with the (edited) head and
    /// decoded body; `Ok(None)` = the client went away; `Err(outcome)` = closed/reset by user.
    async fn response_breakpoint(
        &mut self,
        head: &ResponseHead,
        decoded: Bytes,
        sent: &RequestHead,
        responder: &mut Responder<'_>,
    ) -> Result<Option<(ResponseHead, Bytes)>, Outcome> {
        let editable = EditableResponse {
            status: head.status,
            status_message: head.status_message.clone(),
            headers: head.headers.clone(),
            body: decoded,
        };
        self.ev.paused(BreakpointStage::Response);
        let started = std::time::Instant::now();
        let breakpoints = self.shared.breakpoints.clone();
        let resume = tokio::select! {
            r = breakpoints.on_response(self.ev.id, sent, head, editable) => Some(r),
            _ = responder.client_gone() => None,
            _ = self.shared.hard_stop.cancelled() => None,
        };
        self.ev.resumed(started.elapsed().as_secs_f64() * 1000.0);
        match resume {
            None => {
                self.ev.aborted(client_aborted());
                responder.abort(AbortKind::Close).await;
                Ok(None)
            }
            Some(ResponseResume::Continue { response }) => {
                let mut h = head.clone();
                h.status = response.status;
                h.status_message = response.status_message;
                h.headers = response.headers;
                Ok(Some((h, response.body)))
            }
            Some(ResponseResume::Close) => {
                let err = exchange_error(
                    ErrorKind::BreakpointAborted,
                    "Connection closed at a response breakpoint",
                    None,
                );
                Err(self.abort(err, AbortKind::Close, responder).await)
            }
            Some(ResponseResume::Reset) => {
                let err = exchange_error(
                    ErrorKind::BreakpointAborted,
                    "Connection reset at a response breakpoint",
                    None,
                );
                Err(self.abort(err, AbortKind::Reset, responder).await)
            }
        }
    }

    /// The buffered response, after transforms, scripts or a breakpoint modified it.
    async fn respond_buffered(
        &mut self,
        mut head: ResponseHead,
        collected: Buffered,
        sent: &RequestHead,
        responder: &mut Responder<'_>,
    ) -> Outcome {
        let original_head = head.clone();
        let original_wire = collected.data.clone();
        let Some(mut edit) = EditBody::new(
            &collected.data,
            &head.headers,
            &mut self.reservation,
            self.rule_budget,
        ) else {
            // The decoded copy doesn't fit the budget: deliver the response untouched.
            self.pass_through();
            self.decision
                .response_transforms
                .retain(|t| t.body.is_none());
            self.decision.scripts.clear();
            self.decision.break_on_response = false;
            let body = RespBody::Full(collected.data, collected.trailers);
            // Boxed: this re-enters the response path (which no longer buffers).
            return Box::pin(self.respond_with_request(head, body, sent, responder)).await;
        };

        for t in std::mem::take(&mut self.decision.response_transforms) {
            let mut body = edit.decoded.clone();
            match akev_rules::apply_response_transform(&mut head, &mut body, &t) {
                Ok(()) => edit.decoded = body,
                Err(e) => tracing::warn!(error = %e, "response transform failed; skipped"),
            }
        }
        let response_scripts = self.response_scripts();
        if !response_scripts.is_empty() {
            let (h, b) =
                run_response_scripts(response_scripts, head, edit.decoded.clone(), sent).await;
            head = h;
            edit.decoded = b;
        }
        if self.decision.break_on_response {
            match self
                .response_breakpoint(&head, edit.decoded.clone(), sent, responder)
                .await
            {
                Ok(Some((h, b))) => {
                    head = h;
                    edit.decoded = b;
                }
                Ok(None) => return Outcome::Done { keep_alive: false },
                Err(outcome) => return outcome,
            }
        }

        // HEAD responses and 1xx/204/304 never carry a body, whatever the modifiers produced;
        // their Content-Length (if any) describes the representation and is left alone.
        let bodiless =
            self.request.method.eq_ignore_ascii_case("HEAD") || no_body_status(head.status);
        let body_changed = !bodiless && edit.changed();
        let wire = if bodiless {
            if edit.changed() {
                tracing::debug!(
                    status = head.status,
                    "ignoring a body set on a response that can't have one"
                );
            }
            Bytes::new()
        } else if body_changed {
            let w = edit.encode(&mut head.headers);
            fix_content_length(&mut head.headers, w.len());
            w
        } else {
            original_wire.clone()
        };
        let modified = body_changed || head != original_head;
        self.ev.response_started(head.clone());
        if modified {
            self.ev.response_modified(
                original_head,
                Some(capture_full(&original_wire, self.capture_limit)),
            );
        }
        if self.latency_ms > 0 && !self.sleep_or_gone(self.latency_ms, responder).await {
            return self
                .abort(client_aborted(), AbortKind::Close, responder)
                .await;
        }
        let res = async {
            responder
                .send_head(&head, BodyKind::Full(wire.len() as u64))
                .await?;
            if !wire.is_empty() {
                self.send_throttled(wire.clone(), responder).await?;
            }
            responder.finish(collected.trailers.as_ref()).await
        }
        .await;
        if res.is_err() {
            return self
                .abort(client_aborted(), AbortKind::Incomplete, responder)
                .await;
        }
        self.ev
            .response_completed(capture_full(&wire, self.capture_limit), collected.trailers);
        self.done(responder)
    }
}

/// A local file (a large map-local match) as a body stream of `len` bytes, read in 64 KiB
/// chunks: memory stays flat however large the file is. A file that changes size while it's
/// served ends the stream with an error rather than breaking the declared length.
fn file_stream(file: std::fs::File, len: u64) -> BodyStream {
    use tokio::io::AsyncReadExt;
    let failed = |message: String| UpstreamError {
        kind: ErrorKind::Internal,
        message,
        code: None,
    };
    let file = tokio::fs::File::from_std(file).take(len);
    let reader = tokio_util::io::ReaderStream::with_capacity(file, 64 * 1024);
    let stream = futures::stream::unfold(
        (reader, 0u64, false),
        move |(mut reader, sent, done)| async move {
            if done {
                return None;
            }
            match reader.next().await {
                Some(Ok(data)) => {
                    let sent = sent + data.len() as u64;
                    Some((Ok(BodyChunk::Data(data)), (reader, sent, false)))
                }
                Some(Err(e)) => Some((
                    Err(failed(format!("map-local file read failed: {e}"))),
                    (reader, sent, true),
                )),
                None if sent < len => Some((
                    Err(failed(
                        "map-local file got shorter while it was served".into(),
                    )),
                    (reader, sent, true),
                )),
                None => None,
            }
        },
    );
    Box::pin(stream)
}

/// Read a body stream to the end, keeping a bounded capture.
async fn drain_with_capture(
    mut stream: BodyStream,
    limit: usize,
) -> Result<(CapturedBody, Option<Headers>), ExchangeError> {
    let mut capture = Capture::new(limit);
    let mut trailers = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(BodyChunk::Data(d)) => capture.push(&d),
            Ok(BodyChunk::Trailers(t)) => trailers = Some(t),
            Err(e) => {
                let mut err = e.to_exchange_error();
                err.kind = ErrorKind::ClientAborted;
                return Err(err);
            }
        }
    }
    Ok((capture.finish(), trailers))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn head(url: &str) -> RequestHead {
        let u = parse_absolute_url(url).unwrap();
        let mut headers = Headers::new();
        headers.append("Host", authority_for(&u.scheme, &u.host, u.port));
        RequestHead {
            method: "GET".into(),
            url: url.into(),
            scheme: u.scheme,
            host: u.host,
            port: u.port,
            path: u.path,
            http_version: HttpVersion::Http11,
            headers,
        }
    }

    #[test]
    fn content_length_fixups() {
        let mut h = Headers(vec![("Content-Length".into(), "1".into())]);
        fix_content_length(&mut h, 10);
        assert_eq!(h.get("content-length"), Some("10"));
        assert_eq!(h.0[0].0, "Content-Length");
        let mut h = Headers::new();
        fix_content_length(&mut h, 3);
        assert_eq!(h.0, vec![("content-length".to_string(), "3".to_string())]);
        let mut h = Headers(vec![("Host".into(), "a".into())]);
        fix_content_length(&mut h, 4);
        assert_eq!(h.0[1], ("Content-Length".to_string(), "4".to_string()));
        let mut h = Headers(vec![("Transfer-Encoding".into(), "chunked".into())]);
        fix_content_length(&mut h, 3);
        assert!(!h.contains("content-length"));
    }

    #[test]
    fn breakpoint_url_edit_updates_host_unless_edited() {
        let mut fwd = head("http://example.com/a");
        let mut edit = EditBody {
            original_decoded: Bytes::new(),
            decoded: Bytes::new(),
            decode_ok: true,
        };
        apply_editable_request(
            &mut fwd,
            &mut edit,
            EditableRequest {
                method: "POST".into(),
                url: "https://other.test:8443/b?c".into(),
                headers: Headers(vec![("Host".into(), "example.com".into())]),
                body: Bytes::from_static(b"x"),
            },
        );
        assert_eq!(
            (
                fwd.scheme.as_str(),
                fwd.host.as_str(),
                fwd.port,
                fwd.path.as_str()
            ),
            ("https", "other.test", 8443, "/b?c")
        );
        assert_eq!(fwd.headers.get("host"), Some("other.test:8443"));
        assert_eq!(fwd.method, "POST");
        assert!(edit.changed());

        let mut fwd = head("http://example.com/a");
        apply_editable_request(
            &mut fwd,
            &mut edit,
            EditableRequest {
                method: "GET".into(),
                url: "http://new.test/".into(),
                headers: Headers(vec![("Host".into(), "custom.test".into())]),
                body: Bytes::new(),
            },
        );
        assert_eq!(fwd.headers.get("host"), Some("custom.test"));
        assert_eq!(fwd.url, "http://new.test/");
    }

    #[test]
    fn local_responses_have_length_and_reason() {
        let (h, b) = local_response(
            418,
            String::new(),
            Headers::new(),
            Bytes::from_static(b"tea"),
        );
        assert_eq!(h.status_message, "I'm a teapot");
        assert_eq!(h.headers.content_length(), Some(3));
        assert_eq!(&b[..], b"tea");
    }

    #[test]
    fn scripts_are_classified_by_hook() {
        let request_only = "function onRequest(req) { req.headers['x'] = '1'; return req; }";
        let response_only = "function onResponse(req, res) { return res; }";
        let both = "function onRequest(r) { return r } function onResponse(q, r) { return r }";
        assert!(has_request_hook(request_only) && !has_response_hook(request_only));
        assert!(!has_request_hook(response_only) && has_response_hook(response_only));
        assert!(has_request_hook(both) && has_response_hook(both));
    }

    #[test]
    fn sse_detection() {
        let mut h = ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers(vec![(
                "Content-Type".into(),
                "text/event-stream; charset=utf-8".into(),
            )]),
        };
        assert!(is_event_stream(&h));
        h.status = 500;
        assert!(!is_event_stream(&h));
        assert!(no_body_status(304) && no_body_status(101) && !no_body_status(200));
    }
}
