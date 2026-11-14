import { describe, expect, test } from 'bun:test';
import type { ExchangeSummary } from '../../../api/types';
import { compileFilter } from '../../../api/mock/filter';
import { appendClause } from '../filter/filterEdit';
import { buildRows, domainClause, hasClause, HostIndex, hostClause, registrableDomain, removeClause, sourceClause, type SidebarRow } from './hostIndex';

function summary(id: number, host: string, source?: string): ExchangeSummary {
  return {
    id,
    kind: 'http',
    state: 'complete',
    startedAt: id,
    method: 'GET',
    scheme: 'https',
    host,
    port: 443,
    path: '/',
    httpVersion: 'HTTP/2',
    status: 200,
    contentKind: 'json',
    requestSize: 0,
    responseSize: 0,
    source,
  };
}

/** A tiny stand-in for the traffic store (ascending ids). */
function store(rows: ExchangeSummary[]) {
  const summaries = new Map(rows.map((s) => [s.id, s]));
  return { summaries, order: rows.map((s) => s.id) };
}

const HOSTS = ['api.github.com', 'github.com', 'www.google.com', 'fonts.gstatic.com', 'localhost', '10.0.0.7', 'shop.example.co.uk', 'example.co.uk', 'akev.github.io'];
const SOURCES = ['Chrome', 'curl', undefined, 'node'];

function synth(n: number, from = 0): ExchangeSummary[] {
  return Array.from({ length: n }, (_, i) => summary(from + i, HOSTS[(from + i) % HOSTS.length] as string, SOURCES[(from + i) % SOURCES.length]));
}

describe('registrable domains', () => {
  test('groups hosts the way people read them', () => {
    expect(registrableDomain('api.github.com')).toBe('github.com');
    expect(registrableDomain('github.com')).toBe('github.com');
    expect(registrableDomain('a.b.c.example.org')).toBe('example.org');
    expect(registrableDomain('shop.example.co.uk')).toBe('example.co.uk');
    expect(registrableDomain('www.bbc.co.uk')).toBe('bbc.co.uk');
    expect(registrableDomain('news.yahoo.co.jp')).toBe('yahoo.co.jp');
    expect(registrableDomain('akev.github.io')).toBe('akev.github.io');
    expect(registrableDomain('preview-1.myapp.vercel.app')).toBe('myapp.vercel.app');
    expect(registrableDomain('API.Example.COM.')).toBe('example.com');
    expect(registrableDomain('localhost')).toBe('localhost');
    expect(registrableDomain('10.0.0.7')).toBe('10.0.0.7');
    expect(registrableDomain('::1')).toBe('::1');
    expect(registrableDomain('[2001:db8::1]')).toBe('[2001:db8::1]');
  });
});

describe('HostIndex', () => {
  test('counts per host and app, incrementally', () => {
    const idx = new HostIndex();
    const rows = synth(18);
    const s = store(rows);
    expect(idx.sync(s.order, s.summaries)).toBe(true);
    expect(idx.hosts.get('api.github.com')).toBe(2);
    // Ids 2, 6, 10 and 14 have no source.
    expect(idx.sources.get('')).toBe(4);
    expect(idx.sync(s.order, s.summaries)).toBe(false);
    // Appended traffic is added without a recount.
    const more = synth(9, 18);
    for (const r of more) s.summaries.set(r.id, r);
    s.order.push(...more.map((r) => r.id));
    expect(idx.sync(s.order, s.summaries)).toBe(true);
    expect(idx.hosts.get('api.github.com')).toBe(3);
    // A deletion (store lifecycle) recounts.
    s.order.splice(0, 9);
    idx.invalidate();
    idx.sync(s.order, s.summaries);
    expect(idx.hosts.get('api.github.com')).toBe(2);
    // Older pages of a paged hello arrive in front: counted without a recount.
    const older = [summary(-2, 'old.example.com'), summary(-1, 'old.example.com')];
    for (const r of older) s.summaries.set(r.id, r);
    s.order.unshift(-2, -1);
    expect(idx.sync(s.order, s.summaries)).toBe(true);
    expect(idx.hosts.get('old.example.com')).toBe(2);
    // An id inserted inside the counted run (imports) is detected without a lifecycle event.
    const middle = summary(13.5, 'late.example.com');
    s.summaries.set(13.5, middle);
    s.order.splice(s.order.indexOf(14), 0, 13.5);
    idx.sync(s.order, s.summaries);
    expect(idx.hosts.get('late.example.com')).toBe(1);
    expect(idx.hosts.get('old.example.com')).toBe(2);
    expect([...idx.hosts.values()].reduce((a, b) => a + b, 0)).toBe(s.order.length);
  });

  test('stays fast at 100k exchanges', () => {
    const idx = new HostIndex();
    const rows = synth(100_000);
    const s = store(rows);
    let t = performance.now();
    idx.sync(s.order, s.summaries);
    const full = performance.now() - t;
    // Live traffic: 200 batches of 50 new exchanges.
    t = performance.now();
    for (let b = 0; b < 200; b++) {
      const batch = synth(50, 100_000 + b * 50);
      for (const r of batch) s.summaries.set(r.id, r);
      s.order.push(...batch.map((r) => r.id));
      idx.sync(s.order, s.summaries);
    }
    const perBatch = (performance.now() - t) / 200;
    t = performance.now();
    const built = buildRows(idx, { view: 'hosts', pinned: ['localhost'], expanded: new Set(['github.com']), sort: 'count' });
    const rowsMs = performance.now() - t;
    console.info(`[sidebar] 100k full count ${full.toFixed(1)} ms · 50-exchange batch ${perBatch.toFixed(3)} ms · rows ${rowsMs.toFixed(2)} ms`);
    expect(idx.hosts.get('localhost')).toBe(Array.from({ length: 110_000 }, (_, i) => i).filter((i) => HOSTS[i % HOSTS.length] === 'localhost').length);
    expect(built.length).toBeGreaterThan(5);
    expect(full).toBeLessThan(400);
    expect(perBatch).toBeLessThan(2);
  });
});

describe('sidebar rows', () => {
  const idx = new HostIndex();
  const s = store(synth(90));
  idx.sync(s.order, s.summaries);
  const labels = (rows: SidebarRow[]) => rows.map((r) => (r.kind === 'heading' ? `# ${r.label}` : r.kind === 'domain' ? `${r.expanded ? '-' : '+'} ${r.domain} (${r.hosts})` : r.kind === 'host' ? `${'  '.repeat(r.depth)}${r.host}${r.pinned ? ' *' : ''}` : r.kind === 'source' ? `@${r.source}` : r.label));

  test('pinned first, then domains; single-host domains are just the host', () => {
    const rows = buildRows(idx, { view: 'hosts', pinned: ['localhost', 'gone.example'], expanded: new Set(['github.com']), sort: 'name' });
    expect(labels(rows)).toEqual([
      '# Pinned',
      'localhost *',
      'gone.example *',
      '# Domains',
      '10.0.0.7',
      'akev.github.io',
      '+ example.co.uk (2)',
      '- github.com (2)',
      '  api.github.com',
      '  github.com',
      'www.google.com',
      'fonts.gstatic.com',
      'localhost *',
    ]);
    expect(rows.find((r) => r.kind === 'host' && r.host === 'gone.example')).toMatchObject({ count: 0 });
  });

  test('search opens matching domains; apps view; empty states', () => {
    expect(labels(buildRows(idx, { view: 'hosts', pinned: [], expanded: new Set(), sort: 'name', query: 'shop' }))).toEqual(['# Domains', 'shop.example.co.uk']);
    expect(labels(buildRows(idx, { view: 'hosts', pinned: [], expanded: new Set(), sort: 'name', query: 'co.uk' }))).toEqual(['# Domains', '- example.co.uk (2)', '  example.co.uk', '  shop.example.co.uk']);
    expect(labels(buildRows(idx, { view: 'hosts', pinned: [], expanded: new Set(), sort: 'name', query: 'zzz' }))).toEqual(['# Domains', 'No host matches']);
    expect(labels(buildRows(idx, { view: 'apps', pinned: [], expanded: new Set(), sort: 'count' }))).toEqual(['# Apps', '@Chrome', '@curl', '@', '@node'].map((l) => l));
    expect(labels(buildRows(new HostIndex(), { view: 'apps', pinned: [], expanded: new Set(), sort: 'name' }))).toEqual(['# Apps', 'No traffic yet']);
  });
});

describe('sidebar clauses', () => {
  const matches = (query: string, s: ExchangeSummary) => {
    const c = compileFilter(query);
    if (!c.ok) throw new Error(c.error.message);
    return c.predicate ? c.predicate({ summary: s, url: `https://${s.host}/`, requestHeaders: [], responseHeaders: [] }) : true;
  };

  test('host, domain and app clauses select what the row counts', () => {
    expect(hostClause('api.github.com')).toBe('host=api.github.com');
    expect(domainClause('github.com')).toBe('(host=github.com | host:*.github.com)');
    expect(sourceClause('Google Chrome')).toBe('source="Google Chrome"');
    expect(sourceClause('')).toBe('-source:*');
    const d = domainClause('github.com');
    expect(matches(d, summary(1, 'github.com'))).toBe(true);
    expect(matches(d, summary(1, 'api.github.com'))).toBe(true);
    expect(matches(d, summary(1, 'notgithub.com'))).toBe(false);
    expect(matches(sourceClause(''), summary(1, 'x'))).toBe(true);
    expect(matches(sourceClause(''), summary(1, 'x', 'curl'))).toBe(false);
    expect(matches(sourceClause('curl'), summary(1, 'x', 'curl'))).toBe(true);
  });

  test('composing with the typed filter and taking the clause back out', () => {
    const d = domainClause('github.com');
    const withOr = appendClause('status:4xx | is:error', d);
    expect(withOr).toBe(`(status:4xx | is:error) ${d}`);
    expect(hasClause(withOr, d)).toBe(true);
    expect(removeClause(withOr, d)).toBe('status:4xx | is:error');
    const plain = appendClause('method:post', 'host=a.com');
    expect(removeClause(plain, 'host=a.com')).toBe('method:post');
    // Only whole terms count.
    expect(hasClause('host=a.com.evil', 'host=a.com')).toBe(false);
    expect(removeClause('host=a.com.evil', 'host=a.com')).toBe('host=a.com.evil');
    expect(removeClause('host=a.com', 'host=a.com')).toBe('');
  });
});
