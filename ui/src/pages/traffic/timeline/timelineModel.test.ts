import { describe, expect, test } from 'bun:test';
import type { ExchangeId, ExchangeSummary, ExchangeTiming } from '../../../api/types';
import { rowPlot, timelineIds, timeRange, TimingCache } from './timelineModel';

function summary(id: number, startedAt: number, duration?: number, state: ExchangeSummary['state'] = 'complete'): ExchangeSummary {
  return { id, kind: 'http', state, startedAt, method: 'GET', scheme: 'https', host: 'a.test', port: 443, path: '/x?y=1', httpVersion: 'HTTP/2', contentKind: 'json', requestSize: 0, responseSize: 0, duration };
}

describe('timeline model', () => {
  test('plots the selection when several are selected, else the visible rows; newest first under the cap', () => {
    const visible = Array.from({ length: 12 }, (_, i) => i);
    expect(timelineIds(visible, new Set([3]), 5)).toEqual({ ids: [7, 8, 9, 10, 11], hidden: 7, selection: false });
    expect(timelineIds(visible, new Set([9, 2, 4]), 5)).toEqual({ ids: [2, 4, 9], hidden: 0, selection: true });
    expect(timelineIds([], new Set(), 5)).toEqual({ ids: [], hidden: 0, selection: false });
  });

  test('one wall-clock axis; running exchanges reach now', () => {
    const map = new Map<ExchangeId, ExchangeSummary>([
      [1, summary(1, 1000, 50)],
      [2, summary(2, 1020, 500)],
      [3, summary(3, 1100, undefined, 'waiting')],
    ]);
    expect(timeRange([1, 2], map, 5000)).toEqual({ t0: 1000, t1: 1520 });
    expect(timeRange([1, 2, 3], map, 5000)).toEqual({ t0: 1000, t1: 5000 });
    expect(timeRange([], map, 5000)).toEqual({ t0: 5000, t1: 5001 });
  });

  test('bars: one segment from the summary until the phases are loaded', () => {
    const s = summary(1, 10_000, 120);
    expect(rowPlot(s, undefined, 0).segments).toEqual([{ id: 'total', start: 10_000, span: 120 }]);
    const plot = rowPlot(s, { start: 10_000_000, requestHeaders: 1, dnsDuration: 10, connectDuration: 20, tlsDuration: 30, requestSent: 62, responseHeaders: 100, responseBody: 120 }, 0);
    expect(plot.segments.map((p) => p.id)).toEqual(['received', 'dns', 'connect', 'tls', 'sent', 'wait', 'download']);
    const dns = plot.segments.find((p) => p.id === 'dns');
    expect(dns).toEqual({ id: 'dns', start: 10_001, span: 10 });
    expect(plot.title).toContain('DNS lookup: 10 ms');
    expect(plot.title).toContain('Total: 120 ms');
    expect(plot.title.startsWith('GET a.test/x\n')).toBe(true);
  });

  test('timing cache: batches, refreshes running rows once finished, forgets', async () => {
    const calls: ExchangeId[][] = [];
    const timers: (() => void)[] = [];
    const cache = new TimingCache(
      async (ids) => {
        calls.push(ids);
        return { timings: ids.filter((id) => id !== 3).map((id): ExchangeTiming => ({ id, start: id * 1000, responseBody: 5 })) };
      },
      (fn) => {
        timers.push(fn);
        return 0 as unknown as ReturnType<typeof setTimeout>;
      },
    );
    const tick = async () => {
      for (const t of timers.splice(0)) t();
      await new Promise((r) => setTimeout(r, 0));
    };
    cache.ensure(1, true);
    cache.ensure(2, false);
    cache.ensure(3, true);
    await tick();
    expect(calls).toEqual([[1, 2, 3]]);
    expect(cache.get(1)?.start).toBe(1000);
    expect(cache.get(3)).toBeUndefined();
    cache.ensure(1, true);
    cache.ensure(3, true);
    await tick();
    expect(calls).toHaveLength(1);
    cache.ensure(2, true);
    await tick();
    expect(calls.at(-1)).toEqual([2]);
    cache.forget([1]);
    expect(cache.get(1)).toBeUndefined();
    cache.reset();
    expect(cache.get(2)).toBeUndefined();
  });
});
