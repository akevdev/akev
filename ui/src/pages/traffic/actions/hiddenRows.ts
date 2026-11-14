/**
 * Rows hidden while a destructive action is undoable (DESIGN.md §1.6: "reversible").
 *
 * Deleting or clearing hides the rows immediately and defers the API call for the undo
 * window; Undo simply un-hides them. Committed rows stay hidden until the daemon's
 * `removed`/`cleared` message drops them from the traffic store, so nothing flashes back.
 */

import { useSyncExternalStore } from 'react';
import type { ExchangeId, ExchangeSummary } from '../../../api/types';

export interface HideEntry {
  readonly token: number;
  /** Explicit ids (delete). */
  readonly ids?: ReadonlySet<ExchangeId>;
  /** Every id ≤ `upTo` (clear), optionally sparing pinned rows. */
  readonly upTo?: ExchangeId;
  readonly keepPinned?: boolean;
}

export interface HiddenState {
  readonly entries: readonly HideEntry[];
  /** Increments on every change (memo key). */
  readonly version: number;
}

/** Whether a summary is hidden by any entry. */
export function isHidden(entries: readonly HideEntry[], s: Pick<ExchangeSummary, 'id' | 'pinned'>): boolean {
  for (const e of entries) {
    if (e.ids?.has(s.id)) return true;
    if (e.upTo != null && s.id <= e.upTo && !(e.keepPinned && s.pinned)) return true;
  }
  return false;
}

/**
 * Remove hidden ids from a display list. Returns the input array unchanged (same identity)
 * when nothing is hidden, so downstream memos stay valid.
 */
export function filterHidden(
  list: readonly ExchangeId[],
  entries: readonly HideEntry[],
  get: (id: ExchangeId) => Pick<ExchangeSummary, 'id' | 'pinned'> | undefined,
): readonly ExchangeId[] {
  if (entries.length === 0) return list;
  const out: ExchangeId[] = [];
  for (const id of list) {
    const s = get(id);
    if (!s || !isHidden(entries, s)) out.push(id);
  }
  return out.length === list.length ? list : out;
}

// ---------------------------------------------------------------------------- store

let state: HiddenState = { entries: [], version: 0 };
let nextToken = 1;
const listeners = new Set<() => void>();

function set(entries: readonly HideEntry[]): void {
  state = { entries, version: state.version + 1 };
  for (const l of listeners) l();
}

export function subscribeHidden(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function getHidden(): HiddenState {
  return state;
}

/** Hide rows; returns a token for {@link unhide}. */
export function hide(entry: Omit<HideEntry, 'token'>): number {
  const token = nextToken++;
  set([...state.entries, { ...entry, token }]);
  return token;
}

export function unhide(token: number): void {
  if (!state.entries.some((e) => e.token === token)) return;
  set(state.entries.filter((e) => e.token !== token));
}

/**
 * Keep an entry until none of its rows exist anymore (polls `exists` for up to `maxMs`), then
 * drop it. Used after a successful delete so rows don't reappear before the stream's
 * `removed` message arrives.
 */
export function unhideWhenGone(token: number, exists: (entry: HideEntry) => boolean, maxMs = 5000): void {
  const started = Date.now();
  const tick = () => {
    const entry = state.entries.find((e) => e.token === token);
    if (!entry) return;
    if (!exists(entry) || Date.now() - started > maxMs) unhide(token);
    else setTimeout(tick, 250);
  };
  setTimeout(tick, 150);
}

export function useHidden(): HiddenState {
  return useSyncExternalStore(subscribeHidden, getHidden, getHidden);
}
