import { describe, expect, test } from 'bun:test';
import type { RepeatResult } from '../../api/types';
import { percentile, repeatStats } from './repeatStats';

describe('repeat summary', () => {
  test('nearest-rank percentiles', () => {
    const xs = Array.from({ length: 100 }, (_, i) => i + 1);
    expect(percentile(xs, 95)).toBe(95);
    expect(percentile(xs, 50)).toBe(50);
    expect(percentile([7], 95)).toBe(7);
    expect(percentile([], 95)).toBeNull();
  });

  test('status counts, failures and latency', () => {
    const results: RepeatResult[] = [
      { index: 0, id: 10, status: 200, durationMs: 40 },
      { index: 1, id: 11, status: 200, durationMs: 60 },
      { index: 2, id: 12, status: 503, durationMs: 20 },
      { index: 3, id: 13, error: 'timeout', durationMs: 900 },
      { index: 4, id: 14, error: 'timeout' },
      { index: 5, id: 15, error: 'connection-refused', durationMs: 1 },
      { index: 6, id: 16, status: 204, durationMs: 80 },
    ];
    const s = repeatStats(results);
    expect(s.done).toBe(7);
    expect(s.statuses).toEqual([
      [200, 2],
      [204, 1],
      [503, 1],
    ]);
    expect(s.classes).toEqual({ '1xx': 0, '2xx': 3, '3xx': 0, '4xx': 0, '5xx': 1 });
    expect(s.errors).toEqual([
      ['timeout', 2],
      ['connection-refused', 1],
    ]);
    expect(s.min).toBe(1);
    expect(s.max).toBe(900);
    expect(s.avg).toBeCloseTo(1101 / 6);
    expect(s.p50).toBe(40);
    expect(s.p95).toBe(900);
    expect(s.slowest).toBe(13);
    expect(repeatStats([])).toMatchObject({ done: 0, min: null, avg: null, p95: null, slowest: null });
  });
});
