import { describe, expect, test } from 'bun:test';
import type { ExchangeSummary } from '../../api/types';
import { defaultColumns, type ColumnConfig, type TrafficColumnId } from '../../state/ui';
import { filterHidden, isHidden } from './actions/hiddenRows';
import { appendClause, applySuggestion, clause, historyStep, pathOnly, quoteFilterValue, suggestionContinues } from './filter/filterEdit';
import { hasTopLevelOr, tokenizeFilter, type FilterTokenKind } from './filter/filterTokens';
import { ariaSort, dropTarget, fitColumns, flexColumn, gridTemplate, isNaturalOrder, isNewestFirst, moveColumnTo, nextSort, totalWidth, visibleColumns } from './list/columns';
import { countNewer } from './list/liveTail';
import { IncrementalSort, sortIds, sortKey } from './list/sortIds';
import { canvasHeight, firstVisibleRow, rowWindow, rowsInView, slotPool, windowCovers } from './list/virtual';

const kinds = (q: string) => tokenizeFilter(q).map((t) => [t.kind, t.text] as [FilterTokenKind, string]);

describe('tokenizeFilter', () => {
  test('field / operator / value', () => {
    expect(kinds('status:>=500')).toEqual([
      ['field', 'status'],
      ['operator', ':'],
      ['value', '>=500'],
    ]);
    expect(kinds('method=get,post')).toEqual([
      ['field', 'method'],
      ['operator', '='],
      ['value', 'get'],
      ['punct', ','],
      ['value', 'post'],
    ]);
  });

  test('negation, keywords, parens, free text and strings', () => {
    expect(kinds('-is:ws OR (host:"a b" api)')).toEqual([
      ['negation', '-'],
      ['field', 'is'],
      ['operator', ':'],
      ['value', 'ws'],
      ['space', ' '],
      ['keyword', 'OR'],
      ['space', ' '],
      ['paren', '('],
      ['field', 'host'],
      ['operator', ':'],
      ['string', '"a b"'],
      ['space', ' '],
      ['text', 'api'],
      ['paren', ')'],
    ]);
  });

  test('regex values and dotted header fields', () => {
    expect(kinds('header.x-trace~/ab+c/i')).toEqual([
      ['field', 'header.x-trace'],
      ['operator', '~'],
      ['regex', '/ab+c/i'],
    ]);
  });

  test('covers the input exactly, even when unterminated', () => {
    for (const q of ['', '   ', 'host:"open', 'a  ||  b && !c', '(((', 'ünïcödé:✓ 😀', 'status!=200']) {
      const toks = tokenizeFilter(q);
      expect(toks.map((t) => t.text).join('')).toBe(q);
      let pos = 0;
      for (const t of toks) {
        expect(t.start).toBe(pos);
        pos = t.end;
      }
      expect(pos).toBe(q.length);
    }
  });

  test('a lone dash is free text, != is an operator', () => {
    expect(kinds('- x')[0]).toEqual(['text', '-']);
    expect(kinds('status!=200')).toEqual([
      ['field', 'status'],
      ['operator', '!='],
      ['value', '200'],
    ]);
  });

  test('hasTopLevelOr ignores nested ORs', () => {
    expect(hasTopLevelOr('a OR b')).toBe(true);
    expect(hasTopLevelOr('a | b')).toBe(true);
    expect(hasTopLevelOr('(a OR b) c')).toBe(false);
    expect(hasTopLevelOr('host:"x OR y"')).toBe(false);
  });
});

describe('filterEdit', () => {
  test('applySuggestion replaces a UTF-8 byte range', () => {
    // "é" is 2 bytes: "é st" → replace "st" (bytes 3..5) with "status:"
    const r = applySuggestion('é st', { insert: 'status:', replaceStart: 3, replaceEnd: 5, kind: 'field' });
    expect(r).toEqual({ text: 'é status:', caret: 9 });
  });

  test('completed values get a trailing space, unless one follows', () => {
    expect(applySuggestion('method:po', { insert: 'post', replaceStart: 7, replaceEnd: 9, kind: 'value' })).toEqual({ text: 'method:post ', caret: 12 });
    expect(applySuggestion('method:po host:x', { insert: 'post', replaceStart: 7, replaceEnd: 9, kind: 'value' })).toEqual({ text: 'method:post host:x', caret: 11 });
  });

  test('suggestionContinues for fields/operators', () => {
    expect(suggestionContinues('status:', 'field')).toBe(true);
    expect(suggestionContinues('>=', 'operator')).toBe(true);
    expect(suggestionContinues('404', 'value')).toBe(false);
  });

  test('quoting', () => {
    expect(quoteFilterValue('api.example.com')).toBe('api.example.com');
    expect(quoteFilterValue('My App')).toBe('"My App"');
    expect(quoteFilterValue('a"b')).toBe('"a\\"b"');
    expect(quoteFilterValue('-x')).toBe('"-x"');
    expect(quoteFilterValue('')).toBe('""');
    // `*` is a wildcard when bare; leading operator characters would fuse with the operator.
    expect(quoteFilterValue('*/*')).toBe('"*/*"');
    expect(quoteFilterValue('=x')).toBe('"=x"');
    expect(quoteFilterValue('~x')).toBe('"~x"');
    expect(clause('path', '=', '/api/*')).toBe('path="/api/*"');
    expect(clause('host', '=', 'a.com', true)).toBe('-host=a.com');
  });

  test('appendClause', () => {
    expect(appendClause('', 'host=a.com')).toBe('host=a.com');
    expect(appendClause('  status:4xx ', 'host=a.com')).toBe('status:4xx host=a.com');
    expect(appendClause('a OR b', 'host=a.com')).toBe('(a OR b) host=a.com');
    expect(appendClause('status:4xx host=a.com', 'host=a.com')).toBe('status:4xx host=a.com');
    expect(appendClause('-host=a.com', 'host=a.com')).toBe('-host=a.com host=a.com');
  });

  test('pathOnly / historyStep', () => {
    expect(pathOnly('/a/b?x=1')).toBe('/a/b');
    expect(pathOnly('/a')).toBe('/a');
    const recent = ['one', 'two'];
    expect(historyStep(recent, -1, 1)).toEqual({ index: 0, text: 'one' });
    expect(historyStep(recent, 1, 1)).toEqual({ index: 1, text: 'two' });
    expect(historyStep(recent, 0, -1)).toEqual({ index: -1, text: null });
    expect(historyStep([], -1, 1)).toEqual({ index: -1, text: null });
  });
});

describe('columns', () => {
  const cols = defaultColumns();

  test('visible columns and grid template', () => {
    const vis = visibleColumns(cols);
    expect(vis.map((c) => c.id)).toEqual(['indicator', 'method', 'status', 'host', 'path', 'type', 'size', 'time', 'source']);
    expect(flexColumn(vis)).toBe('path');
    expect(gridTemplate(vis)).toBe('6px 64px 72px 200px minmax(240px, 1fr) 76px 76px 76px 120px');
    expect(totalWidth(vis)).toBe(6 + 64 + 72 + 200 + 240 + 76 + 76 + 76 + 120);
  });

  test('host flexes when path is hidden; filler when both are', () => {
    const noPath = visibleColumns(cols.map((c) => (c.id === 'path' ? { ...c, visible: false } : c)));
    expect(flexColumn(noPath)).toBe('host');
    const none = visibleColumns(cols.map((c) => (c.id === 'path' || c.id === 'host' ? { ...c, visible: false } : c)));
    expect(gridTemplate(none).endsWith('minmax(0, 1fr)')).toBe(true);
  });

  test('sort cycle', () => {
    expect(nextSort(null, 'size')).toEqual({ column: 'size', direction: 'asc' });
    expect(nextSort({ column: 'size', direction: 'asc' }, 'size')).toEqual({ column: 'size', direction: 'desc' });
    expect(nextSort({ column: 'size', direction: 'desc' }, 'size')).toBeNull();
    expect(nextSort({ column: 'size', direction: 'desc' }, 'host')).toEqual({ column: 'host', direction: 'asc' });
    expect(nextSort(null, 'indicator')).toBeNull();
    expect(ariaSort({ column: 'size', direction: 'desc' }, 'size')).toBe('descending');
    expect(ariaSort({ column: 'size', direction: 'desc' }, 'host')).toBe('none');
    expect(isNaturalOrder(null)).toBe(true);
    expect(isNaturalOrder({ column: 'started', direction: 'asc' })).toBe(true);
    expect(isNaturalOrder({ column: 'id', direction: 'desc' })).toBe(false);
    expect(isNewestFirst({ column: 'id', direction: 'desc' })).toBe(true);
  });

  test('moveColumnTo keeps hidden columns and the indicator in place', () => {
    const ids = (c: readonly ColumnConfig[]) => c.map((x) => x.id);
    const moved = moveColumnTo(cols, 'source', 'method', 'before');
    expect(ids(moved).slice(0, 4)).toEqual(['indicator', 'id', 'source', 'method']);
    expect(moved).toHaveLength(cols.length);
    const after = moveColumnTo(cols, 'method', 'time', 'after');
    expect(ids(after).indexOf('method')).toBe(ids(after).indexOf('time') + 1);
    expect(moveColumnTo(cols, 'indicator', 'host', 'after')).toBe(cols);
    expect(moveColumnTo(cols, 'host', 'host', 'after')).toBe(cols);
    expect(ids(moveColumnTo(cols, 'host', 'indicator', 'before'))[0]).toBe('indicator');
    // A no-op move returns the same array.
    expect(moveColumnTo(cols, 'status', 'host', 'before')).toBe(cols);
  });

  test('fitColumns shrinks path, then untouched host/source, never dragged widths', () => {
    const vis = visibleColumns(cols); // 930px with the 6px indicator
    expect(fitColumns(vis, 2000)).toBe(vis);
    const fit = fitColumns(vis, 800);
    const w = (id: string, c = fit) => c.find((x) => x.id === id)!.width;
    expect(totalWidth(fit)).toBe(800);
    expect(w('path')).toBe(200);
    expect(w('host')).toBe(150);
    expect(w('source')).toBe(100);
    expect(w('size')).toBe(68);
    expect(w('type')).toBe(72);
    // At the floors (666px) everything still fits.
    const floors = fitColumns(vis, 666);
    expect(floors.map((c) => c.id)).toEqual(vis.map((c) => c.id));
    expect(w('path', floors)).toBe(120);
    expect(w('host', floors)).toBe(112);
    expect(w('source', floors)).toBe(88);
    expect(w('time', floors)).toBe(68);
    // A host the user resized is respected: the source column makes room instead.
    const custom = vis.map((c) => (c.id === 'host' ? { ...c, width: 260 } : c));
    const fitCustom = fitColumns(custom, 800);
    expect(w('host', fitCustom)).toBe(260);
    expect(fitCustom.some((c) => c.id === 'source')).toBe(false);
    expect(w('path', fitCustom)).toBe(194);
    expect(totalWidth(fitCustom)).toBe(800);
  });

  test('fitColumns leaves out the least important columns instead of scrolling', () => {
    const vis = visibleColumns(cols);
    const ids = (c: readonly { id: string }[]) => c.map((x) => x.id);
    const w = (id: string, c: readonly { id: string; width: number }[]) => c.find((x) => x.id === id)!.width;
    // Just below the floors: only the source column goes, and the rest get its room back.
    const narrow = fitColumns(vis, 640);
    expect(ids(narrow)).toEqual(['indicator', 'method', 'status', 'host', 'path', 'type', 'size', 'time']);
    expect(totalWidth(narrow)).toBe(640);
    expect(w('host', narrow)).toBe(144);
    expect(w('path', narrow)).toBe(150);
    // Narrower: type, size and time go too; method, status, host and path stay.
    const tight = fitColumns(vis, 400);
    expect(ids(tight)).toEqual(['indicator', 'method', 'status', 'host', 'path']);
    expect(totalWidth(tight)).toBe(400);
    // Too narrow even for those: the essential columns at their floors, and the list scrolls.
    const tiny = fitColumns(vis, 300);
    expect(ids(tiny)).toEqual(['indicator', 'method', 'status', 'host', 'path']);
    expect(totalWidth(tiny)).toBe(374);
    // Optional columns the user turned on go before type, size and time.
    const extra = cols.map((c) => (c.id === 'started' || c.id === 'protocol' ? { ...c, visible: true } : c));
    const withExtra = fitColumns(visibleColumns(extra), 640);
    expect(ids(withExtra)).toEqual(['indicator', 'method', 'status', 'host', 'path', 'type', 'size', 'time']);
  });

  test('dropTarget picks the half of the column under the pointer', () => {
    const rects = [
      { id: 'indicator' as const, left: 0, width: 6 },
      { id: 'method' as const, left: 6, width: 64 },
      { id: 'status' as const, left: 70, width: 72 },
    ];
    expect(dropTarget(rects, 10)).toEqual({ id: 'method', place: 'before' });
    expect(dropTarget(rects, 60)).toEqual({ id: 'method', place: 'after' });
    expect(dropTarget(rects, 500)).toEqual({ id: 'status', place: 'after' });
    expect(dropTarget(rects, 2)).toEqual({ id: 'method', place: 'before' });
  });
});

function summary(id: number, over: Partial<ExchangeSummary> = {}): ExchangeSummary {
  return {
    id,
    kind: 'http',
    state: 'complete',
    startedAt: 1_000 + id,
    method: 'GET',
    scheme: 'https',
    host: 'a.com',
    port: 443,
    path: '/',
    httpVersion: 'HTTP/2',
    contentKind: 'json',
    requestSize: 0,
    responseSize: 0,
    ...over,
  };
}

describe('sortIds', () => {
  const rows = [
    summary(1, { status: 404, responseSize: 30, duration: 50, host: 'b.com', source: 'curl' }),
    summary(2, { status: undefined, state: 'waiting', responseSize: 10, host: 'A.com' }),
    summary(3, { status: 200, responseSize: 30, duration: 10, host: 'c.com', source: 'Chrome' }),
    summary(4, { status: undefined, error: 'timeout', responseSize: 5, duration: 900, host: 'a.com' }),
  ];
  const map = new Map(rows.map((r) => [r.id, r]));
  const ids = [1, 2, 3, 4];

  test('natural order is returned as-is, descending reversed', () => {
    expect(sortIds(ids, map, null)).toBe(ids);
    expect(sortIds(ids, map, { column: 'id', direction: 'asc' })).toBe(ids);
    expect(sortIds(ids, map, { column: 'started', direction: 'desc' })).toEqual([4, 3, 2, 1]);
  });

  test('numeric keys with missing values last in both directions', () => {
    expect(sortIds(ids, map, { column: 'status', direction: 'asc' })).toEqual([3, 1, 4, 2]);
    expect(sortIds(ids, map, { column: 'status', direction: 'desc' })).toEqual([4, 1, 3, 2]);
    expect(sortIds(ids, map, { column: 'time', direction: 'asc' })).toEqual([3, 1, 4, 2]);
    expect(sortIds(ids, map, { column: 'time', direction: 'desc' })).toEqual([4, 1, 3, 2]);
  });

  test('ties keep id order (stable) in both directions', () => {
    expect(sortIds(ids, map, { column: 'size', direction: 'asc' })).toEqual([4, 2, 1, 3]);
    expect(sortIds(ids, map, { column: 'size', direction: 'desc' })).toEqual([1, 3, 2, 4]);
  });

  test('strings compare case-insensitively', () => {
    expect(sortIds(ids, map, { column: 'host', direction: 'asc' })).toEqual([2, 4, 1, 3]);
    expect(sortIds(ids, map, { column: 'source', direction: 'asc' })).toEqual([3, 1, 2, 4]);
  });

  test('rule titles resolve through the lookup', () => {
    const r = new Map([
      [1, summary(1, { rule: 'r-b' })],
      [2, summary(2, { rule: 'r-a' })],
      [3, summary(3)],
    ]);
    const titles: Record<string, string> = { 'r-a': 'Zeta', 'r-b': 'alpha' };
    expect(sortIds([1, 2, 3], r, { column: 'rule', direction: 'asc' }, (id) => titles[id])).toEqual([1, 2, 3]);
    expect(sortIds([1, 2, 3], r, { column: 'rule', direction: 'desc' }, (id) => titles[id])).toEqual([2, 1, 3]);
    expect(sortKey(summary(9), 'remoteIp')).toBeNull();
  });

  test('LLM calls sort by model (provider when unknown) and read as LLM in the type column', () => {
    const m = new Map([
      [1, summary(1, { llm: { provider: 'openai', model: 'gpt-4.1' } })],
      [2, summary(2)],
      [3, summary(3, { llm: { provider: 'anthropic', model: 'Claude-Sonnet-4-5' } })],
      [4, summary(4, { llm: { provider: 'azure-openai' } })],
    ]);
    expect(sortIds([1, 2, 3, 4], m, { column: 'model', direction: 'asc' })).toEqual([4, 3, 1, 2]);
    expect(sortIds([1, 2, 3, 4], m, { column: 'model', direction: 'desc' })).toEqual([1, 3, 4, 2]);
    expect(sortKey(m.get(3) as ExchangeSummary, 'model')).toBe('claude-sonnet-4-5');
    expect(sortKey(m.get(1) as ExchangeSummary, 'type')).toBe('LLM');
  });

  test('matches a reference comparator sort on random data (all columns, both directions)', () => {
    let seed = 7;
    const rnd = (n: number) => {
      seed = (seed * 1103515245 + 12345) >>> 0;
      return seed % n;
    };
    const many = new Map<number, ExchangeSummary>();
    const manyIds: number[] = [];
    for (let i = 0; i < 3000; i++) {
      const id = 10 + i * 3;
      many.set(
        id,
        summary(id, {
          status: rnd(5) === 0 ? undefined : [200, 204, 301, 404, 500][rnd(5)],
          error: rnd(9) === 0 ? 'timeout' : undefined,
          responseSize: rnd(4) === 0 ? 0 : rnd(5000),
          duration: rnd(6) === 0 ? undefined : rnd(400) + rnd(1000) / 1000,
          host: ['b.com', 'A.com', 'a.com', 'zeta.io', 'é.fr'][rnd(5)] as string,
          path: `/p/${rnd(40)}`,
          source: rnd(3) === 0 ? undefined : ['curl', 'Chrome', 'node'][rnd(3)],
          method: ['GET', 'post', 'PUT'][rnd(3)] as string,
        }),
      );
      manyIds.push(id);
    }
    // Shuffle the input so "stable" means input order, not id order.
    const input = manyIds.slice().sort(() => rnd(3) - 1);
    const reference = (column: TrafficColumnId, direction: 'asc' | 'desc') => {
      const dir = direction === 'asc' ? 1 : -1;
      const idx = input.map((_, i) => i);
      idx.sort((a, b) => {
        const ka = sortKey(many.get(input[a] as number)!, column);
        const kb = sortKey(many.get(input[b] as number)!, column);
        if (ka === kb) return a - b;
        if (ka === null) return 1;
        if (kb === null) return -1;
        const c = ka < kb ? -1 : ka > kb ? 1 : 0;
        return c === 0 ? a - b : c * dir;
      });
      return idx.map((i) => input[i] as number);
    };
    for (const column of ['status', 'size', 'time', 'host', 'path', 'source', 'method', 'type', 'protocol'] as TrafficColumnId[]) {
      for (const direction of ['asc', 'desc'] as const) {
        expect(sortIds(input, many, { column, direction })).toEqual(reference(column, direction));
      }
    }
  });

  test('100k rows sort quickly', () => {
    const big = new Map<number, ExchangeSummary>();
    const bigIds: number[] = [];
    for (let i = 0; i < 100_000; i++) {
      big.set(i, summary(i, { responseSize: (i * 7919) % 100_003, host: `h${(i * 31) % 997}.com` }));
      bigIds.push(i);
    }
    const t0 = performance.now();
    const sorted = sortIds(bigIds, big, { column: 'size', direction: 'desc' });
    const sortedHost = sortIds(bigIds, big, { column: 'host', direction: 'asc' });
    const elapsed = performance.now() - t0;
    expect(sorted).toHaveLength(100_000);
    expect(sortedHost).toHaveLength(100_000);
    expect(big.get(sorted[0] as number)!.responseSize).toBeGreaterThanOrEqual(big.get(sorted[1] as number)!.responseSize);
    expect(elapsed).toBeLessThan(1500);
  });
});

describe('live tail', () => {
  test('countNewer at the live edge of ascending and descending lists', () => {
    expect(countNewer([1, 2, 3, 5, 8], 3, false)).toBe(2);
    expect(countNewer([1, 2, 3], 3, false)).toBe(0);
    expect(countNewer([], 3, false)).toBe(0);
    expect(countNewer([8, 5, 3, 2, 1], 3, true)).toBe(2);
    expect(countNewer([8, 5], 1, true)).toBe(2);
    expect(countNewer([3, 2], 9, true)).toBe(0);
  });
});

describe('hidden rows', () => {
  const rows = new Map([
    [1, { id: 1, pinned: false }],
    [2, { id: 2, pinned: true }],
    [3, { id: 3, pinned: false }],
    [4, { id: 4, pinned: false }],
  ]);
  const get = (id: number) => rows.get(id);

  test('no entries keeps identity', () => {
    const list = [1, 2, 3, 4];
    expect(filterHidden(list, [], get)).toBe(list);
  });

  test('explicit ids and clear ranges (keeping pinned)', () => {
    const list = [1, 2, 3, 4];
    expect(filterHidden(list, [{ token: 1, ids: new Set([2, 4]) }], get)).toEqual([1, 3]);
    expect(filterHidden(list, [{ token: 1, upTo: 3, keepPinned: true }], get)).toEqual([2, 4]);
    expect(filterHidden(list, [{ token: 1, upTo: 3 }], get)).toEqual([4]);
    expect(isHidden([{ token: 1, upTo: 3 }], { id: 9, pinned: false })).toBe(false);
  });

  test('unchanged result keeps identity', () => {
    const list = [1, 3];
    expect(filterHidden(list, [{ token: 1, ids: new Set([9]) }], get)).toBe(list);
  });
});

describe('fixed-row virtualisation', () => {
  const H = 28; // header
  const R = 30; // row

  test('first visible row and rows in view', () => {
    expect(firstVisibleRow(0, R)).toBe(0);
    expect(firstVisibleRow(29, R)).toBe(0);
    expect(firstVisibleRow(30, R)).toBe(1);
    expect(firstVisibleRow(-50, R)).toBe(0);
    expect(rowsInView(28 + 300, R, H)).toBe(11);
    expect(rowsInView(0, R, H)).toBe(1);
  });

  test('window is clamped and overscanned on both sides', () => {
    expect(rowWindow(0, 328, R, H, 1000, 20)).toEqual({ start: 0, end: 31 });
    expect(rowWindow(500, 328, R, H, 1000, 20)).toEqual({ start: 480, end: 531 });
    expect(rowWindow(995, 328, R, H, 1000, 20)).toEqual({ start: 975, end: 1000 });
    // A stale first row beyond a shrunken list still shows its end.
    expect(rowWindow(5000, 328, R, H, 100, 20)).toEqual({ start: 79, end: 100 });
    expect(rowWindow(3, 328, R, H, 0, 20)).toEqual({ start: 0, end: 0 });
  });

  test('slot pool maps every rendered window to distinct keys', () => {
    const pool = slotPool(328, R, H, 20);
    for (const first of [0, 1, 7, 480, 993]) {
      const w = rowWindow(first, 328, R, H, 1000, 20);
      const keys = new Set<number>();
      for (let i = w.start; i < w.end; i++) keys.add(i % pool);
      expect(keys.size).toBe(w.end - w.start);
    }
  });

  test('coverage decides synchronous rendering', () => {
    const w = rowWindow(100, 328, R, H, 1000, 20);
    expect(windowCovers(w, 100, 328, R, H, 1000)).toBe(true);
    expect(windowCovers(w, 115, 328, R, H, 1000)).toBe(true);
    expect(windowCovers(w, 125, 328, R, H, 1000)).toBe(false);
    expect(windowCovers(w, 5000, 328, R, H, 1000)).toBe(false);
    expect(windowCovers({ start: 0, end: 0 }, 0, 328, R, H, 0)).toBe(true);
    expect(canvasHeight(100_000, R, H)).toBe(28 + 3_000_000);
  });
});

describe('IncrementalSort', () => {
  let seed = 99;
  const rnd = (n: number) => {
    seed = (seed * 1103515245 + 12345) >>> 0;
    return seed % n;
  };
  const make = (id: number) =>
    summary(id, {
      status: rnd(4) === 0 ? undefined : [200, 404, 500][rnd(3)],
      responseSize: rnd(50),
      duration: rnd(5) === 0 ? undefined : rnd(30),
      host: ['b.com', 'a.com', 'c.com'][rnd(3)] as string,
    });

  test('tracks additions, updates and removals exactly like a full sort', () => {
    const map = new Map<number, ExchangeSummary>();
    let visible: number[] = [];
    for (let id = 1; id <= 400; id++) {
      map.set(id, make(id));
      visible.push(id);
    }
    for (const column of ['size', 'time', 'status', 'host'] as TrafficColumnId[]) {
      for (const direction of ['asc', 'desc'] as const) {
        const sort = { column, direction };
        const inc = new IncrementalSort();
        expect(inc.update(visible, map, sort)).toEqual(sortIds(visible, map, sort));
        let nextId = 1000;
        for (let round = 0; round < 25; round++) {
          // new rows
          for (let k = rnd(4); k > 0; k--) {
            const id = nextId++;
            map.set(id, make(id));
            visible = [...visible, id];
          }
          // updates (new identity, maybe new key)
          for (let k = rnd(6); k > 0; k--) {
            const id = visible[rnd(visible.length)] as number;
            map.set(id, { ...make(id) });
          }
          // removals
          if (rnd(3) === 0) {
            const drop = visible[rnd(visible.length)] as number;
            visible = visible.filter((v) => v !== drop);
          }
          expect(inc.update(visible, map, sort)).toEqual(sortIds(visible, map, sort));
        }
      }
    }
  });

  test('returns the same array when nothing changed; context changes force a re-sort', () => {
    const map = new Map<number, ExchangeSummary>([1, 2, 3].map((id) => [id, make(id)]));
    const inc = new IncrementalSort();
    const sort = { column: 'size' as TrafficColumnId, direction: 'asc' as const };
    const a = inc.update([1, 2, 3], map, sort);
    expect(inc.update([1, 2, 3], map, sort)).toBe(a);
    const rules = {};
    const b = inc.update([1, 2, 3], map, sort, undefined, [rules]);
    expect(b).toEqual(a);
    expect(inc.update([1, 2, 3], map, { column: 'id', direction: 'desc' })).toEqual([3, 2, 1]);
  });
});
