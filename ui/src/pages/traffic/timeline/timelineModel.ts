/**
 * The traffic timeline: which exchanges it plots, the shared time axis, each row's phase
 * segments (the Timing tab's waterfall, placed on wall-clock time), and the per-row timing
 * cache that loads phases for the rows on screen. No React.
 */

import type { ExchangeId, ExchangeSummary, ExchangeTiming, Timing } from '../../../api/types';
import { buildWaterfall, PHASE_INFO, type PhaseId } from '../../../components/exchange/model/timing';
import { formatDuration } from '../../../lib/format';

/** Most exchanges the timeline plots (the newest ones). */
export const TIMELINE_CAP = 5000;

export interface TimelineSelection {
  ids: readonly ExchangeId[];
  /** Exchanges left out by {@link TIMELINE_CAP}. */
  hidden: number;
  /** Plotting the selection (≥ 2 selected) rather than everything visible. */
  selection: boolean;
}

/** The selected exchanges when several are, else the visible ones; the newest {@link TIMELINE_CAP}. */
export function timelineIds(visible: readonly ExchangeId[], selected: ReadonlySet<ExchangeId>, cap = TIMELINE_CAP): TimelineSelection {
  const selection = selected.size >= 2;
  const all = selection ? [...selected].sort((a, b) => a - b) : visible;
  return { ids: all.length > cap ? all.slice(all.length - cap) : all, hidden: Math.max(0, all.length - cap), selection };
}

const PENDING: ReadonlySet<string> = new Set(['requesting', 'waiting', 'responding', 'paused', 'streaming']);

/** Whether an exchange is still running (its bar grows to now). */
export function isRunning(s: ExchangeSummary): boolean {
  return PENDING.has(s.state) && s.duration == null;
}

/** End of an exchange on the wall clock (ms). */
export function endOf(s: ExchangeSummary, now: number): number {
  if (s.duration != null) return s.startedAt + s.duration;
  return isRunning(s) ? Math.max(s.startedAt, now) : s.startedAt;
}

/** The shared axis: earliest start → latest end (at least 1 ms wide). */
export function timeRange(ids: readonly ExchangeId[], summaries: ReadonlyMap<ExchangeId, ExchangeSummary>, now: number): { t0: number; t1: number } {
  let t0 = Number.POSITIVE_INFINITY;
  let t1 = Number.NEGATIVE_INFINITY;
  for (const id of ids) {
    const s = summaries.get(id);
    if (!s) continue;
    if (s.startedAt < t0) t0 = s.startedAt;
    const e = endOf(s, now);
    if (e > t1) t1 = e;
  }
  if (!Number.isFinite(t0)) return { t0: now, t1: now + 1 };
  return { t0, t1: Math.max(t1, t0 + 1) };
}

export type SegmentId = PhaseId | 'total';

export interface Segment {
  id: SegmentId;
  /** Wall-clock start (ms). */
  start: number;
  /** Width on the axis (ms). */
  span: number;
}

export interface RowPlot {
  segments: Segment[];
  /** Tooltip: phases and their durations. */
  title: string;
}

/**
 * One row's bar: the Timing tab's phases when the timing is loaded, else a single bar for the
 * whole exchange from its summary.
 */
export function rowPlot(s: ExchangeSummary, timing: Timing | undefined, now: number): RowPlot {
  const label = `${s.method} ${s.host}${s.path.split('?')[0] ?? ''}`;
  if (!timing) {
    const end = endOf(s, now);
    const total = isRunning(s) ? `${formatDuration(end - s.startedAt)} so far` : s.duration != null ? formatDuration(s.duration) : 'no timing';
    return { segments: [{ id: 'total', start: s.startedAt, span: Math.max(0, end - s.startedAt) }], title: `${label}\n${total}` };
  }
  const w = buildWaterfall(timing, { state: s.state, now: isRunning(s) ? now : undefined });
  const segments = w.phases.map((p) => ({ id: p.id, start: w.startedAt + p.start, span: p.span }));
  const lines = w.phases.map((p) => `${PHASE_INFO[p.id].label}: ${formatDuration(p.duration)}`);
  const total = w.total != null ? `Total: ${formatDuration(w.total)}` : 'In progress';
  const paused = w.paused > 0 ? `\nPaused at a breakpoint: ${formatDuration(w.paused)} (left out)` : '';
  return { segments: segments.length ? segments : [{ id: 'total', start: s.startedAt, span: Math.max(0, endOf(s, now) - s.startedAt) }], title: `${label}\n${lines.join('\n')}${lines.length ? '\n' : ''}${total}${paused}` };
}

// =====================================================================================
// Timing cache (rows on screen only)
// =====================================================================================

const CHUNK = 1000;
const MAX_ENTRIES = 6000;
const BATCH_MS = 40;
const RETRY_MS = 3000;

export type FetchTimings = (ids: ExchangeId[], signal: AbortSignal) => Promise<{ timings: ExchangeTiming[] }>;

/**
 * Phase timings for the rows on screen (`POST /api/exchanges/timings`): batched, cached, and
 * fetched again for exchanges that were still running when first loaded.
 */
export class TimingCache {
  private entries = new Map<ExchangeId, { done: boolean; timing: Timing | null }>();
  private queue = new Map<ExchangeId, boolean>();
  private inFlight = new Set<ExchangeId>();
  private timer: ReturnType<typeof setTimeout> | null = null;
  private controller = new AbortController();
  private version = 0;
  private readonly listeners = new Set<() => void>();

  constructor(
    private readonly fetchTimings: FetchTimings,
    private readonly schedule: (fn: () => void, ms: number) => ReturnType<typeof setTimeout> = (fn, ms) => setTimeout(fn, ms),
  ) {}

  readonly subscribe = (l: () => void): (() => void) => {
    this.listeners.add(l);
    return () => this.listeners.delete(l);
  };

  readonly getVersion = (): number => this.version;

  /** Load `id`'s timing unless cached; `done`: finished (cached running timings are refreshed). */
  ensure(id: ExchangeId, done: boolean): void {
    const e = this.entries.get(id);
    if (e && (e.done || !done)) return;
    if (this.inFlight.has(id) || this.queue.get(id) === done) return;
    this.queue.set(id, done);
    if (this.timer == null) this.timer = this.schedule(() => void this.flush(), BATCH_MS);
  }

  get(id: ExchangeId): Timing | undefined {
    return this.entries.get(id)?.timing ?? undefined;
  }

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

  forget(ids: readonly ExchangeId[]): void {
    for (const id of ids) {
      this.entries.delete(id);
      this.queue.delete(id);
    }
    this.notify();
  }

  async flush(): Promise<void> {
    this.timer = null;
    if (this.queue.size === 0) return;
    const queued = [...this.queue];
    this.queue.clear();
    const signal = this.controller.signal;
    for (let i = 0; i < queued.length; i += CHUNK) {
      const chunk = queued.slice(i, i + CHUNK);
      const ids = chunk.map(([id]) => id);
      for (const id of ids) this.inFlight.add(id);
      try {
        const { timings } = await this.fetchTimings(ids, signal);
        if (signal.aborted) return;
        const done = new Map(chunk);
        for (const { id, ...timing } of timings) this.put(id, { done: done.get(id) ?? false, timing });
        for (const id of ids) if (!this.entries.has(id)) this.put(id, { done: true, timing: null });
        this.notify();
      } catch {
        if (signal.aborted) return;
        for (const [id, done] of chunk) if (!this.queue.has(id)) this.queue.set(id, done);
        if (this.timer == null) this.timer = this.schedule(() => void this.flush(), RETRY_MS);
      } finally {
        for (const id of ids) this.inFlight.delete(id);
      }
    }
  }

  private put(id: ExchangeId, entry: { done: boolean; timing: Timing | null }): void {
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
