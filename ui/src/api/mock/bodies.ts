/**
 * Mock body generators and the `BodyView` builder (mirrors what akev-content's `view_body`
 * returns: decoded text, pretty-printed text, structured decodes for binary formats).
 */

import { base64ToBytes, looksLikeText, utf8Decode, utf8Encode } from '../../lib/encoding';
import type { BodyView, ContentKind, GrpcMessage, MultipartPart, SseEvent } from '../types';
import type { Rng } from './rng';

export const TINY_PNG_B64 =
  'iVBORw0KGgoAAAANSUhEUgAAADAAAAAwCAYAAABXAvmHAAADTElEQVR42tWYaUtVURSG7z90HnOeZ72O9zrnlCMVVDRRUBQUJqUVDdIgZR8KgiKHUowko5+gX97WyzpBRObZ++z7YS9YIPjlec7Zd+13nVh/EdDHLgZ62SVAkl0KJNhlQA+7HOhmVwBd7Eqgk10FdLCrgXZ2DRBn1wJt7Drt1nrtlgbt5kbtpibtxmbthhbt+lbtujbt2rh2Tbt2dYd0JxDzGV4FPIav6hIBn+FVwGP4ym4R8BleBTyGr+gRAZ/hVcBj+PKECPgMrwIew5clRcA1fItADw4C09PAyVPakzNA/7AItLmFVwFH8EMDwOVLwNISsLoKfPgIbG4CG5v69+s3wOID4MIViS4jbuBLe0UgKnxnXMFfPAd2doCDAxxa+/K/na/AsxXg/FV5Y4lo8CoQAX64H1i4o0/atNY/A7fvyQgfs4cv6ROBKPD35bjs7cG6vv8A7j4SiXE7eBWwPDZ88lHg/5S4tSiTqs8cvlgeYszmB8szb3NsDj1OX4Cz18zhVcBi2vAH67qWX8kwmTSDLxKWmOmc59PntHFd29/kLVw3g1cBw0uKc/5/o9K2OGIXnsgbSIaHPyYXZswkHvCG5SWVqlp5KwNiKjy8ChhkG8YD3qqpqvdrMp7PhIcvHBIBk2DGXONy+vxda1vAxMXw8CpgkCopsJFKgW0VCAtfOBwIhI3ETJUpPULrMqbPhYcvoIBJnmckZqpMVb18JyN9Jjx8wfFAIOwywjzPSLyfojE6/1Se/kB4+HwKmG5SzPOMxK5raxc4fdMMPn8kEDBZA7mMMM+7rsdyNOOzZvB5FLDZYbmMMM+7qk/b+vRN4fNGAwHTBZybFJcRRuGotfsTuPFQkuioOXwuBWy/HnCT4jISRYLw88tyD83ZweeOBQK2nz64SXEZYZ63OTZ88lHgcygQ9bsNNykuI8zzjMT7Ryz1nDb8wfLM2x6b3/A544GAi49OXEaY5xmJmSoZzJhtGA94w/KS4pwnuM20+Rd8NgVcfzFjnmckZqpkrmEzHvCGNb2kjoLPnggEXH/uM8nzUeCzKOAzfNaJQMBX+EwK+AyfORkI+AqfQQGf4TOmAgFf4dMp4DN8+nQg4Ct8GgV8hk+boYDH8GmzwC+z85N+yBkIoQAAAABJRU5ErkJggg==';

/** A lazily materialised body. */
export interface BodySpec {
  kind: ContentKind;
  contentType: string;
  /** Produces the decoded body. Called at most once (cached by the record). */
  make: (rng: Rng) => string | Uint8Array;
  /** Structured decode for binary formats (protobuf → JSON, form fields, SSE events…). */
  structured?: (rng: Rng) => unknown;
  /** Content-encoding applied on the wire (affects `wireSize`). */
  encoding?: 'gzip' | 'br' | 'zstd';
  /** Simulated capture truncation. */
  captureTruncated?: boolean;
  /** Pretend the decoded size is this (large media/fonts) instead of generating the bytes. */
  reportedSize?: number;
}

export function bodyBytes(body: string | Uint8Array): Uint8Array {
  return typeof body === 'string' ? utf8Encode(body) : body;
}

/** Wire size given a coding (rough compression ratios). */
export function wireSizeFor(size: number, encoding?: string): number {
  if (!encoding) return size;
  const ratio = encoding === 'br' ? 0.22 : encoding === 'zstd' ? 0.25 : 0.3;
  return Math.max(20, Math.round(size * ratio));
}

const TEXTUAL: ReadonlySet<ContentKind> = new Set(['json', 'json-lines', 'xml', 'html', 'javascript', 'css', 'markdown', 'yaml', 'csv', 'text', 'form-url-encoded', 'event-stream', 'graph-ql']);

/** Build the `BodyView` the daemon would return for this body. */
export function buildBodyView(spec: BodySpec | null, data: string | Uint8Array | null, rng: Rng): BodyView {
  if (!spec || data == null) {
    return { kind: 'unknown', size: 0, wireSize: 0, encodings: [], text: null, formatted: null, structured: null, decodeError: null, truncated: false, captureTruncated: false };
  }
  const bytes = bodyBytes(data);
  const size = spec.reportedSize ?? bytes.length;
  let text: string | null = null;
  let formatted: string | null = null;
  let structured: unknown = spec.structured ? spec.structured(rng) : null;
  if (TEXTUAL.has(spec.kind) || (spec.kind === 'unknown' && looksLikeText(bytes))) {
    text = typeof data === 'string' ? data : utf8Decode(bytes);
    if (spec.kind === 'json' || spec.kind === 'graph-ql') {
      try {
        formatted = JSON.stringify(JSON.parse(text), null, 2);
      } catch {
        formatted = null;
      }
    } else if (spec.kind === 'json-lines') {
      formatted = text
        .split('\n')
        .filter(Boolean)
        .map((l) => {
          try {
            return JSON.stringify(JSON.parse(l));
          } catch {
            return l;
          }
        })
        .join('\n');
    } else if (spec.kind === 'form-url-encoded') {
      // akev-content `form::parse_urlencoded`: ordered pairs, duplicates kept.
      structured = structured ?? [...new URLSearchParams(text)];
    } else if (spec.kind === 'event-stream') {
      structured = structured ?? parseSse(text);
    }
  } else if (spec.kind === 'multipart' && structured == null) {
    structured = parseMultipart(typeof data === 'string' ? data : utf8Decode(bytes), spec.contentType);
  } else if ((spec.kind === 'grpc' || spec.kind === 'grpc-web') && structured != null && !Array.isArray(structured)) {
    // akev-content `view_body`: `{messages: [{compressed, decoded, trailers, size}], error?}`.
    structured = { messages: [{ compressed: false, decoded: structured, trailers: null, size: bytes.length } satisfies GrpcMessage & { size: number }] };
  } else if (spec.kind === 'image' && structured == null) {
    structured = { format: spec.contentType.split('/')[1]?.replace('svg+xml', 'svg') ?? 'png', width: 48, height: 48 };
  }
  if (spec.kind === 'image' && spec.contentType === 'image/svg+xml') {
    text = typeof data === 'string' ? data : utf8Decode(bytes);
  }
  return {
    kind: spec.kind,
    size,
    wireSize: wireSizeFor(size, spec.encoding),
    encodings: spec.encoding ? [spec.encoding] : [],
    text,
    formatted,
    structured,
    decodeError: null,
    truncated: false,
    captureTruncated: spec.captureTruncated ?? false,
  };
}

/** Minimal text/event-stream parser producing akev-content `SseEvent`s. */
export function parseSse(text: string): SseEvent[] {
  const events: SseEvent[] = [];
  for (const block of text.split(/\r?\n\r?\n/)) {
    if (!block.trim()) continue;
    const ev: SseEvent = { event: null, id: null, retry: null, data: '' };
    const data: string[] = [];
    for (const line of block.split(/\r?\n/)) {
      if (line.startsWith(':')) continue;
      const i = line.indexOf(':');
      const field = i < 0 ? line : line.slice(0, i);
      const value = i < 0 ? '' : line.slice(i + 1).replace(/^ /, '');
      if (field === 'data') data.push(value);
      else if (field === 'event') ev.event = value;
      else if (field === 'id') ev.id = value;
      else if (field === 'retry' && /^\d+$/.test(value)) ev.retry = Number(value);
    }
    ev.data = data.join('\n');
    events.push(ev);
  }
  return events;
}

/** Minimal multipart parser producing akev-content `Part`s (text bodies only). */
export function parseMultipart(text: string, contentType: string): MultipartPart[] {
  const boundary = /boundary="?([^";]+)"?/i.exec(contentType)?.[1];
  if (!boundary) return [];
  const parts: MultipartPart[] = [];
  for (const chunk of text.split(`--${boundary}`).slice(1)) {
    if (chunk.startsWith('--')) break;
    const body = chunk.replace(/^\r?\n/, '');
    const split = body.search(/\r?\n\r?\n/);
    if (split < 0) continue;
    const head = body.slice(0, split);
    const content = body.slice(split).replace(/^\r?\n\r?\n/, '').replace(/\r?\n$/, '');
    const headers = head
      .split(/\r?\n/)
      .filter(Boolean)
      .map((l): [string, string] => {
        const i = l.indexOf(':');
        return [l.slice(0, i).trim(), l.slice(i + 1).trim()];
      });
    const disposition = headers.find(([k]) => k.toLowerCase() === 'content-disposition')?.[1] ?? '';
    const contentTypeHeader = headers.find(([k]) => k.toLowerCase() === 'content-type')?.[1] ?? null;
    parts.push({
      headers,
      name: /\bname="([^"]*)"/.exec(disposition)?.[1] ?? null,
      filename: /\bfilename="([^"]*)"/.exec(disposition)?.[1] ?? null,
      contentType: contentTypeHeader,
      size: utf8Encode(content).length,
      text: content.length < 65_536 ? content : null,
    });
  }
  return parts;
}

// =====================================================================================
// Generators
// =====================================================================================

const FIRST = ['Ada', 'Grace', 'Linus', 'Margaret', 'Ken', 'Barbara', 'Dennis', 'Radia', 'Guido', 'Frances', 'Bjarne', 'Hedy'];
const LAST = ['Lovelace', 'Hopper', 'Torvalds', 'Hamilton', 'Thompson', 'Liskov', 'Ritchie', 'Perlman', 'van Rossum', 'Allen', 'Stroustrup', 'Lamarr'];
const REPOS = ['akev', 'mockttp', 'hyper', 'tokio', 'react', 'vite', 'rustls', 'axum', 'zustand', 'codemirror'];

export function json(value: unknown): string {
  return JSON.stringify(value);
}

export function usersJson(rng: Rng, n = 8): string {
  return json({
    data: Array.from({ length: n }, (_, i) => {
      const first = rng.pick(FIRST);
      const last = rng.pick(LAST);
      return {
        id: 1000 + i,
        name: `${first} ${last}`,
        email: `${first.toLowerCase()}.${last.toLowerCase().replace(/\s/g, '')}@example.com`,
        role: rng.pick(['admin', 'editor', 'viewer']),
        active: rng.chance(0.8),
        lastLogin: new Date(Date.now() - rng.int(0, 30) * 86_400_000).toISOString(),
        tags: rng.chance(0.5) ? ['beta'] : [],
        manager: null,
      };
    }),
    page: 1,
    perPage: n,
    total: 124,
  });
}

export function repoJson(rng: Rng): string {
  const name = rng.pick(REPOS);
  return json({
    id: rng.int(10_000_000, 99_999_999),
    node_id: 'R_kgDOK' + rng.int(1000, 9999),
    name,
    full_name: `akev/${name}`,
    private: false,
    owner: { login: 'akev', id: 145_221, type: 'Organization', site_admin: false },
    html_url: `https://github.com/akev/${name}`,
    description: 'Blazing fast HTTP(S) interception, inspection and mocking.',
    fork: false,
    created_at: '2025-03-14T09:26:53Z',
    updated_at: new Date().toISOString(),
    stargazers_count: rng.int(100, 40_000),
    watchers_count: rng.int(10, 900),
    language: 'Rust',
    forks_count: rng.int(5, 2000),
    open_issues_count: rng.int(0, 120),
    license: { key: 'mit', name: 'MIT License', spdx_id: 'MIT' },
    topics: ['http', 'proxy', 'debugging', 'mitm', 'rust'],
    default_branch: 'main',
    permissions: { admin: false, push: false, pull: true },
  });
}

export function pullsJson(rng: Rng): string {
  return json(
    Array.from({ length: rng.int(3, 7) }, (_, i) => ({
      number: 400 + i,
      state: 'open',
      title: rng.pick(['Fix HTTP/2 flow control stall', 'Add JA4 fingerprints', 'Virtualise rules list', 'Spill bodies with zstd', 'Support SOCKS5 auth', 'HAR 1.2 import edge cases']),
      user: { login: rng.pick(['ada', 'grace', 'linus', 'radia']) },
      draft: rng.chance(0.2),
      labels: [{ name: rng.pick(['bug', 'enhancement', 'performance']) }],
      created_at: new Date(Date.now() - rng.int(1, 20) * 86_400_000).toISOString(),
    })),
  );
}

export function graphqlRequest(): string {
  return json({
    operationName: 'RepoStars',
    query: 'query RepoStars($owner: String!, $name: String!) { repository(owner: $owner, name: $name) { stargazerCount forkCount } }',
    variables: { owner: 'akev', name: 'akev' },
  });
}

export function graphqlResponse(rng: Rng): string {
  return json({ data: { repository: { stargazerCount: rng.int(1000, 30_000), forkCount: rng.int(10, 900) } } });
}

export function loginRequest(): string {
  return json({ email: 'ada@example.com', password: '••••••••', remember: true });
}

export function loginResponse(ok: boolean): string {
  const payload = btoa(JSON.stringify({ sub: '1000', name: 'Ada Lovelace', iat: Math.floor(Date.now() / 1000), exp: Math.floor(Date.now() / 1000) + 3600, scope: 'read write' })).replace(/=+$/, '');
  return ok
    ? json({ token: `eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.${payload}.c2lnbmF0dXJl`, user: { id: 1000, name: 'Ada Lovelace' }, expiresIn: 3600 })
    : json({ error: { code: 'invalid_credentials', message: 'Email or password is incorrect' } });
}

export function orderJson(rng: Rng): string {
  const items = Array.from({ length: rng.int(1, 4) }, () => ({ sku: `SKU-${rng.int(1000, 9999)}`, qty: rng.int(1, 3), price: rng.int(500, 9900) / 100 }));
  return json({
    id: rng.int(10, 999),
    status: rng.pick(['pending', 'paid', 'shipped', 'delivered']),
    currency: 'EUR',
    items,
    total: Math.round(items.reduce((a, i) => a + i.qty * i.price, 0) * 100) / 100,
    shipping: { method: 'express', address: { city: 'Ljubljana', country: 'SI' } },
    createdAt: new Date().toISOString(),
  });
}

export function errorJson(status: number, message: string): string {
  return json({ error: { status, message, requestId: `req_${Math.random().toString(36).slice(2, 12)}` } });
}

export function suggestJson(rng: Rng): string {
  const q = rng.pick(['akev', 'http proxy', 'rust async', 'mitm certificate']);
  return json([q, [`${q} github`, `${q} tutorial`, `${q} vs http toolkit`, `${q} docker`], [], { 'google:suggesttype': ['QUERY', 'QUERY', 'QUERY', 'QUERY'] }]);
}

export function htmlPage(title: string): string {
  return `<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>${title}</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/bootstrap@5.3.3/dist/css/bootstrap.min.css">
  <script type="module" src="/src/main.tsx"></script>
</head>
<body class="bg-light">
  <header class="navbar navbar-dark bg-dark px-3">
    <a class="navbar-brand" href="/">${title}</a>
  </header>
  <main class="container py-5">
    <h1 class="display-5">Welcome back</h1>
    <p class="lead">Your dashboard is loading&hellip;</p>
    <div id="root"></div>
  </main>
</body>
</html>`;
}

export function jsBundle(rng: Rng): string {
  const fns = Array.from({ length: 24 }, (_, i) => `function ${['a', 'b', 'c', 'd', 'e', 'f'][i % 6]}${i}(t,e){return t&&e?t.concat(e):t||e||[]}`);
  return `/*! react v19.1.0 | (c) Meta Platforms, Inc. | MIT */\n"use strict";(function(){var x=${rng.int(1, 99)};${fns.join(';')};window.__build="${Math.random().toString(36).slice(2, 10)}"})();\n`;
}

export function viteModule(): string {
  return `import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import { App } from './App';
import './index.css';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
`;
}

export function cssBundle(): string {
  return `:root{--bs-blue:#0d6efd;--bs-indigo:#6610f2;--bs-purple:#6f42c1;--bs-font-sans-serif:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
*,::after,::before{box-sizing:border-box}
body{margin:0;font-family:var(--bs-font-sans-serif);font-size:1rem;line-height:1.5;color:#212529;background-color:#fff}
.container{width:100%;padding-right:.75rem;padding-left:.75rem;margin-right:auto;margin-left:auto}
.navbar{position:relative;display:flex;flex-wrap:wrap;align-items:center;justify-content:space-between;padding:.5rem 0}
.btn{display:inline-block;padding:.375rem .75rem;font-size:1rem;border-radius:.375rem;transition:color .15s ease-in-out,background-color .15s ease-in-out}
@media (min-width:768px){.container{max-width:720px}}
`;
}

export function svgLogo(): string {
  return `<svg xmlns="http://www.w3.org/2000/svg" width="120" height="40" viewBox="0 0 120 40"><rect width="120" height="40" rx="8" fill="#7c5cff"/><text x="60" y="26" font-family="Inter, sans-serif" font-size="16" font-weight="700" fill="#fff" text-anchor="middle">akev</text></svg>`;
}

export function pngBytes(): Uint8Array {
  return base64ToBytes(TINY_PNG_B64);
}

/** Deterministic pseudo-random binary blob with a given magic header. */
export function binaryBlob(rng: Rng, size: number, magic: number[] = []): Uint8Array {
  const out = new Uint8Array(size);
  out.set(magic.slice(0, size));
  for (let i = magic.length; i < size; i++) out[i] = rng.int(0, 255);
  return out;
}

/** A realistic-looking protobuf payload (field tags + varints + strings). */
export function protobufBytes(rng: Rng): Uint8Array {
  const parts: number[] = [];
  const str = (field: number, s: string) => {
    const b = utf8Encode(s);
    parts.push((field << 3) | 2, b.length, ...b);
  };
  str(1, 'akev');
  parts.push((2 << 3) | 0, rng.int(1, 120));
  str(3, rng.pick(['Hello, Ada!', 'Hello, Grace!', 'Hello, Linus!']));
  parts.push((4 << 3) | 0, 1);
  return new Uint8Array(parts);
}

export function grpcFrame(rng: Rng): Uint8Array {
  const msg = protobufBytes(rng);
  const out = new Uint8Array(5 + msg.length);
  out[0] = 0;
  new DataView(out.buffer).setUint32(1, msg.length);
  out.set(msg, 5);
  return out;
}

export function formBody(): string {
  return 'amount=2999&currency=eur&payment_method_types%5B%5D=card&description=Akev+Pro&metadata%5Border_id%5D=6735';
}

export function multipartBody(boundary: string): string {
  return [
    `--${boundary}`,
    'Content-Disposition: form-data; name="title"',
    '',
    'Quarterly report',
    `--${boundary}`,
    'Content-Disposition: form-data; name="file"; filename="report.csv"',
    'Content-Type: text/csv',
    '',
    'month,revenue,users\njan,12000,340\nfeb,13250,377\nmar,15100,412',
    `--${boundary}--`,
    '',
  ].join('\r\n');
}

export function sseBody(rng: Rng, n = 6): string {
  return Array.from({ length: n }, (_, i) => `id: ${i + 1}\nevent: ${rng.pick(['update', 'heartbeat', 'notification'])}\ndata: ${json({ seq: i + 1, value: rng.int(0, 100) })}\n\n`).join('');
}

export function xmlFeed(): string {
  return `<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Akev releases</title>
    <link>https://akev.dev/releases</link>
    <item>
      <title>Akev 0.1.0</title>
      <pubDate>Sun, 08 Nov 2026 10:00:00 GMT</pubDate>
      <description>First public release.</description>
    </item>
  </channel>
</rss>`;
}

export function telemetryNdjson(rng: Rng): string {
  return Array.from({ length: 4 }, () => json({ event: rng.pick(['page_view', 'click', 'scroll', 'error']), ts: Date.now() - rng.int(0, 5000), session: 's_8f2k1' })).join('\n');
}

export function npmPackageJson(rng: Rng): string {
  const versions: Record<string, unknown> = {};
  for (let i = 0; i < 30; i++) versions[`19.${Math.floor(i / 10)}.${i % 10}`] = { name: 'react', version: `19.${Math.floor(i / 10)}.${i % 10}`, dist: { shasum: Math.random().toString(16).slice(2), tarball: `https://registry.npmjs.org/react/-/react-19.${i}.tgz` } };
  return json({ _id: 'react', name: 'react', 'dist-tags': { latest: '19.3.1', next: `19.4.0-canary-${rng.int(1000, 9999)}` }, versions, license: 'MIT' });
}
