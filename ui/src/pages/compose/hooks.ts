import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type RefObject } from 'react';
import { useComposeStore } from './store';
import { resolveEnvVars, type VarMap } from './variables';

/** Current time, refreshed every `intervalMs` (relative timestamps). */
export function useNow(intervalMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [intervalMs]);
  return now;
}

/** `value`, updated at most every `ms` after it stops changing. */
export function useDebouncedValue<T>(value: T, ms: number): T {
  const [v, setV] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setV(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return v;
}

/** Effective variables (globals + active environment). */
export function useVars(): VarMap {
  const env = useComposeStore((s) => s.env);
  return useMemo(() => resolveEnvVars(env), [env]);
}

/** Width of an element (ResizeObserver). */
export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => entry && setWidth(entry.contentRect.width));
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return width;
}

/** Milliseconds since `start`, ticking every animation-ish interval while `active`. */
export function useElapsed(start: number | undefined, active: boolean, intervalMs = 100): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!active) return;
    setNow(Date.now());
    const id = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(id);
  }, [active, intervalMs]);
  return start == null ? 0 : Math.max(0, now - start);
}

/** A callback with a stable identity that always runs the latest closure. */
export function useStableCallback<A extends unknown[], R>(fn: (...args: A) => R): (...args: A) => R {
  const ref = useRef(fn);
  useLayoutEffect(() => {
    ref.current = fn;
  });
  return useCallback((...args: A) => ref.current(...args), []);
}
