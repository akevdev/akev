/**
 * Pure column-layout logic for the traffic list (DESIGN.md §6.3): visible columns, the CSS
 * grid template shared by header and rows, sort cycling and drag-reorder.
 */

import { headerColumnDef, isHeaderColumn, parseHeaderColumn, TRAFFIC_COLUMNS, type ColumnConfig, type ColumnDef, type HeaderColumnId, type SortState, type TrafficColumnId } from '../../../state/ui';

const DEFS: ReadonlyMap<TrafficColumnId, ColumnDef> = new Map(TRAFFIC_COLUMNS.map((c) => [c.id, c]));

export function columnDef(id: TrafficColumnId): ColumnDef {
  return DEFS.get(id) ?? headerColumnDef(id as HeaderColumnId);
}

/** Human label for menus (the indicator has no header text). */
export function columnLabel(id: TrafficColumnId): string {
  if (id === 'indicator') return 'Indicator';
  if (id === 'id') return 'ID (#)';
  if (isHeaderColumn(id)) return `${columnDef(id).label} (${parseHeaderColumn(id).side} header)`;
  return columnDef(id).label;
}

/** Visible columns in display order. */
export function visibleColumns(columns: readonly ColumnConfig[]): ColumnConfig[] {
  return columns.filter((c) => c.visible);
}

/**
 * The column that absorbs spare width: `path` when visible, else `host`, else none (a
 * trailing filler track is added instead).
 */
export function flexColumn(cols: readonly ColumnConfig[]): TrafficColumnId | null {
  if (cols.some((c) => c.id === 'path')) return 'path';
  if (cols.some((c) => c.id === 'host')) return 'host';
  return null;
}

/** `grid-template-columns` for the visible columns. */
export function gridTemplate(cols: readonly ColumnConfig[]): string {
  const flex = flexColumn(cols);
  const tracks = cols.map((c) => (c.id === flex ? `minmax(${c.width}px, 1fr)` : `${c.width}px`));
  if (!flex) tracks.push('minmax(0, 1fr)');
  return tracks.join(' ');
}

/** Minimum content width (sum of column widths) — wider than the viewport scrolls horizontally. */
export function totalWidth(cols: readonly { width: number }[]): number {
  let w = 0;
  for (const c of cols) w += c.width;
  return w;
}

/** Whether a column can be sorted by clicking its header. */
export function isSortable(id: TrafficColumnId): boolean {
  return id !== 'indicator' && id !== 'remoteIp' && !isHeaderColumn(id);
}

/** Click on a header: none → asc → desc → none (a different column starts at asc). */
export function nextSort(current: SortState | null, column: TrafficColumnId): SortState | null {
  if (!isSortable(column)) return current;
  if (!current || current.column !== column) return { column, direction: 'asc' };
  if (current.direction === 'asc') return { column, direction: 'desc' };
  return null;
}

/** `aria-sort` value for a header. */
export function ariaSort(sort: SortState | null, column: TrafficColumnId): 'ascending' | 'descending' | 'none' {
  if (!sort || sort.column !== column) return 'none';
  return sort.direction === 'asc' ? 'ascending' : 'descending';
}

/**
 * Whether the sort is equivalent to natural id order (`null`, `id`/`started` ascending).
 * The traffic store's `visible` array is already in that order.
 */
export function isNaturalOrder(sort: SortState | null): boolean {
  return !sort || ((sort.column === 'id' || sort.column === 'started') && sort.direction === 'asc');
}

/** Newest-first sorts, where live traffic arrives at the top. */
export function isNewestFirst(sort: SortState | null): boolean {
  return !!sort && (sort.column === 'id' || sort.column === 'started') && sort.direction === 'desc';
}

/**
 * Move column `fromId` next to `toId` (before or after it) in the full column list (hidden
 * columns keep their relative positions). The indicator column stays first and can't move.
 * Returns the same array when nothing changes.
 */
export function moveColumnTo(columns: readonly ColumnConfig[], fromId: TrafficColumnId, toId: TrafficColumnId, place: 'before' | 'after'): readonly ColumnConfig[] {
  if (fromId === toId || fromId === 'indicator') return columns;
  const from = columns.findIndex((c) => c.id === fromId);
  if (from < 0 || !columns.some((c) => c.id === toId)) return columns;
  const without = columns.filter((c) => c.id !== fromId);
  let to = without.findIndex((c) => c.id === toId);
  if (place === 'after') to += 1;
  if (to < 1) to = 1; // never before the indicator
  const moved = columns[from] as ColumnConfig;
  const next = [...without.slice(0, to), moved, ...without.slice(to)];
  for (let i = 0; i < next.length; i++) if (next[i] !== columns[i]) return next;
  return columns;
}

/**
 * Drop target while dragging a header: the visible column under `x` (content coordinates)
 * and whether the dragged column goes before or after it.
 */
export function dropTarget(cols: readonly { id: TrafficColumnId; left: number; width: number }[], x: number): { id: TrafficColumnId; place: 'before' | 'after' } | null {
  for (const c of cols) {
    if (c.id === 'indicator') continue;
    if (x >= c.left && x < c.left + c.width) return { id: c.id, place: x < c.left + c.width / 2 ? 'before' : 'after' };
  }
  const movable = cols.filter((c) => c.id !== 'indicator');
  const first = movable[0];
  const last = movable[movable.length - 1];
  if (first && x < first.left) return { id: first.id, place: 'before' };
  if (last && x >= last.left + last.width) return { id: last.id, place: 'after' };
  return null;
}

/**
 * Shrink steps used when the columns don't fit: `[column, floor]`, applied in order. Only the
 * flexible path column and columns still at their default width take part, so widths the
 * user dragged are always respected.
 */
const FIT_STEPS: readonly (readonly [TrafficColumnId, number])[] = [
  ['path', 200],
  ['host', 150],
  ['source', 100],
  ['size', 68],
  ['time', 68],
  ['type', 68],
  ['path', 150],
  ['host', 124],
  ['source', 88],
  ['path', 120],
  ['host', 112],
];

/**
 * Columns left out, in this order, while the rest don't fit even at their floors, so a narrow
 * list doesn't scroll sideways. The column choice itself is unchanged: they come back as soon
 * as there's room. The indicator, method, status, host and path always stay.
 */
const HIDE_ORDER: readonly TrafficColumnId[] = ['source', 'started', 'remoteIp', 'protocol', 'rule', 'model', 'id', 'type', 'size', 'time'];

/** Apply {@link FIT_STEPS} until `cols` fit in `available` px (or every floor is reached). */
function shrinkColumns<T extends { id: TrafficColumnId; width: number }>(cols: readonly T[], available: number): readonly T[] {
  let deficit = totalWidth(cols) - available;
  if (deficit <= 0) return cols;
  const widths = new Map(cols.map((c) => [c.id, c.width]));
  const shrinkable = (id: TrafficColumnId) => {
    const c = cols.find((x) => x.id === id);
    if (!c) return false;
    return id === 'path' || c.width === columnDef(id).defaultWidth;
  };
  for (const [id, floor] of FIT_STEPS) {
    if (deficit <= 0) break;
    if (!shrinkable(id)) continue;
    const w = widths.get(id) as number;
    const cut = Math.min(deficit, Math.max(0, w - floor));
    if (cut > 0) {
      widths.set(id, w - cut);
      deficit -= cut;
    }
  }
  let changed = false;
  const out = cols.map((c) => {
    const w = widths.get(c.id) as number;
    if (w === c.width) return c;
    changed = true;
    return { ...c, width: w };
  });
  return changed ? out : cols;
}

/**
 * Fit the visible columns into `available` px (DevTools-like): when they are wider, shrink
 * the path, then untouched host/source columns down to comfortable floors; when that isn't
 * enough, leave out the least important columns ({@link HIDE_ORDER}), keeping the others as
 * wide as possible. Only a list too narrow for the essential columns scrolls horizontally.
 * Returns the input array when nothing changes.
 */
export function fitColumns<T extends { id: TrafficColumnId; width: number }>(cols: readonly T[], available: number): readonly T[] {
  if (!(available > 0) || totalWidth(cols) <= available) return cols;
  let set = cols;
  let fitted = shrinkColumns(set, available);
  // Columns the user added (custom headers) stay longer than the minor built-in ones.
  const added = cols.filter((c) => isHeaderColumn(c.id)).map((c) => c.id).reverse();
  const at = HIDE_ORDER.indexOf('type');
  const hideOrder = [...HIDE_ORDER.slice(0, at), ...added, ...HIDE_ORDER.slice(at)];
  for (const id of hideOrder) {
    if (totalWidth(fitted) <= available) break;
    if (!set.some((c) => c.id === id)) continue;
    set = set.filter((c) => c.id !== id);
    fitted = shrinkColumns(set, available);
  }
  return fitted;
}

/** Clamp a dragged width to the column's limits. */
export function clampWidth(id: TrafficColumnId, width: number): number {
  const def = columnDef(id);
  return Math.max(def.minWidth, Math.min(1200, Math.round(width)));
}
