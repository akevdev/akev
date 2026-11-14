/**
 * Values of the custom header columns (`req:x-request-id`, `res:cache-control`), fetched with
 * `POST /api/exchanges/headers` for the rows on screen only and cached.
 *
 * - Cells call {@link HeaderValueCache.ensure} for their exchange; requests are batched (one
 *   call per ~40 ms, at most {@link CHUNK} ids each) and never repeated while in flight.
 * - An exchange fetched before its response arrived is fetched again once it has one.
 * - The cache holds at most {@link MAX_ENTRIES} exchanges (oldest dropped first), is cleared when
 *   the set of header columns changes, and follows the traffic store's resets, clears and
 *   deletions.
 */

import type { ExchangeId, HeaderValues } from '../../../api/types';
import { isHeaderColumn, parseHeaderColumn, type HeaderColumnId, type TrafficColumnId } from '../../../state/ui';

/** Ids per request. */
const CHUNK = 500;
/** Exchanges kept. */
const MAX_ENTRIES = 5000;
/** Batching delay (ms): scrolling settles into one request. */
const BATCH_MS = 40;
/** Delay before asking again after a failed request (ms). */
const RETRY_MS = 3000;

interface Entry {
  /** Whether the exchange had its response when fetched. */
  complete: boolean;
  values: HeaderValues;
}

export type FetchHeaderValues = (ids: ExchangeId[], names: { request: string[]; response: string[] }, signal: AbortSignal) => Promise<{ exchanges: HeaderValues[] }>;

export class HeaderValueCache {
  private entries = new Map<ExchangeId, Entry>();
  /** Queued ids → whether they had their response when queued. */
  private queue = new Map<ExchangeId, boolean>();
  private inFlight = new Set<ExchangeId>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private names = { request: [] as string[], response: [] as string[] };
  private signature = '';
  private controller = new AbortController();
  private version = 0;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly fetchValues: FetchHeaderValues,
    private readonly schedule: (fn: () => void, ms: number) => ReturnType<typeof setTimeout> = (fn, ms) => setTimeout(fn, ms),
  ) {}

  readonly subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  readonly getVersion = (): number => this.version;

  /** The header columns shown (only their names are fetched). Changing them clears the cache. */
  setColumns(columns: readonly TrafficColumnId[]): void {
    const request: string[] = [];
    const response: string[] = [];
    for (const id of columns) {
      if (!isHeaderColumn(id)) continue;
      const { side, name } = parseHeaderColumn(id);
      (side === 'request' ? request : response).push(name);
    }
    const signature = `${request.join(',')}|${response.join(',')}`;
    if (signature === this.signature) return;
    this.signature = signature;
    this.names = { request, response };
    this.reset();
  }

  /**
   * Make sure `id`'s values are (being) loaded. `complete`: the exchange has its response
   * (status or failure) — values fetched before that are fetched again.
   */
  ensure(id: ExchangeId, complete: boolean): void {
    if (!this.signature || this.signature === '|') return;
    const e = this.entries.get(id);
    if (e && (e.complete || !complete)) return;
    if (this.inFlight.has(id) || this.queue.get(id) === complete) return;
    this.queue.set(id, complete);
    if (this.timer == null) this.timer = this.schedule(() => void this.flush(), BATCH_MS);
  }

  /** A cell's value: `undefined` while loading, `null` when the exchange has no such header. */
  get(id: ExchangeId, column: HeaderColumnId): string | null | undefined {
    const e = this.entries.get(id);
    if (!e) return undefined;
    const { side, name } = parseHeaderColumn(column);
    return e.values[side]?.[name] ?? null;
  }

  /** Forget exchanges (removed from the store). */
  forget(ids: readonly ExchangeId[]): void {
    let any = false;
    for (const id of ids) {
      any = this.entries.delete(id) || any;
      this.queue.delete(id);
    }
    if (any) this.notify();
  }

  /** Drop everything (store reset / cleared, columns changed). */
  reset(): void {
    this.controller.abort();
    this.controller = new AbortController();
    this.entries.clear();
    this.queue.clear();
    this.inFlight.clear();
    if (this.timer != null) clearTimeout(this.timer);
    this.timer = null;
    this.notify();
  }

  /** Send the queued ids now. */
  async flush(): Promise<void> {
    this.timer = null;
    if (this.queue.size === 0) return;
    const queued = [...this.queue];
    this.queue.clear();
    const names = this.names;
    const signal = this.controller.signal;
    for (let i = 0; i < queued.length; i += CHUNK) {
      const chunk = queued.slice(i, i + CHUNK);
      const ids = chunk.map(([id]) => id);
      for (const id of ids) this.inFlight.add(id);
      try {
        const { exchanges } = await this.fetchValues(ids, names, signal);
        if (signal.aborted) return;
        const complete = new Map(chunk);
        for (const values of exchanges) this.put(values.id, { complete: complete.get(values.id) ?? false, values });
        // Unknown ids (deleted meanwhile) count as "no value" so their cells stop loading.
        for (const id of ids) if (!this.entries.has(id)) this.put(id, { complete: true, values: { id } });
        this.notify();
      } catch {
        // Offline or the daemon is restarting: try these rows again a little later.
        if (signal.aborted) return;
        for (const [id, complete] of chunk) if (!this.queue.has(id)) this.queue.set(id, complete);
        if (this.timer == null) this.timer = this.schedule(() => void this.flush(), RETRY_MS);
      } finally {
        for (const id of ids) this.inFlight.delete(id);
      }
    }
  }

  private put(id: ExchangeId, entry: Entry): void {
    this.entries.delete(id);
    this.entries.set(id, entry);
    if (this.entries.size > MAX_ENTRIES) {
      const oldest = this.entries.keys().next().value;
      if (oldest !== undefined) this.entries.delete(oldest);
    }
  }

  private notify(): void {
    this.version++;
    for (const l of this.listeners) l();
  }
}
