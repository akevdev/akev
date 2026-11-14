/**
 * WebSocket tools state (zustand): messages paused at `ws-breakpoint` rules (fed by the stream:
 * `hello.pausedMessages`, `message-paused`, `message-resumed`) and, per exchange, the messages
 * this UI injected (composer history + which recorded `seq`s were injected, for row labels).
 */

import { create } from 'zustand';
import { useShallow } from 'zustand/react/shallow';
import { api } from '../api/client';
import type { Direction, ExchangeId, InjectMessageRequest, MessageResume, PausedMessage } from '../api/types';
import type { ComposerMode } from '../pages/traffic/detail/lib/wsCompose';

export type { ComposerMode };

/** A message sent from the composer. */
export interface InjectedEntry {
  /** Local id (stable React key). */
  key: number;
  direction: Direction;
  /** How the payload was entered (restored when re-using the entry). */
  mode: ComposerMode;
  /** What was typed. */
  input: string;
  /** Unix ms. */
  at: number;
  /** Recorded message seq; `null` while still queued behind a long message. */
  seq: number | null;
}

/** Most recent entries kept per exchange. */
export const HISTORY_LIMIT = 50;

export interface WsToolsState {
  /** Paused messages, oldest first. */
  paused: PausedMessage[];
  /** Injected messages per exchange, oldest first. */
  history: Record<ExchangeId, InjectedEntry[]>;
}

export const useWsToolsStore = create<WsToolsState>()(() => ({ paused: [], history: {} }));

let nextKey = 1;

// =====================================================================================
// Stream reducers (state/connection.ts)
// =====================================================================================

export const wsToolsReducers = {
  hello(paused: PausedMessage[]): void {
    useWsToolsStore.setState({ paused: [...paused].sort((a, b) => a.pauseId - b.pauseId) });
  },
  paused(message: PausedMessage): void {
    const list = useWsToolsStore.getState().paused.filter((p) => p.pauseId !== message.pauseId);
    useWsToolsStore.setState({ paused: [...list, message].sort((a, b) => a.pauseId - b.pauseId) });
  },
  resumed(pauseId: number): void {
    const list = useWsToolsStore.getState().paused;
    if (list.some((p) => p.pauseId === pauseId)) useWsToolsStore.setState({ paused: list.filter((p) => p.pauseId !== pauseId) });
  },
  /** Exchanges were removed / cleared: forget their history. */
  forget(ids: ExchangeId[] | 'all'): void {
    const { history } = useWsToolsStore.getState();
    if (ids === 'all') {
      if (Object.keys(history).length) useWsToolsStore.setState({ history: {} });
      return;
    }
    if (!ids.some((id) => id in history)) return;
    const next = { ...history };
    for (const id of ids) delete next[id];
    useWsToolsStore.setState({ history: next });
  },
};

function removePaused(match: (p: PausedMessage) => boolean): void {
  const list = useWsToolsStore.getState().paused;
  const next = list.filter((p) => !match(p));
  if (next.length !== list.length) useWsToolsStore.setState({ paused: next });
}

// =====================================================================================
// Actions
// =====================================================================================

/** Forward (possibly edited) or drop a paused message. */
export async function resumePausedMessage(pauseId: number, resume: MessageResume): Promise<void> {
  await api.breakpoints.resumeMessage(pauseId, resume);
  // The stream's `message-resumed` also removes it; do it now for snappiness.
  removePaused((p) => p.pauseId === pauseId);
}

/** Forward (or drop) every paused message of exchange `id`. */
export async function resumeAllPausedMessages(id: ExchangeId, action: 'forward' | 'drop'): Promise<number> {
  const { resumed } = await api.breakpoints.resumeAllMessages({ id, action });
  removePaused((p) => p.id === id);
  return resumed;
}

/** Inject a message and remember it in the exchange's history. */
export async function injectMessage(id: ExchangeId, request: InjectMessageRequest, mode: ComposerMode, input: string): Promise<InjectedEntry> {
  const { seq } = await api.exchanges.sendMessage(id, request);
  const entry: InjectedEntry = { key: nextKey++, direction: request.direction, mode, input, at: Date.now(), seq };
  const { history } = useWsToolsStore.getState();
  const list = [...(history[id] ?? []), entry].slice(-HISTORY_LIMIT);
  useWsToolsStore.setState({ history: { ...history, [id]: list } });
  return entry;
}

/** Forget the composer history of one exchange. */
export function clearInjectedHistory(id: ExchangeId): void {
  wsToolsReducers.forget([id]);
}

// =====================================================================================
// Selectors
// =====================================================================================

const EMPTY_PAUSED: PausedMessage[] = [];
const EMPTY_HISTORY: InjectedEntry[] = [];

/** Paused messages of one exchange (stable reference while unchanged). */
export function usePausedMessages(id: ExchangeId): PausedMessage[] {
  return useWsToolsStore(useShallow((s) => (s.paused.some((p) => p.id === id) ? s.paused.filter((p) => p.id === id) : EMPTY_PAUSED)));
}

export function useInjectedHistory(id: ExchangeId): InjectedEntry[] {
  return useWsToolsStore((s) => s.history[id] ?? EMPTY_HISTORY);
}

/** `seq`s of the messages this UI injected into exchange `id`. */
export function injectedSeqs(history: readonly InjectedEntry[]): ReadonlySet<number> {
  const out = new Set<number>();
  for (const e of history) if (e.seq != null) out.add(e.seq);
  return out;
}
