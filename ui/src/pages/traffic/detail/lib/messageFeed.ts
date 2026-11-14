/**
 * Keeps the Messages tab of one WebSocket/SSE exchange in sync with the daemon, one page at a
 * time (a long-lived socket can hold millions of messages).
 *
 * - The first page is the newest one (`?tail=true&limit=<initialPage>`).
 * - Stream notifications only carry message metadata, so every burst is followed by forward
 *   fetches (`?after=<lastSeq>&limit=<pageSize>`, coalesced while one is in flight) until a short
 *   page says the feed has caught up.
 * - {@link MessageFeed.loadOlder} fetches the page before the oldest one held
 *   (`?before=<oldestSeq>&tail=true&limit=<initialPage>`).
 *
 * Notifications are not replayed after a reconnect or an overload resync, so the feed also
 * catches up on every store `reset` and whenever the exchange's summary reports messages past
 * the newest one held.
 */

import type { MessagesPage } from '../../../../api/client';
import { errorMessage } from '../../../../api/client';
import type { ExchangeId, StreamMessage, StreamMessageWithPayload } from '../../../../api/types';
import type { TrafficLifecycleEvent } from '../../../../state/traffic';

export interface MessageFeedSource {
  onMessages(id: ExchangeId, listener: (messages: StreamMessage[]) => void): () => void;
  onLifecycle(listener: (event: TrafficLifecycleEvent) => void): () => void;
}

/** Newest messages loaded first, and per "Load older messages". */
export const INITIAL_PAGE = 500;
/** Page size while catching up forward. */
export const FORWARD_PAGE = 1000;

export interface MessageFeedOptions {
  exchangeId: ExchangeId;
  /** `GET /api/exchanges/:id/messages` with paging parameters. */
  fetch: (id: ExchangeId, page: MessagesPage) => Promise<StreamMessageWithPayload[]>;
  /** Live notifications (the traffic store). */
  source: MessageFeedSource;
  /** A batch of (possibly already seen, possibly older) messages arrived. */
  onBatch: (batch: StreamMessageWithPayload[]) => void;
  /** The initial page loaded. */
  onLoaded: () => void;
  /** The initial page failed (later failures are retried by the next trigger). */
  onError: (message: string) => void;
  /** Whether older messages can be loaded changed. */
  onOlderChange?: (canLoadOlder: boolean) => void;
  /** Overrides for tests. */
  initialPage?: number;
  forwardPage?: number;
}

export class MessageFeed {
  /** Newest seq held (-1 = none). */
  private lastSeq = -1;
  /** Oldest seq held (-1 = none). */
  private oldestSeq = -1;
  private inFlight = false;
  private dirty = false;
  private initialDone = false;
  private alive = false;
  private expected = 0;
  private olderExhausted = false;
  private olderInFlight = false;
  private readonly seen = new Set<number>();
  private readonly initialPage: number;
  private readonly forwardPage: number;

  constructor(private readonly opts: MessageFeedOptions) {
    this.initialPage = opts.initialPage ?? INITIAL_PAGE;
    this.forwardPage = opts.forwardPage ?? FORWARD_PAGE;
  }

  /** Messages received so far (distinct). */
  get received(): number {
    return this.seen.size;
  }

  /** Whether there are older messages than the ones held (see {@link loadOlder}). */
  get canLoadOlder(): boolean {
    return this.initialDone && !this.olderExhausted && this.oldestSeq > 0;
  }

  /** Load the newest page and follow live updates. Returns `stop`. */
  start(): () => void {
    this.alive = true;
    const { source, exchangeId } = this.opts;
    const offMessages = source.onMessages(exchangeId, () => void this.fetchMore());
    const offLifecycle = source.onLifecycle((e) => {
      if (e.type === 'reset') void this.fetchMore();
    });
    void this.fetchMore();
    return () => {
      this.alive = false;
      offMessages();
      offLifecycle();
    };
  }

  /** The exchange summary's `messageCount`: fetch forward when it runs past the newest held. */
  expect(count: number | undefined): void {
    this.expected = count ?? 0;
    if (this.alive && this.initialDone && this.expected > this.lastSeq + 1) void this.fetchMore();
  }

  /** Fetch everything after the newest message held (coalesced); the first call loads the newest page. */
  async fetchMore(): Promise<void> {
    if (!this.alive) return;
    if (this.inFlight) {
      this.dirty = true;
      return;
    }
    this.inFlight = true;
    try {
      do {
        this.dirty = false;
        const first = !this.initialDone;
        const page: MessagesPage = first
          ? { tail: true, limit: this.initialPage }
          : { after: this.lastSeq >= 0 ? this.lastSeq : undefined, limit: this.forwardPage };
        const batch = await this.opts.fetch(this.opts.exchangeId, page);
        if (!this.alive) return;
        const before = this.lastSeq;
        this.take(batch);
        if (first) {
          // A short newest page holds everything there is.
          if (batch.length < this.initialPage) this.olderExhausted = true;
          this.initialDone = true;
          this.opts.onLoaded();
          this.opts.onOlderChange?.(this.canLoadOlder);
          if (this.expected > this.lastSeq + 1) this.dirty = true;
        } else if (batch.length >= this.forwardPage && this.lastSeq > before) {
          // A full page: there may be more right behind it.
          this.dirty = true;
        }
      } while (this.dirty && this.alive);
    } catch (e) {
      if (this.alive && !this.initialDone) this.opts.onError(errorMessage(e));
    } finally {
      this.inFlight = false;
    }
  }

  /** Load the page before the oldest message held. */
  async loadOlder(): Promise<void> {
    if (!this.alive || this.olderInFlight || !this.canLoadOlder) return;
    this.olderInFlight = true;
    try {
      const batch = await this.opts.fetch(this.opts.exchangeId, { before: this.oldestSeq, tail: true, limit: this.initialPage });
      if (!this.alive) return;
      const oldest = this.oldestSeq;
      this.take(batch);
      if (batch.length < this.initialPage || this.oldestSeq >= oldest) this.olderExhausted = true;
      this.opts.onOlderChange?.(this.canLoadOlder);
    } catch {
      // Left to the user: the button stays.
    } finally {
      this.olderInFlight = false;
    }
  }

  private take(batch: StreamMessageWithPayload[]): void {
    if (batch.length === 0) return;
    for (const m of batch) {
      this.seen.add(m.seq);
      if (m.seq > this.lastSeq) this.lastSeq = m.seq;
      if (this.oldestSeq < 0 || m.seq < this.oldestSeq) this.oldestSeq = m.seq;
    }
    this.opts.onBatch(batch);
  }
}
