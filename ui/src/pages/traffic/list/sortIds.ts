/**
 * Client-side column sorting of the visible ids (DESIGN.md §6.3). Sorting is done on a
 * precomputed key array (one pass over the summaries), so 100k rows sort in a few tens of
 * milliseconds. Missing values (pending status/duration, no source…) always sort last, ties
 * keep id order (stable).
 */

import type { ExchangeId, ExchangeSummary } from '../../../api/types';
import { contentKindLabel } from '../../../lib/http';
import type { SortState, TrafficColumnId } from '../../../state/ui';

type Key = number | string | null;

/** Sort key of one summary for a column (`null` = missing, sorts last). */
export function sortKey(s: ExchangeSummary, column: TrafficColumnId, ruleTitle?: (id: string) => string | undefined): Key {
  switch (column) {
    case 'id':
    case 'indicator':
      return s.id;
    case 'started':
      return s.startedAt;
    case 'method':
      return s.method.toUpperCase();
    case 'status':
      if (s.status != null) return s.status;
      if (s.error) return 1000; // errors after every real status
      return null;
    case 'host':
      return s.host.toLowerCase();
    case 'path':
      return s.path.toLowerCase();
    case 'type':
      if (s.llm) return 'LLM';
      return s.contentKind === 'unknown' ? null : contentKindLabel(s.contentKind);
    case 'model':
      return s.llm ? (s.llm.model ?? s.llm.provider).toLowerCase() : null;
    case 'size':
      return s.responseSize;
    case 'time':
      return s.duration ?? null;
    case 'source':
      return s.source ? s.source.toLowerCase() : null;
    case 'protocol':
      return s.httpVersion;
    case 'rule':
      return s.rule ? (ruleTitle?.(s.rule) ?? s.rule).toLowerCase() : null;
    case 'remoteIp':
      return null;
    default:
      // Custom header columns: values are loaded per visible row, so they can't sort.
      return null;
  }
}

/**
 * Dense ranks of the present keys (`ranks[j]` for `keys[present[j]]`, 0 = smallest; equal keys
 * share a rank). Strings rank by code-unit order (like `<`), numbers numerically. Returns the
 * number of distinct keys, or -1 when a column mixes strings and numbers.
 */
function rankKeys(keys: readonly Key[], present: readonly number[], ranks: Float64Array): number {
  const m = present.length;
  if (m === 0) return 0;
  const isString = typeof keys[present[0] as number] === 'string';
  if (isString) {
    const unique = new Set<string>();
    for (let j = 0; j < m; j++) {
      const k = keys[present[j] as number];
      if (typeof k !== 'string') return -1;
      unique.add(k);
    }
    const sorted = [...unique].sort();
    const rank = new Map<string, number>();
    for (let r = 0; r < sorted.length; r++) rank.set(sorted[r] as string, r);
    for (let j = 0; j < m; j++) ranks[j] = rank.get(keys[present[j] as number] as string) as number;
    return sorted.length;
  }
  // Numbers: native sort of a copy, dedupe, then binary-search each key's rank.
  const values = new Float64Array(m);
  for (let j = 0; j < m; j++) {
    const k = keys[present[j] as number];
    if (typeof k !== 'number') return -1;
    values[j] = k;
  }
  values.sort();
  let u = 0;
  for (let j = 0; j < m; j++) if (j === 0 || values[j] !== values[u - 1]) values[u++] = values[j] as number;
  for (let j = 0; j < m; j++) {
    const k = keys[present[j] as number] as number;
    let lo = 0;
    let hi = u - 1;
    while (lo < hi) {
      const mid = (lo + hi) >>> 1;
      if ((values[mid] as number) < k) lo = mid + 1;
      else hi = mid;
    }
    ranks[j] = lo;
  }
  return u;
}

/**
 * Sort `ids` by `sort`. Natural id order is returned as-is (ascending) or reversed
 * (descending) without touching the summaries. Always returns a new array unless the input
 * is already in the requested order.
 *
 * Fast path (every column in practice): keys become dense integer ranks, each row is packed
 * as `rank * n + position` into a `Float64Array` (exact below 2^53) and sorted natively — no
 * comparator calls, ~10× faster than `Array#sort` with a comparator on 100k rows. Packing the
 * position in keeps the sort stable.
 */
export function sortIds(
  ids: readonly ExchangeId[],
  summaries: ReadonlyMap<ExchangeId, ExchangeSummary>,
  sort: SortState | null,
  ruleTitle?: (id: string) => string | undefined,
): readonly ExchangeId[] {
  return sortWithKeys(ids, summaries, sort, ruleTitle).sorted;
}

/** {@link sortIds}, also returning the computed keys (aligned with `ids`; `null` for natural order). */
function sortWithKeys(
  ids: readonly ExchangeId[],
  summaries: ReadonlyMap<ExchangeId, ExchangeSummary>,
  sort: SortState | null,
  ruleTitle?: (id: string) => string | undefined,
): { sorted: readonly ExchangeId[]; keys: readonly Key[] | null } {
  if (!sort) return { sorted: ids, keys: null };
  const { column, direction } = sort;
  const dir = direction === 'asc' ? 1 : -1;
  if (column === 'id' || column === 'started' || column === 'indicator') {
    // `startedAt` follows id order (ids are assigned at start).
    return { sorted: dir === 1 ? ids : ids.slice().reverse(), keys: null };
  }
  const n = ids.length;
  const keys: Key[] = new Array(n);
  const present: number[] = [];
  const missing: number[] = [];
  for (let i = 0; i < n; i++) {
    const s = summaries.get(ids[i] as number);
    const k = s ? sortKey(s, column, ruleTitle) : null;
    keys[i] = k;
    if (k === null || (typeof k === 'number' && Number.isNaN(k))) missing.push(i);
    else present.push(i);
  }
  const m = present.length;
  const ranks = new Float64Array(m);
  const distinct = rankKeys(keys, present, ranks);
  const out = new Array<number>(n);
  let o = 0;
  if (distinct >= 0 && distinct * n < Number.MAX_SAFE_INTEGER) {
    const packed = new Float64Array(m);
    for (let j = 0; j < m; j++) {
      const r = dir === 1 ? (ranks[j] as number) : distinct - 1 - (ranks[j] as number);
      packed[j] = r * n + (present[j] as number);
    }
    packed.sort();
    for (let j = 0; j < m; j++) {
      const c = packed[j] as number;
      out[o++] = ids[c - Math.floor(c / n) * n] as number;
    }
  } else {
    // Mixed key types (never produced by `sortKey` today): generic comparator.
    const order = present.slice();
    order.sort((a, b) => {
      const ka = keys[a] as string | number;
      const kb = keys[b] as string | number;
      const c = ka < kb ? -1 : ka > kb ? 1 : 0;
      return c === 0 ? a - b : c * dir;
    });
    for (const i of order) out[o++] = ids[i] as number;
  }
  // Missing values last in both directions, in their original order.
  for (const i of missing) out[o++] = ids[i] as number;
  return { sorted: out, keys };
}

/** Columns whose order is the id order (or its reverse) — nothing to sort incrementally. */
function isIdOrder(column: TrafficColumnId): boolean {
  return column === 'id' || column === 'started' || column === 'indicator';
}

/** Columns whose sort key can change while an exchange is in flight. */
export function sortDependsOnUpdates(column: TrafficColumnId): boolean {
  return column === 'status' || column === 'type' || column === 'model' || column === 'size' || column === 'time' || column === 'rule' || column === 'source' || column === 'protocol';
}

/** `(key, id)` order used by {@link sortIds}: missing keys last, ties by ascending id. */
function compareEntries(ka: Key, ida: number, kb: Key, idb: number, dir: number): number {
  const ma = ka === null || (typeof ka === 'number' && Number.isNaN(ka));
  const mb = kb === null || (typeof kb === 'number' && Number.isNaN(kb));
  if (ma || mb) return ma && mb ? ida - idb : ma ? 1 : -1;
  const c = (ka as string | number) < (kb as string | number) ? -1 : (ka as string | number) > (kb as string | number) ? 1 : 0;
  return c === 0 ? ida - idb : c * dir;
}

/**
 * A column-sorted view kept up to date incrementally while traffic flows (DESIGN.md §6.3,
 * "smooth with 100k rows"). Summaries are replaced — never mutated — on update, so changed
 * rows are found by identity in one O(n) pass; only those are re-positioned by binary
 * insertion. Large changes (filter switches, bulk loads) fall back to a full {@link sortIds}.
 *
 * Expects `visible` in ascending id order (the store's `visible`), so ties keep id order
 * exactly like the full sort.
 */
export class IncrementalSort {
  private sorted: readonly ExchangeId[] = [];
  private seen = new Map<ExchangeId, ExchangeSummary>();
  private keys = new Map<ExchangeId, Key>();
  private signature: unknown[] = [];
  private visibleRef: readonly ExchangeId[] | null = null;

  /** Above this many changed rows a full re-sort is cheaper than insertions. */
  static readonly MAX_INCREMENTAL = 512;

  /**
   * The sorted ids for `visible`. Returns the previous array (same identity) when nothing
   * that affects the order changed. `context` lists extra inputs of the sort keys (e.g. the
   * rule set for rule titles); any identity change forces a full sort.
   */
  update(
    visible: readonly ExchangeId[],
    summaries: ReadonlyMap<ExchangeId, ExchangeSummary>,
    sort: SortState,
    ruleTitle?: (id: string) => string | undefined,
    context: readonly unknown[] = [],
  ): readonly ExchangeId[] {
    if (isIdOrder(sort.column)) {
      this.reset();
      return sortIds(visible, summaries, sort);
    }
    const signature = [sort.column, sort.direction, ...context];
    const sameSignature = signature.length === this.signature.length && signature.every((v, i) => Object.is(v, this.signature[i]));
    if (!sameSignature || this.visibleRef === null) return this.full(visible, summaries, sort, ruleTitle, signature);

    // Which rows are new or changed (identity), and did any disappear?
    const changed: ExchangeId[] = [];
    for (let i = 0; i < visible.length; i++) {
      const id = visible[i] as number;
      if (this.seen.get(id) !== summaries.get(id)) {
        changed.push(id);
        if (changed.length > IncrementalSort.MAX_INCREMENTAL) return this.full(visible, summaries, sort, ruleTitle, signature);
      }
    }
    const newCount = changed.filter((id) => !this.seen.has(id)).length;
    const removed = this.sorted.length + newCount !== visible.length;
    if (changed.length === 0 && !removed) {
      this.visibleRef = visible;
      return this.sorted;
    }

    const dir = sort.direction === 'asc' ? 1 : -1;
    const changedSet = new Set(changed);
    let keep: (id: ExchangeId) => boolean = (id) => !changedSet.has(id);
    if (removed) {
      const present = new Set(visible);
      keep = (id) => !changedSet.has(id) && present.has(id);
      for (const id of this.seen.keys()) {
        if (!present.has(id)) {
          this.seen.delete(id);
          this.keys.delete(id);
        }
      }
    }
    const next: ExchangeId[] = [];
    for (const id of this.sorted) if (keep(id)) next.push(id);

    for (const id of changed) {
      const s = summaries.get(id);
      const k = s ? sortKey(s, sort.column, ruleTitle) : null;
      if (s) this.seen.set(id, s);
      this.keys.set(id, k);
      // Upper-bound binary search in (key, id) order.
      let lo = 0;
      let hi = next.length;
      while (lo < hi) {
        const mid = (lo + hi) >>> 1;
        const other = next[mid] as number;
        if (compareEntries(this.keys.get(other) ?? null, other, k, id, dir) <= 0) lo = mid + 1;
        else hi = mid;
      }
      next.splice(lo, 0, id);
    }
    this.sorted = next;
    this.visibleRef = visible;
    return next;
  }

  /** Forget everything (next update sorts from scratch). */
  reset(): void {
    this.sorted = [];
    this.seen.clear();
    this.keys.clear();
    this.signature = [];
    this.visibleRef = null;
  }

  private full(
    visible: readonly ExchangeId[],
    summaries: ReadonlyMap<ExchangeId, ExchangeSummary>,
    sort: SortState,
    ruleTitle: ((id: string) => string | undefined) | undefined,
    signature: unknown[],
  ): readonly ExchangeId[] {
    const { sorted, keys } = sortWithKeys(visible, summaries, sort, ruleTitle);
    this.sorted = sorted;
    this.seen.clear();
    this.keys.clear();
    for (let i = 0; i < visible.length; i++) {
      const id = visible[i] as number;
      const s = summaries.get(id);
      if (!s) continue;
      this.seen.set(id, s);
      this.keys.set(id, keys ? (keys[i] as Key) : null);
    }
    this.signature = signature;
    this.visibleRef = visible;
    return this.sorted;
  }
}
