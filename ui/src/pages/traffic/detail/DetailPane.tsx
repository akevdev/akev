import { lazy, Suspense, useEffect } from 'react';
import { Spinner } from '../../../components/ui';
import { useTraffic } from '../../../state/traffic';
import { useUnrecordedPause } from '../unrecordedPause';
import { Placeholder } from './Placeholder';
import styles from './DetailPane.module.css';

// The exchange views (headers, bodies, timing, previews, compare…) are most of the Traffic
// page's code, but nothing needs them for the first paint: load them on demand, and in the
// background once the page is idle so the first selection opens without waiting.
const loadSingle = () => import('./SingleExchange');
const loadMulti = () => import('./MultiSelectPane');
const loadUnrecorded = () => import('./UnrecordedPausedExchange');
const SingleExchange = lazy(() => loadSingle().then((m) => ({ default: m.SingleExchange })));
const MultiSelectPane = lazy(() => loadMulti().then((m) => ({ default: m.MultiSelectPane })));
const UnrecordedPausedExchange = lazy(() => loadUnrecorded().then((m) => ({ default: m.UnrecordedPausedExchange })));

/** Start loading the detail views without rendering them. */
export function preloadDetailViews(): void {
  void loadSingle();
  void loadMulti();
  void loadUnrecorded();
}

function useIdlePreload(): void {
  useEffect(() => {
    const w = window as Window & { requestIdleCallback?: (cb: () => void, opts?: { timeout: number }) => number; cancelIdleCallback?: (id: number) => void };
    if (w.requestIdleCallback) {
      const id = w.requestIdleCallback(preloadDetailViews, { timeout: 2000 });
      return () => w.cancelIdleCallback?.(id);
    }
    const t = setTimeout(preloadDetailViews, 600);
    return () => clearTimeout(t);
  }, []);
}

function Loading() {
  return (
    <div className={styles.center}>
      <Spinner size={16} tone="muted" label="Loading exchange" />
    </div>
  );
}

/**
 * The Traffic detail pane (DESIGN.md §6.4–6.6). Reads the selection from `trafficStore` itself:
 * nothing selected → placeholder (or a paused exchange that isn't in the list, see
 * `unrecordedPause.ts`); one exchange → full detail; several → multi-select summary.
 *
 * CONTRACT: `TrafficPage` renders `<DetailPane />` (no props) inside the detail side of its
 * SplitPane, passing layout hints via `orientation` ('right' | 'bottom').
 *
 * The root is focusable (`#traffic-detail-pane`, `tabIndex=-1`) so the list can move focus into
 * the pane on Enter: `document.getElementById(DETAIL_PANE_ID)?.focus()`.
 */
export interface DetailPaneProps {
  /** Where the pane is docked (affects request/response split direction defaults). */
  orientation?: 'right' | 'bottom';
}

export const DETAIL_PANE_ID = 'traffic-detail-pane';

export function DetailPane({ orientation = 'right' }: DetailPaneProps) {
  // Primitive selectors: re-render only when the *shape* of the selection changes.
  const count = useTraffic((s) => s.selection.ids.size);
  const single = useTraffic((s) => (s.selection.ids.size === 1 ? (s.selection.ids.values().next().value ?? null) : null));
  const unrecorded = useUnrecordedPause((s) => s.id);
  useIdlePreload();

  return (
    <section id={DETAIL_PANE_ID} className={styles.pane} tabIndex={-1} aria-label="Exchange details" data-orientation={orientation}>
      <Suspense fallback={<Loading />}>
        {count === 0 ? (
          unrecorded != null ? (
            <UnrecordedPausedExchange id={unrecorded} orientation={orientation} />
          ) : (
            <Placeholder />
          )
        ) : single != null ? (
          <SingleExchange id={single} orientation={orientation} />
        ) : (
          <MultiSelectPane />
        )}
      </Suspense>
    </section>
  );
}
