import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { api, browserTransport, type Transport } from '../../../api/client';
import type { BodyView, ExchangeSummary } from '../../../api/types';
import { LruCache } from '../../../lib/lru';
import { trafficStore } from '../../../state/traffic';
import { bodyRefreshKey } from '../model/body';
import { bodyCacheStats, bodyViewWeight, fetchBodyView, forgetBodies, VIEW_CACHE_BYTES } from './data';

function view(p: Partial<BodyView>): BodyView {
  return { kind: 'text', size: 0, wireSize: 0, encodings: [], text: '', formatted: null, structured: null, decodeError: null, truncated: false, captureTruncated: false, ...p };
}

function summary(p: Partial<ExchangeSummary>): ExchangeSummary {
  return { id: 1, kind: 'http', state: 'complete', startedAt: 0, method: 'POST', scheme: 'https', host: 'h', port: 443, path: '/', httpVersion: 'HTTP/1.1', contentKind: 'text', requestSize: 0, responseSize: 0, ...p };
}

/** Fake daemon: serves `bodies[url path]`, counting requests. */
let bodies: Record<string, BodyView> = {};
let requests: string[] = [];
const transport: Transport = {
  fetch: async (url) => {
    const path = url.split('?')[0] as string;
    requests.push(path);
    const v = bodies[path];
    return v ? new Response(JSON.stringify(v), { status: 200, headers: { 'Content-Type': 'application/json' } }) : new Response('{"error":{"code":"not-found","message":"nope"}}', { status: 404 });
  },
  openSocket: () => {
    throw new Error('no sockets in tests');
  },
};

beforeEach(() => {
  api.setTransport(transport);
  forgetBodies();
  bodies = {};
  requests = [];
});
afterEach(() => api.setTransport(browserTransport));

describe('body view freshness (request bodies)', () => {
  test('bodyRefreshKey: a request body is pending while uploading or held at a breakpoint, then final', () => {
    expect(bodyRefreshKey(summary({ state: 'requesting', requestSize: 1200 }), 'request')).toBe('pending:requesting');
    expect(bodyRefreshKey(summary({ state: 'paused', paused: 'request', requestSize: 30 }), 'request')).toBe('pending:paused');
    const done = bodyRefreshKey(summary({ state: 'waiting', requestSize: 30000 }), 'request');
    expect(done).toBe('done:30000');
    // Later response-side progress doesn't refetch the request body.
    expect(bodyRefreshKey(summary({ state: 'responding', requestSize: 30000 }), 'request')).toBe(done);
    expect(bodyRefreshKey(summary({ state: 'paused', paused: 'response', requestSize: 30000 }), 'request')).toBe(done);
    expect(bodyRefreshKey(summary({ state: 'complete', requestSize: 30000 }), 'request')).toBe(done);
    // Response side (unchanged behaviour).
    expect(bodyRefreshKey(summary({ state: 'responding' }), 'response')).toBe('responding');
    expect(bodyRefreshKey(summary({ state: 'complete', responseSize: 9 }), 'response')).toBe('done:9');
    expect(bodyRefreshKey(summary({ state: 'aborted', responseSize: 3 }), 'response')).toBe('done:3');
  });

  test('a request body fetched mid-upload is refetched once the upload finished (regression)', async () => {
    const path = '/api/exchanges/1/body/request';
    // During the upload the daemon answers with an empty view…
    bodies[path] = view({ size: 0 });
    const uploading = summary({ state: 'requesting', requestSize: 0 });
    expect((await fetchBodyView(1, 'request', false, bodyRefreshKey(uploading, 'request'))).size).toBe(0);
    // …and with the full body afterwards.
    bodies[path] = view({ size: 30000, text: 'x'.repeat(30000) });
    const finished = summary({ state: 'waiting', requestSize: 30000 });
    expect((await fetchBodyView(1, 'request', false, bodyRefreshKey(finished, 'request'))).size).toBe(30000);
    expect(requests).toEqual([path, path]);
    // Final views are cached; the superseded in-flight view is gone.
    expect((await fetchBodyView(1, 'request', false, bodyRefreshKey(finished, 'request'))).size).toBe(30000);
    expect(requests.length).toBe(2);
    expect(bodyCacheStats().views).toBe(1);
  });
});

describe('body caches are bounded and invalidated', () => {
  test('LruCache evicts by weight as well as count, and refuses oversized values', () => {
    const c = new LruCache<string, string>(10, { maxWeight: 10, weigh: (s) => s.length });
    c.set('a', 'xxxx').set('b', 'xxxx');
    expect(c.totalWeight).toBe(8);
    c.set('c', 'xxxx'); // 12 > 10 → evict the oldest
    expect([c.has('a'), c.has('b'), c.has('c')]).toEqual([false, true, true]);
    expect(c.totalWeight).toBe(8);
    c.get('b'); // b becomes most recent
    c.set('d', 'xxxxxx'); // 14 → evict c (oldest), then 10 fits
    expect([c.has('b'), c.has('c'), c.has('d')]).toEqual([true, false, true]);
    c.set('huge', 'x'.repeat(11));
    expect(c.has('huge')).toBe(false);
    expect(c.totalWeight).toBe(10);
    c.set('b', 'x'); // replacing re-weighs
    expect(c.totalWeight).toBe(7);
    c.deleteWhere((k) => k === 'd');
    expect(c.totalWeight).toBe(1);
    c.clear();
    expect(c.totalWeight).toBe(0);
    // Without a weigher it is a plain count-bounded LRU.
    const plain = new LruCache<number, number>(2);
    plain.set(1, 1).set(2, 2).set(3, 3);
    expect([plain.has(1), plain.size, plain.totalWeight]).toEqual([false, 2, 0]);
  });

  test('cached body views stay within the byte budget however many large bodies are viewed', async () => {
    const big = 'x'.repeat(4 << 20); // 4 Mi chars ≈ 8 MiB text + 8 MiB formatted per view
    for (let id = 1; id <= 30; id++) bodies[`/api/exchanges/${id}/body/response`] = view({ size: big.length, text: big, formatted: big });
    for (let id = 1; id <= 30; id++) await fetchBodyView(id, 'response', false, 'done:1');
    const stats = bodyCacheStats();
    expect(stats.viewBytes).toBeLessThanOrEqual(VIEW_CACHE_BYTES);
    expect(stats.views).toBeLessThan(30);
    expect(stats.views).toBeGreaterThan(0);
    expect(bodyViewWeight(view({ text: big, formatted: big }))).toBeGreaterThan(16 << 20);
  });

  test('removed, cleared and resynced exchanges are dropped from the cache', async () => {
    for (const id of [1, 2, 3]) bodies[`/api/exchanges/${id}/body/response`] = view({ size: id });
    for (const id of [1, 2, 3]) await fetchBodyView(id, 'response', false, 'done:1');
    expect(bodyCacheStats().views).toBe(3);
    trafficStore.remove([2]);
    expect(bodyCacheStats().views).toBe(2);
    requests = [];
    await fetchBodyView(1, 'response', false, 'done:1');
    expect(requests).toEqual([]); // still cached
    trafficStore.reset([]); // e.g. the daemon restarted: ids may now name other exchanges
    expect(bodyCacheStats().views).toBe(0);
    await fetchBodyView(1, 'response', false, 'done:1');
    expect(requests.length).toBe(1);
    trafficStore.clear();
    expect(bodyCacheStats().views).toBe(0);
  });

  test('a load still in flight during a resync does not repopulate the cache', async () => {
    bodies['/api/exchanges/4/body/response'] = view({ size: 4 });
    const p = fetchBodyView(4, 'response', false, 'done:4');
    trafficStore.reset([]);
    expect((await p).size).toBe(4); // the caller still gets its answer…
    expect(bodyCacheStats().views).toBe(0); // …but it isn't cached across the resync
  });
});
