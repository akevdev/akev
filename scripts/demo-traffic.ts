#!/usr/bin/env bun
/**
 * Generate realistic demo traffic through a running Akev proxy (for demos, screenshots and
 * manual QA). Starts a local origin with JSON/HTML/image/SSE/WebSocket/slow/error endpoints and
 * sends a mix of local and public HTTPS requests through the proxy.
 *
 *   bun scripts/demo-traffic.ts --proxy http://127.0.0.1:8000 --ca ~/.local/share/akev/ca.pem
 */
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const args = new Map<string, string>();
for (let i = 2; i < process.argv.length; i += 2) args.set(process.argv[i].replace(/^--/, ''), process.argv[i + 1]);
const proxy = args.get('proxy') ?? 'http://127.0.0.1:8000';
const caPath = args.get('ca');
const ca = caPath ? readFileSync(caPath, 'utf8') : undefined;
const rounds = Number(args.get('rounds') ?? '1');
const png = readFileSync(resolve(import.meta.dir, '../desktop/src-tauri/icons/128x128.png'));

const users = [
  { id: 1, name: 'Ada Lovelace', email: 'ada@example.com', role: 'admin', active: true },
  { id: 2, name: 'Grace Hopper', email: 'grace@example.com', role: 'engineer', active: true },
  { id: 3, name: 'Linus Torvalds', email: 'linus@example.com', role: 'maintainer', active: false },
];

const server = Bun.serve({
  port: 0,
  fetch(req, srv) {
    const url = new URL(req.url);
    switch (url.pathname) {
      case '/api/users':
        return Response.json({ users, total: users.length, page: 1 }, { headers: { 'cache-control': 'private, max-age=60', etag: '"u-42"' } });
      case '/api/login':
        return Response.json({ token: 'eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxIiwibmFtZSI6IkFkYSBMb3ZlbGFjZSIsImV4cCI6MjAwMDAwMDAwMH0.c2lnbmF0dXJl', expiresIn: 3600 }, { status: 201, headers: { 'set-cookie': 'session=abc123; Path=/; HttpOnly; Secure; SameSite=Lax' } });
      case '/':
      case '/index.html':
        return new Response('<!doctype html><html><head><title>Demo</title><style>body{font-family:sans-serif}</style></head><body><h1>Hello from the demo origin</h1><p>Akev is <b>intercepting</b> this page.</p><script>console.log("hi")</script></body></html>', { headers: { 'content-type': 'text/html; charset=utf-8' } });
      case '/static/app.js':
        return new Response('import { render } from "./ui.js";const state={count:0};function inc(){state.count++;render(state)}export default inc;', { headers: { 'content-type': 'application/javascript' } });
      case '/static/logo.png':
        return new Response(png, { headers: { 'content-type': 'image/png', 'cache-control': 'public, max-age=31536000, immutable' } });
      case '/events': {
        let n = 0;
        const stream = new ReadableStream({
          async pull(c) {
            await Bun.sleep(150);
            n++;
            c.enqueue(new TextEncoder().encode(`event: tick\nid: ${n}\ndata: {"tick":${n},"cpu":${(Math.random() * 100).toFixed(1)}}\n\n`));
            if (n >= 6) c.close();
          },
        });
        return new Response(stream, { headers: { 'content-type': 'text/event-stream', 'cache-control': 'no-cache' } });
      }
      case '/ws':
        if (srv.upgrade(req)) return undefined;
        return new Response('expected websocket', { status: 400 });
      case '/slow':
        return Bun.sleep(1200).then(() => Response.json({ ok: true, waited: 1200 }));
      case '/error':
        return Response.json({ error: 'internal_error', message: 'Database connection pool exhausted' }, { status: 500 });
      case '/missing':
        return Response.json({ error: 'not_found' }, { status: 404 });
      case '/redirect':
        return Response.redirect('/api/users', 302);
      case '/graphql':
        return Response.json({ data: { viewer: { login: 'ada', repositories: { totalCount: 42 } } } });
      default:
        return new Response('not found', { status: 404 });
    }
  },
  websocket: {
    message(ws, msg) {
      ws.send(typeof msg === 'string' ? `echo: ${msg}` : msg);
    },
  },
});
const origin = `http://localhost:${server.port}`;
const tls = ca ? { ca } : undefined;
const go = async (url: string, init: RequestInit = {}) => {
  try {
    const res = await fetch(url, { ...init, proxy, tls } as RequestInit);
    await res.arrayBuffer();
    console.log(res.status, init.method ?? 'GET', url);
  } catch (e) {
    console.log('ERR', init.method ?? 'GET', url, (e as Error).message);
  }
};

for (let r = 0; r < rounds; r++) {
  await go(`${origin}/`);
  await go(`${origin}/static/app.js`);
  await go(`${origin}/static/logo.png`);
  await go(`${origin}/api/users?page=1&limit=20`, { headers: { authorization: 'Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxIiwiZXhwIjoyMDAwMDAwMDAwfQ.x', accept: 'application/json' } });
  await go(`${origin}/api/login`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ email: 'ada@example.com', password: 'hunter2' }) });
  await go(`${origin}/graphql`, { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ operationName: 'Viewer', query: 'query Viewer { viewer { login repositories { totalCount } } }', variables: {} }) });
  await go(`${origin}/api/users/2`, { method: 'PATCH', headers: { 'content-type': 'application/x-www-form-urlencoded' }, body: 'role=admin&active=true' });
  await go(`${origin}/missing`);
  await go(`${origin}/error`, { method: 'DELETE' });
  await go(`${origin}/redirect`, { redirect: 'manual' });
  await go(`${origin}/events`);
  await go(`${origin}/slow`);
  await go('https://example.com/');
  await go('https://api.github.com/repos/rust-lang/rust', { headers: { 'user-agent': 'akev-demo' } });
  await go('https://jsonplaceholder.typicode.com/todos?_limit=5');
  await go('https://www.google.com/favicon.ico');
  await go('http://127.0.0.1:1/unreachable');
}

// WebSocket through the proxy (Bun supports `proxy` for WebSocket clients).
try {
  const ws = new WebSocket(`ws://localhost:${server.port}/ws`, { proxy } as unknown as string[]);
  await new Promise<void>((resolveWs) => {
    ws.onopen = () => {
      ws.send(JSON.stringify({ type: 'subscribe', channel: 'prices' }));
      ws.send('ping');
      ws.send(JSON.stringify({ type: 'order', side: 'buy', qty: 3 }));
      setTimeout(() => {
        ws.close(1000, 'done');
        resolveWs();
      }, 800);
    };
    ws.onerror = () => resolveWs();
  });
  console.log('websocket done');
} catch (e) {
  console.log('websocket ERR', (e as Error).message);
}
server.stop(true);
