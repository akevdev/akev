/**
 * Aggregate statistics for the multi-selection pane (DESIGN.md §6.5): total size, time span,
 * status-class breakdown and top hosts. One pass over the summaries; safe for 100k selections.
 */

import type { ExchangeSummary } from '../../../../api/types';

/** Status buckets shown in the breakdown bar, in display order. */
export type StatusBucket = '2xx' | '3xx' | '4xx' | '5xx' | '1xx' | 'err' | 'pending' | 'other';
export const STATUS_BUCKETS: readonly StatusBucket[] = ['2xx', '3xx', '4xx', '5xx', '1xx', 'err', 'pending', 'other'];

export const BUCKET_LABELS: Readonly<Record<StatusBucket, string>> = {
  '1xx': '1xx Info',
  '2xx': '2xx Success',
  '3xx': '3xx Redirect',
  '4xx': '4xx Client error',
  '5xx': '5xx Server error',
  err: 'Failed',
  pending: 'In flight',
  other: 'No response',
};

const IN_FLIGHT = new Set(['requesting', 'waiting', 'responding', 'paused', 'streaming']);

/** Which breakdown bucket an exchange falls into. */
export function statusBucket(s: Pick<ExchangeSummary, 'status' | 'error' | 'state' | 'kind'>): StatusBucket {
  if (s.status != null) {
    if (s.status >= 100 && s.status < 200) return '1xx';
    if (s.status < 300) return '2xx';
    if (s.status < 400) return '3xx';
    if (s.status < 500) return '4xx';
    if (s.status < 600) return '5xx';
    return 'other';
  }
  if (s.error || s.kind === 'tls-failure' || s.state === 'aborted') return 'err';
  if (IN_FLIGHT.has(s.state)) return 'pending';
  return 'other';
}

export interface HostStat {
  host: string;
  count: number;
  /** Request + response bytes. */
  bytes: number;
}

export interface BucketStat {
  bucket: StatusBucket;
  count: number;
}

export interface MultiStats {
  count: number;
  requestBytes: number;
  responseBytes: number;
  totalBytes: number;
  /** Earliest start (unix ms) or `null` when empty. */
  firstStart: number | null;
  /** Latest end (start + duration, or start for unfinished exchanges). */
  lastEnd: number | null;
  /** `lastEnd - firstStart` (0 when empty). */
  spanMs: number;
  /** Mean duration over finished exchanges (`null` if none finished). */
  avgDuration: number | null;
  /** Slowest finished exchange (`null` if none finished). */
  maxDuration: number | null;
  /** Non-empty buckets in display order. */
  buckets: BucketStat[];
  /** Most frequent hosts (by count, then bytes), at most `topN`. */
  topHosts: HostStat[];
  /** Number of distinct hosts. */
  hostCount: number;
  pinned: number;
  /** Failed exchanges (bucket `err`). */
  failed: number;
}

/** Compute {@link MultiStats} in a single pass. */
export function computeMultiStats(summaries: Iterable<ExchangeSummary>, topN = 5): MultiStats {
  let count = 0;
  let requestBytes = 0;
  let responseBytes = 0;
  let firstStart = Infinity;
  let lastEnd = -Infinity;
  let durSum = 0;
  let durCount = 0;
  let maxDuration = -Infinity;
  let pinned = 0;
  const bucketCounts = new Map<StatusBucket, number>();
  const hosts = new Map<string, HostStat>();

  for (const s of summaries) {
    count++;
    const req = Math.max(0, s.requestSize || 0);
    const res = Math.max(0, s.responseSize || 0);
    requestBytes += req;
    responseBytes += res;
    if (s.startedAt < firstStart) firstStart = s.startedAt;
    const end = s.startedAt + (s.duration ?? 0);
    if (end > lastEnd) lastEnd = end;
    if (s.duration != null && s.state !== 'paused') {
      durSum += s.duration;
      durCount++;
      if (s.duration > maxDuration) maxDuration = s.duration;
    }
    if (s.pinned) pinned++;
    const b = statusBucket(s);
    bucketCounts.set(b, (bucketCounts.get(b) ?? 0) + 1);
    const h = hosts.get(s.host);
    if (h) {
      h.count++;
      h.bytes += req + res;
    } else {
      hosts.set(s.host, { host: s.host, count: 1, bytes: req + res });
    }
  }

  const buckets: BucketStat[] = [];
  for (const bucket of STATUS_BUCKETS) {
    const n = bucketCounts.get(bucket);
    if (n) buckets.push({ bucket, count: n });
  }

  const topHosts = [...hosts.values()]
    .sort((a, b) => b.count - a.count || b.bytes - a.bytes || a.host.localeCompare(b.host))
    .slice(0, Math.max(0, topN));

  const empty = count === 0;
  return {
    count,
    requestBytes,
    responseBytes,
    totalBytes: requestBytes + responseBytes,
    firstStart: empty ? null : firstStart,
    lastEnd: empty ? null : lastEnd,
    spanMs: empty ? 0 : Math.max(0, lastEnd - firstStart),
    avgDuration: durCount ? durSum / durCount : null,
    maxDuration: durCount ? maxDuration : null,
    buckets,
    topHosts,
    hostCount: hosts.size,
    pinned,
    failed: bucketCounts.get('err') ?? 0,
  };
}
