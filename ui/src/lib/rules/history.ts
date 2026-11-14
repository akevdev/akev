/**
 * Undo/redo command stack for the Rules page.
 *
 * Entries store `before`/`after` snapshots (rule sets are immutable with structural
 * sharing, so snapshots are cheap). Consecutive pushes with the same `key` within
 * `coalesceMs` merge into one entry — e.g. a burst of typing in one rule becomes a single
 * "Edit rule" step — while any other push, undo or redo breaks the merge.
 */

export interface HistoryEntry<S> {
  id: number;
  /** Human label (`Delete rule`, `Move rule`…) shown in tooltips and toasts. */
  label: string;
  before: S;
  after: S;
  /** Coalescing key (`edit:<ruleId>`), if any. */
  key?: string;
  /** Item to select after undo/redo (e.g. the rule that was edited). */
  focus?: string | null;
  /** Last push time (ms). */
  at: number;
}

export interface PushInput<S> {
  label: string;
  before: S;
  after: S;
  key?: string;
  focus?: string | null;
}

export interface HistoryOptions {
  /** Max undo entries kept (default 200). */
  limit?: number;
  /** Merge window for same-key pushes in ms (default 1500). */
  coalesceMs?: number;
}

export class History<S> {
  private undoStack: HistoryEntry<S>[] = [];
  private redoStack: HistoryEntry<S>[] = [];
  private seq = 0;
  private sealed = false;
  private readonly limit: number;
  private readonly coalesceMs: number;
  private readonly listeners = new Set<() => void>();
  /** Bumps on every change (for `useSyncExternalStore`). */
  version = 0;

  constructor(options: HistoryOptions = {}) {
    this.limit = Math.max(1, options.limit ?? 200);
    this.coalesceMs = options.coalesceMs ?? 1500;
  }

  /** Record a change. Clears the redo stack. Returns the (possibly merged) entry. */
  push(input: PushInput<S>, now: number = Date.now()): HistoryEntry<S> {
    const top = this.undoStack[this.undoStack.length - 1];
    if (top && !this.sealed && input.key && top.key === input.key && now - top.at <= this.coalesceMs && this.redoStack.length === 0) {
      top.after = input.after;
      top.at = now;
      if (input.focus !== undefined) top.focus = input.focus;
      this.changed();
      return top;
    }
    const entry: HistoryEntry<S> = { id: ++this.seq, label: input.label, before: input.before, after: input.after, key: input.key, focus: input.focus, at: now };
    this.undoStack.push(entry);
    if (this.undoStack.length > this.limit) this.undoStack.splice(0, this.undoStack.length - this.limit);
    this.redoStack = [];
    this.sealed = false;
    this.changed();
    return entry;
  }

  /** Pop the latest entry onto the redo stack; the caller applies `entry.before`. */
  undo(): HistoryEntry<S> | null {
    const entry = this.undoStack.pop();
    if (!entry) return null;
    this.redoStack.push(entry);
    this.sealed = true;
    this.changed();
    return entry;
  }

  /** Re-apply the latest undone entry; the caller applies `entry.after`. */
  redo(): HistoryEntry<S> | null {
    const entry = this.redoStack.pop();
    if (!entry) return null;
    this.undoStack.push(entry);
    this.sealed = true;
    this.changed();
    return entry;
  }

  /** Stop the next push from merging into the current top entry. */
  seal(): void {
    this.sealed = true;
  }

  /** Remove an entry if it is the latest one (e.g. its save failed). Returns whether it was removed. */
  dropIfLatest(id: number): boolean {
    const top = this.undoStack[this.undoStack.length - 1];
    if (!top || top.id !== id) return false;
    this.undoStack.pop();
    this.changed();
    return true;
  }

  /** Whether the entry is the latest one (so undoing it undoes exactly that change). */
  isLatest(id: number): boolean {
    return this.undoStack[this.undoStack.length - 1]?.id === id;
  }

  clear(): void {
    this.undoStack = [];
    this.redoStack = [];
    this.changed();
  }

  get canUndo(): boolean {
    return this.undoStack.length > 0;
  }

  get canRedo(): boolean {
    return this.redoStack.length > 0;
  }

  /** Next entry `undo()` would revert. */
  peekUndo(): HistoryEntry<S> | undefined {
    return this.undoStack[this.undoStack.length - 1];
  }

  /** Next entry `redo()` would re-apply. */
  peekRedo(): HistoryEntry<S> | undefined {
    return this.redoStack[this.redoStack.length - 1];
  }

  get size(): { undo: number; redo: number } {
    return { undo: this.undoStack.length, redo: this.redoStack.length };
  }

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  getVersion = (): number => this.version;

  private changed(): void {
    this.version += 1;
    for (const l of this.listeners) l();
  }
}
