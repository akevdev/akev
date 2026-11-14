import { describe, expect, test } from 'bun:test';
import type { ExchangeSummary, ServerMessage } from '../types';
import { multipartBody, parseMultipart, parseSse, sseBody } from './bodies';
import { compileFilter, suggestFilter, type FilterSubject } from './filter';
import { Rng } from './rng';
import { MockServer } from './server';

function subject(patch: Partial<ExchangeSummary> = {}, extra: Partial<FilterSubject> = {}): FilterSubject {
  const summary: ExchangeSummary = {
    id: 7,
    kind: 'http',
    state: 'complete',
    startedAt: 0,
    method: 'POST',
    scheme: 'https',
    host: 'api.github.com',
    port: 443,
    path: '/graphql?x=1',
    httpVersion: 'HTTP/2',
    status: 404,
    contentKind: 'json',
    contentType: 'application/json',
    requestSize: 10,
    responseSize: 2048,
    duration: 1500,
    source: 'Chrome',
    ...patch,
  };
  return { summary, url: `https://${summary.host}${summary.path}`, requestHeaders: [['Authorization', 'Bearer x']], responseHeaders: [['content-type', 'application/json']], ...extra };
}

function matches(query: string, s: FilterSubject = subject()): boolean {
  const c = compileFilter(query);
  if (!c.ok) throw new Error(`${query}: ${c.error.message}`);
  return c.predicate ? c.predicate(s) : true;
}

describe('mock filter language', () => {
  test('fields, operators and values', () => {
    expect(matches('method:post')).toBe(true);
    expect(matches('method:get,post')).toBe(true);
    expect(matches('method:get')).toBe(false);
    expect(matches('status:404')).toBe(true);
    expect(matches('status:4xx')).toBe(true);
    expect(matches('status:>=500')).toBe(false);
    expect(matches('host:github')).toBe(true);
    expect(matches('host:*.github.com')).toBe(true);
    expect(matches('host=github.com')).toBe(false);
    expect(matches('size:>1kb')).toBe(true);
    expect(matches('duration:>2s')).toBe(false);
    expect(matches('type:json')).toBe(true);
    expect(matches('graphql')).toBe(true); // free text on the URL
  });

  test('LLM fields and flag', () => {
    const llm = subject({ llm: { provider: 'anthropic', model: 'claude-sonnet-4-5' } });
    expect(matches('is:llm', llm)).toBe(true);
    expect(matches('is:llm')).toBe(false);
    expect(matches('-is:llm')).toBe(true);
    expect(matches('model:sonnet', llm)).toBe(true);
    expect(matches('model:claude*', llm)).toBe(true);
    expect(matches('model:gpt', llm)).toBe(false);
    expect(matches('provider:anthropic', llm)).toBe(true);
    expect(matches('provider:anthropic,openai', llm)).toBe(true);
    expect(matches('provider:anthro', llm)).toBe(false);
    expect(matches('provider:*thro*', llm)).toBe(true);
    expect(matches('model:claude')).toBe(false);
  });

  test('boolean logic', () => {
    expect(matches('method:get OR status:404')).toBe(true);
    expect(matches('method:post -status:404')).toBe(false);
    expect(matches('!(host:example.com)')).toBe(true);
    expect(matches('(method:get | method:post) status:4xx')).toBe(true);
  });

  test('flags', () => {
    expect(matches('is:complete')).toBe(true);
    expect(matches('is:pending', subject({ state: 'waiting', status: undefined }))).toBe(true);
    expect(matches('is:ws', subject({ kind: 'web-socket' }))).toBe(true);
    expect(matches('has:auth')).toBe(true);
  });

  test('empty query matches everything; errors carry UTF-8 byte spans', () => {
    expect(compileFilter('   ')).toEqual({ ok: true, predicate: null });
    const err = compileFilter('€ bogus:1');
    expect(err.ok).toBe(false);
    if (!err.ok) {
      expect(err.error.message.toLowerCase()).toContain('bogus');
      expect(err.error.start).toBe(4); // "€ " = 3 + 1 bytes
      expect(err.error.end).toBeGreaterThan(err.error.start);
    }
    expect(compileFilter('(method:get').ok).toBe(false);
  });

  test('suggestions complete fields and values with byte ranges', () => {
    const fields = suggestFilter('sta', 3, { hosts: [], methods: [], sources: [] });
    expect(fields[0]).toMatchObject({ label: 'status:', insert: 'status:', replaceStart: 0, replaceEnd: 3 });
    const hosts = suggestFilter('method:get host:api', 19, { hosts: ['api.github.com', 'cdn.jsdelivr.net'], methods: ['GET'], sources: [] });
    expect(hosts.map((s) => s.label)).toContain('api.github.com');
  });
});

describe('mock structured body payloads (akev-content shapes)', () => {
  test('SSE events', () => {
    expect(parseSse('event: tick\nid: 1\ndata: a\ndata: b\n\n: comment\ndata: c\nretry: 50\n\n')).toEqual([
      { event: 'tick', id: '1', retry: null, data: 'a\nb' },
      { event: null, id: null, retry: 50, data: 'c' },
    ]);
    expect(parseSse(sseBody(new Rng(1))).length).toBeGreaterThan(0);
  });

  test('multipart parts', () => {
    const boundary = '----akevTest';
    const parts = parseMultipart(multipartBody(boundary), `multipart/form-data; boundary=${boundary}`);
    expect(parts.length).toBeGreaterThan(0);
    for (const p of parts) {
      expect(Array.isArray(p.headers)).toBe(true);
      expect(p.size).toBeGreaterThanOrEqual(0);
    }
    expect(parts.some((p) => p.name != null)).toBe(true);
    expect(parseMultipart('x', 'multipart/form-data')).toEqual([]);
  });
});

describe('MockServer', () => {
  function collect(server: MockServer) {
    const socket = server.openSocket();
    const messages: ServerMessage[] = [];
    socket.onmessage = (ev) => messages.push(JSON.parse(String(ev.data)) as ServerMessage);
    return { socket, messages };
  }

  test('serves typed REST responses through the Transport interface', async () => {
    const server = new MockServer();
    server.start();
    try {
      const status = await (await server.fetch('/api/status', { method: 'GET' })).json();
      expect(status.version).toContain('mock');
      expect(status.stats.exchanges).toBeGreaterThan(100);
      const list = (await (await server.fetch('/api/exchanges?filter=method:post&limit=5', { method: 'GET' })).json()) as ExchangeSummary[];
      expect(list.length).toBeLessThanOrEqual(5);
      expect(list.every((s) => s.method === 'POST')).toBe(true);
      const first = list[0];
      if (first) {
        const detail = await (await server.fetch(`/api/exchanges/${first.id}`, { method: 'GET' })).json();
        expect(detail.summary.id).toBe(first.id);
        expect(Array.isArray(detail.request.headers)).toBe(true);
      }
      const missing = await server.fetch('/api/exchanges/999999', { method: 'GET' });
      expect(missing.status).toBe(404);
      expect((await missing.json()).error.code).toBe('not-found');
      const notRouted = await server.fetch('/api/nope', { method: 'GET' });
      expect(notRouted.status).toBe(404);
      const interceptors = await (await server.fetch('/api/interceptors', { method: 'GET' })).json();
      expect(interceptors.length).toBeGreaterThanOrEqual(8);
    } finally {
      server.stop();
    }
  });

  test('stream: a large snapshot is paged (newest first, then older chunks)', async () => {
    const server = new MockServer({ helloPage: 50, helloChunk: 80 });
    server.start();
    try {
      const status = await (await server.fetch('/api/status', { method: 'GET' })).json();
      const { messages } = collect(server);
      await new Promise((r) => setTimeout(r, 400));
      const hello = messages[0];
      if (hello?.type !== 'hello') throw new Error('expected hello');
      expect(hello.summaries.length).toBe(50);
      expect(hello.remaining).toBe(status.stats.exchanges - 50);
      const more = messages.filter((m) => m.type === 'hello-more');
      expect(more.length).toBeGreaterThan(1);
      const last = more.at(-1);
      expect(last?.type === 'hello-more' && last.remaining).toBe(0);
      // Every chunk is older than everything before it; together they cover the snapshot.
      let oldest = hello.summaries[0]?.id ?? Infinity;
      let total = hello.summaries.length;
      for (const m of more) {
        if (m.type !== 'hello-more') continue;
        const ids = m.summaries.map((s) => s.id);
        expect(Math.max(...ids)).toBeLessThan(oldest);
        oldest = Math.min(...ids);
        total += ids.length;
      }
      expect(total).toBe(status.stats.exchanges);
    } finally {
      server.stop();
    }
  });

  test('messages are paged (after / before / limit / tail) and diagnostics are served', async () => {
    const server = new MockServer();
    server.start();
    try {
      const list = (await (await server.fetch('/api/exchanges?filter=is:websocket', { method: 'GET' })).json()) as ExchangeSummary[];
      const ws = list.find((s) => (s.messageCount ?? 0) > 3);
      expect(ws).toBeDefined();
      if (ws) {
        const get = async (q: string) => (await (await server.fetch(`/api/exchanges/${ws.id}/messages${q}`, { method: 'GET' })).json()) as { seq: number }[];
        const all = await get('');
        const tail = await get('?tail=1&limit=2');
        expect(tail.map((m) => m.seq)).toEqual(all.slice(-2).map((m) => m.seq));
        const first = await get('?limit=2');
        expect(first.map((m) => m.seq)).toEqual(all.slice(0, 2).map((m) => m.seq));
        const before = await get(`?before=${tail[0]?.seq}&tail=1&limit=1`);
        expect(before.map((m) => m.seq)).toEqual([all.at(-3)?.seq ?? -1]);
        const after = await get(`?after=${first[1]?.seq}&limit=1`);
        expect(after.map((m) => m.seq)).toEqual([all[2]?.seq ?? -1]);
      }
      const d = await (await server.fetch('/api/diagnostics', { method: 'GET' })).json();
      expect(d.version).toContain('mock');
      expect(d.repository).toContain('github.com');
      expect(typeof d.stats.exchanges).toBe('number');
      expect(Array.isArray(d.recentWarnings)).toBe(true);
    } finally {
      server.stop();
    }
  });

  test('stream: hello snapshot, then server-side filtering with deltas', async () => {
    const server = new MockServer();
    server.start();
    try {
      const { socket, messages } = collect(server);
      await new Promise((r) => setTimeout(r, 80));
      const hello = messages[0];
      expect(hello?.type).toBe('hello');
      if (hello?.type !== 'hello') return;
      expect(hello.summaries.length).toBeGreaterThan(100);
      socket.send(JSON.stringify({ type: 'set-filter', query: 'method:get', seq: 1 }));
      const result = messages.find((m) => m.type === 'filter-result');
      expect(result).toMatchObject({ type: 'filter-result', seq: 1, query: 'method:get' });
      if (result?.type === 'filter-result') {
        const byId = new Map(hello.summaries.map((s) => [s.id, s]));
        expect(result.ids?.every((id) => byId.get(id)?.method !== 'POST')).toBe(true);
      }
      socket.send(JSON.stringify({ type: 'set-filter', query: 'host:(', seq: 2 }));
      const bad = messages.filter((m) => m.type === 'filter-result').at(-1);
      expect(bad).toMatchObject({ seq: 2 });
      expect(bad?.type === 'filter-result' && bad.error).toBeTruthy();
      // Like the daemon, an invalid query keeps the previous filter and reports its ids.
      if (bad?.type === 'filter-result' && result?.type === 'filter-result') expect(bad.ids).toEqual(result.ids);
      socket.send(JSON.stringify({ type: 'ping' }));
      expect(messages.at(-1)).toEqual({ type: 'pong' });
      // Live traffic arrives in batches carrying `filtered` deltas once a filter is set.
      await new Promise((r) => setTimeout(r, 900));
      const batch = messages.find((m) => m.type === 'exchanges');
      expect(batch).toBeDefined();
      if (batch?.type === 'exchanges') expect(batch.filtered).toBeDefined();
    } finally {
      server.stop();
    }
  });
});
