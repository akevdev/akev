//! The proxy → store event sink with the "recording" switch (`PUT /api/capture`).
//!
//! When recording is turned off, traffic keeps flowing (and rules still apply) but new
//! exchanges are not stored. Exchanges already in flight when recording stops keep receiving
//! their remaining events, so they complete normally instead of being left "pending" forever.
//!
//! Implementation: exchange ids are allocated monotonically, so each off-period is a half-open
//! id range `[fence_off, fence_on)`. A fence is obtained by allocating an id
//! ([`ExchangeId::next`]), which leaves a harmless gap in the id sequence. The hot path is a
//! single atomic load while nothing was ever paused, and a lock-free scan of a tiny vector
//! afterwards.

use akev_core::{EventSink, ExchangeId, ProxyEvent};
use arc_swap::ArcSwap;
use parking_lot::Mutex;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Event sink forwarding to the store unless recording is paused.
pub struct RecordingSink {
    inner: Arc<dyn EventSink>,
    /// `true` while recording and no off-range was ever recorded (fast path).
    pass_all: AtomicBool,
    recording: AtomicBool,
    /// Half-open id ranges that are not recorded; the last one may be open (`u64::MAX`).
    dropped: ArcSwap<Vec<(u64, u64)>>,
    toggle: Mutex<()>,
}

impl RecordingSink {
    pub fn new(inner: Arc<dyn EventSink>) -> Self {
        RecordingSink {
            inner,
            pass_all: AtomicBool::new(true),
            recording: AtomicBool::new(true),
            dropped: ArcSwap::from_pointee(Vec::new()),
            toggle: Mutex::new(()),
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.load(Ordering::Acquire)
    }

    /// Switch recording on/off. Returns `true` if the state changed.
    pub fn set_recording(&self, on: bool) -> bool {
        let _guard = self.toggle.lock();
        if self.recording.load(Ordering::Acquire) == on {
            return false;
        }
        let fence = ExchangeId::next().0;
        let mut ranges = (**self.dropped.load()).clone();
        if on {
            if let Some(last) = ranges.last_mut()
                && last.1 == u64::MAX
            {
                last.1 = fence;
            }
            // Drop empty ranges (no exchange could have been allocated inside them).
            ranges.retain(|(s, e)| e > s);
        } else {
            ranges.push((fence, u64::MAX));
            self.pass_all.store(false, Ordering::Release);
        }
        self.dropped.store(Arc::new(ranges));
        self.recording.store(on, Ordering::Release);
        true
    }

    /// Whether events of exchange `id` reach the store, i.e. it did not start while
    /// recording was paused.
    pub fn is_recorded(&self, id: ExchangeId) -> bool {
        self.pass_all.load(Ordering::Acquire) || !self.is_dropped(id)
    }

    fn is_dropped(&self, id: ExchangeId) -> bool {
        let ranges = self.dropped.load();
        ranges
            .iter()
            .any(|&(start, end)| id.0 >= start && id.0 < end)
    }
}

impl EventSink for RecordingSink {
    fn emit(&self, event: ProxyEvent) {
        if self.pass_all.load(Ordering::Acquire) || !self.is_dropped(event.exchange_id()) {
            self.inner.emit(event);
        }
    }

    fn backpressure(&self) -> Option<Arc<akev_core::queue::QueueState>> {
        self.inner.backpressure()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use akev_core::Direction;

    fn ev(id: ExchangeId) -> ProxyEvent {
        ProxyEvent::StreamClosed {
            id,
            code: None,
            reason: None,
            by: Direction::Sent,
            at: 0.0,
        }
    }

    fn collecting() -> (Arc<Mutex<Vec<u64>>>, Arc<dyn EventSink>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s = seen.clone();
        let sink: Arc<dyn EventSink> =
            Arc::new(move |e: ProxyEvent| s.lock().push(e.exchange_id().0));
        (seen, sink)
    }

    #[test]
    fn drops_only_exchanges_started_while_paused() {
        let (seen, inner) = collecting();
        let sink = RecordingSink::new(inner);
        let before = ExchangeId::next();
        sink.emit(ev(before));
        assert!(sink.set_recording(false));
        assert!(!sink.set_recording(false), "no-op toggle");
        assert!(!sink.is_recording());
        let during = ExchangeId::next();
        sink.emit(ev(during));
        // The in-flight exchange keeps flowing so it can complete.
        sink.emit(ev(before));
        assert!(sink.set_recording(true));
        let after = ExchangeId::next();
        sink.emit(ev(after));
        // Late events of an exchange that started while paused are still dropped.
        sink.emit(ev(during));
        assert_eq!(*seen.lock(), vec![before.0, before.0, after.0]);
        assert!(sink.is_recorded(before));
        assert!(!sink.is_recorded(during));
        assert!(sink.is_recorded(after));
    }

    #[test]
    fn everything_is_recorded_until_paused() {
        let (_, inner) = collecting();
        let sink = RecordingSink::new(inner);
        assert!(sink.is_recorded(ExchangeId::next()));
        sink.set_recording(false);
        assert!(!sink.is_recorded(ExchangeId::next()));
    }

    #[test]
    fn multiple_pauses() {
        let (seen, inner) = collecting();
        let sink = RecordingSink::new(inner);
        sink.set_recording(false);
        let a = ExchangeId::next();
        sink.set_recording(true);
        let b = ExchangeId::next();
        sink.set_recording(false);
        let c = ExchangeId::next();
        sink.set_recording(true);
        let d = ExchangeId::next();
        for id in [a, b, c, d] {
            sink.emit(ev(id));
        }
        assert_eq!(*seen.lock(), vec![b.0, d.0]);
    }

    #[test]
    fn backpressure_of_the_store_queue_shows_through() {
        let (_, plain) = collecting();
        assert!(RecordingSink::new(plain).backpressure().is_none());
        let (queue, _rx) = akev_core::queue::event_queue(1 << 20);
        let state = Arc::clone(queue.state());
        let sink = RecordingSink::new(Arc::new(queue));
        let seen = sink.backpressure().expect("bounded inner sink");
        assert!(Arc::ptr_eq(&seen, &state));
    }
}
