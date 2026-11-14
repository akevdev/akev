//! The byte-bounded queue between the proxy and the store.
//!
//! Every captured body (up to `capture.maxBodyBytes`, 32 MiB by default) travels from the
//! proxy to the store as part of a [`ProxyEvent`]. An unbounded channel would let a store
//! that falls behind (a burst of large downloads, a slow disk while spilling) grow memory
//! without limit, so the queue counts the bytes it holds ([`ProxyEvent::approx_bytes`]):
//!
//! * Below capacity, events go straight through.
//! * When an event doesn't fit, its body bytes are dropped (sizes kept, body marked
//!   truncated), a [`ProxyEvent::Notice`] tells the store why, and the drop is counted.
//!   Lifecycle events always go through, so exchanges never stay half-recorded.
//! * Stream messages are dropped entirely once the queue holds twice its capacity (a flood
//!   of tiny WebSocket frames must not grow it either).
//! * The proxy waits on [`QueueState::wait_for_space`] (briefly) before starting a new
//!   exchange while the queue is at least half full: backpressure that slows new work
//!   instead of dropping its body, for a little while.
//!
//! The consumer releases the bytes of the events it has applied with
//! [`QueueReceiver::release`].

use crate::events::{EventSink, ProxyEvent, StrippedBody};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::{Notify, mpsc};

/// Shared state of an event queue: occupancy, capacity and drop counters.
#[derive(Debug)]
pub struct QueueState {
    queued: AtomicU64,
    capacity: AtomicU64,
    dropped_bodies: AtomicU64,
    dropped_messages: AtomicU64,
    space: Notify,
}

impl QueueState {
    fn new(capacity: u64) -> Self {
        QueueState {
            queued: AtomicU64::new(0),
            capacity: AtomicU64::new(capacity.max(crate::settings::MIN_EVENT_QUEUE_BYTES)),
            dropped_bodies: AtomicU64::new(0),
            dropped_messages: AtomicU64::new(0),
            space: Notify::new(),
        }
    }

    /// Bytes currently queued.
    pub fn queued_bytes(&self) -> u64 {
        self.queued.load(Ordering::Relaxed)
    }

    /// Capacity in bytes.
    pub fn capacity(&self) -> u64 {
        self.capacity.load(Ordering::Relaxed)
    }

    /// Change the capacity (raised to [`crate::settings::MIN_EVENT_QUEUE_BYTES`]).
    pub fn set_capacity(&self, bytes: u64) {
        self.capacity.store(
            bytes.max(crate::settings::MIN_EVENT_QUEUE_BYTES),
            Ordering::Relaxed,
        );
        self.space.notify_waiters();
    }

    /// Bodies (and message payloads) not recorded because the queue was full.
    pub fn dropped_bodies(&self) -> u64 {
        self.dropped_bodies.load(Ordering::Relaxed)
    }

    /// Stream messages not recorded at all because the queue was far over capacity.
    pub fn dropped_messages(&self) -> u64 {
        self.dropped_messages.load(Ordering::Relaxed)
    }

    /// Whether the queue is at least half full: a new exchange's body might not fit, so
    /// the proxy holds back new exchanges (briefly) until the store catches up.
    pub fn is_congested(&self) -> bool {
        self.queued_bytes() >= self.capacity() / 2
    }

    /// Wait until the queue is no longer congested, at most `max`. Returns whether it is.
    pub async fn wait_for_space(&self, max: Duration) -> bool {
        let deadline = tokio::time::Instant::now() + max;
        loop {
            let notified = self.space.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.is_congested() {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return !self.is_congested();
            }
        }
    }

    fn release(&self, bytes: u64) {
        let mut current = self.queued.load(Ordering::Acquire);
        loop {
            let next = current.saturating_sub(bytes);
            match self.queued.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        if current.saturating_sub(bytes) < self.capacity() / 2 {
            self.space.notify_waiters();
        }
    }
}

/// The sending half: an [`EventSink`] for the proxy.
pub struct QueueSink {
    tx: mpsc::UnboundedSender<(ProxyEvent, u64)>,
    state: Arc<QueueState>,
}

/// The receiving half, drained by the store.
pub struct QueueReceiver {
    rx: mpsc::UnboundedReceiver<(ProxyEvent, u64)>,
    state: Arc<QueueState>,
}

/// Create a queue holding up to `capacity` bytes of events.
pub fn event_queue(capacity: u64) -> (QueueSink, QueueReceiver) {
    let (tx, rx) = mpsc::unbounded_channel();
    let state = Arc::new(QueueState::new(capacity));
    (
        QueueSink {
            tx,
            state: Arc::clone(&state),
        },
        QueueReceiver { rx, state },
    )
}

impl Clone for QueueSink {
    fn clone(&self) -> Self {
        QueueSink {
            tx: self.tx.clone(),
            state: Arc::clone(&self.state),
        }
    }
}

impl QueueSink {
    /// The queue's shared state.
    pub fn state(&self) -> &Arc<QueueState> {
        &self.state
    }

    /// Whether the receiver is gone.
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }

    fn push(&self, event: ProxyEvent, cost: u64) {
        self.state.queued.fetch_add(cost, Ordering::AcqRel);
        if self.tx.send((event, cost)).is_err() {
            // Receiver gone: nothing will release these bytes.
            self.state.release(cost);
        }
    }
}

fn drop_notice(stripped: StrippedBody) -> (&'static str, Option<crate::body::Side>) {
    use crate::body::Side;
    match stripped {
        StrippedBody::Body(Side::Request) => (
            "Request body not recorded: capture was overloaded",
            Some(Side::Request),
        ),
        StrippedBody::Body(Side::Response) => (
            "Response body not recorded: capture was overloaded",
            Some(Side::Response),
        ),
        StrippedBody::OriginalResponse => (
            "Original response body not recorded: capture was overloaded",
            None,
        ),
        StrippedBody::MessagePayload => (
            "Some message payloads not recorded: capture was overloaded",
            None,
        ),
    }
}

impl EventSink for QueueSink {
    fn emit(&self, mut event: ProxyEvent) {
        let cost = event.approx_bytes();
        let queued = self.state.queued_bytes();
        let capacity = self.state.capacity();
        if queued.saturating_add(cost) <= capacity {
            self.push(event, cost);
            return;
        }
        if matches!(event, ProxyEvent::StreamMessage { .. }) && queued >= capacity * 2 {
            self.state.dropped_messages.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let id = event.exchange_id();
        let stripped = event.strip_body();
        let cost = event.approx_bytes();
        self.push(event, cost);
        if let Some(stripped) = stripped {
            self.state.dropped_bodies.fetch_add(1, Ordering::Relaxed);
            let (message, dropped) = drop_notice(stripped);
            let notice = ProxyEvent::Notice {
                id,
                message: message.to_string(),
                dropped,
            };
            let cost = notice.approx_bytes();
            self.push(notice, cost);
        }
    }

    fn backpressure(&self) -> Option<Arc<QueueState>> {
        Some(Arc::clone(&self.state))
    }
}

impl QueueReceiver {
    /// The queue's shared state.
    pub fn state(&self) -> &Arc<QueueState> {
        &self.state
    }

    /// Receive up to `limit` events into `out` (waiting for at least one). Returns the
    /// number received (0 once every sender is gone) and their queued bytes, to pass to
    /// [`QueueReceiver::release`] once they're applied.
    pub async fn recv_many(&mut self, out: &mut Vec<ProxyEvent>, limit: usize) -> (usize, u64) {
        let mut batch = Vec::with_capacity(limit.min(64));
        let n = self.rx.recv_many(&mut batch, limit).await;
        let mut bytes = 0u64;
        for (event, cost) in batch {
            bytes += cost;
            out.push(event);
        }
        (n, bytes)
    }

    /// Like [`QueueReceiver::recv_many`] without waiting (blocking callers).
    pub fn try_recv_many(&mut self, out: &mut Vec<ProxyEvent>, limit: usize) -> (usize, u64) {
        let mut n = 0;
        let mut bytes = 0u64;
        while n < limit {
            match self.rx.try_recv() {
                Ok((event, cost)) => {
                    bytes += cost;
                    out.push(event);
                    n += 1;
                }
                Err(_) => break,
            }
        }
        (n, bytes)
    }

    /// Wait for the next event (`None` once every sender is gone), for blocking consumers.
    pub fn blocking_recv(&mut self) -> Option<(ProxyEvent, u64)> {
        self.rx.blocking_recv()
    }

    /// Give back the bytes of applied events, waking proxy tasks waiting for space.
    pub fn release(&self, bytes: u64) {
        self.state.release(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::{CapturedBody, Side};
    use crate::ids::ExchangeId;
    use bytes::Bytes;

    fn completed(id: u64, len: usize) -> ProxyEvent {
        ProxyEvent::ResponseCompleted {
            id: ExchangeId(id),
            body: CapturedBody::complete(Bytes::from(vec![b'x'; len])),
            trailers: None,
            at: 1.0,
        }
    }

    #[tokio::test]
    async fn bodies_beyond_capacity_are_dropped_and_counted() {
        let cap = crate::settings::MIN_EVENT_QUEUE_BYTES;
        let (sink, mut rx) = event_queue(cap);
        let big = usize::try_from(cap / 2).unwrap();
        sink.emit(completed(1, big));
        sink.emit(completed(2, big));
        assert!(sink.state().queued_bytes() <= cap + 2048);
        let mut out = Vec::new();
        let (n, bytes) = rx.try_recv_many(&mut out, 16);
        // Event 1 kept its body; event 2 lost it and is followed by a notice.
        assert_eq!(n, 3);
        match &out[0] {
            ProxyEvent::ResponseCompleted { body, .. } => assert_eq!(body.bytes.len(), big),
            other => panic!("{other:?}"),
        }
        match &out[1] {
            ProxyEvent::ResponseCompleted { body, .. } => {
                assert!(body.bytes.is_empty());
                assert!(body.truncated);
                assert_eq!(body.wire_size, big as u64);
            }
            other => panic!("{other:?}"),
        }
        match &out[2] {
            ProxyEvent::Notice {
                id,
                dropped,
                message,
            } => {
                assert_eq!(*id, ExchangeId(2));
                assert_eq!(*dropped, Some(Side::Response));
                assert!(message.contains("overloaded"));
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(sink.state().dropped_bodies(), 1);
        rx.release(bytes);
        assert_eq!(sink.state().queued_bytes(), 0);
    }

    #[tokio::test]
    async fn waiters_wake_when_space_frees() {
        let cap = crate::settings::MIN_EVENT_QUEUE_BYTES;
        let (sink, mut rx) = event_queue(cap);
        sink.emit(completed(1, usize::try_from(cap / 2).unwrap() - 1024));
        assert!(!sink.state().is_congested(), "under half full");
        sink.emit(completed(2, 4096));
        assert!(sink.state().is_congested());
        // Times out while nothing drains.
        assert!(!sink.state().wait_for_space(Duration::from_millis(20)).await);
        let state = Arc::clone(sink.state());
        let waiter =
            tokio::spawn(async move { state.wait_for_space(Duration::from_secs(5)).await });
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut out = Vec::new();
        let (_, bytes) = rx.recv_many(&mut out, 64).await;
        rx.release(bytes);
        assert!(waiter.await.unwrap());
    }

    #[test]
    fn message_floods_are_dropped_past_twice_the_capacity() {
        let cap = crate::settings::MIN_EVENT_QUEUE_BYTES;
        let (sink, _rx) = event_queue(cap);
        let message = |id| ProxyEvent::StreamMessage {
            id: ExchangeId(id),
            message: crate::exchange::StreamMessage {
                seq: 0,
                direction: crate::exchange::Direction::Received,
                kind: crate::exchange::MessageKind::Text,
                at: crate::time::UnixMicros::now(),
                size: 10,
                event: None,
                event_id: None,
                modified: false,
            },
            payload: Bytes::from_static(b"0123456789"),
        };
        let mut sent = 0;
        while sink.state().queued_bytes() < cap * 2 + 4096 && sent < 1_000_000 {
            sink.emit(message(1));
            sent += 1;
        }
        let queued = sink.state().queued_bytes();
        for _ in 0..1000 {
            sink.emit(message(1));
        }
        assert_eq!(sink.state().queued_bytes(), queued, "no growth past 2x");
        assert!(sink.state().dropped_messages() >= 1000);
    }
}
