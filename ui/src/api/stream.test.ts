import { describe, expect, test } from 'bun:test';
import type { WebSocketLike } from './client';
import { backoffDelay, StreamClient, type StreamState } from './stream';
import type { ServerMessage } from './types';

/** Deterministic timers. */
class Clock {
  now = 0;
  private seq = 0;
  private timers = new Map<number, { at: number; fn: () => void }>();
  setTimeout = (fn: () => void, ms: number): unknown => {
    const id = ++this.seq;
    this.timers.set(id, { at: this.now + ms, fn });
    return id;
  };
  clearTimeout = (h: unknown): void => {
    this.timers.delete(h as number);
  };
  advance(ms: number): void {
    const end = this.now + ms;
    for (;;) {
      let next: [number, { at: number; fn: () => void }] | null = null;
      for (const entry of this.timers) if (entry[1].at <= end && (!next || entry[1].at < next[1].at)) next = entry;
      if (!next) break;
      this.timers.delete(next[0]);
      this.now = next[1].at;
      next[1].fn();
    }
    this.now = end;
  }
  pending(): number {
    return this.timers.size;
  }
}

class FakeSocket implements WebSocketLike {
  readyState = 0;
  sent: unknown[] = [];
  onopen: ((ev: Event) => void) | null = null;
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onclose: ((ev: CloseEvent) => void) | null = null;
  onerror: ((ev: Event) => void) | null = null;
  closedWith: number | null = null;
  send(data: string): void {
    this.sent.push(JSON.parse(data));
  }
  close(code = 1000): void {
    this.closedWith = code;
    this.readyState = 3;
  }
  // test helpers
  open(): void {
    this.readyState = 1;
    this.onopen?.({} as Event);
  }
  receive(msg: ServerMessage | string): void {
    this.onmessage?.({ data: typeof msg === 'string' ? msg : JSON.stringify(msg) } as MessageEvent);
  }
  drop(code = 1006): void {
    this.readyState = 3;
    this.onclose?.({ code } as CloseEvent);
  }
}

const HELLO = {
  type: 'hello',
  status: {} as never,
  summaries: [],
  paused: [],
  rules: { version: 1, items: [] },
  settings: {} as never,
  pausedMessages: [],
  remaining: 0,
} as ServerMessage;

function setup() {
  const clock = new Clock();
  const sockets: FakeSocket[] = [];
  const client = new StreamClient({
    openSocket: () => {
      const s = new FakeSocket();
      sockets.push(s);
      return s;
    },
    random: () => 0.5, // no jitter
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    minDelay: 250,
    maxDelay: 10_000,
    pingInterval: 15_000,
    pongTimeout: 10_000,
    helloTimeout: 5_000,
  });
  const states: StreamState[] = [];
  client.onState((s) => states.push(s));
  return { clock, sockets, client, states, last: () => sockets[sockets.length - 1] as FakeSocket };
}

describe('backoffDelay', () => {
  test('grows exponentially, capped, with ±20% jitter', () => {
    const mid = () => 0.5;
    expect(backoffDelay(1, 250, 10_000, mid)).toBe(250);
    expect(backoffDelay(2, 250, 10_000, mid)).toBe(500);
    expect(backoffDelay(5, 250, 10_000, mid)).toBe(4000);
    expect(backoffDelay(20, 250, 10_000, mid)).toBe(10_000);
    expect(backoffDelay(1, 250, 10_000, () => 0)).toBe(200);
    expect(backoffDelay(1, 250, 10_000, () => 0.999)).toBeLessThanOrEqual(300);
    expect(backoffDelay(20, 250, 10_000, () => 0.999)).toBe(10_000);
  });
});

describe('StreamClient', () => {
  test('connects, becomes open on hello and dispatches typed messages', () => {
    const { client, last, states } = setup();
    const seen: string[] = [];
    client.on('hello', () => seen.push('hello'));
    client.on('cleared', () => seen.push('cleared'));
    client.onAny((m) => seen.push(`any:${m.type}`));
    client.connect();
    expect(client.state).toBe('connecting');
    last().open();
    expect(client.state).toBe('connecting'); // not open until hello
    last().receive(HELLO);
    last().receive({ type: 'cleared' });
    expect(client.state).toBe('open');
    expect(states).toEqual(['open']);
    expect(seen).toEqual(['hello', 'any:hello', 'cleared', 'any:cleared']);
  });

  test('a paged hello: hello-more chunks are dispatched after their hello, never before', () => {
    const { client, last } = setup();
    const seen: string[] = [];
    client.on('hello', (m) => seen.push(`hello:${m.remaining ?? 0}`));
    client.on('hello-more', (m) => seen.push(`more:${m.remaining}`));
    client.connect();
    last().open();
    last().receive({ type: 'hello-more', summaries: [], remaining: 3 }); // stray: no hello yet
    last().receive({ ...HELLO, remaining: 5 } as ServerMessage);
    last().receive({ type: 'hello-more', summaries: [], remaining: 2 });
    last().receive({ type: 'hello-more', summaries: [], remaining: 0 });
    expect(seen).toEqual(['hello:5', 'more:2', 'more:0']);
  });

  test('ignores malformed frames and isolates throwing handlers', () => {
    const { client, last } = setup();
    let got = 0;
    client.on('cleared', () => {
      throw new Error('boom');
    });
    client.on('cleared', () => got++);
    const err = console.error;
    console.error = () => undefined;
    try {
      client.connect();
      last().open();
      last().receive('{not json');
      last().receive('{"no":"type"}');
      last().receive(HELLO);
      last().receive({ type: 'cleared' });
    } finally {
      console.error = err;
    }
    expect(got).toBe(1);
  });

  test('reconnects with exponential backoff until hello, then resets', () => {
    const { client, sockets, clock, last, states } = setup();
    const failures: number[] = [];
    client.onFailure((f) => failures.push(f.retryIn));
    client.connect();
    last().drop(); // attempt 1 failed
    expect(failures).toEqual([250]);
    clock.advance(249);
    expect(sockets.length).toBe(1);
    clock.advance(1);
    expect(sockets.length).toBe(2);
    last().drop(); // attempt 2
    clock.advance(500);
    last().drop(); // attempt 3
    expect(failures).toEqual([250, 500, 1000]);
    clock.advance(1000);
    last().open();
    last().receive(HELLO);
    expect(client.state).toBe('open');
    // A healthy connection that drops retries immediately and reports `reconnecting`.
    last().drop();
    expect(client.state).toBe('reconnecting');
    expect(failures.at(-1)).toBe(0);
    clock.advance(0);
    expect(sockets.length).toBe(5);
    expect(states).toEqual(['open', 'reconnecting']);
  });

  test('re-sends the current filter after every hello and drops stale filter results', () => {
    const { client, last, clock } = setup();
    const results: number[] = [];
    client.on('filter-result', (m) => results.push(m.seq));
    client.connect();
    // Filter set before connecting is queued until hello.
    expect(client.setFilter('method:post')).toBe(1);
    last().open();
    expect(last().sent).toEqual([]);
    last().receive(HELLO);
    expect(last().sent).toEqual([{ type: 'set-filter', query: 'method:post', seq: 1 }]);
    expect(client.setFilter('status:500')).toBe(2);
    last().receive({ type: 'filter-result', seq: 1, query: 'method:post', ids: [1] }); // stale
    last().receive({ type: 'filter-result', seq: 2, query: 'status:500', ids: [] });
    expect(results).toEqual([2]);
    // Reconnect → same query and seq again.
    last().drop();
    clock.advance(0);
    last().open();
    last().receive(HELLO);
    expect(last().sent).toEqual([{ type: 'set-filter', query: 'status:500', seq: 2 }]);
  });

  test('after a reconnect an invalid query in the box keeps the last accepted filter (regression)', () => {
    const { client, last, clock } = setup();
    const results: { seq: number; ids: number[] | null; error: boolean }[] = [];
    client.on('filter-result', (m) => results.push({ seq: m.seq, ids: m.ids, error: !!m.error }));
    client.connect();
    last().open();
    last().receive(HELLO);
    client.setFilter('status:200'); // seq 1, accepted
    last().receive({ type: 'filter-result', seq: 1, query: 'status:200', ids: [1] });
    client.setFilter('status:'); // seq 2, invalid: the server keeps status:200
    last().receive({ type: 'filter-result', seq: 2, query: 'status:', ids: [1], error: { message: 'Expected a value', start: 7, end: 7 } });
    // Reconnect: restore the accepted filter first, then re-send the current (invalid) one.
    last().drop();
    clock.advance(0);
    last().open();
    last().receive(HELLO);
    expect(last().sent).toEqual([
      { type: 'set-filter', query: 'status:200', seq: 1 },
      { type: 'set-filter', query: 'status:', seq: 2 },
    ]);
    // The restored filter's own result is stale; the error result carries its ids.
    last().receive({ type: 'filter-result', seq: 1, query: 'status:200', ids: [1, 5] });
    last().receive({ type: 'filter-result', seq: 2, query: 'status:', ids: [1, 5], error: { message: 'Expected a value', start: 7, end: 7 } });
    expect(results.slice(2)).toEqual([{ seq: 2, ids: [1, 5], error: true }]);
    // Once a valid query is accepted, reconnects send just that one.
    client.setFilter('status:404'); // seq 3
    last().receive({ type: 'filter-result', seq: 3, query: 'status:404', ids: [] });
    last().drop();
    clock.advance(0);
    last().open();
    last().receive(HELLO);
    expect(last().sent).toEqual([{ type: 'set-filter', query: 'status:404', seq: 3 }]);
  });

  test('keepalive: pings after silence and recycles a dead socket', () => {
    const { client, last, clock, sockets } = setup();
    client.connect();
    last().open();
    last().receive(HELLO);
    clock.advance(15_000);
    expect(last().sent).toEqual([{ type: 'ping' }]);
    last().receive({ type: 'pong' }); // alive → re-armed
    clock.advance(15_000);
    expect(last().sent.length).toBe(2);
    const dead = last();
    clock.advance(10_000); // no answer
    expect(dead.closedWith).toBe(4000);
    expect(client.state).toBe('reconnecting');
    clock.advance(0);
    expect(sockets.length).toBe(2);
  });

  test('a socket that opens but never says hello is recycled', () => {
    const { client, last, clock, sockets } = setup();
    client.connect();
    last().open();
    clock.advance(5_000);
    expect(sockets[0]?.closedWith).toBe(4000);
    clock.advance(250);
    expect(sockets.length).toBe(2);
  });

  test('close() stops everything; send() reports whether it was delivered', () => {
    const { client, last, clock, sockets } = setup();
    client.connect();
    expect(client.send({ type: 'ping' })).toBe(false);
    last().open();
    last().receive(HELLO);
    expect(client.send({ type: 'ping' })).toBe(true);
    client.close();
    expect(client.state).toBe('closed');
    expect(sockets[0]?.closedWith).toBe(1000);
    clock.advance(60_000);
    expect(sockets.length).toBe(1);
    expect(clock.pending()).toBe(0);
  });

  test('reconnectNow skips the backoff wait', () => {
    const { client, last, sockets } = setup();
    client.connect();
    last().drop();
    expect(sockets.length).toBe(1);
    client.reconnectNow();
    expect(sockets.length).toBe(2);
  });

  test('openSocket throwing counts as a failed attempt', () => {
    const clock = new Clock();
    let calls = 0;
    const client = new StreamClient({
      openSocket: () => {
        calls++;
        throw new Error('blocked');
      },
      random: () => 0.5,
      setTimeout: clock.setTimeout,
      clearTimeout: clock.clearTimeout,
    });
    client.connect();
    expect(calls).toBe(1);
    clock.advance(250);
    expect(calls).toBe(2);
    client.close();
  });
});
