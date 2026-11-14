/**
 * Fixed-row-height virtualisation maths for the traffic list. Every row has the same height,
 * so the rendered window is pure arithmetic: O(1) per scroll and per new row, with no
 * per-item measurement cache (a generic virtualiser rebuilds one entry per row whenever the
 * count changes — 100k objects per live batch).
 *
 * Coordinates: the canvas starts with a sticky header of `headerH` px, row `i` occupies
 * `[headerH + i·rowH, headerH + (i+1)·rowH)`.
 */

export interface RowWindow {
  /** First rendered row (inclusive). */
  start: number;
  /** Last rendered row (exclusive). */
  end: number;
}

/** Index of the first row at least partly visible below the sticky header. */
export function firstVisibleRow(scrollTop: number, rowH: number): number {
  if (!(rowH > 0)) return 0;
  return Math.max(0, Math.floor(scrollTop / rowH));
}

/** Rows that fit in the viewport below the header (+1 for a partially visible row). */
export function rowsInView(viewportH: number, rowH: number, headerH: number): number {
  if (!(rowH > 0)) return 1;
  return Math.ceil(Math.max(0, viewportH - headerH) / rowH) + 1;
}

/** Rendered window around `first`, `overscan` rows on each side, clamped to `count`. */
export function rowWindow(first: number, viewportH: number, rowH: number, headerH: number, count: number, overscan: number): RowWindow {
  if (count <= 0) return { start: 0, end: 0 };
  const f = Math.min(Math.max(0, first), count - 1);
  const start = Math.max(0, f - overscan);
  const end = Math.min(count, f + rowsInView(viewportH, rowH, headerH) + overscan);
  return { start, end };
}

/**
 * Number of row slots (React keys) needed so any rendered window maps contiguous indices to
 * distinct slots (`index % pool`). Rows are recycled through slots: scrolling updates a slot's
 * content in place instead of unmounting one row and mounting another.
 */
export function slotPool(viewportH: number, rowH: number, headerH: number, overscan: number): number {
  return rowsInView(viewportH, rowH, headerH) + 2 * overscan + 1;
}

/**
 * Whether the rows actually on screen (no overscan) for `first` are inside the rendered
 * window — if not, the next paint would show blank space, so the update must be synchronous.
 */
export function windowCovers(win: RowWindow, first: number, viewportH: number, rowH: number, headerH: number, count: number): boolean {
  if (count <= 0) return true;
  const f = Math.min(Math.max(0, first), count - 1);
  const last = Math.min(count, f + rowsInView(viewportH, rowH, headerH));
  return f >= win.start && last <= win.end;
}

/** Total scroll height of the canvas. */
export function canvasHeight(count: number, rowH: number, headerH: number): number {
  return headerH + Math.max(0, count) * rowH;
}
