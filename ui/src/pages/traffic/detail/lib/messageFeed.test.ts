import { describe, expect, test } from 'bun:test';
import type { MessagesPage } from '../../../../api/client';
import type { StreamMessageWithPayload } from '../../../../api/types';
import { TrafficStore } from '../../../../state/traffic';
import { MessageFeed } from './messageFeed';
import { mergeMessages } from './messages';

const msg = (seq: number): StreamMessageWithPayload => ({ seq, direction: 'received', kind: 'text', at: seq, size: 1, text: String(seq), base64: null });
const tick = () => new Promise((r) => setTimeout(r, 0));

/** The daemon's paging: exclusive `after`/`before`, ascending, `tail` = newest `limit` of the range. */
function pageOf(all: readonly StreamMessageWithPayload[], page: MessagesPage): StreamMessageWithPayload[] {
  const limit = Math.min(page.limit ?? 1000, 10_000);
  const range = all.filter((m) => (page.after === undefined || m.seq > page.after) && (page.before === undefined || m.seq < page.before));
  return page.tail ? range.slice(Math.max(0, range.length - limit)) : range.slice(0, limit);
}

/** A daemon holding `server` messages, answering pages like the real endpoint. */
function setup(pages: { initialPage?: number; forwardPage?: number } = {}) {
  const store = new TrafficStore((fn) => fn(), 0);
  const server: StreamMessageWithPayload[] = [];
  const calls: MessagesPage[] = [];
  let shown: StreamMessageWithPayload[] = [];
  let loaded = false;
  let older = false;
  const feed = new MessageFeed({
    exchangeId: 7,
    fetch: async (_id, page) => {
      calls.push(page);
      return pageOf(server, page);
    },
    source: store,
    onBatch: (batch) => (shown = mergeMessages(shown, batch)),
    onLoaded: () => (loaded = true),
    onError: () => undefined,
    onOlderChange: (v) => (older = v),
    ...pages,
  });
  return { store, server, calls, feed, shown: () => shown.map((m) => m.seq), loaded: () => loaded, older: () => older };
}

describe('MessageFeed', () => {
  test('newest page first, then one ?after= fetch per live notification', async () => {
    const { store, server, calls, feed, shown, loaded } = setup();
    server.push(msg(0), msg(1));
    const stop = feed.start();
    await tick();
    expect(loaded()).toBe(true);
    expect(shown()).toEqual([0, 1]);
    server.push(msg(2));
    store.notifyMessages(7, [msg(2)]);
    await tick();
    expect(shown()).toEqual([0, 1, 2]);
    expect(calls).toEqual([
      { tail: true, limit: 500 },
      { after: 1, limit: 1000 },
    ]);
    stop();
    server.push(msg(3));
    store.notifyMessages(7, [msg(3)]);
    await tick();
    expect(calls.length).toBe(2);
  });

  test('messages delivered during a reconnect / overload resync are fetched on reset (regression)', async () => {
    const { store, server, feed, shown } = setup();
    server.push(msg(0));
    feed.start();
    await tick();
    // The connection drops (or the daemon resyncs an overloaded client): these messages never
    // produce a `messages` notification…
    server.push(msg(1), msg(2), msg(3));
    expect(shown()).toEqual([0]);
    // …but the resync's `hello` resets the store, and the feed catches up.
    store.reset([]);
    await tick();
    expect(shown()).toEqual([0, 1, 2, 3]);
  });

  test('a summary count ahead of what was received fetches the rest, without looping', async () => {
    const { server, calls, feed, shown } = setup();
    server.push(msg(0));
    feed.start();
    await tick();
    server.push(msg(1), msg(2)); // notification lost
    feed.expect(3);
    await tick();
    expect(shown()).toEqual([0, 1, 2]);
    const n = calls.length;
    feed.expect(3); // already have them all
    await tick();
    expect(calls.length).toBe(n);
    feed.expect(5); // count says more, but the daemon has none newer: one fetch, then stop
    await tick();
    await tick();
    expect(calls.length).toBe(n + 1);
  });

  test('a long stream loads its newest page, then older pages on demand', async () => {
    const { server, calls, feed, shown, older } = setup({ initialPage: 3, forwardPage: 4 });
    for (let i = 0; i < 8; i++) server.push(msg(i));
    feed.start();
    await tick();
    expect(shown()).toEqual([5, 6, 7]);
    expect(older()).toBe(true);
    expect(feed.canLoadOlder).toBe(true);
    await feed.loadOlder();
    expect(shown()).toEqual([2, 3, 4, 5, 6, 7]);
    expect(calls[calls.length - 1]).toEqual({ before: 5, tail: true, limit: 3 });
    await feed.loadOlder();
    expect(shown()).toEqual([0, 1, 2, 3, 4, 5, 6, 7]);
    expect(older()).toBe(false);
    const n = calls.length;
    await feed.loadOlder(); // nothing older: no request
    expect(calls.length).toBe(n);
  });

  test('catching up forward pages through full pages until a short one', async () => {
    const { store, server, calls, feed, shown } = setup({ initialPage: 2, forwardPage: 3 });
    server.push(msg(0), msg(1));
    feed.start();
    await tick();
    expect(feed.canLoadOlder).toBe(false); // the newest page wasn't full: that's everything
    for (let i = 2; i < 10; i++) server.push(msg(i));
    store.notifyMessages(7, [msg(9)]);
    for (let i = 0; i < 6; i++) await tick();
    expect(shown()).toEqual([0, 1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(calls.slice(1)).toEqual([
      { after: 1, limit: 3 },
      { after: 4, limit: 3 },
      { after: 7, limit: 3 },
    ]);
  });
});

describe('mergeMessages', () => {
  test('appends, prepends older pages and merges out-of-order batches', () => {
    const seqs = (list: StreamMessageWithPayload[]) => list.map((m) => m.seq);
    let list = mergeMessages([], [msg(5), msg(6)]);
    list = mergeMessages(list, [msg(7)]);
    expect(seqs(list)).toEqual([5, 6, 7]);
    list = mergeMessages(list, [msg(2), msg(3)]);
    expect(seqs(list)).toEqual([2, 3, 5, 6, 7]);
    list = mergeMessages(list, [msg(4), msg(6)]);
    expect(seqs(list)).toEqual([2, 3, 4, 5, 6, 7]);
  });
});
