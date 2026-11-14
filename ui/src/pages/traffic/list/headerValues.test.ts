import { describe, expect, test } from 'bun:test';
import type { ExchangeId, HeaderValues } from '../../../api/types';
import { columnDefOf, headerColumnId, headerDisplayName, isHeaderColumn, MAX_HEADER_COLUMNS, sanitizePrefs, useUiStore } from '../../../state/ui';
import { columnLabel, fitColumns, isSortable } from './columns';
import { HeaderValueCache } from './headerValues';

interface Call {
  ids: ExchangeId[];
  names: { request: string[]; response: string[] };
}

function harness(respond: (ids: ExchangeId[]) => HeaderValues[] | Error = (ids) => ids.map((id) => ({ id, request: { 'x-request-id': `r${id}` }, response: id % 2 ? { 'cache-control': 'no-store' } : undefined }))) {
  const calls: Call[] = [];
  const timers: (() => void)[] = [];
  const cache = new HeaderValueCache(
    async (ids, names) => {
      calls.push({ ids, names });
      const r = respond(ids);
      if (r instanceof Error) throw r;
      return { exchanges: r };
    },
    (fn) => {
      timers.push(fn);
      return 0 as unknown as ReturnType<typeof setTimeout>;
    },
  );
  const tick = async () => {
    const pending = timers.splice(0);
    for (const t of pending) t();
    await new Promise((r) => setTimeout(r, 0));
  };
  return { cache, calls, tick, timers };
}

describe('header column ids', () => {
  test('ids, labels and definitions', () => {
    const id = headerColumnId('response', ' Cache-Control ');
    expect(id).toBe('res:cache-control');
    expect(isHeaderColumn(id)).toBe(true);
    expect(isHeaderColumn('res:bad name')).toBe(false);
    expect(isHeaderColumn('status')).toBe(false);
    expect(headerDisplayName('x-request-id')).toBe('X-Request-Id');
    expect(columnDefOf(id)).toMatchObject({ label: 'Cache-Control', defaultVisible: true });
    expect(columnLabel(id)).toBe('Cache-Control (response header)');
    expect(isSortable(id)).toBe(false);
  });

  test('prefs keep valid header columns (shown, at most a few)', () => {
    const many = Array.from({ length: MAX_HEADER_COLUMNS + 2 }, (_, i) => ({ id: `req:x-h${i}`, visible: true, width: 150 }));
    const cols = sanitizePrefs({ columns: [{ id: 'res:x-cache', visible: true, width: 20 }, { id: 'req:hidden', visible: false, width: 100 }, { id: 'res:bad name', visible: true, width: 100 }, ...many] }).columns;
    const header = cols.filter((c) => isHeaderColumn(c.id));
    expect(header).toHaveLength(MAX_HEADER_COLUMNS);
    expect(header[0]).toEqual({ id: 'res:x-cache', visible: true, width: 60 });
    expect(cols.some((c) => c.id === 'req:hidden')).toBe(false);
  });

  test('adding and removing through the store', () => {
    const ui = useUiStore.getState();
    ui.resetColumns();
    expect(ui.addHeaderColumn('request', 'X-Request-Id')).toBe(true);
    expect(useUiStore.getState().columns.at(-1)).toEqual({ id: 'req:x-request-id', visible: true, width: 140 });
    expect(useUiStore.getState().addHeaderColumn('request', 'bad name')).toBe(false);
    useUiStore.getState().setColumnVisible('req:x-request-id', false);
    expect(useUiStore.getState().columns.some((c) => isHeaderColumn(c.id))).toBe(false);
    for (let i = 0; i < MAX_HEADER_COLUMNS; i++) useUiStore.getState().addHeaderColumn('response', `x-${i}`);
    expect(useUiStore.getState().addHeaderColumn('response', 'one-too-many')).toBe(false);
    useUiStore.getState().resetColumns();
  });

  test('in a narrow list, header columns go after the minor built-in ones', () => {
    const cols = [
      { id: 'method' as const, width: 64 },
      { id: 'status' as const, width: 72 },
      { id: 'host' as const, width: 200 },
      { id: 'path' as const, width: 240 },
      { id: 'res:x-cache' as const, width: 140 },
      { id: 'source' as const, width: 120 },
    ];
    expect(fitColumns(cols, 560).map((c) => c.id)).toEqual(['method', 'status', 'host', 'path', 'res:x-cache']);
    expect(fitColumns(cols, 420).map((c) => c.id)).toEqual(['method', 'status', 'host', 'path']);
  });
});

describe('HeaderValueCache', () => {
  test('batches the rows on screen, asks only for the shown headers, caches', async () => {
    const { cache, calls, tick } = harness();
    cache.ensure(1, true); // no columns yet: nothing to fetch
    cache.setColumns(['status', 'req:x-request-id', 'res:cache-control']);
    for (const id of [1, 2, 3]) cache.ensure(id, true);
    cache.ensure(2, true);
    expect(cache.get(1, 'req:x-request-id')).toBeUndefined();
    await tick();
    expect(calls).toEqual([{ ids: [1, 2, 3], names: { request: ['x-request-id'], response: ['cache-control'] } }]);
    expect(cache.get(1, 'req:x-request-id')).toBe('r1');
    expect(cache.get(1, 'res:cache-control')).toBe('no-store');
    expect(cache.get(2, 'res:cache-control')).toBeNull();
    cache.ensure(1, true);
    await tick();
    expect(calls).toHaveLength(1);
  });

  test("rows fetched before their response are fetched again once it's there", async () => {
    const { cache, calls, tick } = harness();
    cache.setColumns(['res:cache-control']);
    cache.ensure(5, false);
    await tick();
    cache.ensure(5, false);
    await tick();
    expect(calls).toHaveLength(1);
    cache.ensure(5, true);
    await tick();
    expect(calls.map((c) => c.ids)).toEqual([[5], [5]]);
  });

  test('unknown ids stop loading; column changes, removals and resets drop values', async () => {
    const { cache, calls, tick } = harness((ids) => ids.filter((id) => id !== 9).map((id) => ({ id, request: { a: 'x' } })));
    cache.setColumns(['req:a']);
    cache.ensure(8, true);
    cache.ensure(9, true);
    await tick();
    expect(cache.get(9, 'req:a')).toBeNull();
    cache.forget([8]);
    expect(cache.get(8, 'req:a')).toBeUndefined();
    cache.ensure(8, true);
    await tick();
    cache.setColumns(['req:a', 'req:b']);
    expect(cache.get(8, 'req:a')).toBeUndefined();
    cache.setColumns(['req:a', 'req:b']); // same set: kept
    cache.ensure(8, true);
    await tick();
    expect(calls.at(-1)?.names).toEqual({ request: ['a', 'b'], response: [] });
    cache.reset();
    expect(cache.get(8, 'req:a')).toBeUndefined();
  });

  test('failed requests are retried later', async () => {
    let fail = true;
    const { cache, calls, tick, timers } = harness((ids) => (fail ? new Error('offline') : ids.map((id) => ({ id }))));
    cache.setColumns(['req:a']);
    cache.ensure(1, true);
    await tick();
    expect(calls).toHaveLength(1);
    expect(timers).toHaveLength(1);
    fail = false;
    await tick();
    expect(calls).toHaveLength(2);
    expect(cache.get(1, 'req:a')).toBeNull();
  });

  test('big batches are split', async () => {
    const { cache, calls, tick } = harness();
    cache.setColumns(['req:x-request-id']);
    for (let id = 0; id < 1200; id++) cache.ensure(id, true);
    await tick();
    await new Promise((r) => setTimeout(r, 0));
    expect(calls.map((c) => c.ids.length)).toEqual([500, 500, 200]);
  });
});
