/**
 * Live event stream client (`GET /api/stream`, HTTP API reference § Live stream).
 *
 * - Exponential-backoff reconnect with jitter (250 ms → 10 s), reset after a successful `hello`.
 * - Every (re)connection starts with `hello`, a full snapshot: consumers must treat it as a
 *   complete resync (replace, don't merge). A large snapshot is paged: `hello` carries the
 *   newest summaries and `remaining`, then `hello-more` chunks bring the older ones (merge
 *   those in front).
 * - Keepalive: `ping` every 15 s; if nothing arrives within 10 s of a ping the socket is
 *   considered dead and recycled (laptops waking from sleep, proxies dropping idle sockets).
 * - Server-side filtering: {@link StreamClient.setFilter} sends `set-filter` with a fresh
 *   sequence number and re-sends the current filter after every reconnect. Stale
 *   `filter-result`s (older `seq`) are dropped before dispatch.
 * - Reconnects immediately when the browser comes back online or the tab becomes visible
 *   while disconnected.
 */

import type { WebSocketLike } from './client';
import type { ClientMessage, ServerMessage, ServerMessageOf, ServerMessageType } from './types';

export type StreamState =
  /** Not connected yet (initial attempts, before the first `hello`). */
  | 'connecting'
  /** Socket open and `hello` received. */
  | 'open'
  /** Was connected, lost the connection; retrying with backoff. */
  | 'reconnecting'
  /** Stopped by {@link StreamClient.close}. */
  | 'closed';

export interface StreamOptions {
  /** Creates a socket for the stream URL. */
  openSocket: () => WebSocketLike;
  /** Initial reconnect delay (ms). */
  minDelay?: number;
  /** Maximum reconnect delay (ms). */
  maxDelay?: number;
  /** Keepalive ping interval (ms). */
  pingInterval?: number;
  /** Max silence after a ping before the socket is recycled (ms). */
  pongTimeout?: number;
  /** Max time to wait for `hello` after the socket opens (ms). */
  helloTimeout?: number;
  /** Random source in [0,1) for jitter (injectable for tests). */
  random?: () => number;
  /** Timer functions (injectable for tests). */
  setTimeout?: (fn: () => void, ms: number) => unknown;
  clearTimeout?: (handle: unknown) => void;
}

export interface StreamFailure {
  /** Number of consecutive failed attempts. */
  attempt: number;
  /** Close code reported by the socket, if any. */
  code?: number;
  /** Delay (ms) before the next attempt. */
  retryIn: number;
  /** Whether the failed socket ever received `hello`. */
  hadHello: boolean;
}

type Handler<T extends ServerMessageType> = (msg: ServerMessageOf<T>) => void;
type AnyHandler = (msg: ServerMessage) => void;

/**
 * Compute the reconnect delay for an attempt (1-based): exponential with ±20% jitter.
 * Exported for tests.
 */
export function backoffDelay(attempt: number, minDelay: number, maxDelay: number, random: () => number): number {
  const exp = Math.min(maxDelay, minDelay * 2 ** Math.max(0, attempt - 1));
  const jitter = 1 + (random() * 0.4 - 0.2);
  return Math.round(Math.min(maxDelay, exp * jitter));
}

export class StreamClient {
  private readonly opts: Required<Omit<StreamOptions, 'openSocket'>> & Pick<StreamOptions, 'openSocket'>;
  private socket: WebSocketLike | null = null;
  private stateValue: StreamState = 'connecting';
  private attempt = 0;
  private reconnectTimer: unknown = null;
  private pingTimer: unknown = null;
  private pongTimer: unknown = null;
  private helloTimer: unknown = null;
  private hadHello = false;
  private everOpen = false;
  private started = false;
  private nextRetryAt: number | null = null;

  private filterQuery = '';
  private filterSeq = 0;
  /** Last filter the server accepted (a `filter-result` without error) — its effective filter. */
  private acceptedQuery = '';
  private acceptedSeq = 0;

  private readonly handlers = new Map<ServerMessageType, Set<(msg: never) => void>>();
  private readonly anyHandlers = new Set<AnyHandler>();
  private readonly stateListeners = new Set<(state: StreamState) => void>();
  private readonly failureListeners = new Set<(failure: StreamFailure) => void>();

  constructor(options: StreamOptions) {
    this.opts = {
      minDelay: 250,
      maxDelay: 10_000,
      pingInterval: 15_000,
      pongTimeout: 10_000,
      helloTimeout: 15_000,
      random: Math.random,
      setTimeout: (fn, ms) => globalThis.setTimeout(fn, ms),
      clearTimeout: (h) => globalThis.clearTimeout(h as ReturnType<typeof setTimeout>),
      ...options,
    };
  }

  /** Current connection state. */
  get state(): StreamState {
    return this.stateValue;
  }

  /** Unix ms of the next reconnect attempt while `reconnecting`, else `null`. */
  get retryAt(): number | null {
    return this.nextRetryAt;
  }

  /** The filter query currently requested from the server. */
  get filter(): string {
    return this.filterQuery;
  }

  /** Sequence number of the latest `set-filter` sent. */
  get latestFilterSeq(): number {
    return this.filterSeq;
  }

  /** Start connecting (idempotent). */
  connect(): void {
    if (this.started) return;
    this.started = true;
    this.installEnvironmentListeners();
    this.open();
  }

  /** Close for good (no reconnect). */
  close(): void {
    this.started = false;
    this.clearTimers();
    this.removeEnvironmentListeners();
    const s = this.socket;
    this.socket = null;
    if (s) {
      s.onopen = s.onmessage = s.onerror = s.onclose = null;
      try {
        s.close(1000, 'client closed');
      } catch {
        // ignore
      }
    }
    this.setState('closed');
  }

  /** Skip the backoff wait and try to reconnect now (no-op while open). */
  reconnectNow(): void {
    if (!this.started || this.stateValue === 'open') return;
    if (this.reconnectTimer != null) {
      this.opts.clearTimeout(this.reconnectTimer);
      this.reconnectTimer = null;
    }
    if (!this.socket) this.open();
  }

  /** Send a client message if connected; returns whether it was sent. */
  send(msg: ClientMessage): boolean {
    const s = this.socket;
    if (!s || s.readyState !== 1 /* OPEN */) return false;
    try {
      s.send(JSON.stringify(msg));
      return true;
    } catch {
      return false;
    }
  }

  /**
   * Request server-side filtering. Returns the sequence number; the matching
   * `filter-result` carries the same `seq`. The query is re-sent automatically after reconnects.
   */
  setFilter(query: string): number {
    this.filterQuery = query;
    this.filterSeq += 1;
    if (this.hadHello) this.send({ type: 'set-filter', query, seq: this.filterSeq });
    return this.filterSeq;
  }

  /** Subscribe to one message type. Returns an unsubscribe function. */
  on<T extends ServerMessageType>(type: T, handler: Handler<T>): () => void {
    let set = this.handlers.get(type);
    if (!set) {
      set = new Set();
      this.handlers.set(type, set);
    }
    set.add(handler as (msg: never) => void);
    return () => set.delete(handler as (msg: never) => void);
  }

  /** Subscribe to every message. */
  onAny(handler: AnyHandler): () => void {
    this.anyHandlers.add(handler);
    return () => this.anyHandlers.delete(handler);
  }

  /** Subscribe to connection state changes. */
  onState(listener: (state: StreamState) => void): () => void {
    this.stateListeners.add(listener);
    return () => this.stateListeners.delete(listener);
  }

  /** Called after every failed/lost connection, before the retry is scheduled. */
  onFailure(listener: (failure: StreamFailure) => void): () => void {
    this.failureListeners.add(listener);
    return () => this.failureListeners.delete(listener);
  }

  // ---------------------------------------------------------------------------------

  private setState(state: StreamState): void {
    if (state === this.stateValue) return;
    this.stateValue = state;
    for (const l of this.stateListeners) l(state);
  }

  private open(): void {
    if (!this.started) return;
    this.nextRetryAt = null;
    this.hadHello = false;
    let socket: WebSocketLike;
    try {
      socket = this.opts.openSocket();
    } catch {
      this.handleClose(undefined);
      return;
    }
    this.socket = socket;
    socket.onopen = () => {
      if (this.socket !== socket) return;
      this.helloTimer = this.opts.setTimeout(() => {
        if (this.socket === socket && !this.hadHello) this.recycle(socket);
      }, this.opts.helloTimeout);
    };
    socket.onmessage = (ev) => {
      if (this.socket !== socket) return;
      this.handleRaw(ev.data);
    };
    socket.onerror = () => {
      // onclose always follows; nothing to do here.
    };
    socket.onclose = (ev) => {
      if (this.socket !== socket) return;
      this.handleClose(ev.code);
    };
  }

  private recycle(socket: WebSocketLike): void {
    socket.onopen = socket.onmessage = socket.onerror = socket.onclose = null;
    try {
      socket.close(4000, 'keepalive timeout');
    } catch {
      // ignore
    }
    if (this.socket === socket) this.handleClose(4000);
  }

  private handleClose(code: number | undefined): void {
    const hadHello = this.hadHello;
    this.socket = null;
    this.hadHello = false;
    this.clearTimers();
    if (!this.started) return;
    // A connection that was healthy retries immediately once; after that, back off.
    this.attempt = hadHello ? 1 : this.attempt + 1;
    const retryIn = hadHello ? 0 : backoffDelay(this.attempt, this.opts.minDelay, this.opts.maxDelay, this.opts.random);
    this.setState(this.everOpen ? 'reconnecting' : 'connecting');
    for (const l of this.failureListeners) l({ attempt: this.attempt, code, retryIn, hadHello });
    if (!this.started) return;
    this.nextRetryAt = Date.now() + retryIn;
    this.reconnectTimer = this.opts.setTimeout(() => {
      this.reconnectTimer = null;
      this.open();
    }, retryIn);
  }

  private handleRaw(data: unknown): void {
    let msg: ServerMessage;
    try {
      msg = JSON.parse(typeof data === 'string' ? data : String(data)) as ServerMessage;
    } catch {
      return; // ignore malformed frames
    }
    if (!msg || typeof msg !== 'object' || typeof msg.type !== 'string') return;
    this.armPing();

    if (msg.type === 'hello') {
      this.hadHello = true;
      this.everOpen = true;
      this.attempt = 0;
      if (this.helloTimer != null) {
        this.opts.clearTimeout(this.helloTimer);
        this.helloTimer = null;
      }
      this.setState('open');
      this.dispatch(msg);
      this.resendFilter();
      return;
    }
    // Older chunks of a paged `hello` belong to this socket's snapshot; never before it.
    if (msg.type === 'hello-more' && !this.hadHello) return;
    if (msg.type === 'filter-result' && !msg.error && msg.seq >= this.acceptedSeq) {
      this.acceptedQuery = msg.query;
      this.acceptedSeq = msg.seq;
    }
    if (msg.type === 'filter-result' && msg.seq < this.filterSeq) return; // stale
    if (msg.type === 'pong') return;
    this.dispatch(msg);
  }

  /**
   * After `hello` (a new connection, or a resync after overload): re-establish the filter. When
   * the current query isn't the one the server last accepted (it was invalid, or never
   * answered), first restore the accepted one — so an invalid query in the box keeps the last
   * valid filter in effect across reconnects, exactly as it does on one connection. Its result
   * carries an older `seq` and is dropped as stale; the current query's result (or error, which
   * reports the restored filter's ids) is what the store applies.
   */
  private resendFilter(): void {
    if (!this.filterQuery) return;
    if (this.acceptedQuery && this.acceptedQuery !== this.filterQuery && this.acceptedSeq < this.filterSeq) {
      this.send({ type: 'set-filter', query: this.acceptedQuery, seq: this.acceptedSeq });
    }
    this.send({ type: 'set-filter', query: this.filterQuery, seq: this.filterSeq });
  }

  private dispatch(msg: ServerMessage): void {
    const set = this.handlers.get(msg.type);
    if (set) {
      for (const h of set) {
        try {
          (h as (m: ServerMessage) => void)(msg);
        } catch (e) {
          console.error(`[akev] stream handler for "${msg.type}" failed`, e);
        }
      }
    }
    for (const h of this.anyHandlers) {
      try {
        h(msg);
      } catch (e) {
        console.error('[akev] stream handler failed', e);
      }
    }
  }

  /** (Re)arm the keepalive: any inbound traffic proves liveness. */
  private armPing(): void {
    if (this.pongTimer != null) {
      this.opts.clearTimeout(this.pongTimer);
      this.pongTimer = null;
    }
    if (this.pingTimer != null) this.opts.clearTimeout(this.pingTimer);
    const socket = this.socket;
    this.pingTimer = this.opts.setTimeout(() => {
      this.pingTimer = null;
      if (!socket || this.socket !== socket) return;
      this.send({ type: 'ping' });
      this.pongTimer = this.opts.setTimeout(() => {
        this.pongTimer = null;
        if (this.socket === socket) this.recycle(socket);
      }, this.opts.pongTimeout);
    }, this.opts.pingInterval);
  }

  private clearTimers(): void {
    for (const t of [this.reconnectTimer, this.pingTimer, this.pongTimer, this.helloTimer]) {
      if (t != null) this.opts.clearTimeout(t);
    }
    this.reconnectTimer = this.pingTimer = this.pongTimer = this.helloTimer = null;
  }

  private readonly onOnline = () => this.reconnectNow();
  private readonly onVisible = () => {
    if (document.visibilityState === 'visible') this.reconnectNow();
  };

  private installEnvironmentListeners(): void {
    window.addEventListener('online', this.onOnline);
    document.addEventListener('visibilitychange', this.onVisible);
  }

  private removeEnvironmentListeners(): void {
    window.removeEventListener('online', this.onOnline);
    document.removeEventListener('visibilitychange', this.onVisible);
  }
}
