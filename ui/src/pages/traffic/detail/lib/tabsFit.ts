/**
 * Which tabs fit in a tab strip of a given width (the rest go into a "More ▾" menu). The active
 * tab is always kept visible — it replaces the last tabs that fit when it would overflow — and
 * the original order is preserved.
 */

export interface FitOptions {
  /** Gap between adjacent tabs (px). */
  gap?: number;
  /** Horizontal padding of the strip (both sides, px). */
  padding?: number;
  /** Width reserved for the overflow button when some tabs don't fit (px). */
  more?: number;
}

/** Width of the strip showing `indices` (in px). */
export function stripWidth(widths: readonly number[], indices: readonly number[], gap: number, padding: number): number {
  let w = padding;
  for (let i = 0; i < indices.length; i++) w += (widths[indices[i] as number] ?? 0) + (i > 0 ? gap : 0);
  return w;
}

/**
 * Indices of the tabs to show, ascending. Returns every index when all tabs fit in `available`;
 * otherwise as many leading tabs as fit in `available - more`, with `active` (if ≥ 0) swapped in.
 */
export function fitTabs(widths: readonly number[], available: number, active: number, opts: FitOptions = {}): number[] {
  const { gap = 2, padding = 8, more = 0 } = opts;
  const all = widths.map((_, i) => i);
  if (widths.length === 0 || stripWidth(widths, all, gap, padding) <= available + 0.5) return all;

  const budget = available - more;
  const shown: number[] = [];
  for (let i = 0; i < widths.length; i++) {
    if (stripWidth(widths, [...shown, i], gap, padding) > budget + 0.5) break;
    shown.push(i);
  }
  if (active < 0 || active >= widths.length || shown.includes(active)) return shown.length ? shown : active >= 0 ? [active] : [0];

  // Swap the active tab in, dropping trailing tabs until it fits (it stays even if it alone doesn't).
  const withActive = [...shown, active];
  while (withActive.length > 1 && stripWidth(widths, withActive, gap, padding) > budget + 0.5) withActive.splice(withActive.length - 2, 1);
  return withActive.sort((a, b) => a - b);
}

/** Same result (order-sensitive) — lets callers skip redundant state updates. */
export function sameIndices(a: readonly number[] | null, b: readonly number[] | null): boolean {
  if (a === b) return true;
  if (!a || !b || a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}
