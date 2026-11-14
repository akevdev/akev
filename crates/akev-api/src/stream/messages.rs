//! Server → client and client → server messages of the live stream (`GET /api/stream`).

use crate::app::{Status, StatusStats};
use crate::stream::matchset::FilterDelta;
use akev_core::interceptor::InterceptorInfo;
use akev_core::rules::{RuleSet, RuleStats};
use akev_core::settings::Settings;
use akev_core::{ExchangeId, ExchangeSummary, PausedExchange, PausedMessage, StreamMessage};
use axum::extract::ws::Utf8Bytes;
use serde::{Deserialize, Serialize};

/// A server → client message. Borrowing variants avoid cloning large payloads.
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ServerMessage<'a> {
    /// A full resync. `summaries` are the newest exchanges (id order); `remaining` older
    /// ones follow in `hello-more` chunks.
    Hello {
        status: &'a Status,
        summaries: &'a [ExchangeSummary],
        remaining: usize,
        paused: &'a [PausedExchange],
        rules: &'a RuleSet,
        settings: &'a Settings,
        /// WebSocket messages paused at `ws-breakpoint` rules.
        #[serde(rename = "pausedMessages")]
        paused_messages: &'a [PausedMessage],
    },
    /// The next older chunk of the `hello` snapshot (id order, all older than anything sent
    /// before; current at send time, deleted ones left out). `remaining` older ones follow.
    HelloMore {
        summaries: &'a [ExchangeSummary],
        remaining: usize,
    },
    Exchanges {
        added: &'a [ExchangeSummary],
        updated: &'a [ExchangeSummary],
        #[serde(skip_serializing_if = "Option::is_none")]
        filtered: Option<&'a FilterDelta>,
    },
    Removed {
        ids: &'a [ExchangeId],
    },
    Cleared,
    Messages {
        id: ExchangeId,
        messages: &'a [StreamMessage],
    },
    Paused {
        exchange: &'a PausedExchange,
    },
    Resumed {
        id: ExchangeId,
    },
    Rules {
        rules: &'a RuleSet,
    },
    RuleStats {
        stats: &'a [RuleStats],
    },
    Settings {
        settings: &'a Settings,
    },
    Interceptors {
        interceptors: &'a [InterceptorInfo],
    },
    Stats {
        stats: &'a StatusStats,
    },
    Notice {
        level: &'a str,
        message: &'a str,
    },
    Capture {
        recording: bool,
    },
    /// The plan / licence state changed (also in `Status.license`).
    License {
        license: &'a akev_core::license::LicenseStatus,
    },
    /// The autosaved session finished loading at startup (also in `Status.restoredSession`).
    SessionRestored {
        session: &'a crate::sessions::RestoredSession,
    },
    FilterResult {
        seq: u64,
        query: &'a str,
        ids: Option<&'a [ExchangeId]>,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<&'a akev_filter::ParseError>,
    },
    Pong,
    /// A WebSocket message paused at a `ws-breakpoint`.
    MessagePaused {
        message: &'a PausedMessage,
    },
    /// A paused WebSocket message was forwarded, dropped or abandoned.
    MessageResumed {
        id: ExchangeId,
        #[serde(rename = "pauseId")]
        pause_id: u64,
    },
    /// A device on another machine connected to the proxy for the first time this run (the UI
    /// offers to block it: `proxy.access.block`).
    ClientConnected {
        address: &'a str,
    },
}

impl ServerMessage<'_> {
    /// Serialize to a WebSocket text payload.
    pub fn to_text(&self) -> Option<Utf8Bytes> {
        match serde_json::to_string(self) {
            Ok(s) => Some(Utf8Bytes::from(s)),
            Err(e) => {
                tracing::error!("failed to serialize stream message: {e}");
                None
            }
        }
    }
}

/// A client → server message.
#[derive(Debug, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum ClientMessage {
    Ping,
    SetFilter {
        #[serde(default)]
        query: String,
        #[serde(default)]
        seq: u64,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::batch::tests::summary;
    use akev_core::ExchangeState;
    use serde_json::{Value, json};

    fn parse(m: &ServerMessage<'_>) -> Value {
        serde_json::from_str(m.to_text().unwrap().as_str()).unwrap()
    }

    #[test]
    fn server_message_shapes() {
        assert_eq!(parse(&ServerMessage::Pong), json!({"type": "pong"}));
        assert_eq!(parse(&ServerMessage::Cleared), json!({"type": "cleared"}));
        assert_eq!(
            parse(&ServerMessage::Removed {
                ids: &[ExchangeId(1), ExchangeId(2)]
            }),
            json!({"type": "removed", "ids": [1, 2]})
        );
        assert_eq!(
            parse(&ServerMessage::Capture { recording: false }),
            json!({"type": "capture", "recording": false})
        );
        assert_eq!(
            parse(&ServerMessage::Notice {
                level: "warning",
                message: "CA expires"
            }),
            json!({"type": "notice", "level": "warning", "message": "CA expires"})
        );
        assert_eq!(
            parse(&ServerMessage::RuleStats { stats: &[] }),
            json!({"type": "rule-stats", "stats": []})
        );
        assert_eq!(
            parse(&ServerMessage::Resumed { id: ExchangeId(4) }),
            json!({"type": "resumed", "id": 4})
        );
        let s = [summary(9, ExchangeState::Complete, Some(200))];
        let v = parse(&ServerMessage::HelloMore {
            summaries: &s,
            remaining: 12,
        });
        assert_eq!(v["type"], "hello-more");
        assert_eq!(v["summaries"][0]["id"], 9);
        assert_eq!(v["remaining"], 12);
    }

    #[test]
    fn exchanges_message_carries_filtered_only_when_set() {
        let s = [summary(1, ExchangeState::Complete, Some(200))];
        let v = parse(&ServerMessage::Exchanges {
            added: &s,
            updated: &[],
            filtered: None,
        });
        assert_eq!(v["type"], "exchanges");
        assert_eq!(v["added"][0]["id"], 1);
        assert_eq!(v["added"][0]["status"], 200);
        assert!(v.get("filtered").is_none());
        let d = FilterDelta {
            add: vec![ExchangeId(1)],
            remove: vec![ExchangeId(3)],
        };
        let v = parse(&ServerMessage::Exchanges {
            added: &s,
            updated: &[],
            filtered: Some(&d),
        });
        assert_eq!(v["filtered"], json!({"add": [1], "remove": [3]}));
    }

    #[test]
    fn filter_result_shape() {
        let v = parse(&ServerMessage::FilterResult {
            seq: 3,
            query: "status:2xx",
            ids: None,
            error: None,
        });
        assert_eq!(
            v,
            json!({"type": "filter-result", "seq": 3, "query": "status:2xx", "ids": null})
        );
        let err = akev_filter::ParseError {
            message: "unknown field".into(),
            start: 0,
            end: 4,
        };
        let v = parse(&ServerMessage::FilterResult {
            seq: 4,
            query: "nope:1",
            ids: Some(&[ExchangeId(7)]),
            error: Some(&err),
        });
        assert_eq!(v["ids"], json!([7]));
        assert_eq!(
            v["error"],
            json!({"message": "unknown field", "start": 0, "end": 4})
        );
    }

    #[test]
    fn client_messages() {
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"ping"}"#).unwrap(),
            ClientMessage::Ping
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(
                r#"{"type":"set-filter","query":"method:get","seq":2}"#
            )
            .unwrap(),
            ClientMessage::SetFilter {
                query: "method:get".into(),
                seq: 2
            }
        );
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"set-filter","query":""}"#).unwrap(),
            ClientMessage::SetFilter {
                query: String::new(),
                seq: 0
            }
        );
        assert!(serde_json::from_str::<ClientMessage>(r#"{"type":"unknown"}"#).is_err());
    }

    #[test]
    fn paused_message_notices() {
        let message = PausedMessage {
            id: ExchangeId(3),
            pause_id: 9,
            direction: akev_core::Direction::Received,
            kind: akev_core::MessageKind::Binary,
            payload: bytes::Bytes::from_static(&[1]),
            at: akev_core::UnixMicros(5),
            rule: "r".into(),
        };
        assert_eq!(
            parse(&ServerMessage::MessagePaused { message: &message }),
            json!({"type": "message-paused", "message": {
                "id": 3, "pauseId": 9, "direction": "received", "kind": "binary",
                "payload": "AQ==", "at": 5, "rule": "r"
            }})
        );
        assert_eq!(
            parse(&ServerMessage::MessageResumed {
                id: ExchangeId(3),
                pause_id: 9
            }),
            json!({"type": "message-resumed", "id": 3, "pauseId": 9})
        );
    }
}
