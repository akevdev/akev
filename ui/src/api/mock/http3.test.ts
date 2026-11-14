import { describe, expect, test } from 'bun:test';
import type { SendResponse, Settings, Status } from '../types';
import { MockServer } from './server';

const json = async <T>(r: Response): Promise<T> => (await r.json()) as T;
const call = (server: MockServer, method: string, path: string, body?: unknown) =>
  server.fetch(path, { method, ...(body === undefined ? {} : { body: JSON.stringify(body), headers: { 'content-type': 'application/json' } }) });

describe('mock daemon: HTTP/3', () => {
  test('settings default off; the listener status follows them', async () => {
    const server = new MockServer();
    const settings = await json<Settings>(await call(server, 'GET', '/api/settings'));
    expect(settings.proxy.http3).toEqual({ upstream: 'off', listen: false, listenPort: 0 });
    expect((await json<Status>(await call(server, 'GET', '/api/status'))).proxy.http3).toEqual({ enabled: false });
    const next: Settings = { ...settings, proxy: { ...settings.proxy, http3: { upstream: 'alt-svc', listen: true, listenPort: 0 } } };
    expect((await call(server, 'PUT', '/api/settings', next)).ok).toBe(true);
    const status = await json<Status>(await call(server, 'GET', '/api/status'));
    expect(status.proxy.http3).toEqual({ enabled: true, address: `127.0.0.1:${status.proxy.port}`, port: status.proxy.port });
  });

  test('send honours httpVersion for https URLs', async () => {
    const server = new MockServer();
    const send = async (url: string, httpVersion?: string) =>
      json<SendResponse>(await call(server, 'POST', '/api/send', { request: { method: 'GET', url, headers: [], body: '' }, options: httpVersion ? { httpVersion } : {} }));
    expect((await send('https://h3.example/', 'http3')).response?.httpVersion).toBe('HTTP/3');
    expect((await send('https://h3.example/', 'http2')).response?.httpVersion).toBe('HTTP/2');
    expect((await send('https://h3.example/')).response?.httpVersion).toBe('HTTP/1.1');
    expect((await send('http://plain.example/', 'http3')).response?.httpVersion).toBe('HTTP/1.1');
  });
});
