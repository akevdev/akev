/**
 * Hooks for TextDiff: the line diff (synchronous for small inputs, in a Web Worker for large
 * ones) and lazily computed syntax highlighting.
 */

import { useEffect, useMemo, useState } from 'react';
import { diffText, type DiffOptions, type TextDiffResult } from '../../../lib/diff';
import { highlightLines } from './highlight';
import type { LineToken } from './pieces';

/** Combined input size above which the diff runs off the main thread. */
export const WORKER_THRESHOLD = 160_000;

type Pending = { resolve: (r: TextDiffResult) => void; reject: (e: Error) => void };

let worker: Worker | null | undefined;
let nextId = 1;
const pending = new Map<number, Pending>();

function getWorker(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL('./diff.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (e: MessageEvent<{ id: number; result?: TextDiffResult; error?: string }>) => {
      const p = pending.get(e.data.id);
      if (!p) return;
      pending.delete(e.data.id);
      if (e.data.result) p.resolve(e.data.result);
      else p.reject(new Error(e.data.error ?? 'Diff failed'));
    };
    worker.onerror = () => {
      // A broken worker (CSP, file:// …): fail everything in flight and fall back to timers.
      for (const p of pending.values()) p.reject(new Error('Diff worker failed'));
      pending.clear();
      worker?.terminate();
      worker = null;
    };
  } catch {
    worker = null;
  }
  return worker;
}

/** Diff off the main thread when possible (falls back to a deferred main-thread diff). */
export function diffAsync(left: string, right: string, opts: DiffOptions): Promise<TextDiffResult> {
  const w = getWorker();
  if (!w) {
    return new Promise((resolve, reject) => {
      setTimeout(() => {
        try {
          resolve(diffText(left, right, opts));
        } catch (e) {
          reject(e instanceof Error ? e : new Error(String(e)));
        }
      }, 16);
    });
  }
  const id = nextId++;
  return new Promise<TextDiffResult>((resolve, reject) => {
    pending.set(id, { resolve, reject });
    w.postMessage({ id, left, right, opts });
  }).catch(() => diffText(left, right, opts));
}

export interface TextDiffState {
  result: TextDiffResult | null;
  /** A large diff is being computed off-thread. */
  pending: boolean;
}

/**
 * Line diff of two texts. Small inputs are diffed synchronously (no flash); large ones in a
 * worker, keeping the previous result visible until the new one arrives.
 */
export function useTextDiff(left: string, right: string, ignoreWhitespace: boolean, enabled = true): TextDiffState {
  const big = left.length + right.length > WORKER_THRESHOLD;
  const sync = useMemo(() => (enabled && !big ? diffText(left, right, { ignoreWhitespace }) : null), [enabled, big, left, right, ignoreWhitespace]);
  const [async, setAsync] = useState<{ left: string; right: string; ws: boolean; result: TextDiffResult } | null>(null);
  useEffect(() => {
    if (!enabled || !big) return;
    let cancelled = false;
    void diffAsync(left, right, { ignoreWhitespace }).then((result) => {
      if (!cancelled) setAsync({ left, right, ws: ignoreWhitespace, result });
    });
    return () => {
      cancelled = true;
    };
  }, [enabled, big, left, right, ignoreWhitespace]);
  if (!enabled) return { result: null, pending: false };
  if (!big) return { result: sync, pending: false };
  const fresh = async != null && async.left === left && async.right === right && async.ws === ignoreWhitespace;
  return { result: async?.result ?? null, pending: !fresh };
}

/** Per-line syntax tokens for one side (null until loaded / when not highlightable). */
export function useHighlight(text: string, language: string | undefined, enabled: boolean): LineToken[][] | null {
  const [state, setState] = useState<{ text: string; language: string | undefined; tokens: LineToken[][] | null } | null>(null);
  useEffect(() => {
    if (!enabled || !language) return;
    let cancelled = false;
    void highlightLines(text, language).then((tokens) => {
      if (!cancelled) setState({ text, language, tokens });
    });
    return () => {
      cancelled = true;
    };
  }, [text, language, enabled]);
  if (!enabled || !state || state.text !== text || state.language !== language) return null;
  return state.tokens;
}
