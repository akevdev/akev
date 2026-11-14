/**
 * TrafficStore — the high-performance external store behind the traffic list
 * (docs/DESIGN.md §9).
 *
 * - `summaries`: `Map<id, ExchangeSummary>`; summary objects are replaced (never mutated) on
 *   update, so row components can memoise on object identity.
 * - `order`: all ids ascending (== start order).
 * - `visible`: ids matching the server-side filter (from `filter-result` + incremental
 *   `filtered` deltas), ascending.
 * - Publishing is O(1) for appends: the snapshot's `order`/`visible` are fresh views (new
 *   identity) of arrays the store grows in place, not copies (see {@link liveView}).
 * - A paged `hello` (`remaining` > 0) is followed by `hello-more` chunks of older summaries,
 *   merged in front with {@link TrafficStore.prependOlder}.
 * - Stream messages mutate internal state and schedule ONE notification per animation frame;
 *   user actions (selection) notify synchronously so the UI responds within the same frame.
 * - Components read it with `useSyncExternalStore` through the hooks at the bottom
 *   (`useTraffic(selector)`, `useExchangeSummary(id)`, `useSelection()`...).
 *
 * The store never talks to the network itself: the filter query is handed to a
 * {@link FilterSink} (wired to `StreamClient.setFilter` in `state/connection.ts`).
 */

import { useRef, useSyncExternalStore } from 'react';
import type { ExchangeId, ExchangeSummary, FilterError, FilteredDelta, StreamMessage } from '../api/types';
import { typeChipClause, type TypeChip } from '../lib/http';
import { utf8Length } from '../lib/encoding';

// =====================================================================================
// Types
// =====================================================================================

export interface FilterState {
  /** The user's query text (filter input). */
  text: string;
  /** Active type chip. */
  chip: TypeChip;
  /** Full query sent to the daemon (user text AND chip clause). */
  query: string;
  /** Sequence number of the latest query sent (0 = none). */
  seq: number;
  /** True while waiting for the `filter-result` of the latest query. */
  pending: boolean;
  /**
   * Parse error for the user's text, if any. `start`/`end` are UTF-8 byte offsets into
   * `text` (convert with `byteOffsetToIndex`).
   */
  error: FilterError | null;
  /** Whether any filtering is in effect (text or chip). */
  active: boolean;
}

export interface SelectionState {
  /** Selected ids in selection order (insertion order of the Set). */
  readonly ids: ReadonlySet<ExchangeId>;
  /** Anchor for shift-range selection. */
  readonly anchor: ExchangeId | null;
  /** Keyboard-focus row (usually, not always, selected). */
  readonly active: ExchangeId | null;
}

export interface TrafficSnapshot {
  /** Increments on every published change. */
  readonly version: number;
  /** All known summaries. The Map instance is stable; look rows up by id. */
  readonly summaries: ReadonlyMap<ExchangeId, ExchangeSummary>;
  /**
   * All ids, ascending. A new identity on every change, but a live view: rows appended after
   * this snapshot was published may already show in it (treat it as read-only, as of now).
   */
  readonly order: readonly ExchangeId[];
  /** Ids matching the filter, ascending. Identical to `order` when unfiltered. Same live-view caveat. */
  readonly visible: readonly ExchangeId[];
  readonly filter: FilterState;
  readonly selection: SelectionState;
  /** `hello` received at least once (initial snapshot loaded). */
  readonly loaded: boolean;
  /** Older exchanges of a paged `hello` still on their way (0 when the snapshot is complete). */
  readonly loadingOlder: number;
}

/** Receives the composed query to send to the daemon; returns the sequence number used. */
export type FilterSink = (query: string) => number;

/**
 * Store-wide changes that invalidate per-exchange data held elsewhere (body caches, message
 * lists): a `reset` is a full resync from `hello` (reconnect or overload — anything streamed in
 * between was not delivered), `removed`/`cleared` drop exchanges.
 */
export type TrafficLifecycleEvent = { type: 'reset' } | { type: 'removed'; ids: readonly ExchangeId[] } | { type: 'cleared' };

/** Schedules a flush (default: `requestAnimationFrame`, `setTimeout` fallback). */
export type FlushScheduler = (flush: () => void) => void;

const defaultScheduler: FlushScheduler = (flush) => {
  // Hidden tabs don't run animation frames.
  if (document.visibilityState !== 'hidden') {
    requestAnimationFrame(flush);
  } else {
    setTimeout(flush, 16);
  }
};

const EMPTY_SELECTION: SelectionState = Object.freeze({ ids: new Set<ExchangeId>(), anchor: null, active: null });

const VIEW_HANDLER: ProxyHandler<number[]> = {};

/**
 * A new identity for `arr` in O(1): a transparent proxy reading the same elements. The store
 * appends to its arrays in place and publishes a fresh view per change, so a frame that adds
 * k rows costs O(k), not a copy of every id (which at 1M rows and 60 frames/s would be the
 * dominant cost). Consumers that memoise on identity still see every change.
 */
export function liveView(arr: number[]): readonly number[] {
  return new Proxy(arr, VIEW_HANDLER);
}

// =====================================================================================
// Pure helpers (exported for tests and pages)
// =====================================================================================

/** Binary search in an ascending id array. Returns the index or -1. */
export function indexOfSorted(arr: readonly number[], id: number): number {
  let lo = 0;
  let hi = arr.length - 1;
  while (lo <= hi) {
    const mid = (lo + hi) >>> 1;
    const v = arr[mid] as number;
    if (v === id) return mid;
    if (v < id) lo = mid + 1;
    else hi = mid - 1;
  }
  return -1;
}

/** Insertion point for `id` in an ascending array. */
export function insertionIndex(arr: readonly number[], id: number): number {
  let lo = 0;
  let hi = arr.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((arr[mid] as number) < id) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}

const indexCache = new WeakMap<readonly number[], Map<number, number>>();

/**
 * Index of `id` in an arbitrary display list (e.g. a column-sorted view). Ascending lists use
 * binary search; others build an id→index map once per array identity (cached weakly).
 */
export function indexInList(list: readonly number[], id: number, sortedAscending = false): number {
  if (sortedAscending) return indexOfSorted(list, id);
  let map = indexCache.get(list);
  if (!map) {
    map = new Map();
    for (let i = 0; i < list.length; i++) map.set(list[i] as number, i);
    indexCache.set(list, map);
  }
  return map.get(id) ?? -1;
}

/**
 * Compose the daemon query from the user's text and a type chip. Returns the query and the
 * byte offset where the user's text starts inside it (for mapping error spans back).
 */
export function composeFilterQuery(text: string, chip: TypeChip): { query: string; offset: number } {
  const clause = typeChipClause(chip);
  const hasText = text.trim().length > 0;
  if (!clause) return { query: hasText ? text : '', offset: 0 };
  if (!hasText) return { query: clause, offset: 0 };
  return { query: `(${text}) ${clause}`, offset: 1 };
}

/** Map a daemon error span (bytes into the composed query) back into the user's text. */
export function mapFilterError(error: FilterError, text: string, offset: number): FilterError {
  const len = utf8Length(text);
  const clamp = (n: number) => Math.max(0, Math.min(len, n));
  if (error.start - offset > len) return { message: error.message, start: 0, end: len };
  return { message: error.message, start: clamp(error.start - offset), end: clamp(error.end - offset) };
}

/**
 * Range of ids between `anchor` and `target` (inclusive) in `list`, ordered from the anchor
 * towards the target. Returns `null` if either is not in the list.
 */
export function rangeBetween(list: readonly number[], anchor: number, target: number, sortedAscending = false): number[] | null {
  const a = indexInList(list, anchor, sortedAscending);
  const b = indexInList(list, target, sortedAscending);
  if (a < 0 || b < 0) return null;
  const out: number[] = [];
  if (a <= b) for (let i = a; i <= b; i++) out.push(list[i] as number);
  else for (let i = a; i >= b; i--) out.push(list[i] as number);
  return out;
}

// =====================================================================================
// Store
// =====================================================================================

export class TrafficStore {
  private readonly summaries = new Map<ExchangeId, ExchangeSummary>();
  private order: number[] = [];
  /** `null` = filter matches everything. May contain ids not (yet) known locally. */
  private filterIds: Set<number> | null = null;
  private visibleArr: number[] = [];
  private visibleNeedsRebuild = false;

  private orderDirty = false;
  private visibleDirty = false;

  private filterState: FilterState = {
    text: '',
    chip: 'all',
    query: '',
    seq: 0,
    pending: false,
    error: null,
    active: false,
  };
  private filterOffset = 0;
  /** True while `filterSink` is being called (see `applyFilterResult`). */
  private sendingFilter = false;
  private earlyFilterResult: { seq: number; ids: readonly ExchangeId[] | null; error?: FilterError } | null = null;
  private filterTimer: ReturnType<typeof setTimeout> | null = null;
  private filterSink: FilterSink | null = null;

  private selection: SelectionState = EMPTY_SELECTION;
  /** Selection snapshot taken at the start of a shift-extension sequence. */
  private selectionBase: ReadonlySet<number> | null = null;

  private loaded = false;
  /** Older summaries of a paged `hello` still to come. */
  private loadingOlder = 0;
  /** While paging: the oldest id delivered so far (older ones are still to come), else `null`. */
  private pagingBelow: number | null = null;
  private version = 0;
  private snapshot: TrafficSnapshot;
  private scheduled = false;
  private readonly listeners = new Set<() => void>();
  private readonly messageListeners = new Map<ExchangeId, Set<(messages: StreamMessage[]) => void>>();
  private readonly lifecycleListeners = new Set<(event: TrafficLifecycleEvent) => void>();

  constructor(
    private readonly scheduler: FlushScheduler = defaultScheduler,
    /** Debounce for sending filter queries (DESIGN.md §9: 120 ms). */
    private readonly filterDebounceMs = 120,
  ) {
    this.snapshot = this.buildSnapshot();
  }

  // ---------------------------------------------------------------- subscription API

  /** Subscribe to published snapshots (for `useSyncExternalStore`). */
  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  /** Current immutable snapshot (stable between notifications). */
  readonly getSnapshot = (): TrafficSnapshot => this.snapshot;

  /** Summary by id (live data, may be newer than the last snapshot). */
  get(id: ExchangeId): ExchangeSummary | undefined {
    return this.summaries.get(id);
  }

  /** Subscribe to new WebSocket/SSE message notifications for one exchange. */
  onMessages(id: ExchangeId, listener: (messages: StreamMessage[]) => void): () => void {
    let set = this.messageListeners.get(id);
    if (!set) {
      set = new Set();
      this.messageListeners.set(id, set);
    }
    set.add(listener);
    return () => {
      set.delete(listener);
      if (set.size === 0) this.messageListeners.delete(id);
    };
  }

  /** Subscribe to resyncs / removals / clears (see {@link TrafficLifecycleEvent}). */
  onLifecycle(listener: (event: TrafficLifecycleEvent) => void): () => void {
    this.lifecycleListeners.add(listener);
    return () => this.lifecycleListeners.delete(listener);
  }

  private emitLifecycle(event: TrafficLifecycleEvent): void {
    for (const l of [...this.lifecycleListeners]) {
      try {
        l(event);
      } catch (e) {
        console.error('[akev] traffic lifecycle listener failed', e);
      }
    }
  }

  /**
   * Connect the store to the stream's `setFilter`. The stream re-sends the current query (with
   * the same `seq`) after every reconnect, so the store never needs to resend.
   */
  setFilterSink(sink: FilterSink | null): void {
    this.filterSink = sink;
  }

  // ---------------------------------------------------------------- stream ingestion

  /**
   * Replace everything with a `hello` snapshot (full resync after (re)connect). `remaining` =
   * older summaries still to come in `hello-more` chunks ({@link prependOlder}).
   */
  reset(summaries: readonly ExchangeSummary[], remaining = 0): void {
    this.loadingOlder = Math.max(0, remaining);
    this.summaries.clear();
    for (const s of summaries) this.summaries.set(s.id, s);
    this.order = summaries.map((s) => s.id);
    // The daemon sends summaries in id order; sort defensively if not.
    for (let i = 1; i < this.order.length; i++) {
      if ((this.order[i] as number) < (this.order[i - 1] as number)) {
        this.order.sort((a, b) => a - b);
        break;
      }
    }
    this.pagingBelow = this.loadingOlder > 0 ? ((this.order[0] as number | undefined) ?? Infinity) : null;
    // The server-side filter is per-connection, so a new connection starts unfiltered. Keep
    // showing the previous filtered set (intersected with the new data) until the re-sent
    // query's result arrives, instead of flashing unfiltered rows.
    if (this.filterState.query) {
      this.filterState = { ...this.filterState, pending: true };
    } else {
      this.filterIds = null;
    }
    this.visibleNeedsRebuild = true;
    this.pruneSelection();
    this.loaded = true;
    this.orderDirty = this.visibleDirty = true;
    this.schedule();
    this.emitLifecycle({ type: 'reset' });
  }

  /**
   * Merge a `hello-more` chunk: summaries older than everything delivered so far (ids removed in
   * the meantime are already omitted by the daemon). Rows already known (e.g. from a racing
   * `updated`) take the chunk's version, which was read later. O(n) per chunk.
   */
  prependOlder(summaries: readonly ExchangeSummary[], remaining: number): void {
    this.loadingOlder = Math.max(0, remaining);
    const fresh: number[] = [];
    let lowest = this.pagingBelow ?? Infinity;
    for (const s of summaries) {
      if (!this.summaries.has(s.id)) fresh.push(s.id);
      this.summaries.set(s.id, s);
      if (s.id < lowest) lowest = s.id;
    }
    this.pagingBelow = this.loadingOlder > 0 ? lowest : null;
    if (fresh.length) {
      fresh.sort((a, b) => a - b);
      const first = this.order.length ? (this.order[0] as number) : Infinity;
      if ((fresh[fresh.length - 1] as number) < first) {
        this.order = fresh.concat(this.order);
        if (!this.visibleNeedsRebuild) {
          const ids = this.filterIds;
          const head = ids === null ? fresh : fresh.filter((id) => ids.has(id));
          if (head.length) this.visibleArr = head.concat(this.visibleArr);
        }
      } else {
        // Not strictly older (shouldn't happen): merge and rebuild the visible set.
        this.order = this.order.concat(fresh).sort((a, b) => a - b);
        this.visibleNeedsRebuild = true;
      }
      this.orderDirty = this.visibleDirty = true;
    }
    this.schedule();
  }

  /** Apply an `exchanges` batch. */
  applyExchanges(added: readonly ExchangeSummary[], updated: readonly ExchangeSummary[], filtered?: FilteredDelta): void {
    if (added.length === 0 && updated.length === 0 && !filtered) return;
    const last = (): number => (this.order.length ? (this.order[this.order.length - 1] as number) : -Infinity);

    for (const s of added) {
      const exists = this.summaries.has(s.id);
      this.summaries.set(s.id, s);
      if (exists) continue;
      if (s.id > last()) this.order.push(s.id);
      else this.order.splice(insertionIndex(this.order, s.id), 0, s.id);
      this.orderDirty = true;
      if (this.filterIds === null || this.filterIds.has(s.id)) this.addVisible(s.id);
    }
    for (const s of updated) {
      if (!this.summaries.has(s.id)) {
        // Not delivered yet by a paged `hello`: its chunk (read later) brings the newer version.
        if (this.pagingBelow !== null && s.id < this.pagingBelow) continue;
        // Unknown id (e.g. missed `added` during a race): treat as added.
        this.summaries.set(s.id, s);
        this.order.splice(insertionIndex(this.order, s.id), 0, s.id);
        this.orderDirty = true;
        if (this.filterIds === null || this.filterIds.has(s.id)) this.addVisible(s.id);
      } else {
        this.summaries.set(s.id, s);
      }
    }
    if (filtered && this.filterIds !== null) {
      for (const id of filtered.add) {
        this.filterIds.add(id);
        if (this.summaries.has(id)) this.addVisible(id);
      }
      for (const id of filtered.remove) {
        this.filterIds.delete(id);
        this.removeVisible(id);
      }
    }
    this.schedule();
  }

  /** Apply a `removed` message. */
  remove(ids: readonly ExchangeId[]): void {
    if (ids.length === 0) return;
    this.emitLifecycle({ type: 'removed', ids });
    const set = new Set(ids);
    let any = false;
    for (const id of ids) {
      if (this.summaries.delete(id)) any = true;
      this.filterIds?.delete(id);
    }
    if (!any) return;
    if (ids.length <= 8) {
      for (const id of ids) {
        const i = indexOfSorted(this.order, id);
        if (i >= 0) this.order.splice(i, 1);
        this.removeVisible(id);
      }
    } else {
      this.order = this.order.filter((id) => !set.has(id));
      this.visibleNeedsRebuild = true;
    }
    for (const id of ids) this.messageListeners.delete(id);
    this.pruneSelection();
    this.orderDirty = this.visibleDirty = true;
    this.schedule();
  }

  /** Apply a `cleared` message. */
  clear(): void {
    this.loadingOlder = 0;
    this.pagingBelow = null;
    this.summaries.clear();
    this.order = [];
    this.visibleArr = [];
    if (this.filterIds) this.filterIds = new Set();
    this.messageListeners.clear();
    this.selection = EMPTY_SELECTION;
    this.selectionBase = null;
    this.orderDirty = this.visibleDirty = true;
    this.schedule();
    this.emitLifecycle({ type: 'cleared' });
  }

  /** Apply a `filter-result`. Results for superseded queries are ignored. */
  applyFilterResult(seq: number, ids: readonly ExchangeId[] | null, error?: FilterError): void {
    if (this.sendingFilter) {
      // The sink answered synchronously (e.g. the in-browser mock) before `sendFilter` could
      // record the new seq; apply it right after.
      this.earlyFilterResult = { seq, ids, error };
      return;
    }
    if (seq !== this.filterState.seq) return;
    if (error) {
      // The daemon keeps its previous filter and reports that filter's ids (`null` = none, as on
      // a fresh connection). Adopt them: incremental `filtered` deltas are relative to the
      // daemon's set, so keeping a different local set (e.g. one from before a reconnect)
      // would freeze the list.
      this.filterIds = ids === null ? null : new Set(ids);
      this.visibleNeedsRebuild = true;
      this.visibleDirty = true;
      this.filterState = {
        ...this.filterState,
        pending: false,
        error: mapFilterError(error, this.filterState.text, this.filterOffset),
      };
    } else {
      this.filterIds = ids === null ? null : new Set(ids);
      this.visibleNeedsRebuild = true;
      this.visibleDirty = true;
      this.filterState = { ...this.filterState, pending: false, error: null };
    }
    this.schedule();
  }

  /** Forward a `messages` notification to listeners of that exchange. */
  notifyMessages(id: ExchangeId, messages: StreamMessage[]): void {
    const set = this.messageListeners.get(id);
    if (!set) return;
    for (const l of set) {
      try {
        l(messages);
      } catch (e) {
        console.error('[akev] message listener failed', e);
      }
    }
  }

  // ---------------------------------------------------------------- filter (user)

  /** Update the filter text (debounced send). */
  setFilterText(text: string): void {
    if (text === this.filterState.text) return;
    this.updateFilter(text, this.filterState.chip);
  }

  /** Update the type chip (sent immediately). */
  setTypeChip(chip: TypeChip): void {
    if (chip === this.filterState.chip) return;
    this.updateFilter(this.filterState.text, chip, true);
  }

  /** Clear text and chip. */
  clearFilter(): void {
    this.updateFilter('', 'all', true);
  }

  /** Send any debounced filter change now (e.g. on Enter). */
  flushFilter(): void {
    if (this.filterTimer != null) {
      clearTimeout(this.filterTimer);
      this.filterTimer = null;
      this.sendFilter();
    }
  }

  private updateFilter(text: string, chip: TypeChip, immediate = false): void {
    const { query, offset } = composeFilterQuery(text, chip);
    const changed = query !== this.filterState.query;
    this.filterOffset = offset;
    // Keep the previous error until the daemon answers, so the underline doesn't flicker.
    this.filterState = { ...this.filterState, text, chip, query, active: query.length > 0 };
    if (changed && query === '') {
      // Clearing the filter is instant locally; the daemon confirms with ids: null.
      this.filterIds = null;
      this.visibleNeedsRebuild = true;
      this.visibleDirty = true;
      this.filterState = { ...this.filterState, error: null };
    }
    if (this.filterTimer != null) {
      clearTimeout(this.filterTimer);
      this.filterTimer = null;
    }
    if (changed) {
      this.filterState = { ...this.filterState, pending: true };
      if (immediate || this.filterDebounceMs <= 0) this.sendFilter();
      else
        this.filterTimer = setTimeout(() => {
          this.filterTimer = null;
          this.sendFilter();
        }, this.filterDebounceMs);
    }
    this.flushNow();
  }

  private sendFilter(): void {
    const query = this.filterState.query;
    this.sendingFilter = true;
    let seq: number;
    try {
      seq = this.filterSink ? this.filterSink(query) : this.filterState.seq + 1;
    } finally {
      this.sendingFilter = false;
    }
    this.filterState = { ...this.filterState, seq, pending: this.filterSink != null };
    if (!this.filterSink) {
      // Offline/unwired: nothing will answer; don't stay pending forever.
      this.filterState = { ...this.filterState, pending: false };
    }
    const early = this.earlyFilterResult;
    this.earlyFilterResult = null;
    if (early) this.applyFilterResult(early.seq, early.ids, early.error);
    this.schedule();
  }

  // ---------------------------------------------------------------- selection (user)

  /** Select exactly one row (plain click / arrow keys). `null` clears. */
  select(id: ExchangeId | null): void {
    this.selectionBase = null;
    if (id === null) return this.setSelection(EMPTY_SELECTION);
    this.setSelection({ ids: new Set([id]), anchor: id, active: id });
  }

  /** Ctrl/⌘-click: toggle a row, making it the anchor and active row. */
  toggle(id: ExchangeId): void {
    this.selectionBase = null;
    const ids = new Set(this.selection.ids);
    if (ids.has(id)) ids.delete(id);
    else ids.add(id);
    this.setSelection({ ids, anchor: id, active: id });
  }

  /**
   * Shift-click / Shift+arrows: select the range from the anchor to `id` in `list`
   * (default: visible order), unioned with the selection as it was when the shift sequence
   * started — so repeated extensions grow and shrink the range while keeping earlier
   * Ctrl-picks. Falls back to a single select without a usable anchor.
   */
  extendTo(id: ExchangeId, list?: readonly ExchangeId[]): void {
    const anchor = this.selection.anchor;
    const displayed = list ?? this.snapshot.visible;
    const sorted = list === undefined;
    if (anchor == null) return this.select(id);
    const range = rangeBetween(displayed, anchor, id, sorted);
    if (!range) return this.select(id);
    if (this.selectionBase === null) {
      const base = new Set(this.selection.ids);
      // The previous single selection at the anchor is part of the range, not the base.
      base.delete(anchor);
      this.selectionBase = base;
    }
    const ids = new Set<number>(this.selectionBase);
    for (const r of range) ids.add(r);
    this.setSelection({ ids, anchor, active: id });
  }

  /**
   * Keyboard navigation. `delta` rows (±Infinity for Home/End). With no active row, a
   * positive delta selects the first row and a negative one the last. Returns the new active
   * id (so the caller can scroll it into view), or `null` if the list is empty.
   */
  move(delta: number, extend = false, list?: readonly ExchangeId[]): ExchangeId | null {
    const displayed = list ?? this.snapshot.visible;
    if (displayed.length === 0) return null;
    const active = this.selection.active;
    const current = active == null ? -1 : indexInList(displayed, active, list === undefined);
    let next: number;
    if (current < 0) next = delta >= 0 ? 0 : displayed.length - 1;
    else if (delta === Infinity) next = displayed.length - 1;
    else if (delta === -Infinity) next = 0;
    else next = Math.max(0, Math.min(displayed.length - 1, current + delta));
    const id = displayed[next] as number;
    if (extend) this.extendTo(id, list);
    else this.select(id);
    return id;
  }

  /** Ctrl/⌘+A: select every row in `list` (default: visible). */
  selectAll(list?: readonly ExchangeId[]): void {
    const displayed = list ?? this.snapshot.visible;
    this.selectionBase = null;
    if (displayed.length === 0) return this.setSelection(EMPTY_SELECTION);
    const active = this.selection.active ?? (displayed[0] as number);
    this.setSelection({ ids: new Set(displayed), anchor: this.selection.anchor ?? active, active });
  }

  /** Esc: clear the selection. */
  clearSelection(): void {
    this.select(null);
  }

  /** Replace the selection wholesale (e.g. restoring from the URL). */
  setSelectedIds(ids: Iterable<ExchangeId>, active: ExchangeId | null = null): void {
    this.selectionBase = null;
    const set = new Set(ids);
    this.setSelection({ ids: set, anchor: active, active });
  }

  /** Selected ids in visible (id) order — handy for bulk actions. */
  selectedInOrder(): ExchangeId[] {
    return [...this.selection.ids].filter((id) => this.summaries.has(id)).sort((a, b) => a - b);
  }

  private setSelection(next: SelectionState): void {
    this.selection = next;
    this.flushNow();
  }

  private pruneSelection(): void {
    const { ids, anchor, active } = this.selection;
    if (ids.size === 0 && anchor == null && active == null) return;
    let changed = false;
    const kept = new Set<number>();
    for (const id of ids) {
      if (this.summaries.has(id)) kept.add(id);
      else changed = true;
    }
    const nextAnchor = anchor != null && this.summaries.has(anchor) ? anchor : null;
    const nextActive = active != null && this.summaries.has(active) ? active : null;
    if (changed || nextAnchor !== anchor || nextActive !== active) {
      this.selection = { ids: kept, anchor: nextAnchor, active: nextActive };
      this.selectionBase = null;
    }
  }

  // ---------------------------------------------------------------- visible set internals

  private addVisible(id: number): void {
    if (this.visibleNeedsRebuild) return;
    const arr = this.visibleArr;
    const lastId = arr.length ? (arr[arr.length - 1] as number) : -Infinity;
    if (id > lastId) arr.push(id);
    else {
      const i = insertionIndex(arr, id);
      if (arr[i] === id) return;
      arr.splice(i, 0, id);
    }
    this.visibleDirty = true;
  }

  private removeVisible(id: number): void {
    if (this.visibleNeedsRebuild) return;
    const i = indexOfSorted(this.visibleArr, id);
    if (i >= 0) {
      this.visibleArr.splice(i, 1);
      this.visibleDirty = true;
    }
  }

  // ---------------------------------------------------------------- publishing

  private schedule(): void {
    if (this.scheduled) return;
    this.scheduled = true;
    this.scheduler(() => this.flushNow());
  }

  /** Publish pending changes synchronously. */
  flushNow(): void {
    this.scheduled = false;
    if (this.visibleNeedsRebuild) {
      const ids = this.filterIds;
      this.visibleArr = ids === null ? this.order.slice() : this.order.filter((id) => ids.has(id));
      this.visibleNeedsRebuild = false;
      this.visibleDirty = true;
    }
    this.version++;
    this.snapshot = this.buildSnapshot();
    this.orderDirty = this.visibleDirty = false;
    for (const l of this.listeners) l();
  }

  private buildSnapshot(): TrafficSnapshot {
    const prev = this.snapshot as TrafficSnapshot | undefined;
    // O(1): new views of the live arrays, never copies (see `liveView`).
    const order = !prev || this.orderDirty ? liveView(this.order) : prev.order;
    let visible: readonly number[];
    if (!prev || this.visibleDirty || this.orderDirty) {
      visible = this.filterIds === null && this.visibleArr.length === this.order.length ? order : liveView(this.visibleArr);
    } else {
      visible = prev.visible;
    }
    return {
      version: this.version,
      summaries: this.summaries,
      order,
      visible,
      filter: this.filterState,
      selection: this.selection,
      loaded: this.loaded,
      loadingOlder: this.loadingOlder,
    };
  }
}

/** The app-wide traffic store. */
export const trafficStore = new TrafficStore();

// =====================================================================================
// React hooks
// =====================================================================================

/**
 * Select a slice of the traffic snapshot. Re-renders only when the selected value changes
 * (by `isEqual`, default `Object.is`). Selectors should return stable references (arrays and
 * summaries from the snapshot) or pass a custom `isEqual`.
 *
 * ```ts
 * const visible = useTraffic((s) => s.visible);
 * const counts = useTraffic((s) => [s.visible.length, s.order.length] as const, shallowEqual);
 * ```
 */
export function useTraffic<T>(
  selector: (snapshot: TrafficSnapshot) => T,
  isEqual: (a: T, b: T) => boolean = Object.is,
  store: TrafficStore = trafficStore,
): T {
  // Cache keyed on (snapshot, selector): selectors that close over props (`useIsSelected(id)`)
  // must be re-evaluated when the prop changes even if the store did not — virtualised rows
  // are recycled with different ids. Inline selectors are re-run once per render (cheap) and
  // `isEqual` keeps the previous reference when the result is unchanged.
  const cache = useRef<{ snap: TrafficSnapshot; selector: (snapshot: TrafficSnapshot) => T; value: T } | null>(null);
  const getSelection = (): T => {
    const snap = store.getSnapshot();
    const c = cache.current;
    if (c && c.snap === snap && c.selector === selector) return c.value;
    const next = selector(snap);
    if (c && isEqual(c.value, next)) {
      cache.current = { snap, selector, value: c.value };
      return c.value;
    }
    cache.current = { snap, selector, value: next };
    return next;
  };
  return useSyncExternalStore(store.subscribe, getSelection, getSelection);
}

/** Shallow equality for arrays / plain objects (selector helper). */
export function shallowEqual<T>(a: T, b: T): boolean {
  if (Object.is(a, b)) return true;
  if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!Object.is(a[i], b[i])) return false;
    return true;
  }
  const ka = Object.keys(a);
  const kb = Object.keys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) if (!Object.is((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k])) return false;
  return true;
}

/** One summary; re-renders only when that exchange changes. */
export function useExchangeSummary(id: ExchangeId | null | undefined): ExchangeSummary | undefined {
  return useTraffic((s) => (id == null ? undefined : s.summaries.get(id)));
}

/** `{ visible, total }` counts. */
export function useTrafficCounts(): { visible: number; total: number } {
  return useTraffic((s) => ({ visible: s.visible.length, total: s.order.length }), shallowEqual);
}

export function useFilterState(): FilterState {
  return useTraffic((s) => s.filter);
}

export function useSelection(): SelectionState {
  return useTraffic((s) => s.selection);
}

/** Whether one row is selected (cheap per-row subscription). */
export function useIsSelected(id: ExchangeId): boolean {
  return useTraffic((s) => s.selection.ids.has(id));
}

/** The single selected exchange id, or `null` when zero or several are selected. */
export function useSingleSelectedId(): ExchangeId | null {
  return useTraffic((s) => (s.selection.ids.size === 1 ? (s.selection.ids.values().next().value ?? null) : null));
}
