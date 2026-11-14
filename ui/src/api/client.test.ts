import { describe, expect, test } from 'bun:test';
import { ApiClient, ApiError, errorMessage, isApiError, type Transport, type WebSocketLike } from './client';

interface Recorded {
  url: string;
  init: RequestInit;
}

function fakeTransport(respond: (url: string, init: RequestInit) => Response | Promise<Response>): { transport: Transport; calls: Recorded[] } {
  const calls: Recorded[] = [];
  return {
    calls,
    transport: {
      fetch: async (url, init) => {
        calls.push({ url, init });
        return respond(url, init);
      },
      openSocket: () => ({}) as WebSocketLike,
    },
  };
}

const json = (body: unknown, status = 200) => new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } });

describe('ApiClient', () => {
  test('sends the bearer token, JSON bodies and query parameters', async () => {
    const { transport, calls } = fakeTransport(() => json([]));
    const client = new ApiClient({ baseUrl: 'http://127.0.0.1:45456', token: 'tok' }, transport);
    await client.exchanges.list({ filter: 'method:post', limit: 50 });
    await client.exchanges.patch({ ids: [1, 2], pinned: true });
    const first = calls[0];
    expect(first?.url).toBe('http://127.0.0.1:45456/api/exchanges?filter=method%3Apost&limit=50');
    expect((first?.init.headers as Record<string, string>).Authorization).toBe('Bearer tok');
    expect(calls[1]?.init.method).toBe('PATCH');
    expect(calls[1]?.init.body).toBe('{"ids":[1,2],"pinned":true}');
    expect((calls[1]?.init.headers as Record<string, string>)['Content-Type']).toBe('application/json');
  });

  test('without a token: no Authorization header, and never cookies', async () => {
    const { transport, calls } = fakeTransport(() => json({ recording: true }));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    await client.capture.get();
    expect((calls[0]?.init.headers as Record<string, string>).Authorization).toBeUndefined();
    // The daemon only accepts the Bearer token; cookies are never sent to the API.
    expect(calls[0]?.init.credentials).toBe('omit');
  });

  test('omits empty query values and encodes booleans / arrays', async () => {
    const { transport, calls } = fakeTransport(() => json({}));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    await client.exchanges.body(4, 'response', { original: true });
    await client.exchanges.body(4, 'request');
    expect(calls[0]?.url).toBe('/api/exchanges/4/body/response?original=1');
    expect(calls[1]?.url).toBe('/api/exchanges/4/body/request');
    expect(client.transfer.harUrl({ ids: [1, 2, 3] })).toBe('/api/export/har?ids=1%2C2%2C3');
  });

  test('download URLs carry ?token= (headers cannot be set on <a>/<img>)', () => {
    const client = new ApiClient({ baseUrl: 'http://h:1', token: 'a b' }, fakeTransport(() => json({})).transport);
    expect(client.ca.pemUrl()).toBe('http://h:1/api/ca/cert.pem?token=a+b');
    expect(client.exchanges.rawUrl(3, 'response', { decoded: true, download: true })).toBe('http://h:1/api/exchanges/3/body/response/raw?decoded=1&download=1&token=a+b');
    expect(client.streamUrl()).toBe('ws://h:1/api/stream?token=a%20b');
    // …except the terminal setup script: its one-liner sends the token as a header.
    expect(client.interceptors.terminalScriptUrl('bash')).toBe('http://h:1/api/setup/terminal?shell=bash');
  });

  test('204 and empty bodies resolve to undefined', async () => {
    const { transport } = fakeTransport(() => new Response(null, { status: 204 }));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    expect(await client.exchanges.delete([1])).toBeUndefined();
  });

  test('non-2xx responses become ApiError with the daemon code & message', async () => {
    const { transport } = fakeTransport(() => json({ error: { code: 'not-found', message: 'Exchange 9 not found' } }, 404));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    const err = await client.exchanges.get(9).catch((e: unknown) => e);
    expect(isApiError(err)).toBe(true);
    const e = err as ApiError;
    expect(e.status).toBe(404);
    expect(e.code).toBe('not-found');
    expect(e.message).toBe('Exchange 9 not found');
    expect(e.isNotFound).toBe(true);
    expect(e.method).toBe('GET');
    expect(e.path).toBe('/api/exchanges/9');
    expect(errorMessage(e)).toBe('Exchange 9 not found');
  });

  test('non-JSON error bodies and 401s are handled', async () => {
    const { transport } = fakeTransport((url) => (url.includes('status') ? new Response('', { status: 401, statusText: 'Unauthorized' }) : new Response('<html>Bad gateway</html>', { status: 502 })));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    const unauthorized = (await client.status().catch((e: unknown) => e)) as ApiError;
    expect(unauthorized.code).toBe('unauthorized');
    expect(unauthorized.isUnauthorized).toBe(true);
    const gateway = (await client.rules.get().catch((e: unknown) => e)) as ApiError;
    expect(gateway.status).toBe(502);
    expect(gateway.message).toContain('Bad gateway');
  });

  test('network failures, aborts and timeouts are distinguished', async () => {
    const net = new ApiClient({ baseUrl: '', token: null }, fakeTransport(() => Promise.reject(new TypeError('Failed to fetch'))).transport);
    const n = (await net.status().catch((e: unknown) => e)) as ApiError;
    expect(n.code).toBe('network');
    expect(n.isNetworkError).toBe(true);

    const hang: Transport = {
      fetch: (_u, init) =>
        new Promise((_res, rej) => {
          init.signal?.addEventListener('abort', () => rej(new DOMException('aborted', 'AbortError')));
        }),
      openSocket: () => ({}) as WebSocketLike,
    };
    const slow = new ApiClient({ baseUrl: '', token: null }, hang);
    const t = (await slow.request('GET', '/api/status', { timeoutMs: 5 }).catch((e: unknown) => e)) as ApiError;
    expect(t.code).toBe('timeout');
    const ctrl = new AbortController();
    const p = slow.status(ctrl.signal).catch((e: unknown) => e);
    ctrl.abort();
    expect(((await p) as ApiError).code).toBe('aborted');
    const pre = new AbortController();
    pre.abort();
    expect(((await slow.status(pre.signal).catch((e: unknown) => e)) as ApiError).isAborted).toBe(true);
  });

  test('invalid JSON from a 200 is a bad-response error', async () => {
    const { transport } = fakeTransport(() => new Response('{oops', { status: 200 }));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    expect(((await client.settings.get().catch((e: unknown) => e)) as ApiError).code).toBe('bad-response');
  });

  test('raw uploads keep their bytes and content type', async () => {
    const { transport, calls } = fakeTransport(() => json({ ids: [1] }));
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    const bytes = new Uint8Array([1, 2, 3]);
    await client.transfer.importSession(bytes);
    await client.transfer.importHar('{"log":{}}');
    expect(calls[0]?.init.body).toBe(bytes);
    expect((calls[0]?.init.headers as Record<string, string>)['Content-Type']).toBe('application/octet-stream');
    expect((calls[1]?.init.headers as Record<string, string>)['Content-Type']).toBe('application/json');
  });

  test('every documented endpoint has a typed method hitting the right method + path', async () => {
    const { transport, calls } = fakeTransport(() => json({}));
    const c = new ApiClient({ baseUrl: '', token: null }, transport);
    const req = { method: 'GET', url: 'https://x/', headers: [] as [string, string][], body: '' };
    const res = { status: 200, statusMessage: 'OK', headers: [] as [string, string][], body: '' };
    const expected: [() => Promise<unknown>, string][] = [
      [() => c.status(), 'GET /api/status'],
      [() => c.shutdown(), 'POST /api/shutdown'],
      [() => c.exchanges.list(), 'GET /api/exchanges'],
      [() => c.exchanges.get(1), 'GET /api/exchanges/1'],
      [() => c.exchanges.body(1, 'request'), 'GET /api/exchanges/1/body/request'],
      [() => c.exchanges.rawBytes(1, 'response'), 'GET /api/exchanges/1/body/response/raw'],
      [() => c.exchanges.messages(1, 5), 'GET /api/exchanges/1/messages?after=5'],
      [() => c.exchanges.snippet(1, 'curl'), 'GET /api/exchanges/1/snippet?target=curl'],
      [() => c.exchanges.delete([1]), 'POST /api/exchanges/delete'],
      [() => c.exchanges.clear({ keepPinned: true }), 'POST /api/exchanges/clear'],
      [() => c.exchanges.patch({ ids: [1], color: null }), 'PATCH /api/exchanges'],
      [() => c.exchanges.replay(1), 'POST /api/exchanges/1/replay'],
      [() => c.exchanges.editableRequest(1), 'GET /api/exchanges/1/editable/request'],
      [() => c.exchanges.editableResponse(1), 'GET /api/exchanges/1/editable/response'],
      [() => c.filter.validate('x'), 'POST /api/filter/validate'],
      [() => c.filter.suggest('x', 1), 'POST /api/filter/suggest'],
      [() => c.filter.query('x'), 'POST /api/filter/query'],
      [() => c.filter.docs(), 'GET /api/filter/docs'],
      [() => c.rules.get(), 'GET /api/rules'],
      [() => c.rules.put({ version: 1, items: [] }), 'PUT /api/rules'],
      [() => c.rules.stats(), 'GET /api/rules/stats'],
      [() => c.rules.test({ exchangeId: 1 }), 'POST /api/rules/test'],
      [() => c.rules.import({ version: 1, items: [] }), 'POST /api/rules/import'],
      [() => c.breakpoints.list(), 'GET /api/breakpoints'],
      [() => c.breakpoints.resume(1, { action: 'continue', request: req }), 'POST /api/breakpoints/1/resume'],
      [() => c.breakpoints.resume(1, { action: 'continue', response: res }), 'POST /api/breakpoints/1/resume'],
      [() => c.breakpoints.resumeAll(), 'POST /api/breakpoints/resume-all'],
      [() => c.send(req, { followRedirects: true }), 'POST /api/send'],
      [() => c.importCurl('curl x'), 'POST /api/import/curl'],
      [() => c.codegen.targets(), 'GET /api/codegen/targets'],
      [() => c.codegen.generate(req, 'python-requests'), 'POST /api/codegen'],
      [() => c.interceptors.list(), 'GET /api/interceptors'],
      [() => c.interceptors.activate('android-adb', { deviceId: 'x' }), 'POST /api/interceptors/android-adb/activate'],
      [() => c.interceptors.deactivate('chrome'), 'POST /api/interceptors/chrome/deactivate'],
      [() => c.interceptors.terminalScript('fish'), 'GET /api/setup/terminal?shell=fish'],
      [() => c.settings.get(), 'GET /api/settings'],
      [() => c.settings.put({} as never), 'PUT /api/settings'],
      [() => c.settings.patchUi({ theme: 'dark' }), 'PATCH /api/settings/ui'],
      [() => c.ca.get(), 'GET /api/ca'],
      [() => c.ca.regenerate(), 'POST /api/ca/regenerate'],
      [() => c.transfer.importHar('{}'), 'POST /api/import/har'],
      [() => c.transfer.importSession(new Uint8Array()), 'POST /api/import/session'],
      [() => c.capture.get(), 'GET /api/capture'],
      [() => c.capture.set(false), 'PUT /api/capture'],
      [() => c.license.get(), 'GET /api/license'],
      [() => c.license.activate('AKEV1.x'), 'PUT /api/license'],
      [() => c.license.remove(), 'DELETE /api/license'],
      [() => c.license.startTrial(), 'POST /api/license/trial'],
    ];
    for (const [call] of expected) await call();
    expect(calls.map((x) => `${x.init.method} ${x.url}`)).toEqual(expected.map(([, e]) => e));
    // URL-only endpoints (downloads)
    expect(c.rules.exportUrl()).toBe('/api/rules/export');
    expect(c.ca.derUrl()).toBe('/api/ca/cert.der');
    expect(c.transfer.sessionUrl()).toBe('/api/export/session');
    expect(c.interceptors.terminalScriptUrl('bash')).toBe('/api/setup/terminal?shell=bash');
  });

  test('extra error fields (402 pro-required) are kept; error listeners see every daemon error', async () => {
    const { transport } = fakeTransport((url) =>
      url.endsWith('/api/wireguard')
        ? json({ error: { code: 'pro-required', message: 'WireGuard capture requires Akev Pro', feature: 'wireguard' } }, 402)
        : json({ error: { code: 'not-found', message: 'nope' } }, 404),
    );
    const c = new ApiClient({ baseUrl: '', token: null }, transport);
    const seen: string[] = [];
    const off = c.onError((e) => seen.push(`${e.method} ${e.code}`));
    c.onError(() => {
      throw new Error('a broken listener must not hide the error');
    });
    const e = (await c.wireguard.update({ enabled: true }).catch((x: unknown) => x)) as ApiError;
    expect(e).toBeInstanceOf(ApiError);
    expect([e.status, e.code, e.message]).toEqual([402, 'pro-required', 'WireGuard capture requires Akev Pro']);
    expect(e.isProRequired).toBe(true);
    expect(e.feature).toBe('wireguard');
    expect(e.details).toEqual({ feature: 'wireguard' });
    const other = (await c.exchanges.get(1).catch((x: unknown) => x)) as ApiError;
    expect([other.isProRequired, other.feature, other.details]).toEqual([false, undefined, {}]);
    expect(seen).toEqual(['PUT pro-required', 'GET not-found']);
    off();
    await c.exchanges.get(1).catch(() => undefined);
    expect(seen.length).toBe(2);
  });

  test('a transport can rewrite DOM-facing URLs (mock data: URLs)', () => {
    const client = new ApiClient({ baseUrl: '', token: null }, { ...fakeTransport(() => json({})).transport, resolveUrl: (u) => `mock:${u}` });
    expect(client.ca.pemUrl()).toBe('mock:/api/ca/cert.pem');
  });
});

describe('WebSocket tools endpoints', () => {
  test('inject & paused-message calls hit the right method, path and body', async () => {
    const { transport, calls } = fakeTransport(() => json({ seq: 7 }, 202));
    const c = new ApiClient({ baseUrl: '', token: null }, transport);
    expect(await c.exchanges.sendMessage(4, { direction: 'sent', text: 'hi' })).toEqual({ seq: 7 });
    await c.breakpoints.messages();
    await c.breakpoints.resumeMessage(9, { action: 'forward', kind: 'binary', base64: 'AA==' });
    await c.breakpoints.resumeMessage(9, { action: 'drop' });
    await c.breakpoints.resumeAllMessages({ id: 4, action: 'drop' });
    await c.breakpoints.resumeAllMessages();
    expect(calls.map((x) => `${x.init.method} ${x.url}`)).toEqual([
      'POST /api/exchanges/4/messages',
      'GET /api/breakpoints/messages',
      'POST /api/breakpoints/messages/9/resume',
      'POST /api/breakpoints/messages/9/resume',
      'POST /api/breakpoints/messages/resume-all',
      'POST /api/breakpoints/messages/resume-all',
    ]);
    expect(JSON.parse(String(calls[0]?.init.body))).toEqual({ direction: 'sent', text: 'hi' });
    expect(JSON.parse(String(calls[2]?.init.body))).toEqual({ action: 'forward', kind: 'binary', base64: 'AA==' });
    expect(JSON.parse(String(calls[4]?.init.body))).toEqual({ id: 4, action: 'drop' });
  });
});
