import { useEffect, useLayoutEffect, useRef, useState, type RefObject } from 'react';

/** Width of an element, tracked with a ResizeObserver (0 until measured). */
export function useElementWidth<T extends HTMLElement>(): [RefObject<T | null>, number] {
  const ref = useRef<T | null>(null);
  const [width, setWidth] = useState(0);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    setWidth(el.getBoundingClientRect().width);
    const ro = new ResizeObserver(([entry]) => {
      if (entry) setWidth(entry.contentRect.width);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return [ref, width];
}

/** `Date.now()` re-rendered every `intervalMs` while `enabled`. */
export function useNow(intervalMs = 1000, enabled = true): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!enabled) return;
    setNow(Date.now());
    const t = setInterval(() => setNow(Date.now()), intervalMs);
    return () => clearInterval(t);
  }, [intervalMs, enabled]);
  return now;
}

/**
 * Whether `el`'s content overflows its box vertically (for "show more" toggles). Re-checked on
 * resize and whenever `deps` change.
 */
export function useOverflow<T extends HTMLElement>(deps: unknown[]): [RefObject<T | null>, boolean] {
  const ref = useRef<T | null>(null);
  const [overflow, setOverflow] = useState(false);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const check = () => setOverflow(el.scrollHeight > el.clientHeight + 1);
    check();
    const ro = new ResizeObserver(check);
    ro.observe(el);
    return () => ro.disconnect();
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, deps);
  return [ref, overflow];
}
