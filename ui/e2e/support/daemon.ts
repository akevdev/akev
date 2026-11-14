/**
 * Talking to the e2e daemon from the test process: its REST API, its proxy, and a local
 * upstream server for the proxied requests (so no test depends on the network).
 */

import { createServer, request as httpRequest, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { test as base, expect, type Page } from '@playwright/test';
import { API_URL, TOKEN } from './env';

/** Call the daemon's API with the token (the raw response). */
export function apiFetch(path: string, init: { method?: string; body?: unknown } = {}): Promise<Response> {
  return fetch(`${API_URL}${path}`, {
    method: init.method ?? 'GET',
    headers: { authorization: `Bearer ${TOKEN}`, ...(init.body !== undefined ? { 'content-type': 'application/json' } : {}) },
    body: init.body !== undefined ? JSON.stringify(init.body) : undefined,
  });
}

/** Call the daemon's API with the token; throws unless it succeeds. */
export async function api<T = unknown>(path: string, init: { method?: string; body?: unknown } = {}): Promise<T> {
  const res = await apiFetch(path, init);
  if (!res.ok) throw new Error(`${init.method ?? 'GET'} ${path}: HTTP ${res.status} ${await res.text()}`);
  const text = await res.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

/** The proxy's port (the daemon was started with `--proxy-port 0`). */
export async function proxyPort(): Promise<number> {
  const status = await api<{ proxy: { port: number } }>('/api/status');
  return status.proxy.port;
}

export interface ProxiedResponse {
  status: number;
  headers: IncomingMessage['headers'];
  body: string;
}

/** Send an HTTP request through Akev's proxy (absolute-form request line, like any HTTP client with a proxy). */
export async function viaProxy(url: string, init: { method?: string; body?: string; headers?: Record<string, string> } = {}): Promise<ProxiedResponse> {
  const port = await proxyPort();
  const target = new URL(url);
  return new Promise((resolve, reject) => {
    const req = httpRequest(
      { host: '127.0.0.1', port, method: init.method ?? 'GET', path: target.href, headers: { host: target.host, ...init.headers } },
      (res) => {
        const chunks: Buffer[] = [];
        res.on('data', (c: Buffer) => chunks.push(c));
        res.on('end', () => resolve({ status: res.statusCode ?? 0, headers: res.headers, body: Buffer.concat(chunks).toString('utf8') }));
        res.on('error', reject);
      },
    );
    req.on('error', reject);
    if (init.body) req.write(init.body);
    req.end();
  });
}

/** Clear traffic and rules so a test starts from a known state. */
export async function resetDaemon(): Promise<void> {
  await api('/api/exchanges/clear', { method: 'POST', body: {} });
  await api('/api/rules', { method: 'PUT', body: { version: 1, items: [] } });
}

/**
 * A local HTTP server the proxied requests go to. It answers JSON describing the request; `/missing*` is 404,
 * `/slow` waits a little.
 */
export interface Upstream {
  url: string;
  host: string;
  requests: { method: string; path: string }[];
}

async function startUpstream(): Promise<{ upstream: Upstream; server: Server }> {
  const requests: Upstream['requests'] = [];
  const server = createServer((req, res) => {
    const path = req.url ?? '/';
    requests.push({ method: req.method ?? 'GET', path });
    const body = JSON.stringify({ ok: true, method: req.method, path, served: requests.length });
    const status = path.startsWith('/missing') ? 404 : 200;
    res.writeHead(status, { 'content-type': 'application/json', 'x-upstream': 'akev-e2e' });
    res.end(body);
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  return { upstream: { url: `http://127.0.0.1:${port}`, host: `127.0.0.1:${port}`, requests }, server };
}

/** Open the UI with the token, past the first-run setup (unless `firstRun` is kept). */
export async function openApp(page: Page, route = 'traffic', options: { keepFirstRun?: boolean } = {}): Promise<void> {
  if (!options.keepFirstRun) {
    // A finished setup in the local UI preferences (the daemon's copy may be empty).
    await page.addInitScript(() => {
      try {
        const key = 'akev.ui.v1';
        const prefs = JSON.parse(window.localStorage.getItem(key) ?? '{}') as { pages?: Record<string, unknown>; updatedAt?: number };
        window.localStorage.setItem(key, JSON.stringify({ ...prefs, version: 1, updatedAt: Math.max(prefs.updatedAt ?? 0, 1), pages: { ...prefs.pages, 'firstRun.done': true } }));
      } catch {
        // storage unavailable: the test will see the setup
      }
    });
  }
  await page.goto(`/?token=${TOKEN}#/${route}`);
  // Connected: the status bar shows the proxy address.
  await expect(page.getByRole('contentinfo').getByText(/127\.0\.0\.1:\d+/).first()).toBeVisible();
}

/** Playwright `test` with a fresh daemon state and a local upstream per test. */
export const test = base.extend<{ upstream: Upstream }>({
  // biome-ignore lint/correctness/noEmptyPattern: Playwright reads a fixture's dependencies from this destructuring pattern
  upstream: async ({}, use) => {
    await resetDaemon();
    const { upstream, server } = await startUpstream();
    await use(upstream);
    await new Promise<void>((resolve) => server.close(() => resolve()));
  },
});

export { expect };
