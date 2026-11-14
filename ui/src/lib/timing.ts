/** Debounce / throttle helpers with explicit cancel & flush. */

export interface Debounced<A extends unknown[]> {
  (...args: A): void;
  /** Drop a pending call. */
  cancel(): void;
  /** Run a pending call immediately. */
  flush(): void;
  /** Whether a call is pending. */
  pending(): boolean;
}

/** Trailing-edge debounce. */
export function debounce<A extends unknown[]>(fn: (...args: A) => void, wait: number): Debounced<A> {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let lastArgs: A | null = null;
  const run = () => {
    timer = null;
    const args = lastArgs;
    lastArgs = null;
    if (args) fn(...args);
  };
  const d = ((...args: A) => {
    lastArgs = args;
    if (timer != null) clearTimeout(timer);
    timer = setTimeout(run, wait);
  }) as Debounced<A>;
  d.cancel = () => {
    if (timer != null) clearTimeout(timer);
    timer = null;
    lastArgs = null;
  };
  d.flush = () => {
    if (timer != null) {
      clearTimeout(timer);
      run();
    }
  };
  d.pending = () => timer != null;
  return d;
}

/** Leading + trailing throttle: runs at most once per `wait` ms, always with the latest args. */
export function throttle<A extends unknown[]>(fn: (...args: A) => void, wait: number): Debounced<A> {
  let last = 0;
  let timer: ReturnType<typeof setTimeout> | null = null;
  let lastArgs: A | null = null;
  const run = () => {
    timer = null;
    last = Date.now();
    const args = lastArgs;
    lastArgs = null;
    if (args) fn(...args);
  };
  const t = ((...args: A) => {
    lastArgs = args;
    const remaining = wait - (Date.now() - last);
    if (remaining <= 0) {
      if (timer != null) clearTimeout(timer);
      run();
    } else if (timer == null) {
      timer = setTimeout(run, remaining);
    }
  }) as Debounced<A>;
  t.cancel = () => {
    if (timer != null) clearTimeout(timer);
    timer = null;
    lastArgs = null;
  };
  t.flush = () => {
    if (timer != null) {
      clearTimeout(timer);
      run();
    }
  };
  t.pending = () => timer != null;
  return t;
}
