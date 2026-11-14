import { afterEach, describe, expect, test } from 'bun:test';
import { withDownloadTicket } from './platform';

const realFetch = globalThis.fetch;

afterEach(() => {
  globalThis.fetch = realFetch;
});

describe('withDownloadTicket', () => {
  test('swaps the token for a one-time ticket', async () => {
    const calls: { url: string; init?: RequestInit }[] = [];
    globalThis.fetch = (async (url: string, init?: RequestInit) => {
      calls.push({ url: String(url), init });
      return new Response(JSON.stringify({ ticket: 'tkt123', expiresInMs: 60000 }), { status: 200 });
    }) as typeof fetch;
    const out = await withDownloadTicket('http://127.0.0.1:45456/api/export/har?filter=a&token=secret');
    expect(out).toBe('http://127.0.0.1:45456/api/export/har?filter=a&ticket=tkt123');
    expect(out).not.toContain('secret');
    expect(calls[0]?.url).toBe('http://127.0.0.1:45456/api/download-ticket');
    expect((calls[0]?.init?.headers as Record<string, string>).Authorization).toBe('Bearer secret');
    expect(JSON.parse(String(calls[0]?.init?.body))).toEqual({ path: '/api/export/har' });
  });

  test('leaves URLs without a token alone, and falls back when the daemon refuses', async () => {
    globalThis.fetch = (async () => new Response('{}', { status: 404 })) as unknown as typeof fetch;
    expect(await withDownloadTicket('blob:abc')).toBe('blob:abc');
    expect(await withDownloadTicket('http://127.0.0.1:1/api/ca/cert.pem')).toBe('http://127.0.0.1:1/api/ca/cert.pem');
    const withToken = 'http://127.0.0.1:1/api/ca/cert.pem?token=t';
    expect(await withDownloadTicket(withToken)).toBe(withToken);
  });
});
