//! The live event stream: `GET /api/stream` (WebSocket).
//!
//! Per connection:
//! * a `hello` snapshot with the newest [`HELLO_FIRST`] summaries, the older ones following
//!   in `hello-more` chunks of [`HELLO_CHUNK`] (one per flush while the client keeps up), so a
//!   store of a million exchanges never becomes one giant frame,
//! * store events coalesced into `exchanges` batches every ~33 ms (each id at most once per
//!   batch; removals in one `removed` message per batch),
//! * an optional server-side filter (`set-filter` → `filter-result`, then `filtered` deltas on
//!   every batch, re-evaluating only the exchanges that changed),
//! * app-level control messages (paused/resumed/rules/settings/interceptors/capture/stats/
//!   rule-stats/notice) fanned out from the app bus.
//!
//! Slow clients: outgoing frames go through a bounded queue to a writer task. When the queue
//! is full (or the store/bus broadcast lagged), pending batches are discarded and, once the
//! client has caught up, a fresh full `hello` is sent — never a silently inconsistent state.

pub mod batch;
pub mod matchset;
pub mod messages;

use crate::app::App;
use akev_core::{ExchangeId, ExchangeSummary};
use akev_store::StoreEvent;
use axum::extract::State;
use axum::extract::ws::{Message, Utf8Bytes, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use batch::{Batch, Batcher};
use futures::{SinkExt, StreamExt};
use matchset::{FilterDelta, MatchSet};
use messages::{ClientMessage, ServerMessage};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;

/// Batch flush interval (≈30 Hz).
const FLUSH_INTERVAL: Duration = Duration::from_millis(33);
/// Outgoing frame queue per connection.
const OUT_CAPACITY: usize = 256;
/// How long ids of the hello snapshot are remembered to de-duplicate racing `Added` events.
const SNAPSHOT_MEMORY: Duration = Duration::from_secs(10);
/// Max store events drained per wake-up.
const DRAIN_LIMIT: usize = 4096;
/// Batches up to this size are filtered inline (header/summary-only filters are cheap);
/// larger ones, and any filter with body terms, run on the blocking pool.
const INLINE_EVAL_MAX: usize = 256;
/// Summaries in the `hello` itself (the newest exchanges).
pub const HELLO_FIRST: usize = 2_000;
/// Summaries per `hello-more` chunk.
pub const HELLO_CHUNK: usize = 5_000;

/// `GET /api/stream`
pub async fn ws_handler(ws: WebSocketUpgrade, State(app): State<Arc<App>>) -> Response {
    ws.max_message_size(1 << 20)
        .on_upgrade(move |socket| connection(app, socket))
}

struct ConnFilter {
    query: String,
    seq: u64,
    filter: akev_filter::Filter,
    set: MatchSet,
}

/// State of one stream connection.
struct Conn {
    app: Arc<App>,
    out: mpsc::Sender<Message>,
    batcher: Batcher,
    filter: Option<ConnFilter>,
    /// Sorted ids sent in the last hello (see [`SNAPSHOT_MEMORY`]).
    snapshot_ids: Vec<ExchangeId>,
    snapshot_at: Instant,
    /// Ids of the snapshot still to send in `hello-more` chunks (ascending; sent from the
    /// end, newest first).
    older: Vec<ExchangeId>,
    overloaded: bool,
    closed: bool,
}

async fn connection(app: Arc<App>, socket: WebSocket) {
    let (mut sink, mut incoming) = socket.split();
    let (out_tx, mut out_rx) = mpsc::channel::<Message>(OUT_CAPACITY);
    let writer = tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // Subscribe before taking the snapshot so nothing falls between the two.
    let mut store_rx = app.store.subscribe();
    let mut bus_rx = app.bus.subscribe();
    let mut shutdown = app.shutdown_receiver();
    let mut conn = Conn {
        app: app.clone(),
        out: out_tx,
        batcher: Batcher::new(),
        filter: None,
        snapshot_ids: Vec::new(),
        snapshot_at: Instant::now(),
        older: Vec::new(),
        overloaded: false,
        closed: false,
    };
    conn.resync().await;
    conn.initial_notices();

    let mut flush = tokio::time::interval(FLUSH_INTERVAL);
    flush.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut keepalive = tokio::time::interval(Duration::from_secs(25));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    keepalive.tick().await;

    while !conn.closed {
        tokio::select! {
            biased;
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    break;
                }
            }
            _ = flush.tick() => conn.flush().await,
            msg = incoming.next() => match msg {
                Some(Ok(Message::Text(text))) => conn.on_client_message(&text, &mut store_rx).await,
                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                Some(Ok(_)) => {}
            },
            msg = bus_rx.recv() => match msg {
                Ok(text) => conn.send_text(text),
                Err(RecvError::Lagged(_)) => conn.mark_overloaded(),
                Err(RecvError::Closed) => break,
            },
            ev = store_rx.recv() => match ev {
                Ok(ev) => {
                    conn.on_store_event(ev);
                    conn.drain_store(&mut store_rx);
                }
                Err(RecvError::Lagged(_)) => conn.mark_overloaded(),
                Err(RecvError::Closed) => break,
            },
            _ = keepalive.tick() => conn.send(Message::Ping(bytes::Bytes::new())),
        }
    }
    drop(conn);
    // The writer ends once the queue is closed and drained; don't hang on a dead peer.
    if tokio::time::timeout(Duration::from_secs(2), writer)
        .await
        .is_err()
    {
        tracing::debug!("stream writer did not finish in time");
    }
}

impl Conn {
    fn send(&mut self, msg: Message) {
        if self.overloaded || self.closed {
            return;
        }
        match self.out.try_send(msg) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => self.mark_overloaded(),
            Err(TrySendError::Closed(_)) => self.closed = true,
        }
    }

    fn send_text(&mut self, text: Utf8Bytes) {
        self.send(Message::Text(text));
    }

    fn send_message(&mut self, msg: &ServerMessage<'_>) {
        if let Some(text) = msg.to_text() {
            self.send_text(text);
        }
    }

    /// The client can't keep up: drop pending work and resync once it has drained its queue.
    fn mark_overloaded(&mut self) {
        if !self.overloaded {
            tracing::debug!("stream client is lagging; will resync");
        }
        self.overloaded = true;
        self.batcher.reset();
    }

    fn on_store_event(&mut self, ev: StoreEvent) {
        if self.overloaded {
            return; // the resync snapshot will cover it
        }
        match ev {
            StoreEvent::Added(summary) => {
                let known = self.snapshot_ids.binary_search(&summary.id).is_ok();
                self.batcher.added(summary, known);
            }
            StoreEvent::Updated(summary) => self.batcher.updated(summary),
            StoreEvent::Removed(ids) => {
                self.batcher.removed(&ids);
                if let Some(f) = &mut self.filter {
                    f.set.remove(&ids);
                }
            }
            StoreEvent::Cleared => {
                self.batcher.cleared();
                self.snapshot_ids.clear();
                self.older.clear();
                if let Some(f) = &mut self.filter {
                    f.set.clear();
                }
                self.send_message(&ServerMessage::Cleared);
            }
            StoreEvent::Message { id, message } => self.batcher.message(id, message),
        }
    }

    fn drain_store(&mut self, rx: &mut tokio::sync::broadcast::Receiver<StoreEvent>) {
        for _ in 0..DRAIN_LIMIT {
            match rx.try_recv() {
                Ok(ev) => self.on_store_event(ev),
                Err(TryRecvError::Lagged(_)) => self.mark_overloaded(),
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    async fn flush(&mut self) {
        if self.closed {
            return;
        }
        if self.overloaded {
            if self.out.capacity() >= OUT_CAPACITY / 2 {
                self.overloaded = false;
                self.resync().await;
            }
            return;
        }
        if !self.snapshot_ids.is_empty()
            && self.older.is_empty()
            && self.snapshot_at.elapsed() > SNAPSHOT_MEMORY
        {
            self.snapshot_ids = Vec::new();
        }
        if !self.batcher.is_empty() {
            let batch = self.batcher.take();
            self.send_batch(batch).await;
        }
        if !self.older.is_empty() && self.out.capacity() >= OUT_CAPACITY / 2 {
            self.send_older().await;
        }
    }

    /// Send the next `hello-more` chunk: the newest of the snapshot's ids not sent yet, with
    /// their current summaries (deleted ones are left out).
    async fn send_older(&mut self) {
        let start = self.older.len().saturating_sub(HELLO_CHUNK);
        let ids = self.older.split_off(start);
        let remaining = self.older.len();
        let app = self.app.clone();
        let text = tokio::task::spawn_blocking(move || {
            let summaries = app.store.summaries_of(&ids);
            ServerMessage::HelloMore {
                summaries: &summaries,
                remaining,
            }
            .to_text()
        })
        .await;
        if let Ok(Some(text)) = text {
            self.send_text(text);
        }
    }

    async fn send_batch(&mut self, batch: Batch) {
        if !batch.removed.is_empty() {
            self.send_message(&ServerMessage::Removed {
                ids: &batch.removed,
            });
        }
        if !batch.added.is_empty() || !batch.updated.is_empty() {
            let delta = self.filter_delta(&batch.added, &batch.updated).await;
            self.send_message(&ServerMessage::Exchanges {
                added: &batch.added,
                updated: &batch.updated,
                filtered: delta.as_ref(),
            });
        }
        for (id, messages) in &batch.messages {
            self.send_message(&ServerMessage::Messages { id: *id, messages });
        }
    }

    /// Compute the filtered delta for a batch (None when no filter was ever set).
    ///
    /// Only the exchanges of this batch are re-evaluated, through the store's own filter
    /// evaluator (the same one that computed the initial `filter-result`, so the incremental
    /// set never diverges from a full query).
    async fn filter_delta(
        &mut self,
        added: &[ExchangeSummary],
        updated: &[ExchangeSummary],
    ) -> Option<FilterDelta> {
        let f = self.filter.as_mut()?;
        let added_ids: Vec<ExchangeId> = added.iter().map(|s| s.id).collect();
        let updated_ids: Vec<ExchangeId> = updated.iter().map(|s| s.id).collect();
        if matches!(f.set, MatchSet::All) {
            return Some(f.set.apply(&added_ids, &updated_ids, |_| true));
        }
        let ids: Vec<ExchangeId> = added_ids.iter().chain(&updated_ids).copied().collect();
        let matching: HashSet<ExchangeId> = if f.filter.needs_body() || ids.len() > INLINE_EVAL_MAX
        {
            // Body terms decode bodies: evaluate off the async workers.
            let store = self.app.store.clone();
            let filter = f.filter.clone();
            tokio::task::spawn_blocking(move || store.filter_ids(&ids, &filter))
                .await
                .map(|v| v.into_iter().collect())
                .unwrap_or_default()
        } else {
            self.app
                .store
                .filter_ids(&ids, &f.filter)
                .into_iter()
                .collect()
        };
        let f = self.filter.as_mut()?;
        Some(
            f.set
                .apply(&added_ids, &updated_ids, |id| matching.contains(&id)),
        )
    }

    /// Send a full snapshot: hello (+ filter-result, capture, cached control state).
    async fn resync(&mut self) {
        let app = self.app.clone();
        self.batcher.reset();
        // Snapshot + serialization of a large store is CPU work: keep it off the async workers.
        let snapshot = {
            let app = app.clone();
            tokio::task::spawn_blocking(move || {
                // Ids are cheap; summaries only for the first page.
                let ids = app.store.ids();
                let first = ids.len().saturating_sub(HELLO_FIRST);
                let summaries = app.store.summaries_of(&ids[first..]);
                let older = ids[..first].to_vec();
                let status = app.status();
                let paused = app.breakpoints.list();
                let paused_messages = app.breakpoints.messages().list();
                let rules = app.rules.rules();
                let settings = app.settings();
                let text = ServerMessage::Hello {
                    status: &status,
                    summaries: &summaries,
                    remaining: older.len(),
                    paused: &paused,
                    rules: &rules,
                    settings: &settings,
                    paused_messages: &paused_messages,
                }
                .to_text();
                (ids, older, text)
            })
            .await
        };
        let Ok((ids, older, Some(hello))) = snapshot else {
            tracing::warn!("failed to build the stream snapshot; closing the connection");
            self.closed = true;
            return;
        };
        self.snapshot_ids = ids;
        self.older = older;
        self.snapshot_at = Instant::now();
        self.send_text(hello);
        if let Some(f) = &self.filter {
            let (query, seq, filter) = (f.query.clone(), f.seq, f.filter.clone());
            let set = self.compute_set(&filter).await;
            let ids = set.sorted_ids();
            self.send_message(&ServerMessage::FilterResult {
                seq,
                query: &query,
                ids: ids.as_deref(),
                error: None,
            });
            if let Some(f) = &mut self.filter {
                f.set = set;
            }
        }
        self.send_message(&ServerMessage::Capture {
            recording: app.recording.is_recording(),
        });
        let stats = app.rules.stats();
        self.send_message(&ServerMessage::RuleStats { stats: &stats });
        let cached = app.last_interceptors.lock().clone();
        match cached {
            Some(text) => self.send_text(text),
            // First client: detect interceptors in the background; the result is broadcast.
            None => {
                let app = app.clone();
                tokio::spawn(async move {
                    app.refresh_interceptors(true).await;
                });
            }
        }
    }

    fn initial_notices(&mut self) {
        if let Some(message) = self.app.port_fallback_notice() {
            self.send_message(&ServerMessage::Notice {
                level: "warning",
                message: &message,
            });
        }
        if let Some(message) = self
            .app
            .ca()
            .info()
            .expiry_warning("in Settings → Certificates")
        {
            self.send_message(&ServerMessage::Notice {
                level: "warning",
                message: &message,
            });
        }
    }

    async fn compute_set(&self, filter: &akev_filter::Filter) -> MatchSet {
        if filter.is_match_all() {
            return MatchSet::All;
        }
        let app = self.app.clone();
        let filter = filter.clone();
        tokio::task::spawn_blocking(move || MatchSet::from_ids(app.store.query_ids(&filter)))
            .await
            .unwrap_or_else(|_| MatchSet::from_ids([]))
    }

    async fn on_client_message(
        &mut self,
        text: &str,
        store_rx: &mut tokio::sync::broadcast::Receiver<StoreEvent>,
    ) {
        match serde_json::from_str::<ClientMessage>(text) {
            Ok(ClientMessage::Ping) => self.send_message(&ServerMessage::Pong),
            Ok(ClientMessage::SetFilter { query, seq }) => {
                self.set_filter(query, seq, store_rx).await
            }
            Err(e) => tracing::debug!("ignoring unknown stream message: {e}"),
        }
    }

    async fn set_filter(
        &mut self,
        query: String,
        seq: u64,
        store_rx: &mut tokio::sync::broadcast::Receiver<StoreEvent>,
    ) {
        let filter = match akev_filter::Filter::parse(&query) {
            Ok(f) => f,
            Err(err) => {
                let ids = self.filter.as_ref().and_then(|f| f.set.sorted_ids());
                self.send_message(&ServerMessage::FilterResult {
                    seq,
                    query: &query,
                    ids: ids.as_deref(),
                    error: Some(&err),
                });
                return;
            }
        };
        // Deliver everything the client should know about before the new id set, so ids in
        // the result refer to exchanges the client has (modulo events racing the query).
        self.drain_store(store_rx);
        if !self.overloaded && !self.batcher.is_empty() {
            let batch = self.batcher.take();
            self.send_batch(batch).await;
        }
        let set = self.compute_set(&filter).await;
        let ids = set.sorted_ids();
        self.send_message(&ServerMessage::FilterResult {
            seq,
            query: &query,
            ids: ids.as_deref(),
            error: None,
        });
        self.filter = Some(ConnFilter {
            query,
            seq,
            filter,
            set,
        });
    }
}

/// Background tasks feeding the bus: `stats` every 2 s, `rule-stats` (≤ 2/s, on change),
/// interceptor state polling, and the CA expiry check.
pub(crate) fn spawn_background_tasks(app: &Arc<App>) {
    let mut tasks = Vec::new();

    let a = app.clone();
    tasks.push(tokio::spawn(async move {
        let mut shutdown = a.shutdown_receiver();
        let mut stats_tick = tokio::time::interval(Duration::from_secs(2));
        let mut rule_tick = tokio::time::interval(Duration::from_millis(500));
        let mut interceptor_tick = tokio::time::interval(Duration::from_secs(10));
        let mut ca_tick = tokio::time::interval(Duration::from_secs(6 * 3600));
        for t in [&mut stats_tick, &mut rule_tick, &mut interceptor_tick, &mut ca_tick] {
            t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        }
        ca_tick.tick().await; // the first notice is sent per connection
        let mut last_rule_stats = String::new();
        loop {
            tokio::select! {
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                _ = stats_tick.tick() => {
                    if a.has_stream_clients() {
                        let stats = a.stats();
                        a.broadcast(&ServerMessage::Stats { stats: &stats });
                    }
                }
                _ = rule_tick.tick() => {
                    if !a.has_stream_clients() {
                        continue;
                    }
                    let stats = a.rules.stats();
                    let Some(text) = (ServerMessage::RuleStats { stats: &stats }).to_text() else { continue };
                    if text.as_str() != last_rule_stats {
                        last_rule_stats = text.as_str().to_string();
                        let _ = a.bus.send(text);
                    }
                }
                _ = interceptor_tick.tick() => {
                    if a.has_stream_clients() {
                        a.refresh_interceptors(false).await;
                    }
                }
                _ = ca_tick.tick() => {
                    if let Some(warning) = a.ca().info().expiry_warning("in Settings → Certificates") {
                        a.notice("warning", &warning);
                    }
                }
            }
        }
    }));

    app.tasks.lock().extend(tasks);
}
