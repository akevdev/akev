//! # akev-store
//!
//! Turns the proxy's [`akev_core::ProxyEvent`] stream into queryable exchange records, built
//! to stay fast and lean with 100k+ exchanges:
//!
//! * Records live in an ordered map keyed by `ExchangeId` behind a `parking_lot::RwLock`.
//!   Each record keeps everything [`ExchangeDetail`] exposes in a compact layout (request-line
//!   fields stored once, client info shared per connection, rare data boxed) plus an
//!   always-current [`ExchangeSummary`], so list/query reads are clone-only.
//! * Body bytes are kept in memory until `memory_budget_bytes` is exceeded; then the oldest
//!   bodies are spilled (zstd-compressed) to append-only temp files and read back on demand
//!   with positioned reads. Disk space is reclaimed as spilled exchanges are deleted/evicted.
//! * `max_exchanges` evicts oldest unpinned exchanges.
//! * Changes are broadcast as [`StoreEvent`]s for live UIs (the API batches them).
//! * HAR 1.2 import/export, and a native `.akev` session format (zstd-compressed JSON lines
//!   containing full details, bodies, stream messages, notes, pins & colours).
//!
//! ## Ingestion
//! [`Store::sink`] hands the proxy an [`EventSink`] that only pushes into an unbounded
//! channel; a background tokio task drains it in small batches (one write-lock acquisition
//! per batch) and runs spill compression on the blocking pool. Without a tokio runtime the
//! sink applies events inline. [`Store::ingest`] applies one event synchronously.
//!
//! Events are tolerated out of order: events for unknown exchanges (e.g. a late stream
//! message after deletion) are ignored, duplicates of creating events are dropped. Requests to
//! hosts matching `capture.ignore_hosts` (globs, `*`/`?`) are not recorded, nor are tunnels or
//! TLS failures when `record_tunnels` / `record_tls_failures` are off. Bodies (and stream
//! message payloads) larger than `max_body_bytes` are truncated (and flagged) on ingest.
//!
//! ## Notifications
//! `Added` for `RequestStarted` / `TlsFailed` / `TunnelOpened` / `ClientError` (and inserted
//! exchanges); `Updated` whenever the summary visibly changes and on every body completion;
//! `Message` per stream message (followed by `Updated` for the new message count); `Removed`
//! for deletes, evictions and keep-pinned clears; `Cleared` when everything was removed.

mod foreign;
mod har;
mod library;
mod record;
mod redact;
mod session;
mod spill;
mod util;
mod view;

use akev_core::queue::{QueueReceiver, QueueSink, event_queue};
use akev_core::settings::CaptureSettings;
use akev_core::*;
use bytes::Bytes;
use parking_lot::{Mutex, RwLock};
use record::{
    ApplyCtx, BodyData, BodyRef, Ledger, Outcome, Payload, Record, Slot, StoredBody, StreamClose,
};
use spill::{Spill, SpillHandle};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};
use tokio::sync::broadcast;
use util::HostGlob;
use view::{OwnedRecord, Records};

pub use library::{
    AUTOSAVE_ID, AUTOSAVE_NAME, AUTOSAVE_PREVIOUS_ID, AUTOSAVE_PREVIOUS_NAME, LibraryError,
    LibraryResult, OpenMode, SessionInfo, SessionKind, SessionLibrary,
};
pub use redact::ExportOptions;
pub use session::SessionLimits;

/// Errors from file import/export operations.
#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    /// Reading or writing failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    /// The input isn't a valid HAR / session file.
    #[error("invalid file: {0}")]
    InvalidFile(String),
    /// No such exchange.
    #[error("not found")]
    NotFound,
}

/// Result alias for store operations.
pub type Result<T> = std::result::Result<T, StoreError>;

/// Live change notifications.
#[derive(Clone, Debug)]
pub enum StoreEvent {
    /// A new exchange row.
    Added(ExchangeSummary),
    /// An exchange's summary changed (or one of its bodies completed).
    Updated(ExchangeSummary),
    /// Exchanges were deleted or evicted.
    Removed(Vec<ExchangeId>),
    /// Everything was removed.
    Cleared,
    /// A new stream message on an exchange (payload not included; fetch via `messages`).
    Message {
        /// The exchange the message belongs to.
        id: ExchangeId,
        /// The message metadata.
        message: StreamMessage,
    },
}

/// Store counters.
#[derive(Clone, Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreStats {
    /// Exchanges currently held.
    pub exchanges: u64,
    /// Body and stream-message payload bytes held in memory.
    pub memory_body_bytes: u64,
    /// Logical (uncompressed) size of bodies spilled to disk.
    pub spilled_body_bytes: u64,
    /// Exchanges evicted by `max_exchanges` since the store was created.
    pub evicted: u64,
    /// Bytes the spill files occupy on disk.
    pub spill_disk_bytes: u64,
    /// Spilled bodies deleted to stay within `spill_max_bytes`.
    pub disk_dropped_bodies: u64,
    /// Bodies and message payloads not recorded because the proxy → store queue was full.
    pub dropped_bodies: u64,
    /// Stream messages not recorded at all because the queue was far over capacity.
    pub dropped_messages: u64,
    /// Bytes waiting in the proxy → store queue.
    pub queued_bytes: u64,
}

/// A stream message with its payload.
#[derive(Clone, Debug)]
pub struct MessageWithPayload {
    /// Message metadata (sequence, direction, kind, time, size...).
    pub message: StreamMessage,
    /// Payload bytes (possibly truncated to `max_body_bytes`; `message.size` is the full size).
    pub payload: Bytes,
}

/// Which stream messages [`Store::messages_page`] returns: those with `after < seq <
/// before` (bounds optional), at most `limit` of them — the oldest, or the newest with
/// `tail` — in ascending sequence order.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MessagePage {
    /// Only messages with a greater sequence number.
    pub after: Option<u32>,
    /// Only messages with a smaller sequence number.
    pub before: Option<u32>,
    /// At most this many messages (all when `None`).
    pub limit: Option<usize>,
    /// Take the newest `limit` messages in the range instead of the oldest.
    pub tail: bool,
}

/// Patch applied to user-editable exchange attributes.
#[derive(Clone, Debug, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExchangePatch {
    /// Pin/unpin (pinned exchanges survive `max_exchanges` eviction and keep-pinned clears).
    pub pinned: Option<bool>,
    /// `Some(None)` clears the colour.
    #[serde(default, with = "double_option")]
    pub color: Option<Option<String>>,
    /// `Some(None)` (or an empty string) clears the note.
    #[serde(default, with = "double_option")]
    pub note: Option<Option<String>>,
}

mod double_option {
    use serde::{Deserialize, Deserializer};
    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> Result<Option<Option<String>>, D::Error> {
        Ok(Some(Option::<String>::deserialize(d)?))
    }
}

/// An exchange to insert: capture, imports and sessions (which also restore original bodies
/// and stream-close info).
pub(crate) struct NewRecord {
    pub detail: ExchangeDetail,
    pub request_body: CapturedBody,
    pub response_body: CapturedBody,
    pub original_request_body: Option<CapturedBody>,
    pub original_response_body: Option<CapturedBody>,
    pub messages: Vec<MessageWithPayload>,
    pub close: Option<StreamClose>,
    /// Keep the summary's content kind and duration exactly as given (session loads);
    /// otherwise they're derived when unset.
    pub exact: bool,
}

impl NewRecord {
    pub(crate) fn from_detail(detail: ExchangeDetail) -> Self {
        NewRecord {
            detail,
            request_body: CapturedBody::empty(),
            response_body: CapturedBody::empty(),
            original_request_body: None,
            original_response_body: None,
            messages: Vec::new(),
            close: None,
            exact: false,
        }
    }
}

/// Everything about one exchange, copied out for export (taken under a short read lock).
pub(crate) struct Snapshot {
    pub detail: ExchangeDetail,
    pub request_body: CapturedBody,
    pub response_body: CapturedBody,
    pub original_request_body: Option<CapturedBody>,
    pub original_response_body: Option<CapturedBody>,
    pub messages: Vec<MessageWithPayload>,
    pub close: Option<StreamClose>,
}

/// Capacity of the [`StoreEvent`] broadcast channel.
const EVENT_CAPACITY: usize = 4096;
/// Max events applied per write-lock acquisition by the ingest task.
const INGEST_BATCH: usize = 64;
/// After a spill failure (disk full, unwritable dir...), don't retry for this long.
const SPILL_RETRY_AFTER: Duration = Duration::from_secs(5);

struct Inner {
    records: Records,
    ledger: Ledger,
    settings: CaptureSettings,
    ignore: Vec<HostGlob>,
    /// Interned client info per connection (exchanges on one connection share it).
    clients: HashMap<ConnectionId, Arc<ClientInfo>>,
    clients_prune_at: usize,
    /// Number of pinned records (exempt from `max_exchanges`).
    pinned: usize,
    evicted: u64,
    /// Spilled bodies deleted for the disk cap.
    disk_dropped: u64,
    /// Bumped on every visible change (invalidates cached suggestions).
    generation: u64,
}

const CLIENTS_PRUNE_MIN: usize = 1024;

impl Inner {
    fn new(settings: CaptureSettings) -> Self {
        Inner {
            records: Records::new(),
            ledger: Ledger::default(),
            ignore: compile_globs(&settings.ignore_hosts),
            settings,
            clients: HashMap::new(),
            clients_prune_at: CLIENTS_PRUNE_MIN,
            pinned: 0,
            evicted: 0,
            disk_dropped: 0,
            generation: 0,
        }
    }

    fn is_ignored(&self, host: &str) -> bool {
        self.ignore.iter().any(|g| g.matches(host))
    }

    fn intern_client(&mut self, client: ClientInfo) -> Arc<ClientInfo> {
        if let Some(existing) = self.clients.get(&client.connection_id)
            && **existing == client
        {
            return Arc::clone(existing);
        }
        let arc = Arc::new(client);
        self.clients.insert(arc.connection_id, Arc::clone(&arc));
        if self.clients.len() > self.clients_prune_at {
            self.clients.retain(|_, c| Arc::strong_count(c) > 1);
            self.clients_prune_at = (self.clients.len() * 2).max(CLIENTS_PRUNE_MIN);
        }
        arc
    }

    /// Add a record (accounting its bodies). Ids are unique, but a clash never leaks.
    fn insert_record(&mut self, rec: Box<Record>, spill: &Spill) {
        // Forget a clashing record first: its index entries share keys with the new one's.
        if let Some(previous) = self.records.remove(&rec.id()) {
            self.forget(&previous, spill);
        }
        rec.account_all(&mut self.ledger);
        if rec.summary.pinned {
            self.pinned += 1;
        }
        self.records.insert(rec.id(), rec);
    }

    /// Remove a record, releasing its memory accounting and spill blobs.
    fn remove_record(&mut self, id: &ExchangeId, spill: &Spill) -> Option<Box<Record>> {
        let rec = self.records.remove(id)?;
        self.forget(&rec, spill);
        Some(rec)
    }

    fn forget(&mut self, rec: &Record, spill: &Spill) {
        rec.release(&mut self.ledger, spill);
        if rec.summary.pinned {
            self.pinned = self.pinned.saturating_sub(1);
        }
    }

    /// Evict the oldest unpinned exchanges beyond `max_exchanges`. Pinned exchanges are
    /// exempt: they're never evicted and don't count towards the limit (otherwise pinning
    /// `max_exchanges` rows would make every new exchange vanish on arrival).
    fn enforce_limit(&mut self, spill: &Spill) -> Vec<ExchangeId> {
        let max = self.settings.max_exchanges;
        if max == 0 {
            return Vec::new();
        }
        let max = usize::try_from(max).unwrap_or(usize::MAX);
        let unpinned = self.records.len().saturating_sub(self.pinned);
        let excess = unpinned.saturating_sub(max);
        if excess == 0 {
            return Vec::new();
        }
        let victims: Vec<ExchangeId> = self
            .records
            .iter()
            .filter(|(_, r)| !r.summary.pinned)
            .map(|(id, _)| *id)
            .take(excess)
            .collect();
        for id in &victims {
            self.remove_record(id, spill);
        }
        self.evicted += victims.len() as u64;
        victims
    }

    fn over_budget(&self) -> bool {
        self.ledger.memory_bytes > self.settings.memory_budget_bytes
    }
}

fn compile_globs(patterns: &[String]) -> Vec<HostGlob> {
    patterns.iter().filter_map(|p| HostGlob::new(p)).collect()
}

/// A body (or a set of message payloads) selected for spilling.
enum Candidate {
    Body {
        id: ExchangeId,
        slot: Slot,
        bytes: Bytes,
    },
    Messages {
        id: ExchangeId,
        parts: Vec<(usize, Bytes)>,
    },
}

fn same_bytes(a: &Bytes, b: &Bytes) -> bool {
    a.as_ptr() == b.as_ptr() && a.len() == b.len()
}

/// The captured-traffic store. Create with [`Store::new`]; share as `Arc<Store>`.
pub struct Store {
    inner: RwLock<Inner>,
    spill: Spill,
    events: broadcast::Sender<StoreEvent>,
    /// The bounded proxy → store queue (created by the first [`Store::sink`] call).
    sink: Mutex<Option<QueueSink>>,
    /// Serialises spilling; holds the retry deadline after a spill failure.
    spill_gate: Mutex<Option<Instant>>,
    suggest_cache: Mutex<Option<(u64, akev_filter::SuggestContext)>>,
    /// Rule id → title for `rule:` filter terms (set by the app whenever rules change).
    rule_titles: RwLock<Arc<view::RuleTitles>>,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("stats", &self.stats())
            .finish_non_exhaustive()
    }
}

/// Sink used when no tokio runtime is available: applies events inline.
struct InlineSink(Weak<Store>);

impl EventSink for InlineSink {
    fn emit(&self, event: ProxyEvent) {
        if let Some(store) = self.0.upgrade() {
            store.ingest(event);
        }
    }
}

/// The ingest thread: applies queued events in batches (one write-lock acquisition each) and
/// spills when over budget. It runs on its own OS thread, so taking the write lock (which
/// may wait for a reader) and compressing bodies never stall a Tokio worker. It ends when
/// every sink is gone or the store is dropped.
fn ingest_thread(mut rx: QueueReceiver, store: Weak<Store>) {
    let mut batch = Vec::with_capacity(INGEST_BATCH);
    while let Some((first, cost)) = rx.blocking_recv() {
        batch.push(first);
        let (_, more) = rx.try_recv_many(&mut batch, INGEST_BATCH - 1);
        let Some(store) = store.upgrade() else { break };
        // One bad event must never stop capture: contain panics to the batch.
        let applied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            store.apply_batch(batch.drain(..))
        }));
        batch.clear();
        // The bytes now live in the store (and count against its memory budget).
        rx.release(cost + more);
        match applied {
            Ok(true) => store.maybe_spill(),
            Ok(false) => {}
            Err(_) => tracing::error!("store event ingestion panicked; batch dropped"),
        }
    }
}

impl Store {
    /// Create a store. `spill_dir` is where body spill files go (defaults to the OS temp dir).
    pub fn new(settings: CaptureSettings, spill_dir: Option<&Path>) -> Arc<Store> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Store {
            inner: RwLock::new(Inner::new(settings)),
            spill: Spill::new(spill_dir),
            events,
            sink: Mutex::new(None),
            spill_gate: Mutex::new(None),
            suggest_cache: Mutex::new(None),
            rule_titles: RwLock::new(Arc::new(view::RuleTitles::new())),
        })
    }

    /// Update capture settings live (budget/limits/ignore list). Lowering `max_exchanges`
    /// evicts immediately; lowering the budget spills immediately. The ignore list applies
    /// to exchanges that start afterwards.
    pub fn update_settings(&self, settings: CaptureSettings) {
        if let Some(sink) = self.sink.lock().as_ref() {
            sink.state().set_capacity(settings.event_queue_bytes);
        }
        {
            let mut guard = self.inner.write();
            let inner = &mut *guard;
            inner.ignore = compile_globs(&settings.ignore_hosts);
            inner.settings = settings;
            let evicted = inner.enforce_limit(&self.spill);
            if !evicted.is_empty() {
                inner.generation += 1;
                self.notify(StoreEvent::Removed(evicted));
            }
        }
        self.maybe_spill();
        self.enforce_disk_cap();
    }

    /// An [`EventSink`] that feeds this store. It pushes into a byte-bounded queue
    /// (`capture.event_queue_bytes`, see [`akev_core::queue`]) drained by a dedicated ingest
    /// thread, so the proxy hot path never blocks on store locks and a store that falls
    /// behind costs dropped bodies, not unbounded memory. Without a Tokio runtime (plain
    /// tests) events are applied inline.
    pub fn sink(self: &Arc<Self>) -> Arc<dyn EventSink> {
        if tokio::runtime::Handle::try_current().is_err() {
            return Arc::new(InlineSink(Arc::downgrade(self)));
        }
        let mut slot = self.sink.lock();
        if let Some(existing) = slot.as_ref().filter(|s| !s.is_closed()) {
            return Arc::new(existing.clone());
        }
        let capacity = self.inner.read().settings.event_queue_bytes;
        let (sink, receiver) = event_queue(capacity);
        let store = Arc::downgrade(self);
        let started = std::thread::Builder::new()
            .name("akev-store-ingest".into())
            .spawn(move || ingest_thread(receiver, store));
        if let Err(e) = started {
            tracing::error!(error = %e, "cannot start the store ingest thread; storing inline");
            return Arc::new(InlineSink(Arc::downgrade(self)));
        }
        *slot = Some(sink.clone());
        Arc::new(sink)
    }

    /// Apply one event synchronously (used by the sink's task and by tests).
    pub fn ingest(&self, event: ProxyEvent) {
        if self.apply_batch(std::iter::once(event)) {
            self.maybe_spill();
        }
    }

    /// Subscribe to live [`StoreEvent`]s (capacity 4096; slow receivers see `Lagged`).
    pub fn subscribe(&self) -> broadcast::Receiver<StoreEvent> {
        self.events.subscribe()
    }

    /// All summaries in id order.
    pub fn summaries(&self) -> Vec<ExchangeSummary> {
        self.inner
            .read()
            .records
            .values()
            .map(|r| r.summary.clone())
            .collect()
    }

    /// The newest `limit` summaries, in id order.
    pub fn newest_summaries(&self, limit: usize) -> Vec<ExchangeSummary> {
        let inner = self.inner.read();
        let mut out: Vec<ExchangeSummary> = inner
            .records
            .values()
            .rev()
            .take(limit)
            .map(|r| r.summary.clone())
            .collect();
        out.reverse();
        out
    }

    /// Every exchange id, ascending.
    pub fn ids(&self) -> Vec<ExchangeId> {
        self.inner.read().records.keys().copied().collect()
    }

    /// Summaries of the given exchanges that still exist, in the order given.
    pub fn summaries_of(&self, ids: &[ExchangeId]) -> Vec<ExchangeSummary> {
        let inner = self.inner.read();
        ids.iter()
            .filter_map(|id| inner.records.get(id).map(|r| r.summary.clone()))
            .collect()
    }

    /// Summaries matching a filter, in id order, optionally limited to the last `limit`.
    pub fn query(
        &self,
        filter: &akev_filter::Filter,
        limit: Option<usize>,
    ) -> Vec<ExchangeSummary> {
        if filter.needs_body() {
            let ids = self.scan_body_filter(filter, limit);
            let inner = self.inner.read();
            return ids
                .iter()
                .filter_map(|id| inner.records.get(id).map(|r| r.summary.clone()))
                .collect();
        }
        let titles = self.rule_titles.read().clone();
        let inner = self.inner.read();
        view::matching(&inner.records, &self.spill, &titles, filter, limit)
            .into_iter()
            .map(|r| r.summary.clone())
            .collect()
    }

    /// Evaluate a filter with body terms without holding the records lock while bodies are
    /// read and decompressed: copy [`view::SCAN_CHUNK`] records at a time (newest first)
    /// under the read lock, release it, evaluate the copies. Stops once `limit` matched.
    /// Returns ids in id order.
    fn scan_body_filter(
        &self,
        filter: &akev_filter::Filter,
        limit: Option<usize>,
    ) -> Vec<ExchangeId> {
        let titles = self.rule_titles.read().clone();
        let mut out = Vec::new();
        let mut before: Option<ExchangeId> = None;
        if limit == Some(0) {
            return out;
        }
        loop {
            let chunk: Vec<OwnedRecord> = {
                let inner = self.inner.read();
                let range = match before {
                    Some(b) => inner.records.range(..b),
                    None => inner.records.range(..),
                };
                range
                    .rev()
                    .take(view::SCAN_CHUNK)
                    .map(|(_, r)| OwnedRecord::of(r, &titles))
                    .collect()
            };
            let Some(last) = chunk.last() else { break };
            before = Some(last.id());
            for id in view::matching_owned(&chunk, &self.spill, filter) {
                out.push(id);
                if limit.is_some_and(|l| out.len() >= l) {
                    out.reverse();
                    return out;
                }
            }
        }
        out.reverse();
        out
    }

    /// Copy the given records for body-filter evaluation outside the lock.
    fn owned_records(&self, ids: &[ExchangeId]) -> Vec<OwnedRecord> {
        let titles = self.rule_titles.read().clone();
        let inner = self.inner.read();
        ids.iter()
            .filter_map(|id| inner.records.get(id))
            .map(|r| OwnedRecord::of(r, &titles))
            .collect()
    }

    /// Register rule titles (rule id → title) so `rule:<title>` filter terms can match.
    /// Call whenever the rule set changes.
    pub fn set_rule_titles(&self, titles: std::collections::HashMap<String, String>) {
        *self.rule_titles.write() = Arc::new(titles);
    }

    /// Ids matching a filter (cheaper than full summaries for the UI's filtered view).
    pub fn query_ids(&self, filter: &akev_filter::Filter) -> Vec<ExchangeId> {
        if filter.needs_body() {
            return self.scan_body_filter(filter, None);
        }
        let titles = self.rule_titles.read().clone();
        let inner = self.inner.read();
        view::matching(&inner.records, &self.spill, &titles, filter, None)
            .into_iter()
            .map(Record::id)
            .collect()
    }

    /// Whether one exchange matches a filter (`None` if it doesn't exist). Lets callers
    /// maintain filtered views incrementally on [`StoreEvent::Added`]/[`StoreEvent::Updated`].
    pub fn matches(&self, id: ExchangeId, filter: &akev_filter::Filter) -> Option<bool> {
        if filter.needs_body() {
            let rec = self.owned_records(&[id]).pop()?;
            return Some(view::owned_matches(filter, &rec, &self.spill));
        }
        let titles = self.rule_titles.read().clone();
        let inner = self.inner.read();
        let rec = inner.records.get(&id)?;
        Some(filter.matches(&view::RecordView::new(rec, &self.spill, &titles)))
    }

    /// The subset of `ids` that exist and match `filter` (input order). Filters with body
    /// terms are evaluated on copies, outside the records lock.
    pub fn filter_ids(&self, ids: &[ExchangeId], filter: &akev_filter::Filter) -> Vec<ExchangeId> {
        if filter.needs_body() && !filter.is_match_all() {
            let mut out = Vec::new();
            for part in ids.chunks(view::SCAN_CHUNK) {
                let records = self.owned_records(part);
                out.extend(view::matching_owned(&records, &self.spill, filter));
            }
            return out;
        }
        let titles = self.rule_titles.read().clone();
        let inner = self.inner.read();
        let all = filter.is_match_all();
        ids.iter()
            .copied()
            .filter(|id| {
                inner.records.get(id).is_some_and(|rec| {
                    all || filter.matches(&view::RecordView::new(rec, &self.spill, &titles))
                })
            })
            .collect()
    }

    /// One exchange's summary.
    pub fn summary(&self, id: ExchangeId) -> Option<ExchangeSummary> {
        self.inner
            .read()
            .records
            .get(&id)
            .map(|r| r.summary.clone())
    }

    /// Call `f` with the timing and the request / response headers of each exchange in `ids`
    /// that exists, in input order, under one read lock and without touching bodies: cheap
    /// per-row data for list columns and the timeline.
    pub fn visit_heads(
        &self,
        ids: &[ExchangeId],
        mut f: impl FnMut(ExchangeId, &Timing, &Headers, Option<&Headers>),
    ) {
        let inner = self.inner.read();
        for &id in ids {
            if let Some(r) = inner.records.get(&id) {
                f(
                    id,
                    &r.timing,
                    &r.request_headers,
                    r.response.as_ref().map(|h| &h.headers),
                );
            }
        }
    }

    /// One exchange's full detail (computes and caches decoded body sizes when cheap).
    pub fn detail(&self, id: ExchangeId) -> Option<ExchangeDetail> {
        // Decoded sizes need the body bytes (maybe a spill read and decompression): work
        // them out without the lock, then cache them on the record.
        let jobs = self.inner.read().records.get(&id)?.decoded_size_jobs();
        if !jobs.is_empty() {
            let sizes: Vec<(Side, BodyRef, Option<u64>)> = jobs
                .into_iter()
                .map(|(side, body, encodings)| {
                    let size = record::decoded_size_of(&body, &encodings, &self.spill);
                    (side, body, size)
                })
                .collect();
            let inner = self.inner.read();
            if let Some(rec) = inner.records.get(&id) {
                for (side, body, size) in &sizes {
                    rec.set_decoded_size(*side, body, *size);
                }
            }
        }
        let inner = self.inner.read();
        inner.records.get(&id).map(|r| r.detail(&self.spill, false))
    }

    /// Body as captured (still content-encoded). For modified exchanges this is what was
    /// actually sent/received after modification.
    pub fn body(&self, id: ExchangeId, side: Side) -> Option<CapturedBody> {
        let body = {
            let inner = self.inner.read();
            let rec = inner.records.get(&id)?;
            match side {
                Side::Request => rec.request_body.to_ref(),
                Side::Response => rec.response_body.to_ref(),
            }
        };
        Some(body.to_captured(&self.spill))
    }

    /// Original (pre-modification) body if a rule/breakpoint changed it.
    pub fn original_body(&self, id: ExchangeId, side: Side) -> Option<CapturedBody> {
        let slot = match side {
            Side::Request => Slot::OriginalRequest,
            Side::Response => Slot::OriginalResponse,
        };
        let body = self
            .inner
            .read()
            .records
            .get(&id)?
            .slot(slot)
            .map(StoredBody::to_ref)?;
        Some(body.to_captured(&self.spill))
    }

    /// Stream messages (WebSocket/SSE) with payloads, in arrival order.
    pub fn messages(&self, id: ExchangeId) -> Vec<MessageWithPayload> {
        self.messages_page(id, MessagePage::default())
            .unwrap_or_default()
    }

    /// Stream messages with `seq > after` (for incremental fetching: only the returned
    /// payloads are loaded, so spilled history isn't read back needlessly).
    pub fn messages_after(&self, id: ExchangeId, after: u32) -> Vec<MessageWithPayload> {
        let page = MessagePage {
            after: Some(after),
            ..MessagePage::default()
        };
        self.messages_page(id, page).unwrap_or_default()
    }

    /// One page of an exchange's stream messages (`None` if the exchange doesn't exist).
    /// Payloads are read after the records lock is released, and only for the page.
    pub fn messages_page(
        &self,
        id: ExchangeId,
        page: MessagePage,
    ) -> Option<Vec<MessageWithPayload>> {
        let MessagePage {
            after,
            before,
            limit,
            tail,
        } = page;
        let keep =
            |m: &StreamMessage| after.is_none_or(|a| m.seq > a) && before.is_none_or(|b| m.seq < b);
        let refs = self.inner.read().records.get(&id)?.message_refs(
            keep,
            limit.unwrap_or(usize::MAX),
            tail,
        );
        Some(refs.load(&self.spill))
    }

    /// Delete exchanges (unknown ids are ignored). Broadcasts `Removed` for those deleted.
    pub fn delete(&self, ids: &[ExchangeId]) {
        let mut dropped = Vec::new();
        {
            let mut guard = self.inner.write();
            let inner = &mut *guard;
            let mut removed = Vec::new();
            for id in ids {
                if let Some(rec) = inner.remove_record(id, &self.spill) {
                    removed.push(*id);
                    dropped.push(rec);
                }
            }
            if removed.is_empty() {
                return;
            }
            if inner.records.is_empty() {
                inner.clients.clear();
            }
            inner.generation += 1;
            self.notify(StoreEvent::Removed(removed));
        }
        // Deallocate outside the lock.
        drop(dropped);
    }

    /// Remove everything (or everything unpinned). Broadcasts `Cleared` when the store ends
    /// up empty, otherwise `Removed` with the unpinned ids.
    pub fn clear(&self, keep_pinned: bool) {
        let dropped: Records;
        {
            let mut guard = self.inner.write();
            let inner = &mut *guard;
            let old = std::mem::take(&mut inner.records);
            let any_pinned = keep_pinned && old.values().any(|r| r.summary.pinned);
            if any_pinned {
                let mut removed = Vec::new();
                let mut gone = Records::new();
                for (id, rec) in old {
                    if rec.summary.pinned {
                        inner.records.insert(id, rec);
                    } else {
                        // Unpinned: the pinned counter is unaffected.
                        rec.release(&mut inner.ledger, &self.spill);
                        removed.push(id);
                        gone.insert(id, rec);
                    }
                }
                inner.clients.retain(|_, c| Arc::strong_count(c) > 1);
                if !removed.is_empty() {
                    self.notify(StoreEvent::Removed(removed));
                }
                dropped = gone;
            } else {
                for rec in old.values() {
                    rec.release_spill(&self.spill);
                }
                inner.ledger = Ledger::default();
                inner.pinned = 0;
                inner.clients.clear();
                self.notify(StoreEvent::Cleared);
                dropped = old;
            }
            inner.generation += 1;
        }
        drop(dropped);
    }

    /// Update pinned / colour / note on exchanges. Broadcasts `Updated` for changed rows.
    pub fn patch(&self, ids: &[ExchangeId], patch: &ExchangePatch) {
        let mut guard = self.inner.write();
        let notify = self.has_listeners();
        let mut changed = false;
        for id in ids {
            if let Some(rec) = guard.records.get_mut(id) {
                let was_pinned = rec.summary.pinned;
                if rec.apply_patch(patch) {
                    changed = true;
                    let now_pinned = rec.summary.pinned;
                    if notify {
                        self.notify(StoreEvent::Updated(rec.summary.clone()));
                    }
                    match (was_pinned, now_pinned) {
                        (false, true) => guard.pinned += 1,
                        (true, false) => guard.pinned = guard.pinned.saturating_sub(1),
                        _ => {}
                    }
                }
            }
        }
        if changed {
            // Unpinning may push the unpinned count over the limit.
            let evicted = guard.enforce_limit(&self.spill);
            if !evicted.is_empty() {
                self.notify(StoreEvent::Removed(evicted));
            }
            guard.generation += 1;
        }
    }

    /// Store counters.
    pub fn stats(&self) -> StoreStats {
        let queue = self.sink.lock().as_ref().map(|s| Arc::clone(s.state()));
        let mut stats = {
            let inner = self.inner.read();
            StoreStats {
                exchanges: inner.records.len() as u64,
                memory_body_bytes: inner.ledger.memory_bytes,
                spilled_body_bytes: inner.ledger.spilled_bytes,
                evicted: inner.evicted,
                disk_dropped_bodies: inner.disk_dropped,
                ..StoreStats::default()
            }
        };
        stats.spill_disk_bytes = self.spill.disk_bytes();
        if let Some(q) = queue {
            stats.dropped_bodies = q.dropped_bodies();
            stats.dropped_messages = q.dropped_messages();
            stats.queued_bytes = q.queued_bytes();
        }
        stats
    }

    /// Distinct values for filter autocompletion: hosts (most frequent first, max 200),
    /// methods, sources, response content types and header names (lowercase, from recent
    /// exchanges). Cached until the store changes.
    pub fn suggest_context(&self) -> akev_filter::SuggestContext {
        let inner = self.inner.read();
        let mut cache = self.suggest_cache.lock();
        if let Some((generation, ctx)) = cache.as_ref()
            && *generation == inner.generation
        {
            return ctx.clone();
        }
        let ctx = view::suggest_context(&inner.records);
        *cache = Some((inner.generation, ctx.clone()));
        ctx
    }

    /// Export as HAR 1.2 JSON (all exchanges, or the given ids). Includes `_akev` extension
    /// fields (pinned, color, note, websocket messages as `_webSocketMessages` like Chrome).
    ///
    /// This builds the whole document — every exported body, decoded — in memory; use
    /// [`Store::write_har`] to write it out (it streams one exchange at a time).
    pub fn export_har(&self, ids: Option<&[ExchangeId]>) -> serde_json::Value {
        let ids = self.resolve_ids(ids);
        har::export(ids.into_iter().filter_map(|id| self.snapshot(id)))
    }

    /// Write the HAR export of [`Store::export_har`] to `out`, pretty-printed (byte for byte
    /// what `serde_json::to_writer_pretty` gives for that value), streaming one exchange at a
    /// time: memory use stays around one exchange's bodies however large the export is.
    /// Bodies decode to at most 64 MiB each and 512 MiB per export; beyond that they're
    /// exported still encoded (flagged in `_akev`, which import understands). Returns the
    /// number of exchanges written.
    pub fn write_har(
        &self,
        out: &mut dyn std::io::Write,
        ids: Option<&[ExchangeId]>,
    ) -> Result<usize> {
        self.write_har_with(out, ids, ExportOptions::default())
    }

    /// [`Store::write_har`] with export options (e.g. secrets redacted).
    pub fn write_har_with(
        &self,
        out: &mut dyn std::io::Write,
        ids: Option<&[ExchangeId]>,
        options: ExportOptions,
    ) -> Result<usize> {
        let ids = self.resolve_ids(ids);
        // Sort the ids into the HAR arrays (one short read lock), keeping their order.
        let mut groups: [Vec<ExchangeId>; 4] = Default::default();
        {
            let inner = self.inner.read();
            for id in ids {
                if let Some(rec) = inner.records.get(&id) {
                    let group = har::Group::of(rec.summary.kind);
                    if let Some(i) = har::Group::ALL.iter().position(|g| *g == group) {
                        groups[i].push(id);
                    }
                }
            }
        }
        let groups = &groups;
        let mut out = std::io::BufWriter::with_capacity(64 * 1024, out);
        let written = har::write(&mut out, move |group| {
            let i = har::Group::ALL
                .iter()
                .position(|g| *g == group)
                .unwrap_or(0);
            groups[i]
                .iter()
                .filter_map(move |&id| self.snapshot(id))
                .map(move |s| options.apply(s))
        })?;
        // `har::write` flushed the buffer (and `out`), reporting any error.
        Ok(written)
    }

    /// Import a HAR file (Chrome/Firefox/Charles/Proxyman/HTTP Toolkit exports). Returns new ids.
    pub fn import_har(&self, har: &[u8]) -> Result<Vec<ExchangeId>> {
        let records = har::parse(har)?;
        Ok(self.insert_records(records))
    }

    /// Write a native `.akev` session file.
    pub fn save_session(
        &self,
        writer: &mut dyn std::io::Write,
        ids: Option<&[ExchangeId]>,
    ) -> Result<()> {
        self.save_session_named(writer, ids, None).map(drop)
    }

    /// Load a native `.akev` session file. Returns new ids. The file is fully validated
    /// before anything is inserted. Files exceeding the default [`SessionLimits`] (huge
    /// lines, implausible compression ratios) are rejected.
    pub fn load_session(&self, reader: &mut dyn std::io::Read) -> Result<Vec<ExchangeId>> {
        self.load_session_with_limits(reader, SessionLimits::default())
    }

    /// [`Store::load_session`] with explicit size limits.
    pub fn load_session_with_limits(
        &self,
        reader: &mut dyn std::io::Read,
        limits: SessionLimits,
    ) -> Result<Vec<ExchangeId>> {
        let records = session::load(reader, limits)?;
        Ok(self.insert_records(records))
    }

    // -----------------------------------------------------------------------------------

    fn has_listeners(&self) -> bool {
        self.events.receiver_count() > 0
    }

    fn notify(&self, event: StoreEvent) {
        // No receivers is fine: nobody is listening.
        let _ = self.events.send(event);
    }

    fn resolve_ids(&self, ids: Option<&[ExchangeId]>) -> Vec<ExchangeId> {
        match ids {
            None => self.inner.read().records.keys().copied().collect(),
            Some(ids) => {
                let mut v = ids.to_vec();
                v.sort_unstable();
                v.dedup();
                v
            }
        }
    }

    /// Everything about one exchange for export. Only references are copied under the
    /// records lock; bodies are read (and decompressed) after it's released.
    fn snapshot(&self, id: ExchangeId) -> Option<Snapshot> {
        let spill = &self.spill;
        let (detail, bodies, original_request, original_response, messages, close) = {
            let inner = self.inner.read();
            let rec = inner.records.get(&id)?;
            (
                rec.detail(spill, false),
                [rec.request_body.to_ref(), rec.response_body.to_ref()],
                rec.slot(Slot::OriginalRequest).map(StoredBody::to_ref),
                rec.slot(Slot::OriginalResponse).map(StoredBody::to_ref),
                rec.message_refs(|_| true, usize::MAX, false),
                rec.close().cloned(),
            )
        };
        let [request_body, response_body] = bodies;
        Some(Snapshot {
            detail,
            request_body: request_body.to_captured(spill),
            response_body: response_body.to_captured(spill),
            original_request_body: original_request.map(|b| b.to_captured(spill)),
            original_response_body: original_response.map(|b| b.to_captured(spill)),
            messages: messages.load(spill),
            close,
        })
    }

    fn insert_records(&self, items: Vec<NewRecord>) -> Vec<ExchangeId> {
        // Build (content detection etc.) outside the lock.
        let records: Vec<Box<Record>> = items
            .into_iter()
            .map(|n| Box::new(Record::from_new(ExchangeId::next(), n)))
            .collect();
        let ids = records.iter().map(|r| r.id()).collect();
        let over = {
            let mut guard = self.inner.write();
            let notify = self.has_listeners();
            for rec in records {
                self.add_record(&mut guard, rec, notify);
            }
            guard.over_budget()
        };
        if over {
            self.maybe_spill();
        }
        ids
    }

    fn add_record(&self, inner: &mut Inner, rec: Box<Record>, notify: bool) {
        let summary = notify.then(|| rec.summary.clone());
        inner.insert_record(rec, &self.spill);
        inner.generation += 1;
        if let Some(summary) = summary {
            self.notify(StoreEvent::Added(summary));
        }
        let evicted = inner.enforce_limit(&self.spill);
        if notify && !evicted.is_empty() {
            self.notify(StoreEvent::Removed(evicted));
        }
    }

    /// Apply events under one write lock. Returns whether bodies now exceed the budget.
    fn apply_batch(&self, events: impl IntoIterator<Item = ProxyEvent>) -> bool {
        let mut guard = self.inner.write();
        let notify = self.has_listeners();
        for event in events {
            self.apply_event(&mut guard, event, notify);
        }
        guard.over_budget()
    }

    fn apply_event(&self, inner: &mut Inner, event: ProxyEvent, notify: bool) {
        let id = event.exchange_id();
        let creating = matches!(
            event,
            ProxyEvent::RequestStarted { .. }
                | ProxyEvent::TlsFailed { .. }
                | ProxyEvent::TunnelOpened { .. }
                | ProxyEvent::ClientError { .. }
        );
        if creating && inner.records.contains_key(&id) {
            tracing::debug!(%id, "duplicate creating event ignored");
            return;
        }
        let created = match event {
            ProxyEvent::RequestStarted {
                id,
                kind,
                head,
                client,
                timing,
            } => {
                if inner.is_ignored(&head.host) {
                    return;
                }
                let client = inner.intern_client(client);
                Record::new_http(id, kind, head, client, timing)
            }
            ProxyEvent::TlsFailed {
                id,
                client,
                host,
                port,
                error,
                at,
            } => {
                if !inner.settings.record_tls_failures || inner.is_ignored(&host) {
                    return;
                }
                let client = inner.intern_client(client);
                Record::new_tls_failure(id, client, host, port, error, at)
            }
            ProxyEvent::TunnelOpened {
                id,
                client,
                host,
                port,
                reason,
                at,
            } => {
                if !inner.settings.record_tunnels || inner.is_ignored(&host) {
                    return;
                }
                let client = inner.intern_client(client);
                Record::new_tunnel(id, client, host, port, reason, at)
            }
            ProxyEvent::ClientError {
                id,
                client,
                error,
                raw_prefix,
                at,
            } => {
                let client = inner.intern_client(client);
                let max_body = inner.settings.max_body_bytes;
                Record::new_client_error(id, client, error, raw_prefix, at, max_body)
            }
            other => {
                self.update(inner, id, other, notify);
                return;
            }
        };
        self.add_record(inner, Box::new(created), notify);
    }

    fn update(&self, inner: &mut Inner, id: ExchangeId, event: ProxyEvent, notify: bool) {
        let Inner {
            records,
            ledger,
            settings,
            generation,
            ..
        } = inner;
        let Some(rec) = records.get_mut(&id) else {
            tracing::trace!(%id, "event for unknown exchange ignored");
            return;
        };
        let mut ctx = ApplyCtx {
            ledger,
            spill: &self.spill,
            max_body: settings.max_body_bytes,
        };
        let outcome = rec.apply(event, &mut ctx);
        if outcome != Outcome::Unchanged {
            *generation += 1;
        }
        if !notify {
            return;
        }
        match outcome {
            Outcome::Unchanged => {}
            Outcome::Changed | Outcome::Completed => {
                self.notify(StoreEvent::Updated(rec.summary.clone()))
            }
            Outcome::Message(message) => {
                self.notify(StoreEvent::Message { id, message });
                self.notify(StoreEvent::Updated(rec.summary.clone()));
            }
        }
    }

    /// Spill the oldest in-memory bodies until memory is back under ~7/8 of the budget.
    ///
    /// Three phases keep the records lock short: select candidates (write lock, cheap
    /// `Bytes` clones), compress + write them with no store lock held, then swap in the
    /// spill handles if the bodies are still the same (otherwise the blob is released).
    fn maybe_spill(&self) {
        let Some(mut gate) = self.spill_gate.try_lock() else {
            return;
        };
        if let Some(until) = *gate {
            if Instant::now() < until {
                return;
            }
            *gate = None;
        }

        let batch = {
            let mut guard = self.inner.write();
            let inner = &mut *guard;
            let budget = inner.settings.memory_budget_bytes;
            if inner.ledger.memory_bytes <= budget {
                return;
            }
            let target = budget - budget / 8;
            let mut excess = inner.ledger.memory_bytes - target;
            let mut batch = Vec::new();
            while excess > 0 {
                let Some((id, slot)) = inner.ledger.mem_index.pop_first() else {
                    break;
                };
                let Some(rec) = inner.records.get(&id) else {
                    continue;
                };
                if slot == Slot::Messages {
                    let parts: Vec<(usize, Bytes)> = rec
                        .messages
                        .iter()
                        .enumerate()
                        .filter_map(|(i, m)| match &m.payload {
                            Payload::Memory(b) if !b.is_empty() => Some((i, b.clone())),
                            _ => None,
                        })
                        .collect();
                    let total: u64 = parts.iter().map(|(_, b)| b.len() as u64).sum();
                    if total > 0 {
                        excess = excess.saturating_sub(total);
                        batch.push(Candidate::Messages { id, parts });
                    }
                } else if let Some(bytes) = rec.slot(slot).and_then(StoredBody::memory_bytes) {
                    excess = excess.saturating_sub(bytes.len() as u64);
                    batch.push(Candidate::Body {
                        id,
                        slot,
                        bytes: bytes.clone(),
                    });
                }
            }
            batch
        };
        if batch.is_empty() {
            return;
        }

        let mut failed = false;
        let written: Vec<(Candidate, Option<SpillHandle>)> = batch
            .into_iter()
            .map(|c| {
                if failed {
                    return (c, None);
                }
                let result = match &c {
                    Candidate::Body { bytes, .. } => self.spill.write(bytes),
                    Candidate::Messages { parts, .. } => {
                        let mut buf = Vec::with_capacity(parts.iter().map(|(_, b)| b.len()).sum());
                        for (_, b) in parts {
                            buf.extend_from_slice(b);
                        }
                        self.spill.write(&buf)
                    }
                };
                match result {
                    Ok(handle) => (c, Some(handle)),
                    Err(e) => {
                        tracing::debug!(error = %e, "body spill failed");
                        failed = true;
                        (c, None)
                    }
                }
            })
            .collect();

        {
            let mut guard = self.inner.write();
            let Inner {
                records, ledger, ..
            } = &mut *guard;
            for (candidate, handle) in written {
                match (candidate, handle) {
                    (Candidate::Body { id, slot, bytes }, Some(handle)) => {
                        let body = records.get_mut(&id).and_then(|r| r.slot_mut(slot)).filter(
                            |b| matches!(&b.data, BodyData::Memory(m) if same_bytes(m, &bytes)),
                        );
                        match body {
                            Some(body) => {
                                body.data = BodyData::Spilled(handle);
                                ledger.memory_bytes =
                                    ledger.memory_bytes.saturating_sub(bytes.len() as u64);
                                ledger.spilled_bytes += bytes.len() as u64;
                                ledger.spill_index.insert((id, slot));
                            }
                            // Deleted or replaced meanwhile: the blob is garbage.
                            None => self.spill.free(&handle),
                        }
                    }
                    (Candidate::Body { id, slot, bytes }, None) => {
                        let still_there = records
                            .get(&id)
                            .and_then(|r| r.slot(slot))
                            .and_then(StoredBody::memory_bytes)
                            .is_some_and(|b| same_bytes(b, &bytes));
                        if still_there {
                            ledger.mem_index.insert((id, slot));
                        }
                    }
                    (Candidate::Messages { id, parts }, Some(handle)) => {
                        let Some(rec) = records.get_mut(&id) else {
                            self.spill.free(&handle);
                            continue;
                        };
                        let blob = u32::try_from(rec.extra().map_or(0, |e| e.msg_blobs.len()))
                            .unwrap_or(u32::MAX);
                        let mut offset = 0u64;
                        let mut swapped = 0u64;
                        for (index, bytes) in &parts {
                            let len = bytes.len() as u64;
                            if let Some(m) = rec.messages.get_mut(*index)
                                && matches!(&m.payload, Payload::Memory(b) if same_bytes(b, bytes))
                            {
                                m.payload = Payload::Spilled { blob, offset, len };
                                swapped += len;
                            }
                            offset += len;
                        }
                        if swapped > 0 {
                            rec.extra_mut().msg_blobs.push(handle);
                            ledger.memory_bytes = ledger.memory_bytes.saturating_sub(swapped);
                            ledger.spilled_bytes += swapped;
                            ledger.spill_index.insert((id, Slot::Messages));
                        } else {
                            self.spill.free(&handle);
                        }
                    }
                    (Candidate::Messages { id, .. }, None) => {
                        let pending = records.get(&id).is_some_and(|r| {
                            r.messages
                                .iter()
                                .any(|m| matches!(&m.payload, Payload::Memory(b) if !b.is_empty()))
                        });
                        if pending {
                            ledger.mem_index.insert((id, Slot::Messages));
                        }
                    }
                }
            }
        }
        if failed {
            *gate = Some(Instant::now() + SPILL_RETRY_AFTER);
        }
        drop(gate);
        self.enforce_disk_cap();
    }

    /// Keep the spill files within `spill_max_bytes`: delete the oldest spilled bodies
    /// (pinned exchanges' are kept) until disk use is back under ~7/8 of the cap. The
    /// exchanges stay, with their bodies marked dropped and a notice saying why. Space is
    /// reclaimed per spill segment, so this drops until whole segments drain.
    fn enforce_disk_cap(&self) {
        let cap = self.inner.read().settings.spill_max_bytes;
        if cap == 0 || self.spill.disk_bytes() <= cap {
            return;
        }
        let target = cap - cap / 8;
        let mut changed = Vec::new();
        {
            let mut guard = self.inner.write();
            let Inner {
                records,
                ledger,
                disk_dropped,
                generation,
                ..
            } = &mut *guard;
            let mut kept = Vec::new();
            while self.spill.disk_bytes() > target {
                let Some((id, slot)) = ledger.spill_index.pop_first() else {
                    break;
                };
                let Some(rec) = records.get_mut(&id) else {
                    continue;
                };
                if rec.summary.pinned {
                    kept.push((id, slot));
                    continue;
                }
                if rec.drop_spilled(slot, ledger, &self.spill) > 0 {
                    *disk_dropped += 1;
                    if changed.last() != Some(&id) {
                        changed.push(id);
                    }
                }
            }
            ledger.spill_index.extend(kept);
            if !changed.is_empty() {
                *generation += 1;
                if self.has_listeners() {
                    for id in &changed {
                        if let Some(rec) = records.get(id) {
                            self.notify(StoreEvent::Updated(rec.summary.clone()));
                        }
                    }
                }
            }
        }
        if !changed.is_empty() {
            tracing::info!(
                exchanges = changed.len(),
                "deleted the oldest spilled bodies to stay within the spill disk limit"
            );
        }
    }

    /// A store whose spill segments are sealed at `segment_target` bytes (small segments
    /// make the disk cap testable).
    #[cfg(test)]
    fn with_segment_target(
        settings: CaptureSettings,
        spill_dir: Option<&Path>,
        segment_target: u64,
    ) -> Arc<Store> {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Arc::new(Store {
            inner: RwLock::new(Inner::new(settings)),
            spill: Spill::with_segment_target(spill_dir, segment_target),
            events,
            sink: Mutex::new(None),
            spill_gate: Mutex::new(None),
            suggest_cache: Mutex::new(None),
            rule_titles: RwLock::new(Arc::new(view::RuleTitles::new())),
        })
    }

    #[cfg(test)]
    fn spill_disk_bytes(&self) -> u64 {
        self.spill.disk_bytes()
    }
}

#[cfg(test)]
mod tests;
