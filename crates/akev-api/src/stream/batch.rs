//! Coalescing of store events into `exchanges` / `messages` stream batches.
//!
//! Between two flushes (~33 ms) any number of store events may arrive for the same exchange;
//! each id is emitted at most once per batch, with its latest summary, in `added` if the
//! client hasn't seen it yet and in `updated` otherwise. Removals drop pending entries so a
//! deleted exchange never reappears after its `removed` message, and are themselves sent once
//! per batch (with `maxExchanges` reached, every new exchange evicts one: one `removed`
//! message per flush rather than one per exchange).

use akev_core::{ExchangeId, ExchangeSummary, StreamMessage};
use std::collections::{BTreeMap, HashMap};

#[derive(Debug)]
struct PendingExchange {
    summary: ExchangeSummary,
    /// The client hasn't been told about this exchange yet.
    added: bool,
}

/// One flushed batch.
#[derive(Debug, Default, PartialEq)]
pub struct Batch {
    /// New exchanges, ascending id.
    pub added: Vec<ExchangeSummary>,
    /// Changed exchanges the client already knows, ascending id.
    pub updated: Vec<ExchangeSummary>,
    /// New stream messages per exchange (ascending id; messages in arrival order).
    pub messages: Vec<(ExchangeId, Vec<StreamMessage>)>,
    /// Deleted / evicted exchanges (sent before the rest of the batch).
    pub removed: Vec<ExchangeId>,
}

impl Batch {
    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.added.is_empty()
            && self.updated.is_empty()
            && self.messages.is_empty()
            && self.removed.is_empty()
    }
}

/// Accumulates store events until the next flush.
#[derive(Debug, Default)]
pub struct Batcher {
    exchanges: HashMap<ExchangeId, PendingExchange>,
    messages: BTreeMap<ExchangeId, Vec<StreamMessage>>,
    removed: Vec<ExchangeId>,
}

impl Batcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// A new exchange. `already_known` = the client received it in its snapshot (an event
    /// that raced the `hello`), so it's reported as an update.
    pub fn added(&mut self, summary: ExchangeSummary, already_known: bool) {
        let id = summary.id;
        match self.exchanges.get_mut(&id) {
            Some(p) => p.summary = summary,
            None => {
                self.exchanges.insert(
                    id,
                    PendingExchange {
                        summary,
                        added: !already_known,
                    },
                );
            }
        }
    }

    /// An exchange changed.
    pub fn updated(&mut self, summary: ExchangeSummary) {
        let id = summary.id;
        match self.exchanges.get_mut(&id) {
            Some(p) => p.summary = summary,
            None => {
                self.exchanges.insert(
                    id,
                    PendingExchange {
                        summary,
                        added: false,
                    },
                );
            }
        }
    }

    /// Exchanges were deleted: forget anything pending for them and report them with the
    /// next batch.
    pub fn removed(&mut self, ids: &[ExchangeId]) {
        for id in ids {
            self.exchanges.remove(id);
            self.messages.remove(id);
        }
        self.removed.extend_from_slice(ids);
    }

    /// Everything was cleared.
    pub fn cleared(&mut self) {
        self.exchanges.clear();
        self.messages.clear();
        self.removed.clear();
    }

    /// A new stream message on an exchange.
    pub fn message(&mut self, id: ExchangeId, message: StreamMessage) {
        self.messages.entry(id).or_default().push(message);
    }

    pub fn is_empty(&self) -> bool {
        self.exchanges.is_empty() && self.messages.is_empty() && self.removed.is_empty()
    }

    /// Drop everything pending (used before a full resync).
    pub fn reset(&mut self) {
        self.cleared();
    }

    /// Take the pending batch.
    pub fn take(&mut self) -> Batch {
        let mut added = Vec::new();
        let mut updated = Vec::new();
        for (_, p) in self.exchanges.drain() {
            if p.added {
                added.push(p.summary);
            } else {
                updated.push(p.summary);
            }
        }
        added.sort_unstable_by_key(|s| s.id);
        updated.sort_unstable_by_key(|s| s.id);
        let messages = std::mem::take(&mut self.messages).into_iter().collect();
        Batch {
            added,
            updated,
            messages,
            removed: std::mem::take(&mut self.removed),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use akev_core::*;

    pub(crate) fn summary(id: u64, state: ExchangeState, status: Option<u16>) -> ExchangeSummary {
        ExchangeSummary {
            id: ExchangeId(id),
            kind: ExchangeKind::Http,
            state,
            started_at: 0.0,
            method: "GET".into(),
            scheme: "http".into(),
            host: "example.com".into(),
            port: 80,
            path: format!("/{id}"),
            http_version: HttpVersion::Http11,
            status,
            content_kind: ContentKind::Unknown,
            content_type: None,
            request_size: 0,
            response_size: 0,
            duration: None,
            error: None,
            paused: None,
            source: None,
            rule: None,
            modified: false,
            mocked: false,
            pinned: false,
            color: None,
            message_count: 0,
            has_note: false,
            llm: None,
        }
    }

    fn msg(seq: u32) -> StreamMessage {
        StreamMessage {
            seq,
            direction: Direction::Received,
            kind: MessageKind::Text,
            at: UnixMicros(0),
            size: 1,
            event: None,
            event_id: None,
            modified: false,
        }
    }

    #[test]
    fn coalesces_each_id_once_with_latest_summary() {
        let mut b = Batcher::new();
        b.added(summary(2, ExchangeState::Requesting, None), false);
        b.updated(summary(2, ExchangeState::Waiting, None));
        b.updated(summary(2, ExchangeState::Complete, Some(200)));
        b.added(summary(1, ExchangeState::Requesting, None), false);
        b.updated(summary(7, ExchangeState::Complete, Some(404)));
        let batch = b.take();
        assert_eq!(
            batch.added.iter().map(|s| s.id.0).collect::<Vec<_>>(),
            vec![1, 2]
        );
        assert_eq!(batch.added[1].state, ExchangeState::Complete);
        assert_eq!(batch.added[1].status, Some(200));
        assert_eq!(
            batch.updated.iter().map(|s| s.id.0).collect::<Vec<_>>(),
            vec![7]
        );
        assert!(b.is_empty());
        assert!(b.take().is_empty());
    }

    #[test]
    fn known_exchanges_are_updates() {
        let mut b = Batcher::new();
        b.added(summary(3, ExchangeState::Complete, Some(200)), true);
        let batch = b.take();
        assert!(batch.added.is_empty());
        assert_eq!(batch.updated.len(), 1);
    }

    #[test]
    fn removal_drops_pending_entries() {
        let mut b = Batcher::new();
        b.added(summary(1, ExchangeState::Requesting, None), false);
        b.added(summary(2, ExchangeState::Requesting, None), false);
        b.message(ExchangeId(1), msg(0));
        b.removed(&[ExchangeId(1)]);
        let batch = b.take();
        assert_eq!(
            batch.added.iter().map(|s| s.id.0).collect::<Vec<_>>(),
            vec![2]
        );
        assert!(batch.messages.is_empty());
        assert_eq!(batch.removed, vec![ExchangeId(1)]);

        // Removals coalesce into one list per batch.
        for id in 10..20 {
            b.removed(&[ExchangeId(id)]);
        }
        let batch = b.take();
        assert_eq!(batch.removed.len(), 10);
        assert!(b.is_empty());

        b.added(summary(5, ExchangeState::Requesting, None), false);
        b.removed(&[ExchangeId(6)]);
        b.message(ExchangeId(5), msg(0));
        b.cleared();
        assert!(b.is_empty());
    }

    #[test]
    fn messages_grouped_per_exchange_in_order() {
        let mut b = Batcher::new();
        b.message(ExchangeId(9), msg(0));
        b.message(ExchangeId(4), msg(0));
        b.message(ExchangeId(9), msg(1));
        let batch = b.take();
        assert_eq!(batch.messages.len(), 2);
        assert_eq!(batch.messages[0].0, ExchangeId(4));
        assert_eq!(
            batch.messages[1]
                .1
                .iter()
                .map(|m| m.seq)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }
}
