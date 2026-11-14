//! Breakpoints: pausing an in-flight exchange so the user can inspect & edit it.
//!
//! Flow: the rules engine decides an exchange should break → the proxy builds an editable copy
//! and awaits [`BreakpointHandler::on_request`] / [`BreakpointHandler::on_response`] → the
//! handler (implemented in the API layer) notifies the UI and waits for the user's decision.

use crate::http::{Headers, RequestHead, ResponseHead};
use crate::ids::ExchangeId;
use async_trait::async_trait;
use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Editable request at a breakpoint. Body is decoded (content-encoding removed) so the user
/// edits plain content; the proxy re-encodes / fixes Content-Length when forwarding.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableRequest {
    pub method: String,
    pub url: String,
    pub headers: Headers,
    #[serde(with = "base64_bytes")]
    pub body: Bytes,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditableResponse {
    pub status: u16,
    pub status_message: String,
    pub headers: Headers,
    #[serde(with = "base64_bytes")]
    pub body: Bytes,
}

/// User decision for a paused request.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum RequestResume {
    /// Forward (possibly edited) request upstream.
    Continue { request: EditableRequest },
    /// Don't forward; reply with this response instead.
    Respond { response: EditableResponse },
    /// Close the client connection.
    Close,
    /// Reset (RST) the client connection.
    Reset,
}

/// User decision for a paused response.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum ResponseResume {
    Continue { response: EditableResponse },
    Close,
    Reset,
}

/// Snapshot shown to the user while paused.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedExchange {
    pub id: ExchangeId,
    pub stage: crate::exchange::BreakpointStage,
    /// When it started waiting. Lets clients show the wait even for exchanges that aren't in
    /// the traffic list (recording paused, or the capture settings ignore the host).
    #[serde(default)]
    pub paused_at: crate::time::UnixMicros,
    pub request: EditableRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response: Option<EditableResponse>,
}

#[async_trait]
pub trait BreakpointHandler: Send + Sync + 'static {
    async fn on_request(
        &self,
        id: ExchangeId,
        head: &RequestHead,
        request: EditableRequest,
    ) -> RequestResume;
    async fn on_response(
        &self,
        id: ExchangeId,
        request: &RequestHead,
        head: &ResponseHead,
        response: EditableResponse,
    ) -> ResponseResume;
    /// A WebSocket message paused by a `ws-breakpoint` rule. Resolves with the user's decision;
    /// dropping the future (connection closed) abandons the pause. Default: forward unchanged.
    async fn on_message(&self, message: PausedMessage) -> MessageResume {
        MessageResume::Forward {
            kind: message.kind,
            payload: message.payload,
        }
    }
}

/// A WebSocket message held at a `ws-breakpoint` (it reaches its destination only once
/// resumed). Only this message's direction waits; the other one keeps flowing.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PausedMessage {
    /// The WebSocket exchange.
    pub id: ExchangeId,
    /// Identifies this pause (unique per proxy run); resume with it.
    pub pause_id: u64,
    pub direction: crate::exchange::Direction,
    /// `text` or `binary`.
    pub kind: crate::exchange::MessageKind,
    /// The complete, unmasked payload.
    #[serde(with = "base64_bytes")]
    pub payload: Bytes,
    /// When the message was paused.
    pub at: crate::time::UnixMicros,
    /// The `ws-breakpoint` rule that paused it.
    pub rule: crate::ids::RuleId,
}

/// User decision for a paused WebSocket message.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "kebab-case")]
pub enum MessageResume {
    /// Send the (possibly edited) message on to its destination.
    Forward {
        /// `text` or `binary`.
        kind: crate::exchange::MessageKind,
        #[serde(with = "base64_bytes")]
        payload: Bytes,
    },
    /// Discard the message.
    Drop,
}

/// Handler that never pauses (immediately continues unchanged).
pub struct NoBreakpoints;

#[async_trait]
impl BreakpointHandler for NoBreakpoints {
    async fn on_request(
        &self,
        _id: ExchangeId,
        _head: &RequestHead,
        request: EditableRequest,
    ) -> RequestResume {
        RequestResume::Continue { request }
    }
    async fn on_response(
        &self,
        _id: ExchangeId,
        _request: &RequestHead,
        _head: &ResponseHead,
        response: EditableResponse,
    ) -> ResponseResume {
        ResponseResume::Continue { response }
    }
}

/// Serde helper: `Bytes` <-> base64 string.
pub mod base64_bytes {
    use base64::Engine;
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.as_bytes())
            .map(Bytes::from)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::exchange::{Direction, MessageKind};

    fn paused() -> PausedMessage {
        PausedMessage {
            id: ExchangeId(7),
            pause_id: 3,
            direction: Direction::Sent,
            kind: MessageKind::Text,
            payload: Bytes::from_static(b"hello"),
            at: crate::time::UnixMicros(1_000),
            rule: "r1".into(),
        }
    }

    #[test]
    fn paused_message_json_shape() {
        let v = serde_json::to_value(paused()).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "id": 7, "pauseId": 3, "direction": "sent", "kind": "text",
                "payload": "aGVsbG8=", "at": 1000, "rule": "r1"
            })
        );
        let resume: MessageResume =
            serde_json::from_str(r#"{"action":"forward","kind":"binary","payload":"AQI="}"#)
                .unwrap();
        assert_eq!(
            resume,
            MessageResume::Forward {
                kind: MessageKind::Binary,
                payload: Bytes::from_static(&[1, 2]),
            }
        );
        assert_eq!(
            serde_json::from_str::<MessageResume>(r#"{"action":"drop"}"#).unwrap(),
            MessageResume::Drop
        );
    }

    #[test]
    fn default_handler_forwards_messages_unchanged() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let decision = rt.block_on(NoBreakpoints.on_message(paused()));
        assert_eq!(
            decision,
            MessageResume::Forward {
                kind: MessageKind::Text,
                payload: Bytes::from_static(b"hello"),
            }
        );
    }
}
