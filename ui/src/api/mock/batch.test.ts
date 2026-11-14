import { describe, expect, test } from 'bun:test';
import { ApiClient, isApiError } from '../client';
import type { ExchangeSummary, RepeatResult } from '../types';
import { MockServer } from './server';

function client(): { server: MockServer; api: ApiClient } {
  const server = new MockServer();
  server.start();
  return { server, api: new ApiClient({ baseUrl: '', token: 't' }, server) };
}

describe('mock batched rows and repeat', () => {
  test('header values and timings for many ids', async () => {
    const { server, api } = client();
    try {
      const list: ExchangeSummary[] = await api.exchanges.list();
      const ids = list.slice(0, 40).map((s) => s.id);
      const { exchanges } = await api.exchanges.headerValues([...ids, 9_999_999], { request: ['User-Agent'], response: ['content-type', 'x-request-id'] });
      expect(exchanges.map((e) => e.id)).toEqual(ids);
      expect(exchanges.some((e) => e.request?.['user-agent'])).toBe(true);
      expect(exchanges.some((e) => e.response?.['content-type'])).toBe(true);
      const { timings } = await api.exchanges.timings(ids);
      expect(timings).toHaveLength(ids.length);
      expect(timings.every((t) => typeof t.start === 'number')).toBe(true);
      await expect(api.exchanges.headerValues(ids, { request: ['bad name'] })).rejects.toMatchObject({ status: 400 });
    } finally {
      server.stop();
    }
  });

  test('repeat streams results, finishes with a done line and can be cancelled', async () => {
    const { server, api } = client();
    try {
      const seen: RepeatResult[] = [];
      const done = await api.repeat({ request: { method: 'GET', url: 'http://localhost:3000/ping', headers: [], body: '' }, count: 6, concurrency: 3 }, (r) => seen.push(r));
      expect(done).toEqual({ done: true, sent: 6 });
      expect(seen.map((r) => r.index).sort()).toEqual([0, 1, 2, 3, 4, 5]);

      const controller = new AbortController();
      const partial: RepeatResult[] = [];
      const run = api.repeat({ request: { method: 'GET', url: 'http://localhost:3000/ping', headers: [], body: '' }, count: 500, concurrency: 2 }, (r) => {
        partial.push(r);
        if (partial.length === 2) controller.abort();
      }, controller.signal);
      const err = await run.then(
        () => null,
        (e: unknown) => e,
      );
      expect(isApiError(err) && err.code).toBe('aborted');
      const count = partial.length;
      await new Promise((r) => setTimeout(r, 400));
      expect(partial.length).toBe(count);
      expect(count).toBeLessThan(10);

      await expect(api.repeat({ count: 0 }, () => undefined)).rejects.toMatchObject({ status: 400 });
    } finally {
      server.stop();
    }
  });
});
