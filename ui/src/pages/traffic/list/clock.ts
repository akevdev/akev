/**
 * One shared clock for live "elapsed" timers of pending rows: a single requestAnimationFrame
 * loop (throttled to ~10 updates/s) that runs only while at least one row subscribes — no
 * per-row timers.
 */

import { useSyncExternalStore } from 'react';

const INTERVAL_MS = 100;

let now = Date.now();
let frame: number | null = null;
let last = 0;
const listeners = new Set<() => void>();

function loop(t: number): void {
  if (t - last >= INTERVAL_MS) {
    last = t;
    now = Date.now();
    for (const l of listeners) l();
  }
  frame = listeners.size > 0 ? requestAnimationFrame(loop) : null;
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  if (frame == null && typeof requestAnimationFrame === 'function') {
    now = Date.now();
    frame = requestAnimationFrame(loop);
  }
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && frame != null) {
      cancelAnimationFrame(frame);
      frame = null;
    }
  };
}

function getNow(): number {
  return now;
}

/** Current time in ms, updated ~10×/s while mounted. */
export function useNow(): number {
  return useSyncExternalStore(subscribe, getNow, getNow);
}
