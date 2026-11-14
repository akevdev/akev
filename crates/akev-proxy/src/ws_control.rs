//! Control channels of live WebSockets: a registry of the open intercepted WebSockets (by
//! exchange id) through which messages are injected into either direction.
//!
//! Every relayed (or `ws-echo`) WebSocket registers one bounded queue per direction for its
//! lifetime. The relay task of that direction writes queued messages as soon as its outgoing
//! stream is between messages (never inside a frame or a fragmented message), masks them when
//! they go to the server, records them as `modified` stream messages and reports their `seq`.

use akev_core::{Direction, ExchangeId, MessageKind};
use bytes::Bytes;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// Messages that may wait per direction before injection is refused as busy.
const QUEUE_DEPTH: usize = 64;

/// A message to write into a live WebSocket.
pub(crate) struct Injection {
    pub kind: MessageKind,
    pub payload: Bytes,
    /// Receives the recorded message's `seq` once the frame was written.
    pub done: oneshot::Sender<u32>,
}

/// Why a message couldn't be injected into a WebSocket.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WsInjectError {
    /// No open intercepted WebSocket with this id (closed, never upgraded, or not a WebSocket),
    /// or the requested direction already finished.
    #[error("exchange {0} is not an open WebSocket")]
    NotOpen(ExchangeId),
    /// The WebSocket is answered by a `ws-echo` rule: there is no server to send to.
    #[error("this WebSocket is answered by a ws-echo rule; there is no server to send to")]
    NoServer,
    /// Only `text` and `binary` messages can be injected.
    #[error("only text and binary messages can be injected")]
    UnsupportedKind,
    /// Text messages must be valid UTF-8 (receivers fail the connection otherwise).
    #[error("text messages must be valid UTF-8")]
    InvalidText,
    /// Too many injected messages are still waiting to be written.
    #[error("too many messages are waiting to be sent on this WebSocket")]
    Busy,
    /// The WebSocket closed before the message could be written.
    #[error("the WebSocket closed before the message was sent")]
    Closed,
}

struct Queues {
    /// Client → server (`None` for `ws-echo`).
    to_server: Option<mpsc::Sender<Injection>>,
    /// Server → client.
    to_client: mpsc::Sender<Injection>,
}

/// Receiving ends of a registered WebSocket's queues.
pub(crate) struct Receivers {
    pub to_server: Option<mpsc::Receiver<Injection>>,
    pub to_client: mpsc::Receiver<Injection>,
}

/// Open WebSockets by exchange id.
#[derive(Default)]
pub(crate) struct WsRegistry {
    open: Mutex<HashMap<ExchangeId, Queues>>,
}

/// Unregisters a WebSocket when dropped.
pub(crate) struct Registration {
    registry: Arc<WsRegistry>,
    id: ExchangeId,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.registry.open.lock().remove(&self.id);
    }
}

impl WsRegistry {
    /// Register an open WebSocket. `server` = it has an upstream server (relayed, not echoed).
    pub(crate) fn register(
        self: &Arc<Self>,
        id: ExchangeId,
        server: bool,
    ) -> (Receivers, Registration) {
        let (to_client, client_rx) = mpsc::channel(QUEUE_DEPTH);
        let (to_server, server_rx) = if server {
            let (tx, rx) = mpsc::channel(QUEUE_DEPTH);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        self.open.lock().insert(
            id,
            Queues {
                to_server,
                to_client,
            },
        );
        (
            Receivers {
                to_server: server_rx,
                to_client: client_rx,
            },
            Registration {
                registry: Arc::clone(self),
                id,
            },
        )
    }

    /// Whether `id` is an open WebSocket.
    pub(crate) fn is_open(&self, id: ExchangeId) -> bool {
        self.open.lock().contains_key(&id)
    }

    /// Queue a message. `direction`: `Sent` = to the server, `Received` = to the client.
    pub(crate) fn inject(
        &self,
        id: ExchangeId,
        direction: Direction,
        kind: MessageKind,
        payload: Bytes,
    ) -> Result<oneshot::Receiver<u32>, WsInjectError> {
        match kind {
            MessageKind::Text if std::str::from_utf8(&payload).is_err() => {
                return Err(WsInjectError::InvalidText);
            }
            MessageKind::Text | MessageKind::Binary => {}
            _ => return Err(WsInjectError::UnsupportedKind),
        }
        let tx = {
            let open = self.open.lock();
            let queues = open.get(&id).ok_or(WsInjectError::NotOpen(id))?;
            match direction {
                Direction::Sent => queues.to_server.clone().ok_or(WsInjectError::NoServer)?,
                Direction::Received => queues.to_client.clone(),
            }
        };
        let (done, rx) = oneshot::channel();
        tx.try_send(Injection {
            kind,
            payload,
            done,
        })
        .map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => WsInjectError::Busy,
            mpsc::error::TrySendError::Closed(_) => WsInjectError::NotOpen(id),
        })?;
        Ok(rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn register_inject_and_unregister() {
        let registry = Arc::new(WsRegistry::default());
        let id = ExchangeId(41);
        assert_eq!(
            registry
                .inject(id, Direction::Sent, MessageKind::Text, Bytes::new())
                .unwrap_err(),
            WsInjectError::NotOpen(id)
        );
        let (mut rx, registration) = registry.register(id, true);
        assert!(registry.is_open(id));
        let done = registry
            .inject(
                id,
                Direction::Sent,
                MessageKind::Text,
                Bytes::from_static(b"hi"),
            )
            .unwrap();
        let queued = rx.to_server.as_mut().unwrap().recv().await.unwrap();
        assert_eq!(&queued.payload[..], b"hi");
        queued.done.send(5).unwrap();
        assert_eq!(done.await.unwrap(), 5);
        registry
            .inject(
                id,
                Direction::Received,
                MessageKind::Binary,
                Bytes::from_static(&[0xff]),
            )
            .unwrap();
        assert_eq!(rx.to_client.recv().await.unwrap().kind, MessageKind::Binary);
        // A direction whose relay finished refuses new messages.
        drop(rx.to_server.take());
        assert_eq!(
            registry
                .inject(id, Direction::Sent, MessageKind::Text, Bytes::new())
                .unwrap_err(),
            WsInjectError::NotOpen(id)
        );
        drop(registration);
        assert!(!registry.is_open(id));
    }

    #[test]
    fn validation_echo_and_back_pressure() {
        let registry = Arc::new(WsRegistry::default());
        let id = ExchangeId(42);
        let (_rx, _registration) = registry.register(id, false);
        let inject = |direction, kind, payload: &'static [u8]| {
            registry
                .inject(id, direction, kind, Bytes::from_static(payload))
                .map(|_| ())
        };
        assert_eq!(
            inject(Direction::Received, MessageKind::Text, b"\xff\xfe"),
            Err(WsInjectError::InvalidText)
        );
        assert_eq!(
            inject(Direction::Received, MessageKind::Ping, b""),
            Err(WsInjectError::UnsupportedKind)
        );
        assert_eq!(
            inject(Direction::Sent, MessageKind::Text, b"x"),
            Err(WsInjectError::NoServer)
        );
        for _ in 0..QUEUE_DEPTH {
            inject(Direction::Received, MessageKind::Text, b"x").unwrap();
        }
        assert_eq!(
            inject(Direction::Received, MessageKind::Text, b"x"),
            Err(WsInjectError::Busy)
        );
    }
}
