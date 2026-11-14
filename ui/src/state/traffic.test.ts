import { describe, expect, test } from 'bun:test';
import type { ExchangeSummary } from '../api/types';
import { composeFilterQuery, indexInList, indexOfSorted, insertionIndex, mapFilterError, rangeBetween, shallowEqual, TrafficStore } from './traffic';

function summary(id: number, patch: Partial<ExchangeSummary> = {}): ExchangeSummary {
  return {
    id,
    kind: 'http',
    state: 'complete',
    startedAt: 1_700_000_000_000 + id,
    method: 'GET',
    scheme: 'https',
    host: 'example.com',
    port: 443,
    path: `/r/${id}`,
    httpVersion: 'HTTP/2',
    status: 200,
    contentKind: 'json',
    requestSize: 0,
    responseSize: 10,
    ...patch,
  };
}

/** A store whose notifications are flushed manually (`flush()`), with a filter sink spy. */
function setup() {
  let pending: (() => void) | null = null;
  const store = new TrafficStore((fn) => {
    pending = fn;
  }, 0);
  const sent: string[] = [];
  let seq = 0;
  store.setFilterSink((q) => {
    sent.push(q);
    return ++seq;
  });
  let notifications = 0;
  store.subscribe(() => notifications++);
  const flush = () => {
    const fn = pending;
    pending = null;
    fn?.();
  };
  return { store, sent, flush, notifications: () => notifications, hasPending: () => pending !== null };
}

describe('sorted helpers', () => {
  test('indexOfSorted / insertionIndex', () => {
    const arr = [1, 3, 5, 9];
    expect(indexOfSorted(arr, 5)).toBe(2);
    expect(indexOfSorted(arr, 4)).toBe(-1);
    expect(insertionIndex(arr, 4)).toBe(2);
    expect(insertionIndex(arr, 0)).toBe(0);
    expect(insertionIndex(arr, 10)).toBe(4);
  });

  test('indexInList works for unsorted display lists', () => {
    const list = [9, 3, 7];
    expect(indexInList(list, 7)).toBe(2);
    expect(indexInList(list, 1)).toBe(-1);
  });

  test('rangeBetween is ordered from anchor to target', () => {
    expect(rangeBetween([1, 2, 3, 4, 5], 4, 2, true)).toEqual([4, 3, 2]);
    expect(rangeBetween([5, 1, 4], 5, 4)).toEqual([5, 1, 4]);
    expect(rangeBetween([1, 2], 1, 9, true)).toBeNull();
  });

  test('composeFilterQuery + mapFilterError', () => {
    expect(composeFilterQuery('', 'all')).toEqual({ query: '', offset: 0 });
    expect(composeFilterQuery('  ', 'all')).toEqual({ query: '', offset: 0 });
    expect(composeFilterQuery('', 'doc')).toEqual({ query: 'type:html', offset: 0 });
    const { query, offset } = composeFilterQuery('status:>=400', 'doc');
    expect(query).toBe('(status:>=400) type:html');
    // Error at bytes 1..7 of the composed query == 0..6 of the user's text.
    expect(mapFilterError({ message: 'x', start: 1, end: 7 }, 'status:>=400', offset)).toEqual({ message: 'x', start: 0, end: 6 });
    // Error inside the chip clause → whole user text.
    expect(mapFilterError({ message: 'x', start: 20, end: 24 }, 'abc', 1)).toEqual({ message: 'x', start: 0, end: 3 });
  });

  test('shallowEqual', () => {
    expect(shallowEqual([1, 2], [1, 2])).toBe(true);
    expect(shallowEqual([1, 2], [1, 3])).toBe(false);
    expect(shallowEqual({ a: 1 }, { a: 1 })).toBe(true);
    expect(shallowEqual({ a: 1 }, { a: 1, b: 2 })).toBe(false);
  });
});

describe('TrafficStore ingestion', () => {
  test('hello snapshot replaces everything and sorts defensively', () => {
    const { store, flush } = setup();
    store.reset([summary(3), summary(1), summary(2)]);
    flush();
    const snap = store.getSnapshot();
    expect(snap.loaded).toBe(true);
    expect(snap.order).toEqual([1, 2, 3]);
    expect(snap.visible).toEqual([1, 2, 3]);
    store.reset([summary(10)]);
    flush();
    expect(store.getSnapshot().order).toEqual([10]);
    expect(store.get(1)).toBeUndefined();
  });

  test('added rows append; out-of-order ids are inserted in place; duplicates ignored', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(5)]);
    store.applyExchanges([summary(7), summary(3), summary(5, { status: 404 })], []);
    flush();
    const snap = store.getSnapshot();
    expect(snap.order).toEqual([1, 3, 5, 7]);
    expect(snap.summaries.get(5)?.status).toBe(404);
  });

  test('updates replace summary objects (identity changes) without touching order arrays', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2)]);
    flush();
    const before = store.getSnapshot();
    const s1 = before.summaries.get(1);
    store.applyExchanges([], [summary(1, { state: 'responding', status: undefined })]);
    flush();
    const after = store.getSnapshot();
    expect(after.version).toBeGreaterThan(before.version);
    expect(after.summaries.get(1)).not.toBe(s1);
    expect(after.summaries.get(1)?.state).toBe('responding');
    // Row lists are referentially stable when only summaries changed → the list doesn't re-render.
    expect(after.order).toBe(before.order);
    expect(after.visible).toBe(before.visible);
  });

  test('an update for an unknown id is treated as an add (missed-message race)', () => {
    const { store, flush } = setup();
    store.reset([summary(1)]);
    store.applyExchanges([], [summary(4)]);
    flush();
    expect(store.getSnapshot().order).toEqual([1, 4]);
  });

  test('notifications are batched into one flush per frame', () => {
    const { store, flush, notifications, hasPending } = setup();
    store.reset([]);
    for (let i = 1; i <= 100; i++) store.applyExchanges([summary(i)], []);
    expect(hasPending()).toBe(true);
    const n = notifications();
    flush();
    expect(notifications()).toBe(n + 1);
    expect(store.getSnapshot().order.length).toBe(100);
  });

  test('removed (few and many) and cleared', () => {
    const { store, flush } = setup();
    store.reset(Array.from({ length: 20 }, (_, i) => summary(i + 1)));
    store.remove([2, 4]);
    flush();
    expect(store.getSnapshot().order.slice(0, 4)).toEqual([1, 3, 5, 6]);
    store.remove(Array.from({ length: 10 }, (_, i) => i + 5)); // 5..14 (bulk path)
    flush();
    expect(store.getSnapshot().order).toEqual([1, 3, 15, 16, 17, 18, 19, 20]);
    expect(store.getSnapshot().visible).toEqual([1, 3, 15, 16, 17, 18, 19, 20]);
    store.clear();
    flush();
    expect(store.getSnapshot().order).toEqual([]);
    expect(store.getSnapshot().visible).toEqual([]);
  });

  test('message listeners receive WS/SSE message batches for their exchange only', () => {
    const { store } = setup();
    const got: number[] = [];
    const off = store.onMessages(7, (m) => got.push(...m.map((x) => x.seq)));
    store.notifyMessages(7, [{ seq: 0, direction: 'sent', kind: 'text', at: 0, size: 1 }]);
    store.notifyMessages(8, [{ seq: 5, direction: 'sent', kind: 'text', at: 0, size: 1 }]);
    off();
    store.notifyMessages(7, [{ seq: 1, direction: 'sent', kind: 'text', at: 0, size: 1 }]);
    expect(got).toEqual([0]);
  });
});

describe('TrafficStore server-side filtering', () => {
  test('filter text is sent (composed with the chip) and results define the visible set', () => {
    const { store, sent, flush } = setup();
    store.reset([summary(1), summary(2), summary(3)]);
    store.setFilterText('method:post');
    expect(sent).toEqual(['method:post']);
    expect(store.getSnapshot().filter.pending).toBe(true);
    store.applyFilterResult(1, [2]);
    flush();
    expect(store.getSnapshot().visible).toEqual([2]);
    expect(store.getSnapshot().filter.pending).toBe(false);
    store.setTypeChip('doc');
    expect(sent.at(-1)).toBe('(method:post) type:html');
  });

  test('stale filter results are ignored', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2)]);
    store.setFilterText('a'); // seq 1
    store.setFilterText('ab'); // seq 2
    store.applyFilterResult(1, [1]);
    flush();
    expect(store.getSnapshot().visible).toEqual([1, 2]);
    store.applyFilterResult(2, [2]);
    flush();
    expect(store.getSnapshot().visible).toEqual([2]);
  });

  test('incremental deltas add/remove ids as exchanges start or stop matching', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2)]);
    store.setFilterText('status:404');
    store.applyFilterResult(1, []);
    // New exchange arrives pending (doesn't match yet), then completes with 404.
    store.applyExchanges([summary(3, { status: undefined, state: 'waiting' })], [], { add: [], remove: [] });
    flush();
    expect(store.getSnapshot().visible).toEqual([]);
    expect(store.getSnapshot().order).toEqual([1, 2, 3]);
    store.applyExchanges([], [summary(3, { status: 404 })], { add: [3], remove: [] });
    flush();
    expect(store.getSnapshot().visible).toEqual([3]);
    store.applyExchanges([], [summary(3, { status: 200 })], { add: [], remove: [3] });
    flush();
    expect(store.getSnapshot().visible).toEqual([]);
  });

  test('a delta may reference an id whose summary arrives in the same batch', () => {
    const { store, flush } = setup();
    store.reset([summary(1)]);
    store.setFilterText('host:x');
    store.applyFilterResult(1, []);
    store.applyExchanges([summary(2)], [], { add: [2], remove: [] });
    flush();
    expect(store.getSnapshot().visible).toEqual([2]);
  });

  test('filter error keeps the previous visible set and maps the span into the user text', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2)]);
    store.setFilterText('host:a');
    store.applyFilterResult(1, [1]);
    store.setTypeChip('img'); // seq 2, offset 1
    store.applyFilterResult(2, [1]);
    store.setFilterText('host:(');
    // The daemon keeps its previous filter and reports that filter's ids with the error.
    store.applyFilterResult(3, [1], { message: 'Unclosed (', start: 6, end: 7 });
    flush();
    const snap = store.getSnapshot();
    expect(snap.visible).toEqual([1]);
    expect(snap.filter.error).toEqual({ message: 'Unclosed (', start: 5, end: 6 });
  });

  test('an invalid query across a reconnect follows the daemon instead of freezing the list (regression)', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2), summary(3), summary(4)]);
    store.setFilterText('status:200'); // seq 1
    store.applyFilterResult(1, [1]);
    store.setFilterText('status:'); // seq 2 — invalid; the daemon keeps status:200
    store.applyFilterResult(2, [1], { message: 'Expected a value', start: 7, end: 7 });
    flush();
    expect(store.getSnapshot().visible).toEqual([1]);
    // Reconnect to a connection without any filter: the re-sent invalid query is answered with
    // the daemon's effective (empty) filter — ids: null — and no `filtered` deltas follow.
    store.reset([summary(1), summary(2), summary(3), summary(4)]);
    store.applyFilterResult(2, null, { message: 'Expected a value', start: 7, end: 7 });
    store.applyExchanges([summary(5)], []);
    flush();
    expect(store.getSnapshot().visible).toEqual([1, 2, 3, 4, 5]);
    expect(store.getSnapshot().filter.error?.message).toBe('Expected a value');
    // With the accepted filter restored first (StreamClient), the error carries its ids and
    // later deltas apply on top of them.
    store.reset([summary(1), summary(2), summary(3), summary(4), summary(5)]);
    store.applyFilterResult(2, [1, 3], { message: 'Expected a value', start: 7, end: 7 });
    store.applyExchanges([summary(6)], [], { add: [6], remove: [] });
    flush();
    expect(store.getSnapshot().visible).toEqual([1, 3, 6]);
  });

  test('clearing the filter is instant locally', () => {
    const { store, sent, flush } = setup();
    store.reset([summary(1), summary(2)]);
    store.setFilterText('x');
    store.applyFilterResult(1, [1]);
    store.clearFilter();
    flush();
    expect(sent.at(-1)).toBe('');
    expect(store.getSnapshot().visible).toEqual([1, 2]);
    expect(store.getSnapshot().filter.active).toBe(false);
  });

  test('reconnect (hello) with an active filter keeps showing filtered rows until the new result', () => {
    const { store, flush } = setup();
    store.reset([summary(1), summary(2), summary(3)]);
    store.setFilterText('x');
    store.applyFilterResult(1, [2]);
    flush();
    // The stream re-sends the same query+seq after hello; meanwhile no unfiltered flash.
    store.reset([summary(1), summary(2), summary(3), summary(4)]);
    flush();
    expect(store.getSnapshot().visible).toEqual([2]);
    expect(store.getSnapshot().filter.pending).toBe(true);
    store.applyFilterResult(1, [2, 4]);
    flush();
    expect(store.getSnapshot().visible).toEqual([2, 4]);
  });

  test('filter text is debounced when a debounce is configured', async () => {
    const store = new TrafficStore((fn) => fn(), 20);
    const sent: string[] = [];
    store.setFilterSink((q) => sent.push(q));
    store.setFilterText('a');
    store.setFilterText('ab');
    store.setFilterText('abc');
    expect(sent).toEqual([]);
    await new Promise((r) => setTimeout(r, 40));
    expect(sent).toEqual(['abc']);
    store.setFilterText('abcd');
    store.flushFilter();
    expect(sent).toEqual(['abc', 'abcd']);
  });
});

describe('TrafficStore selection', () => {
  function loaded() {
    const ctx = setup();
    ctx.store.reset(Array.from({ length: 10 }, (_, i) => summary(i + 1)));
    ctx.flush();
    return ctx;
  }

  test('select, toggle and clear', () => {
    const { store } = loaded();
    store.select(3);
    expect([...store.getSnapshot().selection.ids]).toEqual([3]);
    store.toggle(5);
    expect([...store.getSnapshot().selection.ids]).toEqual([3, 5]);
    expect(store.getSnapshot().selection.anchor).toBe(5);
    store.toggle(3);
    expect([...store.getSnapshot().selection.ids]).toEqual([5]);
    store.clearSelection();
    expect(store.getSnapshot().selection.ids.size).toBe(0);
  });

  test('selection changes notify synchronously (same frame as the click)', () => {
    const { store, notifications } = loaded();
    const n = notifications();
    store.select(1);
    expect(notifications()).toBe(n + 1);
  });

  test('shift-range grows and shrinks from the anchor, keeping earlier ctrl-picks', () => {
    const { store } = loaded();
    store.select(2);
    store.toggle(8); // anchor 8, ids {2, 8}
    store.extendTo(6);
    expect([...store.getSnapshot().selection.ids].sort((a, b) => a - b)).toEqual([2, 6, 7, 8]);
    store.extendTo(7); // shrink
    expect([...store.getSnapshot().selection.ids].sort((a, b) => a - b)).toEqual([2, 7, 8]);
    store.extendTo(10); // flip to the other side of the anchor
    expect([...store.getSnapshot().selection.ids].sort((a, b) => a - b)).toEqual([2, 8, 9, 10]);
    expect(store.getSnapshot().selection.anchor).toBe(8);
    expect(store.getSnapshot().selection.active).toBe(10);
  });

  test('extendTo without an anchor selects a single row', () => {
    const { store } = loaded();
    store.extendTo(4);
    expect([...store.getSnapshot().selection.ids]).toEqual([4]);
  });

  test('keyboard movement, Home/End, extend and custom display lists', () => {
    const { store } = loaded();
    expect(store.move(1)).toBe(1); // nothing active → first row
    expect(store.move(1)).toBe(2);
    expect(store.move(-5)).toBe(1); // clamped
    expect(store.move(Infinity)).toBe(10);
    expect(store.move(-Infinity)).toBe(1);
    store.move(1, true);
    store.move(1, true);
    expect([...store.getSnapshot().selection.ids].sort((a, b) => a - b)).toEqual([1, 2, 3]);
    // A column-sorted view (descending) is navigated in display order.
    const desc = [10, 9, 8, 7, 6, 5, 4, 3, 2, 1];
    store.select(5);
    expect(store.move(1, false, desc)).toBe(4);
  });

  test('move on an empty list returns null', () => {
    const { store } = setup();
    store.reset([]);
    expect(store.move(1)).toBeNull();
  });

  test('select all / selectedInOrder', () => {
    const { store } = loaded();
    store.selectAll();
    expect(store.getSnapshot().selection.ids.size).toBe(10);
    store.setSelectedIds([9, 2, 5], 5);
    expect(store.selectedInOrder()).toEqual([2, 5, 9]);
  });

  test('selection is pruned when exchanges are removed, never stolen by new traffic', () => {
    const { store, flush } = loaded();
    store.setSelectedIds([2, 3], 3);
    store.applyExchanges([summary(11)], []);
    flush();
    expect([...store.getSnapshot().selection.ids]).toEqual([2, 3]);
    store.remove([3]);
    flush();
    expect([...store.getSnapshot().selection.ids]).toEqual([2]);
    expect(store.getSnapshot().selection.active).toBeNull();
    store.clear();
    flush();
    expect(store.getSnapshot().selection.ids.size).toBe(0);
  });
});

describe('TrafficStore at scale', () => {
  test('ingests 100k exchanges quickly', () => {
    const { store, flush } = setup();
    store.reset([]);
    const t0 = performance.now();
    for (let batch = 0; batch < 1000; batch++) {
      const added = Array.from({ length: 100 }, (_, i) => summary(batch * 100 + i + 1));
      store.applyExchanges(added, []);
      if (batch % 2 === 0) flush(); // ~30 Hz batches, one frame each
    }
    flush();
    const elapsed = performance.now() - t0;
    expect(store.getSnapshot().order.length).toBe(100_000);
    expect(store.getSnapshot().visible.length).toBe(100_000);
    // Generous bound for CI noise; typically ~50-150 ms.
    expect(elapsed).toBeLessThan(2500);
  });
});

describe('TrafficStore paged hello', () => {
  test('older chunks are merged in front until nothing remains', () => {
    const { store, flush } = setup();
    store.reset([summary(8), summary(9), summary(10)], 7);
    flush();
    expect(store.getSnapshot().order).toEqual([8, 9, 10]);
    expect(store.getSnapshot().loadingOlder).toBe(7);
    store.prependOlder([summary(4), summary(5), summary(6), summary(7)], 3);
    flush();
    expect(store.getSnapshot().order).toEqual([4, 5, 6, 7, 8, 9, 10]);
    expect(store.getSnapshot().visible).toEqual([4, 5, 6, 7, 8, 9, 10]);
    // Exchange 2 was removed while paging: the daemon omits it from the last chunk.
    store.prependOlder([summary(1), summary(3)], 0);
    flush();
    expect(store.getSnapshot().order).toEqual([1, 3, 4, 5, 6, 7, 8, 9, 10]);
    expect(store.getSnapshot().loadingOlder).toBe(0);
  });

  test('live traffic during paging: new rows append, updates of rows still to come wait for their chunk', () => {
    const { store, flush } = setup();
    store.reset([summary(5), summary(6)], 4);
    // A new exchange, an update of a known one, and an update of an older one not delivered yet.
    store.applyExchanges([summary(7)], [summary(6, { status: 500 }), summary(2, { status: 404 })]);
    flush();
    expect(store.getSnapshot().order).toEqual([5, 6, 7]);
    expect(store.get(2)).toBeUndefined();
    // The chunk read later carries exchange 2's current state.
    store.prependOlder([summary(1), summary(2, { status: 418 }), summary(3), summary(4)], 0);
    flush();
    expect(store.getSnapshot().order).toEqual([1, 2, 3, 4, 5, 6, 7]);
    expect(store.get(2)?.status).toBe(418);
    expect(store.get(6)?.status).toBe(500);
    // Paging is over: unknown updates are adds again (missed-message race).
    store.applyExchanges([], [summary(0)]);
    flush();
    expect(store.getSnapshot().order[0]).toBe(0);
  });

  test('a filter result naming rows still to come shows them as they arrive', () => {
    const { store, flush } = setup();
    store.reset([summary(5), summary(6)], 4);
    store.setFilterText('status:404');
    store.applyFilterResult(1, [2, 6]);
    flush();
    expect(store.getSnapshot().visible).toEqual([6]);
    store.prependOlder([summary(1), summary(2), summary(3), summary(4)], 0);
    flush();
    expect(store.getSnapshot().visible).toEqual([2, 6]);
    expect(store.getSnapshot().order).toEqual([1, 2, 3, 4, 5, 6]);
  });

  test('a new hello supersedes a paging in progress; clear ends it', () => {
    const { store, flush } = setup();
    store.reset([summary(5)], 4);
    store.reset([summary(9)]);
    flush();
    expect(store.getSnapshot().loadingOlder).toBe(0);
    store.applyExchanges([], [summary(3)]);
    flush();
    expect(store.getSnapshot().order).toEqual([3, 9]);
    store.reset([summary(20)], 10);
    store.clear();
    flush();
    expect(store.getSnapshot().loadingOlder).toBe(0);
  });
});

describe('TrafficStore publishing cost', () => {
  test('snapshots get a new identity per change without copying the id lists', () => {
    const { store, flush } = setup();
    store.reset(Array.from({ length: 3 }, (_, i) => summary(i + 1)));
    flush();
    const a = store.getSnapshot();
    store.applyExchanges([summary(4)], []);
    flush();
    const b = store.getSnapshot();
    expect(b.order).not.toBe(a.order);
    expect(b.visible).not.toBe(a.visible);
    expect(Array.isArray(b.order)).toBe(true);
    expect([...b.order]).toEqual([1, 2, 3, 4]);
    expect(b.order.slice(-2)).toEqual([3, 4]);
  });

  test('appending one row per frame to a huge list stays cheap (no O(n) copy per flush)', () => {
    const { store, flush } = setup();
    const n = 500_000;
    store.reset(Array.from({ length: n }, (_, i) => summary(i + 1)));
    flush();
    const t0 = performance.now();
    for (let i = 1; i <= 2000; i++) {
      store.applyExchanges([summary(n + i)], []);
      flush();
    }
    const elapsed = performance.now() - t0;
    expect(store.getSnapshot().order.length).toBe(n + 2000);
    expect(store.getSnapshot().visible[n + 1999]).toBe(n + 2000);
    // Copying both lists per flush would move 2 × 500k ids 2000 times (seconds); views cost ~nothing.
    expect(elapsed).toBeLessThan(750);
  });
});
