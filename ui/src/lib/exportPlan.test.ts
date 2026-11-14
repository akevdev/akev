import { afterEach, describe, expect, test } from 'bun:test';
import { ApiClient, api, browserTransport, type Transport } from '../api/client';
import { exportHarSelection, saveSessionSelection } from '../pages/traffic/actions/exportSelection';
import { idFilter, idRanges, MAX_EXPORT_URL, mergeHar, planSelectionExport, type SelectionQuery } from './exportPlan';
import { FILE_NAME_HEADER, resetTauriBridgeForTests } from './platform';

const client = new ApiClient({ baseUrl: 'http://127.0.0.1:8100', token: 'a'.repeat(43) });
const harUrl = (q: SelectionQuery) => client.transfer.harUrl(q);

/** Expand `id:` range filters back into ids. */
function idsOf(q: SelectionQuery): number[] {
  if ('ids' in q) return q.ids;
  const out: number[] = [];
  for (const r of q.filter.replace(/^id:/, '').split(',')) {
    const [a, b] = r.split('-').map(Number) as [number, number | undefined];
    for (let i = a; i <= (b ?? a); i++) out.push(i);
  }
  return out;
}

const range = (from: number, to: number, step = 1) => Array.from({ length: Math.floor((to - from) / step) + 1 }, (_, i) => from + i * step);

describe('export planning for large selections', () => {
  test('id ranges', () => {
    expect(idRanges([8, 1, 2, 3, 5, 7, 3])).toEqual(['1-3', '5', '7-8']);
    expect(idFilter(idRanges([4]))).toBe('id:4');
    expect(idRanges([])).toEqual([]);
  });

  test('small selections keep the plain id list', () => {
    expect(planSelectionExport([3, 1, 2], harUrl)).toEqual([{ ids: [1, 2, 3] }]);
    expect(planSelectionExport([], harUrl)).toEqual([]);
  });

  test('Ctrl+A over ~15k rows: one short request instead of a 414 (regression)', () => {
    const ids = range(1, 15_000).filter((id) => id % 1000 !== 0); // a few deleted rows
    // Before: every id in the query string → far beyond the daemon's 65 534-byte URI limit.
    expect(harUrl({ ids }).length).toBeGreaterThan(65_534);
    const plan = planSelectionExport(ids, harUrl);
    expect(plan.length).toBe(1);
    expect(harUrl(plan[0] as SelectionQuery).length).toBeLessThanOrEqual(MAX_EXPORT_URL);
    expect(idsOf(plan[0] as SelectionQuery)).toEqual(ids);
  });

  test('scattered selections are split into chunks that each fit and together are exact', () => {
    const ids = range(1, 60_000, 2); // every other row (e.g. filtered): ranges don't compress
    const plan = planSelectionExport(ids, harUrl);
    expect(plan.length).toBeGreaterThan(1);
    for (const q of plan) expect(harUrl(q).length).toBeLessThanOrEqual(MAX_EXPORT_URL);
    expect(plan.flatMap(idsOf)).toEqual(ids);
    // The length estimate is tight: chunks are well filled.
    expect(harUrl(plan[0] as SelectionQuery).length).toBeGreaterThan(MAX_EXPORT_URL - 20);
  });

  test('HAR chunks merge into one log, keeping entry order and exact numbers', () => {
    const a = JSON.stringify({ log: { version: '1.2', creator: { name: 'Akev' }, pages: [{ id: 'p1' }], entries: [{ n: 1 }, { n: 2 }] } });
    const b = '{"log":{"version":"1.2","creator":{"name":"Akev"},"pages":[{"id":"p1"},{"id":"p2"}],"entries":[{"n":3,"_akev":{"big":18446744073709551615}}]}}';
    const merged = mergeHar([a, b]);
    expect(merged).toContain('"big": 18446744073709551615');
    const parsed = JSON.parse(merged) as { log: { entries: { n: number }[]; pages: { id: string }[] } };
    expect(parsed.log.entries.map((e) => e.n)).toEqual([1, 2, 3]);
    expect(parsed.log.pages.map((p) => p.id)).toEqual(['p1', 'p2']);
    expect(mergeHar([a])).toBe(a);
    expect(() => mergeHar([a, '{"error":"x"}'])).toThrow();
  });
});

describe('exportHarSelection / saveSessionSelection', () => {
  const g = globalThis as { window?: unknown };
  const hadWindow = 'window' in g;
  const realWindow = g.window;
  const realFetch = globalThis.fetch;
  afterEach(() => {
    api.setTransport(browserTransport);
    if (!hadWindow) delete g.window;
    else g.window = realWindow;
    globalThis.fetch = realFetch;
    resetTauriBridgeForTests();
  });

  /** The desktop app: saves go to the shell's `save_file` command (URLs are fetched first). */
  function desktop() {
    const saved: { suggestedName: string; url?: string; data?: string }[] = [];
    let fetched: string | undefined;
    globalThis.fetch = (async (url: string | URL | Request) => {
      fetched = String(url);
      return new Response('downloaded');
    }) as typeof fetch;
    const invoke = async (cmd: string, args: unknown, options?: { headers?: Record<string, string> }) => {
      if (cmd !== 'save_file') return undefined;
      const suggestedName = decodeURIComponent(options?.headers?.[FILE_NAME_HEADER] ?? '');
      saved.push(fetched != null ? { suggestedName, url: fetched } : { suggestedName, data: new TextDecoder().decode(args as Uint8Array) });
      fetched = undefined;
      return true;
    };
    g.window = { __AKEV__: { desktop: true }, __TAURI_INTERNALS__: { invoke } };
    resetTauriBridgeForTests();
    return saved;
  }

  test('a huge scattered selection is fetched in chunks and saved as one merged HAR', async () => {
    const saved = desktop();
    const requested: string[] = [];
    const transport: Transport = {
      fetch: async (url) => {
        requested.push(url);
        const filter = new URL(url, 'http://x').searchParams.get('filter') ?? '';
        const ids = idsOf({ filter });
        return new Response(JSON.stringify({ log: { version: '1.2', entries: ids.map((id) => ({ id })) } }), { status: 200 });
      },
      openSocket: () => {
        throw new Error('unused');
      },
    };
    api.setTransport(transport);
    const ids = range(1, 40_000, 2);
    await exportHarSelection(ids, 'x.har');
    expect(requested.length).toBeGreaterThan(1);
    for (const u of requested) expect(u.length).toBeLessThanOrEqual(MAX_EXPORT_URL + 100);
    expect(saved.length).toBe(1);
    const har = JSON.parse(saved[0]?.data as string) as { log: { entries: { id: number }[] } };
    expect(har.log.entries.map((e) => e.id)).toEqual(ids);
  });

  test('a selection that fits is a single download link', async () => {
    const saved = desktop();
    await exportHarSelection([5, 6, 7], 'y.har');
    // HAR exports redact secrets by default (`lib/exportRedaction`).
    expect(saved.map((s) => s.url?.replace(/^.*\/api/, '/api'))).toEqual(['/api/export/har?ids=5%2C6%2C7&redact=1']);
    await saveSessionSelection(range(1, 20_000), 'z.akev');
    expect(saved[1]?.url).toContain('filter=id%3A1-20000');
    expect(saved[1]?.url).not.toContain('redact');
    // Sessions can't be merged: an unsplittable selection is refused rather than sent as a 414.
    await saveSessionSelection(range(1, 60_000, 2), 'w.akev');
    expect(saved.length).toBe(2);
  });
});
