/** Summary of a repeat run (`POST /api/send/repeat` results). No React. */

import type { ErrorKind, ExchangeId, RepeatResult } from '../../api/types';

export type StatusClass = '1xx' | '2xx' | '3xx' | '4xx' | '5xx';

export interface RepeatStats {
  done: number;
  /** Responses by status code, ascending. */
  statuses: [number, number][];
  /** Responses by status class. */
  classes: Record<StatusClass, number>;
  /** Failures (no response) by kind, most frequent first. */
  errors: [ErrorKind, number][];
  /** Latency over the requests that measured one (ms). */
  min: number | null;
  avg: number | null;
  p50: number | null;
  p95: number | null;
  max: number | null;
  /** The slowest request's exchange. */
  slowest: ExchangeId | null;
}

/** Nearest-rank percentile of an ascending list. */
export function percentile(sorted: readonly number[], p: number): number | null {
  if (sorted.length === 0) return null;
  const rank = Math.ceil((p / 100) * sorted.length);
  return sorted[Math.min(sorted.length, Math.max(1, rank)) - 1] ?? null;
}

export function repeatStats(results: readonly RepeatResult[]): RepeatStats {
  const statuses = new Map<number, number>();
  const errors = new Map<ErrorKind, number>();
  const classes: Record<StatusClass, number> = { '1xx': 0, '2xx': 0, '3xx': 0, '4xx': 0, '5xx': 0 };
  const durations: number[] = [];
  let sum = 0;
  let slowest: { id: ExchangeId; ms: number } | null = null;
  for (const r of results) {
    if (r.status != null) {
      statuses.set(r.status, (statuses.get(r.status) ?? 0) + 1);
      const c = `${Math.min(5, Math.max(1, Math.floor(r.status / 100)))}xx` as StatusClass;
      classes[c]++;
    } else if (r.error) {
      errors.set(r.error, (errors.get(r.error) ?? 0) + 1);
    }
    if (r.durationMs != null && Number.isFinite(r.durationMs)) {
      durations.push(r.durationMs);
      sum += r.durationMs;
      if (!slowest || r.durationMs > slowest.ms) slowest = { id: r.id, ms: r.durationMs };
    }
  }
  durations.sort((a, b) => a - b);
  return {
    done: results.length,
    statuses: [...statuses].sort((a, b) => a[0] - b[0]),
    classes,
    errors: [...errors].sort((a, b) => b[1] - a[1]),
    min: durations[0] ?? null,
    avg: durations.length ? sum / durations.length : null,
    p50: percentile(durations, 50),
    p95: percentile(durations, 95),
    max: durations[durations.length - 1] ?? null,
    slowest: slowest?.id ?? null,
  };
}
