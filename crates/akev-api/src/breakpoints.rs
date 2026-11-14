//! Breakpoint manager: the API-side implementation of [`BreakpointHandler`].
//!
//! When the proxy pauses an exchange, a oneshot channel is registered for it and a `paused`
//! notification is broadcast to UIs. `POST /api/breakpoints/:id/resume` (or resume-all)
//! resolves the channel and the proxy continues with the user's decision. If the proxy drops
//! the pending future (the client disconnected, the proxy shut down), a guard removes the
//! entry and broadcasts `resumed`, so UIs never show stale paused exchanges.
//!
//! Limits (`settings.breakpoints`, [`PauseLimits`]): at most `maxPaused` exchanges (and,
//! separately, WebSocket messages) wait at once — further matches continue without pausing —
//! and with `autoReleaseSecs` a pause continues unchanged after that long. Either way the
//! exchange gets a notice saying why, UIs get a `notice`, and it's counted in the status.

use crate::ws_breakpoints::MessageBreakpoints;
use akev_core::settings::BreakpointSettings;
use akev_core::{
    BreakpointHandler, BreakpointStage, CapturedBody, EditableRequest, EditableResponse,
    ExchangeId, MessageResume, PausedExchange, PausedMessage, RequestHead, RequestResume,
    ResponseHead, ResponseResume, UnixMicros,
};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

/// Notifications emitted by the manager.
#[derive(Clone, Debug, PartialEq)]
pub enum BreakpointNotice {
    Paused(PausedExchange),
    Resumed(ExchangeId),
    /// A WebSocket message paused at a `ws-breakpoint` (see [`crate::ws_breakpoints`]).
    MessagePaused(PausedMessage),
    /// A paused WebSocket message was forwarded, dropped or abandoned.
    MessageResumed {
        id: ExchangeId,
        pause_id: u64,
    },
    /// A pause was released automatically or skipped because of a limit; `message` says why.
    Limited {
        id: ExchangeId,
        message: String,
    },
}

/// Callback receiving notices (must be cheap and non-blocking).
pub type Notifier = Arc<dyn Fn(BreakpointNotice) + Send + Sync>;

/// Records a notice on an exchange (the store shows it with the exchange).
pub type ExchangeNotices = Arc<dyn Fn(ExchangeId, &str) + Send + Sync>;

/// Limits on paused exchanges and messages (`settings.breakpoints`), shared by the exchange
/// and WebSocket message breakpoints.
pub struct PauseLimits {
    max_paused: AtomicU32,
    auto_release_secs: AtomicU64,
    limited: AtomicU64,
    notify: Notifier,
    exchange_notices: Mutex<Option<ExchangeNotices>>,
}

impl PauseLimits {
    pub(crate) fn new(notify: Notifier) -> Self {
        let defaults = BreakpointSettings::default();
        PauseLimits {
            max_paused: AtomicU32::new(defaults.max_paused),
            auto_release_secs: AtomicU64::new(defaults.auto_release_secs),
            limited: AtomicU64::new(0),
            notify,
            exchange_notices: Mutex::new(None),
        }
    }

    pub(crate) fn set(&self, settings: &BreakpointSettings) {
        self.max_paused
            .store(settings.max_paused, Ordering::Relaxed);
        self.auto_release_secs
            .store(settings.auto_release_secs, Ordering::Relaxed);
    }

    /// Whether `paused` already-waiting pauses leave room for another one.
    pub(crate) fn has_room(&self, paused: usize) -> bool {
        let max = self.max_paused.load(Ordering::Relaxed);
        max == 0 || paused < max as usize
    }

    pub(crate) fn max_paused(&self) -> u32 {
        self.max_paused.load(Ordering::Relaxed)
    }

    /// How long a pause may wait before it continues by itself, if limited.
    pub(crate) fn auto_release(&self) -> Option<Duration> {
        let secs = self.auto_release_secs.load(Ordering::Relaxed);
        (secs > 0).then(|| Duration::from_secs(secs))
    }

    /// Pauses released automatically or skipped so far.
    pub(crate) fn count(&self) -> u64 {
        self.limited.load(Ordering::Relaxed)
    }

    /// A limit decided for the user: count it, tell UIs and record why on the exchange.
    pub(crate) fn limited(&self, id: ExchangeId, message: String) {
        self.limited.fetch_add(1, Ordering::Relaxed);
        tracing::info!(%id, "{message}");
        let record = self.exchange_notices.lock().clone();
        if let Some(record) = record {
            record(id, &message);
        }
        (self.notify)(BreakpointNotice::Limited { id, message });
    }
}

/// Wait for a decision, continuing on our own after the auto-release delay (if any).
/// `Err(())` = released automatically; `release` removes the pending entry and reports
/// whether it was still waiting (a decision may have arrived at the same moment).
pub(crate) async fn wait_or_release<T>(
    rx: oneshot::Receiver<T>,
    after: Option<Duration>,
    release: impl FnOnce() -> bool,
) -> Result<Option<T>, ()> {
    let Some(after) = after else {
        return Ok(rx.await.ok());
    };
    let mut rx = rx;
    tokio::select! {
        r = &mut rx => Ok(r.ok()),
        _ = tokio::time::sleep(after) => {
            if release() {
                Err(())
            } else {
                // Resolved just now: take the decision.
                Ok(rx.await.ok())
            }
        }
    }
}

/// Looks up the captured (still content-encoded) request body of an exchange, so the
/// response-stage snapshot can show the request that produced the response.
pub type RequestBodySource = Arc<dyn Fn(ExchangeId) -> Option<CapturedBody> + Send + Sync>;

enum Decision {
    Request(RequestResume),
    Response(ResponseResume),
}

struct Pending {
    paused: PausedExchange,
    tx: oneshot::Sender<Decision>,
}

/// Errors resolving a breakpoint.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ResumeError {
    #[error("exchange {0} is not paused")]
    NotPaused(ExchangeId),
    #[error("invalid resume payload: {0}")]
    Invalid(String),
}

/// Tracks exchanges paused at breakpoints.
pub struct BreakpointManager {
    pending: Mutex<BTreeMap<ExchangeId, Pending>>,
    notify: Notifier,
    request_bodies: Option<RequestBodySource>,
    /// WebSocket messages paused at `ws-breakpoint` rules.
    messages: MessageBreakpoints,
    limits: Arc<PauseLimits>,
}

impl BreakpointManager {
    pub fn new(notify: Notifier) -> Self {
        let limits = Arc::new(PauseLimits::new(notify.clone()));
        BreakpointManager {
            pending: Mutex::new(BTreeMap::new()),
            messages: MessageBreakpoints::new(notify.clone(), limits.clone()),
            notify,
            request_bodies: None,
            limits,
        }
    }

    /// Record why a pause was limited on the exchange itself (see [`ExchangeNotices`]).
    pub fn with_exchange_notices(self, record: ExchangeNotices) -> Self {
        *self.limits.exchange_notices.lock() = Some(record);
        self
    }

    /// Apply `settings.breakpoints` (new pauses only; waiting ones keep their deadline).
    pub fn set_limits(&self, settings: &BreakpointSettings) {
        self.limits.set(settings);
    }

    /// Pauses (exchanges and messages) released automatically or skipped because of
    /// `maxPaused` since start.
    pub fn limited_count(&self) -> u64 {
        self.limits.count()
    }

    /// Whether another exchange may pause now; otherwise records why it didn't.
    fn admit(&self, id: ExchangeId) -> bool {
        let paused = self.pending.lock().len();
        if self.limits.has_room(paused) {
            return true;
        }
        self.limits.limited(
            id,
            format!(
                "Not paused at the breakpoint: {} exchanges were already paused (the limit set in Settings)",
                self.limits.max_paused()
            ),
        );
        false
    }

    /// Wait for the decision on a registered pause, releasing it after the auto-release
    /// delay. `None` = continue unchanged.
    async fn decision(&self, id: ExchangeId, rx: oneshot::Receiver<Decision>) -> Option<Decision> {
        let after = self.limits.auto_release();
        let released = wait_or_release(rx, after, || {
            let removed = self.pending.lock().remove(&id).is_some();
            if removed {
                (self.notify)(BreakpointNotice::Resumed(id));
            }
            removed
        })
        .await;
        match released {
            Ok(decision) => decision,
            Err(()) => {
                let secs = after.map_or(0, |d| d.as_secs());
                self.limits.limited(
                    id,
                    format!(
                        "Breakpoint released automatically after {secs} s; the exchange continued unchanged"
                    ),
                );
                None
            }
        }
    }

    /// Paused WebSocket messages.
    pub fn messages(&self) -> &MessageBreakpoints {
        &self.messages
    }

    /// Use `source` to fill in the request body of response-stage snapshots.
    pub fn with_request_bodies(mut self, source: RequestBodySource) -> Self {
        self.request_bodies = Some(source);
        self
    }

    /// Currently paused exchanges (id order).
    pub fn list(&self) -> Vec<PausedExchange> {
        self.pending
            .lock()
            .values()
            .map(|p| p.paused.clone())
            .collect()
    }

    /// Number of paused exchanges.
    pub fn len(&self) -> usize {
        self.pending.lock().len()
    }

    /// Resolve a paused exchange with a `RequestResume` / `ResponseResume` JSON payload
    /// (depending on the stage it's paused at). `{"action":"continue"}` without an edited
    /// request/response (or an empty payload) continues unchanged.
    pub fn resume(&self, id: ExchangeId, payload: Value) -> Result<(), ResumeError> {
        let mut map = self.pending.lock();
        let Some(pending) = map.get(&id) else {
            return Err(ResumeError::NotPaused(id));
        };
        let decision = parse_decision(&pending.paused, payload)?;
        let Some(pending) = map.remove(&id) else {
            return Err(ResumeError::NotPaused(id));
        };
        drop(map);
        // The waiter may already be gone (client disconnected); that's fine.
        let _ = pending.tx.send(decision);
        (self.notify)(BreakpointNotice::Resumed(id));
        Ok(())
    }

    /// Continue every paused exchange (and forward every paused WebSocket message) unchanged.
    /// Returns how many were resumed.
    pub fn resume_all(&self) -> usize {
        self.messages.resume_all(None, false) + self.resume_all_exchanges()
    }

    fn resume_all_exchanges(&self) -> usize {
        let drained: Vec<(ExchangeId, Pending)> = std::mem::take(&mut *self.pending.lock())
            .into_iter()
            .collect();
        let n = drained.len();
        for (id, pending) in drained {
            let decision = match pending.paused.stage {
                BreakpointStage::Request => Decision::Request(RequestResume::Continue {
                    request: pending.paused.request,
                }),
                BreakpointStage::Response => match pending.paused.response {
                    Some(response) => Decision::Response(ResponseResume::Continue { response }),
                    // Response stage without a response snapshot can't happen; dropping the
                    // sender makes the waiter fall back to "continue unchanged".
                    None => {
                        (self.notify)(BreakpointNotice::Resumed(id));
                        continue;
                    }
                },
            };
            let _ = pending.tx.send(decision);
            (self.notify)(BreakpointNotice::Resumed(id));
        }
        n
    }

    fn register(&self, paused: PausedExchange) -> oneshot::Receiver<Decision> {
        let (tx, rx) = oneshot::channel();
        let id = paused.id;
        let previous = self.pending.lock().insert(
            id,
            Pending {
                paused: paused.clone(),
                tx,
            },
        );
        if previous.is_some() {
            tracing::debug!("exchange {id} paused again before the previous pause resolved");
        }
        (self.notify)(BreakpointNotice::Paused(paused));
        rx
    }

    fn forget(&self, id: ExchangeId) {
        let removed = self.pending.lock().remove(&id).is_some();
        if removed {
            (self.notify)(BreakpointNotice::Resumed(id));
        }
    }
}

/// Removes the pending entry if the waiting future is dropped before being resolved.
struct PendingGuard<'a> {
    manager: &'a BreakpointManager,
    id: ExchangeId,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        self.manager.forget(self.id);
    }
}

fn action_of(payload: &Value) -> Option<&str> {
    payload.get("action").and_then(Value::as_str)
}

fn parse_decision(paused: &PausedExchange, mut payload: Value) -> Result<Decision, ResumeError> {
    if payload.is_null() || payload.as_object().is_some_and(|o| o.is_empty()) {
        payload = serde_json::json!({ "action": "continue" });
    }
    let Some(obj) = payload.as_object_mut() else {
        return Err(ResumeError::Invalid("expected a JSON object".into()));
    };
    if !obj.contains_key("action") {
        return Err(ResumeError::Invalid("missing `action`".into()));
    }
    match paused.stage {
        BreakpointStage::Request => {
            if action_of(&payload) == Some("continue") && payload.get("request").is_none() {
                return Ok(Decision::Request(RequestResume::Continue {
                    request: paused.request.clone(),
                }));
            }
            serde_json::from_value::<RequestResume>(payload)
                .map(Decision::Request)
                .map_err(|e| ResumeError::Invalid(e.to_string()))
        }
        BreakpointStage::Response => {
            if action_of(&payload) == Some("continue") && payload.get("response").is_none() {
                let Some(response) = paused.response.clone() else {
                    return Err(ResumeError::Invalid(
                        "paused response snapshot missing".into(),
                    ));
                };
                return Ok(Decision::Response(ResponseResume::Continue { response }));
            }
            serde_json::from_value::<ResponseResume>(payload)
                .map(Decision::Response)
                .map_err(|e| ResumeError::Invalid(e.to_string()))
        }
    }
}

#[async_trait]
impl BreakpointHandler for BreakpointManager {
    async fn on_request(
        &self,
        id: ExchangeId,
        _head: &RequestHead,
        request: EditableRequest,
    ) -> RequestResume {
        if !self.admit(id) {
            return RequestResume::Continue { request };
        }
        let rx = self.register(PausedExchange {
            id,
            stage: BreakpointStage::Request,
            paused_at: UnixMicros::now(),
            request: request.clone(),
            response: None,
        });
        let guard = PendingGuard { manager: self, id };
        let outcome = self.decision(id, rx).await;
        drop(guard);
        match outcome {
            Some(Decision::Request(decision)) => decision,
            _ => RequestResume::Continue { request },
        }
    }

    async fn on_response(
        &self,
        id: ExchangeId,
        request: &RequestHead,
        head: &ResponseHead,
        response: EditableResponse,
    ) -> ResponseResume {
        let body = self
            .request_bodies
            .as_ref()
            .and_then(|lookup| lookup(id))
            .map(|captured| crate::body::decode_or_raw(&captured.bytes, &request.headers).0)
            .unwrap_or_default();
        let editable_request = EditableRequest {
            method: request.method.clone(),
            url: request.url.clone(),
            headers: request.headers.clone(),
            body,
        };
        let _ = head;
        if !self.admit(id) {
            return ResponseResume::Continue { response };
        }
        let rx = self.register(PausedExchange {
            id,
            stage: BreakpointStage::Response,
            paused_at: UnixMicros::now(),
            request: editable_request,
            response: Some(response.clone()),
        });
        let guard = PendingGuard { manager: self, id };
        let outcome = self.decision(id, rx).await;
        drop(guard);
        match outcome {
            Some(Decision::Response(decision)) => decision,
            _ => ResponseResume::Continue { response },
        }
    }

    async fn on_message(&self, message: PausedMessage) -> MessageResume {
        self.messages.wait(message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::{Headers, HttpVersion};
    use bytes::Bytes;
    use serde_json::json;
    use std::time::Duration;

    fn manager() -> (Arc<BreakpointManager>, Arc<Mutex<Vec<BreakpointNotice>>>) {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n = notices.clone();
        let m = Arc::new(BreakpointManager::new(Arc::new(move |notice| {
            n.lock().push(notice)
        })));
        (m, notices)
    }

    fn head() -> RequestHead {
        RequestHead {
            method: "GET".into(),
            url: "http://example.com/a".into(),
            scheme: "http".into(),
            host: "example.com".into(),
            port: 80,
            path: "/a".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        }
    }

    fn editable() -> EditableRequest {
        EditableRequest {
            method: "GET".into(),
            url: "http://example.com/a".into(),
            headers: Headers::new(),
            body: Bytes::from_static(b"hi"),
        }
    }

    async fn wait_paused(m: &BreakpointManager, n: usize) {
        for _ in 0..200 {
            if m.len() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("expected {n} paused exchanges, have {}", m.len());
    }

    #[tokio::test]
    async fn resume_with_edited_request() {
        let (m, notices) = manager();
        let id = ExchangeId(1001);
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(id, &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        let paused = m.list();
        assert_eq!(paused[0].id, id);
        assert_eq!(paused[0].stage, BreakpointStage::Request);
        let edited = json!({
            "action": "continue",
            "request": {"method": "POST", "url": "http://example.com/b", "headers": [["x-a", "1"]], "body": "Ynll"}
        });
        m.resume(id, edited).unwrap();
        match task.await.unwrap() {
            RequestResume::Continue { request } => {
                assert_eq!(request.method, "POST");
                assert_eq!(request.body, Bytes::from_static(b"bye"));
            }
            other => panic!("unexpected {other:?}"),
        }
        assert_eq!(m.len(), 0);
        let n = notices.lock();
        assert!(matches!(n[0], BreakpointNotice::Paused(_)));
        assert_eq!(n[1], BreakpointNotice::Resumed(id));
        assert_eq!(n.len(), 2, "exactly one resumed notice");
    }

    #[tokio::test]
    async fn pauses_beyond_the_limit_continue_and_say_why() {
        let recorded = Arc::new(Mutex::new(Vec::<(ExchangeId, String)>::new()));
        let (m, notices) = manager();
        let r = recorded.clone();
        let m = Arc::new(Arc::into_inner(m).unwrap().with_exchange_notices(Arc::new(
            move |id, msg: &str| r.lock().push((id, msg.to_string())),
        )));
        m.set_limits(&BreakpointSettings {
            max_paused: 1,
            auto_release_secs: 0,
        });
        let first = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(ExchangeId(1), &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        // The second one doesn't wait at all.
        let second = tokio::time::timeout(
            Duration::from_secs(5),
            m.on_request(ExchangeId(2), &head(), editable()),
        )
        .await
        .expect("not paused");
        assert_eq!(
            second,
            RequestResume::Continue {
                request: editable()
            }
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m.limited_count(), 1);
        let why = recorded.lock().clone();
        assert_eq!(why.len(), 1);
        assert_eq!(why[0].0, ExchangeId(2));
        assert!(
            why[0]
                .1
                .starts_with("Not paused at the breakpoint: 1 exchanges"),
            "{}",
            why[0].1
        );
        assert!(notices.lock().iter().any(|n| matches!(
            n,
            BreakpointNotice::Limited { id, .. } if *id == ExchangeId(2)
        )));
        m.resume(ExchangeId(1), json!({})).unwrap();
        first.await.unwrap();
        // 0 = no limit.
        m.set_limits(&BreakpointSettings {
            max_paused: 0,
            auto_release_secs: 0,
        });
        let tasks: Vec<_> = (10..13)
            .map(|i| {
                let m = m.clone();
                tokio::spawn(async move { m.on_request(ExchangeId(i), &head(), editable()).await })
            })
            .collect();
        wait_paused(&m, 3).await;
        assert_eq!(m.resume_all(), 3);
        for t in tasks {
            t.await.unwrap();
        }
    }

    #[tokio::test]
    async fn pauses_release_themselves_after_the_delay() {
        let (m, notices) = manager();
        m.set_limits(&BreakpointSettings {
            max_paused: 10,
            auto_release_secs: 1,
        });
        let started = std::time::Instant::now();
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(ExchangeId(7), &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        let outcome = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("released")
            .unwrap();
        assert!(started.elapsed() >= Duration::from_millis(900));
        assert_eq!(
            outcome,
            RequestResume::Continue {
                request: editable()
            }
        );
        assert_eq!(m.len(), 0);
        assert_eq!(m.limited_count(), 1);
        let n = notices.lock();
        assert!(n.contains(&BreakpointNotice::Resumed(ExchangeId(7))));
        assert!(n.iter().any(|n| matches!(
            n,
            BreakpointNotice::Limited { message, .. }
                if message == "Breakpoint released automatically after 1 s; the exchange continued unchanged"
        )));
        // A resume after the release is simply too late.
        assert!(m.resume(ExchangeId(7), json!({})).is_err());
    }

    #[tokio::test]
    async fn continue_without_payload_keeps_original() {
        let (m, _) = manager();
        let id = ExchangeId(1002);
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(id, &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        m.resume(id, json!({"action": "continue"})).unwrap();
        assert_eq!(
            task.await.unwrap(),
            RequestResume::Continue {
                request: editable()
            }
        );
    }

    #[tokio::test]
    async fn respond_close_and_errors() {
        let (m, _) = manager();
        let id = ExchangeId(1003);
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(id, &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        assert_eq!(
            m.resume(ExchangeId(9), json!({})),
            Err(ResumeError::NotPaused(ExchangeId(9)))
        );
        assert!(matches!(
            m.resume(id, json!({"action": "explode"})),
            Err(ResumeError::Invalid(_))
        ));
        assert!(matches!(
            m.resume(id, json!([1])),
            Err(ResumeError::Invalid(_))
        ));
        assert_eq!(m.len(), 1, "invalid payload keeps the exchange paused");
        m.resume(
            id,
            json!({"action": "respond", "response": {"status": 418, "statusMessage": "Teapot", "headers": [], "body": ""}}),
        )
        .unwrap();
        match task.await.unwrap() {
            RequestResume::Respond { response } => assert_eq!(response.status, 418),
            other => panic!("unexpected {other:?}"),
        }

        let id = ExchangeId(1004);
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(id, &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        m.resume(id, json!({"action": "close"})).unwrap();
        assert_eq!(task.await.unwrap(), RequestResume::Close);
    }

    #[tokio::test]
    async fn response_stage() {
        let (m, _) = manager();
        let id = ExchangeId(1005);
        let response = EditableResponse {
            status: 200,
            status_message: "OK".into(),
            headers: Headers::new(),
            body: Bytes::from_static(b"{}"),
        };
        let rhead = ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let task = {
            let m = m.clone();
            let response = response.clone();
            tokio::spawn(async move { m.on_response(id, &head(), &rhead, response).await })
        };
        wait_paused(&m, 1).await;
        let paused = m.list();
        assert_eq!(paused[0].stage, BreakpointStage::Response);
        assert_eq!(paused[0].response.as_ref(), Some(&response));
        // `respond` is not valid at the response stage.
        assert!(matches!(
            m.resume(id, json!({"action": "respond", "response": {"status": 1, "statusMessage": "", "headers": [], "body": ""}})),
            Err(ResumeError::Invalid(_))
        ));
        m.resume(id, json!({"action": "continue", "response": {"status": 500, "statusMessage": "Nope", "headers": [], "body": ""}}))
            .unwrap();
        match task.await.unwrap() {
            ResponseResume::Continue { response } => assert_eq!(response.status, 500),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn response_stage_snapshot_includes_request_body() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n = notices.clone();
        let m = Arc::new(
            BreakpointManager::new(Arc::new(move |notice| n.lock().push(notice)))
                .with_request_bodies(Arc::new(|id: ExchangeId| {
                    (id == ExchangeId(1010))
                        .then(|| CapturedBody::complete(Bytes::from_static(b"request payload")))
                })),
        );
        let rhead = ResponseHead {
            status: 200,
            status_message: "OK".into(),
            http_version: HttpVersion::Http11,
            headers: Headers::new(),
        };
        let response = EditableResponse {
            status: 200,
            status_message: "OK".into(),
            headers: Headers::new(),
            body: Bytes::new(),
        };
        let task = {
            let m = m.clone();
            tokio::spawn(async move {
                m.on_response(ExchangeId(1010), &head(), &rhead, response)
                    .await
            })
        };
        wait_paused(&m, 1).await;
        assert_eq!(
            m.list()[0].request.body,
            Bytes::from_static(b"request payload")
        );
        assert_eq!(m.resume_all(), 1);
        assert!(matches!(
            task.await.unwrap(),
            ResponseResume::Continue { .. }
        ));
    }

    #[tokio::test]
    async fn resume_all_continues_unchanged() {
        let (m, notices) = manager();
        let mut tasks = Vec::new();
        for i in 0..3 {
            let m = m.clone();
            tasks.push(tokio::spawn(async move {
                m.on_request(ExchangeId(2000 + i), &head(), editable())
                    .await
            }));
        }
        wait_paused(&m, 3).await;
        assert_eq!(m.resume_all(), 3);
        for t in tasks {
            assert_eq!(
                t.await.unwrap(),
                RequestResume::Continue {
                    request: editable()
                }
            );
        }
        assert_eq!(m.len(), 0);
        let resumed = notices
            .lock()
            .iter()
            .filter(|n| matches!(n, BreakpointNotice::Resumed(_)))
            .count();
        assert_eq!(resumed, 3);
    }

    #[tokio::test]
    async fn dropped_waiter_cleans_up() {
        let (m, notices) = manager();
        let id = ExchangeId(1006);
        let task = {
            let m = m.clone();
            tokio::spawn(async move { m.on_request(id, &head(), editable()).await })
        };
        wait_paused(&m, 1).await;
        // Client disconnect → the proxy drops the future.
        task.abort();
        let _ = task.await;
        assert_eq!(m.len(), 0);
        assert_eq!(notices.lock().last(), Some(&BreakpointNotice::Resumed(id)));
        assert_eq!(m.resume(id, json!({})), Err(ResumeError::NotPaused(id)));
    }
}
