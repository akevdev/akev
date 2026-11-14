/**
 * Draft/save controller for the Settings page.
 *
 * Edits apply to a local draft immediately (the UI never waits for the daemon), then save:
 * immediately for toggles/selects, debounced for text fields. Saves are serialised and
 * coalesced — at most one request in flight, always sending the latest draft. When a save
 * fails, the draft reverts to the last server copy and every pending `edit()` promise
 * rejects. Server broadcasts replace the draft only while there are no local edits.
 *
 * Generic over the value type so it is testable without the API.
 */

export type SyncPhase = 'idle' | 'pending' | 'saving' | 'saved' | 'error';

export interface SyncSnapshot<T> {
  /** Current draft (what the UI shows). `null` until the first server copy arrives. */
  draft: T | null;
  phase: SyncPhase;
  error: string | null;
  /** Unix ms of the last successful save. */
  savedAt: number | null;
}

export interface SyncOptions<T> {
  save: (value: T) => Promise<T>;
  /** Debounce for text fields (ms). Default 500. */
  debounceMs?: number;
  /** How long the "Saved" phase lingers before returning to idle (ms). Default 2000. */
  savedLingerMs?: number;
  /** Transform the draft right before sending (e.g. merge fields owned elsewhere). */
  prepare?: (draft: T) => T;
  onError?: (error: unknown) => void;
}

interface Waiter<T> {
  resolve: (value: T) => void;
  reject: (error: unknown) => void;
}

export interface EditOptions {
  /** Debounce the save (text fields). */
  debounce?: boolean;
}

export class SettingsSync<T> {
  private server: T | null = null;
  private dirty = false;
  private inFlight: Promise<void> | null = null;
  private timer: ReturnType<typeof setTimeout> | null = null;
  private lingerTimer: ReturnType<typeof setTimeout> | null = null;
  private waiters: Waiter<T>[] = [];
  private snapshot: SyncSnapshot<T> = { draft: null, phase: 'idle', error: null, savedAt: null };
  private readonly listeners = new Set<() => void>();

  constructor(private readonly opts: SyncOptions<T>) {}

  readonly subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  readonly getSnapshot = (): SyncSnapshot<T> => this.snapshot;

  /** Whether local edits are waiting to be (or being) saved. */
  get busy(): boolean {
    return this.dirty || this.inFlight != null;
  }

  private set(partial: Partial<SyncSnapshot<T>>): void {
    this.snapshot = { ...this.snapshot, ...partial };
    for (const l of this.listeners) l();
  }

  /** A server copy arrived (initial load, broadcast or save echo). */
  setServer(value: T): void {
    this.server = value;
    if (!this.busy && this.snapshot.draft !== value) this.set({ draft: value });
  }

  /** Apply an edit to the draft and schedule a save. Resolves with the saved value. */
  edit(mutate: (draft: T) => T, options: EditOptions = {}): Promise<T> {
    const base = this.snapshot.draft ?? this.server;
    if (base == null) return Promise.reject(new Error('Settings have not loaded yet'));
    const next = mutate(base);
    const promise = new Promise<T>((resolve, reject) => this.waiters.push({ resolve, reject }));
    promise.catch(() => undefined); // callers may fire-and-forget; failures surface via onError
    this.dirty = true;
    this.clearLinger();
    this.set({ draft: next, phase: this.inFlight ? 'saving' : 'pending', error: null });
    if (this.timer != null) clearTimeout(this.timer);
    this.timer = setTimeout(() => {
      this.timer = null;
      void this.run();
    }, options.debounce ? (this.opts.debounceMs ?? 500) : 0);
    return promise;
  }

  /** Save pending edits now (e.g. on blur / before leaving). */
  async flush(): Promise<void> {
    if (this.timer != null) {
      clearTimeout(this.timer);
      this.timer = null;
    }
    await this.run();
  }

  /** On unmount: save pending debounced edits now and drop the linger timer. */
  dispose(): void {
    void this.flush();
    this.clearLinger();
  }

  private clearLinger(): void {
    if (this.lingerTimer != null) clearTimeout(this.lingerTimer);
    this.lingerTimer = null;
  }

  private async run(): Promise<void> {
    if (this.inFlight) {
      await this.inFlight;
      if (this.dirty && this.timer == null) await this.run();
      return;
    }
    const sending = this.snapshot.draft;
    if (!this.dirty || sending == null) return;
    const waiters = this.waiters.splice(0);
    this.dirty = false;
    this.set({ phase: 'saving', error: null });

    const flight = (async () => {
      let saved: T;
      try {
        saved = await this.opts.save(this.opts.prepare ? this.opts.prepare(sending) : sending);
      } catch (e) {
        this.inFlight = null;
        // Later edits were built on top of the failed draft: revert everything.
        if (this.timer != null) clearTimeout(this.timer);
        this.timer = null;
        this.dirty = false;
        const all = [...waiters, ...this.waiters.splice(0)];
        this.set({ draft: this.server, phase: 'error', error: e instanceof Error ? e.message : String(e) });
        for (const w of all) w.reject(e);
        this.opts.onError?.(e);
        return;
      }
      // Clear before resolving so awaiting callers observe an idle controller.
      this.inFlight = null;
      this.server = saved;
      if (this.dirty) {
        this.set({ phase: 'pending', savedAt: Date.now() });
      } else {
        this.set({ draft: saved, phase: 'saved', savedAt: Date.now() });
        this.clearLinger();
        this.lingerTimer = setTimeout(() => {
          this.lingerTimer = null;
          if (this.snapshot.phase === 'saved') this.set({ phase: 'idle' });
        }, this.opts.savedLingerMs ?? 2000);
      }
      for (const w of waiters) w.resolve(saved);
    })();
    this.inFlight = flight;
    await flight;
    // A save that threw synchronously cleared `inFlight` before it was assigned.
    if (this.inFlight === flight) this.inFlight = null;
    if (this.dirty && this.timer == null) await this.run();
  }
}
