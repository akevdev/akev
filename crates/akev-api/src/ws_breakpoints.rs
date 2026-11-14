//! Paused WebSocket messages: the API side of [`akev_core::BreakpointHandler::on_message`]
//! (`ws-breakpoint` rules), mirroring the exchange breakpoints of [`crate::breakpoints`].
//!
//! Each paused message gets a oneshot channel and a `message-paused` notice. `POST
//! /api/breakpoints/messages/:pauseId/resume` (forward, possibly edited, or drop) or resume-all
//! resolves it. When the proxy abandons the pause (the connection closed), a guard removes the
//! entry and broadcasts `message-resumed`, so UIs never show stale paused messages.

use crate::breakpoints::{BreakpointNotice, Notifier, PauseLimits, wait_or_release};
use akev_core::{ExchangeId, MessageKind, MessageResume, PausedMessage};
use base64::Engine;
use bytes::Bytes;
use parking_lot::Mutex;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::oneshot;

/// Largest message payload accepted from API clients (injected or edited).
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024 * 1024;

/// A message payload in API JSON: `text` (sent as UTF-8) or `base64`, plus an optional `kind`.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageBody {
    #[serde(default)]
    pub kind: Option<MessageKind>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub base64: Option<String>,
}

impl MessageBody {
    /// The message to send, or `None` when neither `text` nor `base64` is given. Without a
    /// `kind`, `text` payloads are text messages and `base64` ones `default_kind`.
    pub fn resolve(
        self,
        default_kind: MessageKind,
    ) -> Result<Option<(MessageKind, Bytes)>, String> {
        let (payload, kind) = match (self.text, self.base64) {
            (Some(_), Some(_)) => return Err("give either `text` or `base64`, not both".into()),
            (None, None) => {
                return match self.kind {
                    Some(_) => Err("`kind` needs a `text` or `base64` payload".into()),
                    None => Ok(None),
                };
            }
            (Some(text), None) => (Bytes::from(text), self.kind.unwrap_or(MessageKind::Text)),
            (None, Some(b64)) => {
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(b64.trim().as_bytes())
                    .map_err(|e| format!("invalid base64: {e}"))?;
                (Bytes::from(bytes), self.kind.unwrap_or(default_kind))
            }
        };
        match kind {
            MessageKind::Text if std::str::from_utf8(&payload).is_err() => {
                Err("a text message must be valid UTF-8 (send it as `kind: \"binary\"`)".into())
            }
            MessageKind::Text | MessageKind::Binary if payload.len() > MAX_MESSAGE_BYTES => {
                Err(format!(
                    "message too large ({} bytes, max {MAX_MESSAGE_BYTES})",
                    payload.len()
                ))
            }
            MessageKind::Text | MessageKind::Binary => Ok(Some((kind, payload))),
            _ => Err("`kind` must be `text` or `binary`".into()),
        }
    }
}

/// Errors resolving a paused message.
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum MessageResumeError {
    #[error("message {0} is not paused")]
    NotPaused(u64),
    #[error("invalid resume payload: {0}")]
    Invalid(String),
}

struct Pending {
    paused: PausedMessage,
    tx: oneshot::Sender<MessageResume>,
}

/// Tracks WebSocket messages paused at `ws-breakpoint` rules.
pub struct MessageBreakpoints {
    pending: Mutex<BTreeMap<u64, Pending>>,
    notify: Notifier,
    limits: Arc<PauseLimits>,
}

impl MessageBreakpoints {
    pub(crate) fn new(notify: Notifier, limits: Arc<PauseLimits>) -> Self {
        MessageBreakpoints {
            pending: Mutex::new(BTreeMap::new()),
            notify,
            limits,
        }
    }

    /// Paused messages, in pause order.
    pub fn list(&self) -> Vec<PausedMessage> {
        self.pending
            .lock()
            .values()
            .map(|p| p.paused.clone())
            .collect()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.pending.lock().len()
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Resolve a paused message with `{"action": "forward", kind?, text?, base64?}` (without a
    /// payload: unchanged) or `{"action": "drop"}`. An empty payload forwards unchanged.
    pub fn resume(&self, pause_id: u64, payload: Value) -> Result<(), MessageResumeError> {
        let mut map = self.pending.lock();
        let Some(pending) = map.get(&pause_id) else {
            return Err(MessageResumeError::NotPaused(pause_id));
        };
        let decision = parse_decision(&pending.paused, payload)?;
        let Some(pending) = map.remove(&pause_id) else {
            return Err(MessageResumeError::NotPaused(pause_id));
        };
        drop(map);
        self.release(pending, decision);
        Ok(())
    }

    /// Forward (or drop) every paused message, or only those of exchange `id`. Returns how many
    /// were resolved.
    pub fn resume_all(&self, id: Option<ExchangeId>, drop_them: bool) -> usize {
        let drained: Vec<Pending> = {
            let mut map = self.pending.lock();
            let ids: Vec<u64> = map
                .iter()
                .filter(|(_, p)| id.is_none_or(|id| p.paused.id == id))
                .map(|(k, _)| *k)
                .collect();
            ids.iter().filter_map(|k| map.remove(k)).collect()
        };
        let n = drained.len();
        for pending in drained {
            let decision = if drop_them {
                MessageResume::Drop
            } else {
                MessageResume::Forward {
                    kind: pending.paused.kind,
                    payload: pending.paused.payload.clone(),
                }
            };
            self.release(pending, decision);
        }
        n
    }

    fn release(&self, pending: Pending, decision: MessageResume) {
        let (id, pause_id) = (pending.paused.id, pending.paused.pause_id);
        // The relay may already be gone (connection closed); that's fine.
        let _ = pending.tx.send(decision);
        (self.notify)(BreakpointNotice::MessageResumed { id, pause_id });
    }

    /// Pause `message` until resolved (or until the returned future is dropped). Past the
    /// `maxPaused` limit the message goes on without pausing; with `autoReleaseSecs` it's
    /// forwarded unchanged after that long.
    pub async fn wait(&self, message: PausedMessage) -> MessageResume {
        let (tx, rx) = oneshot::channel();
        let pause_id = message.pause_id;
        let id = message.id;
        let original = MessageResume::Forward {
            kind: message.kind,
            payload: message.payload.clone(),
        };
        if !self.limits.has_room(self.pending.lock().len()) {
            self.limits.limited(
                id,
                format!(
                    "A message was not paused: {} messages were already paused (the limit set in Settings)",
                    self.limits.max_paused()
                ),
            );
            return original;
        }
        self.pending.lock().insert(
            pause_id,
            Pending {
                paused: message.clone(),
                tx,
            },
        );
        (self.notify)(BreakpointNotice::MessagePaused(message));
        let guard = Guard {
            manager: self,
            pause_id,
        };
        let after = self.limits.auto_release();
        let outcome = wait_or_release(rx, after, || {
            let removed = self.pending.lock().remove(&pause_id).is_some();
            if removed {
                (self.notify)(BreakpointNotice::MessageResumed { id, pause_id });
            }
            removed
        })
        .await;
        drop(guard);
        match outcome {
            Ok(decision) => decision.unwrap_or(original),
            Err(()) => {
                let secs = after.map_or(0, |d| d.as_secs());
                self.limits.limited(
                    id,
                    format!("A paused message was forwarded automatically after {secs} s"),
                );
                original
            }
        }
    }

    fn forget(&self, pause_id: u64) {
        let removed = self.pending.lock().remove(&pause_id);
        if let Some(p) = removed {
            (self.notify)(BreakpointNotice::MessageResumed {
                id: p.paused.id,
                pause_id,
            });
        }
    }
}

/// Removes the entry when the waiting future is dropped before being resolved.
struct Guard<'a> {
    manager: &'a MessageBreakpoints,
    pause_id: u64,
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.manager.forget(self.pause_id);
    }
}

fn parse_decision(
    paused: &PausedMessage,
    payload: Value,
) -> Result<MessageResume, MessageResumeError> {
    let invalid = |m: String| MessageResumeError::Invalid(m);
    let mut obj = match payload {
        Value::Null => serde_json::Map::new(),
        Value::Object(o) => o,
        _ => return Err(invalid("expected a JSON object".into())),
    };
    let action = match obj.remove("action") {
        None => "forward".to_string(),
        Some(Value::String(a)) => a,
        Some(_) => return Err(invalid("`action` must be a string".into())),
    };
    match action.as_str() {
        "forward" => {
            let body: MessageBody =
                serde_json::from_value(Value::Object(obj)).map_err(|e| invalid(e.to_string()))?;
            Ok(match body.resolve(paused.kind).map_err(invalid)? {
                Some((kind, payload)) => MessageResume::Forward { kind, payload },
                None => MessageResume::Forward {
                    kind: paused.kind,
                    payload: paused.payload.clone(),
                },
            })
        }
        "drop" => Ok(MessageResume::Drop),
        other => Err(invalid(format!(
            "unknown action {other:?} (expected `forward` or `drop`)"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::{Direction, UnixMicros};
    use serde_json::json;
    use std::sync::Arc;
    use std::time::Duration;

    fn manager() -> (Arc<MessageBreakpoints>, Arc<Mutex<Vec<BreakpointNotice>>>) {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n = notices.clone();
        let notify: Notifier = Arc::new(move |notice| n.lock().push(notice));
        let limits = Arc::new(PauseLimits::new(notify.clone()));
        (Arc::new(MessageBreakpoints::new(notify, limits)), notices)
    }

    fn message(id: u64, pause_id: u64) -> PausedMessage {
        PausedMessage {
            id: ExchangeId(id),
            pause_id,
            direction: Direction::Sent,
            kind: MessageKind::Text,
            payload: Bytes::from_static(b"{\"a\":1}"),
            at: UnixMicros(0),
            rule: "r".into(),
        }
    }

    async fn wait_paused(m: &MessageBreakpoints, n: usize) {
        for _ in 0..200 {
            if m.len() == n {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("expected {n} paused messages, have {}", m.len());
    }

    fn spawn(
        m: &Arc<MessageBreakpoints>,
        msg: PausedMessage,
    ) -> tokio::task::JoinHandle<MessageResume> {
        let m = m.clone();
        tokio::spawn(async move { m.wait(msg).await })
    }

    #[tokio::test]
    async fn forward_edited_unchanged_and_drop() {
        let (m, notices) = manager();
        let t = spawn(&m, message(1, 10));
        wait_paused(&m, 1).await;
        assert_eq!(m.list()[0].pause_id, 10);
        m.resume(10, json!({"action": "forward", "text": "{\"a\":2}"}))
            .unwrap();
        assert_eq!(
            t.await.unwrap(),
            MessageResume::Forward {
                kind: MessageKind::Text,
                payload: Bytes::from_static(b"{\"a\":2}")
            }
        );
        let t = spawn(&m, message(1, 11));
        wait_paused(&m, 1).await;
        m.resume(11, json!({})).unwrap();
        assert_eq!(
            t.await.unwrap(),
            MessageResume::Forward {
                kind: MessageKind::Text,
                payload: Bytes::from_static(b"{\"a\":1}")
            }
        );
        let t = spawn(&m, message(1, 12));
        wait_paused(&m, 1).await;
        m.resume(
            12,
            json!({"action": "forward", "kind": "binary", "base64": "AAE="}),
        )
        .unwrap();
        assert_eq!(
            t.await.unwrap(),
            MessageResume::Forward {
                kind: MessageKind::Binary,
                payload: Bytes::from_static(&[0, 1])
            }
        );
        let t = spawn(&m, message(1, 13));
        wait_paused(&m, 1).await;
        m.resume(13, json!({"action": "drop"})).unwrap();
        assert_eq!(t.await.unwrap(), MessageResume::Drop);
        let n = notices.lock();
        assert_eq!(n.len(), 8, "one paused + one resumed notice each");
        assert!(matches!(&n[0], BreakpointNotice::MessagePaused(p) if p.pause_id == 10));
        assert_eq!(
            n[1],
            BreakpointNotice::MessageResumed {
                id: ExchangeId(1),
                pause_id: 10
            }
        );
    }

    #[tokio::test]
    async fn message_pauses_are_limited_too() {
        let notices = Arc::new(Mutex::new(Vec::new()));
        let n = notices.clone();
        let notify: Notifier = Arc::new(move |notice| n.lock().push(notice));
        let limits = Arc::new(PauseLimits::new(notify.clone()));
        limits.set(&akev_core::settings::BreakpointSettings {
            max_paused: 1,
            auto_release_secs: 1,
        });
        let m = Arc::new(MessageBreakpoints::new(notify, limits.clone()));
        let t = spawn(&m, message(1, 40));
        wait_paused(&m, 1).await;
        // Over the limit: forwarded at once.
        let second = tokio::time::timeout(Duration::from_secs(5), m.wait(message(2, 41)))
            .await
            .unwrap();
        assert_eq!(
            second,
            MessageResume::Forward {
                kind: MessageKind::Text,
                payload: message(2, 41).payload,
            }
        );
        // The paused one goes on unchanged after a second.
        let first = tokio::time::timeout(Duration::from_secs(5), t)
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, MessageResume::Forward { .. }));
        assert!(m.is_empty());
        assert_eq!(limits.count(), 2);
        let why: Vec<String> = notices
            .lock()
            .iter()
            .filter_map(|n| match n {
                BreakpointNotice::Limited { message, .. } => Some(message.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(
            why,
            vec![
                "A message was not paused: 1 messages were already paused (the limit set in Settings)"
                    .to_string(),
                "A paused message was forwarded automatically after 1 s".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn invalid_payloads_keep_the_message_paused() {
        let (m, _) = manager();
        let t = spawn(&m, message(2, 20));
        wait_paused(&m, 1).await;
        assert_eq!(
            m.resume(99, json!({})),
            Err(MessageResumeError::NotPaused(99))
        );
        for bad in [
            json!({"action": "explode"}),
            json!([1]),
            json!({"action": 1}),
            json!({"action": "forward", "text": "a", "base64": "YQ=="}),
            json!({"action": "forward", "kind": "text", "base64": "/w=="}),
            json!({"action": "forward", "kind": "ping", "text": "x"}),
            json!({"action": "forward", "base64": "not base64!"}),
            json!({"action": "forward", "kind": "binary"}),
        ] {
            assert!(
                matches!(
                    m.resume(20, bad.clone()),
                    Err(MessageResumeError::Invalid(_))
                ),
                "{bad}"
            );
        }
        assert_eq!(m.len(), 1);
        m.resume(20, json!({"action": "forward", "base64": "aGk="}))
            .unwrap();
        // base64 without a kind keeps the paused message's kind (text).
        assert_eq!(
            t.await.unwrap(),
            MessageResume::Forward {
                kind: MessageKind::Text,
                payload: Bytes::from_static(b"hi")
            }
        );
    }

    #[tokio::test]
    async fn resume_all_per_exchange_and_globally() {
        let (m, _) = manager();
        let a = spawn(&m, message(1, 30));
        let b = spawn(&m, message(2, 31));
        let c = spawn(&m, message(2, 32));
        wait_paused(&m, 3).await;
        assert_eq!(m.resume_all(Some(ExchangeId(2)), true), 2);
        assert_eq!(b.await.unwrap(), MessageResume::Drop);
        assert_eq!(c.await.unwrap(), MessageResume::Drop);
        assert_eq!(m.resume_all(None, false), 1);
        assert!(matches!(a.await.unwrap(), MessageResume::Forward { .. }));
        assert!(m.is_empty());
    }

    #[tokio::test]
    async fn abandoned_pause_cleans_up() {
        let (m, notices) = manager();
        let t = spawn(&m, message(3, 40));
        wait_paused(&m, 1).await;
        t.abort();
        let _ = t.await;
        assert!(m.is_empty());
        assert_eq!(
            notices.lock().last(),
            Some(&BreakpointNotice::MessageResumed {
                id: ExchangeId(3),
                pause_id: 40
            })
        );
        assert_eq!(
            m.resume(40, json!({})),
            Err(MessageResumeError::NotPaused(40))
        );
    }

    #[test]
    fn message_bodies() {
        let body = |v: Value| serde_json::from_value::<MessageBody>(v).unwrap();
        assert_eq!(
            body(json!({"text": "hé"})).resolve(MessageKind::Binary),
            Ok(Some((MessageKind::Text, Bytes::from("hé"))))
        );
        assert_eq!(
            body(json!({"text": "raw", "kind": "binary"})).resolve(MessageKind::Text),
            Ok(Some((MessageKind::Binary, Bytes::from_static(b"raw"))))
        );
        assert_eq!(
            body(json!({"base64": "/w=="})).resolve(MessageKind::Binary),
            Ok(Some((MessageKind::Binary, Bytes::from_static(&[0xff]))))
        );
        assert_eq!(body(json!({})).resolve(MessageKind::Text), Ok(None));
        assert!(
            body(json!({"text": ""}))
                .resolve(MessageKind::Text)
                .unwrap()
                .is_some()
        );
    }
}
