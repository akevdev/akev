/**
 * Pretty-printing of large JSON bodies off the main thread (`pretty.worker.ts`), with a
 * deferred main-thread fallback when workers aren't available (CSP, file://…).
 */

import { formatJson, formatJsonLines } from '../model/body';

/** Pretty-printed text, or the parse error (the caller then shows the raw text). */
export type PrettyResult = { ok: true; text: string } | { ok: false; error: string };

type Pending = (r: PrettyResult) => void;

let worker: Worker | null | undefined;
let nextId = 1;
const pending = new Map<number, Pending>();

function onMainThread(text: string, lines: boolean): PrettyResult {
  if (lines) return { ok: true, text: formatJsonLines(text) };
  const f = formatJson(text);
  return f.ok ? { ok: true, text: f.text } : { ok: false, error: f.error };
}

function getWorker(): Worker | null {
  if (worker !== undefined) return worker;
  try {
    worker = new Worker(new URL('./pretty.worker.ts', import.meta.url), { type: 'module' });
    worker.onmessage = (e: MessageEvent<{ id: number; text?: string; error?: string }>) => {
      const resolve = pending.get(e.data.id);
      if (!resolve) return;
      pending.delete(e.data.id);
      resolve(e.data.text != null ? { ok: true, text: e.data.text } : { ok: false, error: e.data.error ?? 'Formatting failed' });
    };
    worker.onerror = () => {
      // A broken worker: nothing in flight will answer; let callers fall back.
      for (const resolve of pending.values()) resolve({ ok: false, error: 'worker-failed' });
      pending.clear();
      worker?.terminate();
      worker = null;
    };
  } catch {
    worker = null;
  }
  return worker;
}

/** Pretty-print JSON (or JSON Lines) in the worker; falls back to a deferred main-thread run. */
export function prettyAsync(text: string, lines: boolean): Promise<PrettyResult> {
  const w = getWorker();
  const fallback = () =>
    new Promise<PrettyResult>((resolve) => {
      // Yield a frame so the "Formatting…" state paints before the synchronous work.
      setTimeout(() => resolve(onMainThread(text, lines)), 16);
    });
  if (!w) return fallback();
  const id = nextId++;
  return new Promise<PrettyResult>((resolve) => {
    pending.set(id, resolve);
    w.postMessage({ id, text, lines });
  }).then((r) => (!r.ok && r.error === 'worker-failed' ? fallback() : r));
}
