import clsx from 'clsx';
import { ArrowDown, ArrowUp } from 'lucide-react';
import { memo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type PointerEvent as ReactPointerEvent } from 'react';
import { isMac } from '../../../lib/platform';
import { isHeaderColumn, parseHeaderColumn, type ColumnConfig, type SortState, type TrafficColumnId } from '../../../state/ui';
import { ariaSort, clampWidth, columnDef, dropTarget, isSortable } from './columns';
import styles from './TrafficList.module.css';

export interface ListHeaderProps {
  columns: readonly ColumnConfig[];
  sort: SortState | null;
  onSort: (column: TrafficColumnId) => void;
  /** Live (`commit=false`) and final widths while dragging a column edge. */
  onResize: (column: TrafficColumnId, width: number, commit: boolean) => void;
  onReorder: (from: TrafficColumnId, to: TrafficColumnId, place: 'before' | 'after') => void;
}

interface DragState {
  id: TrafficColumnId;
  pointerId: number;
  startX: number;
  dx: number;
  active: boolean;
  rects: { id: TrafficColumnId; left: number; width: number }[];
  drop: { id: TrafficColumnId; place: 'before' | 'after' } | null;
}

const DRAG_THRESHOLD = 4;
/** Keyboard resize step (px); with Alt held, a larger one. */
const KEY_STEP = 16;
const KEY_STEP_LARGE = 64;
const HELP_ID = 'traffic-header-help';

/**
 * Sticky column header: click to sort (asc → desc → none), drag a header to reorder, drag
 * its right edge to resize (double-click resets the width). Right-click opens the column
 * menu (handled by the list's context menu via `data-col`).
 *
 * Keyboard: the headers are one tab stop (roving). ←/→, Home/End move between columns,
 * Enter/Space sorts, Shift+←/→ resizes (Alt for bigger steps), Mod+Shift+←/→ moves the column,
 * ↓ goes to the rows.
 */
export const ListHeader = memo(function ListHeader({ columns, sort, onSort, onResize, onReorder }: ListHeaderProps) {
  const rowRef = useRef<HTMLDivElement | null>(null);
  const [drag, setDrag] = useState<DragState | null>(null);
  const dragRef = useRef<DragState | null>(null);
  const suppressClick = useRef(false);
  const [resizing, setResizing] = useState<TrafficColumnId | null>(null);
  const [focusCol, setFocusCol] = useState<TrafficColumnId | null>(null);
  const keyboardCols = columns.filter((c) => c.id !== 'indicator');
  const tabbable = keyboardCols.some((c) => c.id === focusCol) ? focusCol : (keyboardCols[0]?.id ?? null);

  const focusHeader = (id: TrafficColumnId | undefined) => {
    if (id) rowRef.current?.querySelector<HTMLButtonElement>(`[data-col="${id}"] button`)?.focus();
  };

  const onHeaderKeyDown = (e: ReactKeyboardEvent<HTMLButtonElement>, col: ColumnConfig) => {
    const i = keyboardCols.findIndex((c) => c.id === col.id);
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
      const dir = e.key === 'ArrowLeft' ? -1 : 1;
      if (e.shiftKey && mod) {
        const target = keyboardCols[i + dir];
        if (target) {
          onReorder(col.id, target.id, dir < 0 ? 'before' : 'after');
          // Reordering moves the DOM node, which drops focus.
          requestAnimationFrame(() => focusHeader(col.id));
        }
      } else if (e.shiftKey) {
        const step = e.altKey ? KEY_STEP_LARGE : KEY_STEP;
        onResize(col.id, clampWidth(col.id, col.width + dir * step), true);
      } else focusHeader(keyboardCols[i + dir]?.id);
    } else if (e.key === 'Home') focusHeader(keyboardCols[0]?.id);
    else if (e.key === 'End') focusHeader(keyboardCols[keyboardCols.length - 1]?.id);
    else if (e.key === 'ArrowDown') e.currentTarget.closest<HTMLElement>('[role="grid"]')?.focus();
    else return;
    e.preventDefault();
    e.stopPropagation();
  };

  const update = (next: DragState | null) => {
    dragRef.current = next;
    setDrag(next);
  };

  const measure = (): DragState['rects'] => {
    const row = rowRef.current;
    if (!row) return [];
    const out: DragState['rects'] = [];
    for (const el of row.querySelectorAll<HTMLElement>('[data-col]')) {
      out.push({ id: el.dataset.col as TrafficColumnId, left: el.offsetLeft, width: el.offsetWidth });
    }
    return out;
  };

  const onHeaderPointerDown = (e: ReactPointerEvent<HTMLButtonElement>, id: TrafficColumnId) => {
    if (e.button !== 0 || id === 'indicator') return;
    e.currentTarget.setPointerCapture(e.pointerId);
    suppressClick.current = false;
    update({ id, pointerId: e.pointerId, startX: e.clientX, dx: 0, active: false, rects: measure(), drop: null });
  };

  const onHeaderPointerMove = (e: ReactPointerEvent<HTMLButtonElement>) => {
    const d = dragRef.current;
    if (!d || d.pointerId !== e.pointerId) return;
    const dx = e.clientX - d.startX;
    const active = d.active || Math.abs(dx) > DRAG_THRESHOLD;
    if (!active) return;
    const rowLeft = rowRef.current?.getBoundingClientRect().left ?? 0;
    const drop = dropTarget(d.rects, e.clientX - rowLeft);
    update({ ...d, dx, active, drop: drop && drop.id !== d.id ? drop : null });
  };

  const onHeaderPointerUp = (e: ReactPointerEvent<HTMLButtonElement>) => {
    const d = dragRef.current;
    if (!d || d.pointerId !== e.pointerId) return;
    if (d.active) {
      suppressClick.current = true;
      if (d.drop) onReorder(d.id, d.drop.id, d.drop.place);
    }
    update(null);
  };

  const onResizePointerDown = (e: ReactPointerEvent<HTMLDivElement>, col: ColumnConfig) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const el = e.currentTarget;
    el.setPointerCapture(e.pointerId);
    const startX = e.clientX;
    const startW = (el.parentElement?.offsetWidth ?? col.width) || col.width;
    setResizing(col.id);
    document.documentElement.dataset.resizing = 'col';
    let last = col.width;
    const move = (ev: PointerEvent) => {
      last = clampWidth(col.id, startW + ev.clientX - startX);
      onResize(col.id, last, false);
    };
    const up = () => {
      el.removeEventListener('pointermove', move);
      el.removeEventListener('pointerup', up);
      el.removeEventListener('pointercancel', up);
      delete document.documentElement.dataset.resizing;
      setResizing(null);
      onResize(col.id, last, true);
    };
    el.addEventListener('pointermove', move);
    el.addEventListener('pointerup', up);
    el.addEventListener('pointercancel', up);
  };

  const dropX = (() => {
    if (!drag?.active || !drag.drop) return null;
    const r = drag.rects.find((c) => c.id === drag.drop!.id);
    if (!r) return null;
    return drag.drop.place === 'before' ? r.left : r.left + r.width;
  })();

  return (
    <div className={styles.headerGroup} role="rowgroup">
      <span id={HELP_ID} className="sr-only">
        Enter sorts. Shift plus Left or Right Arrow resizes the column; {isMac ? 'Command' : 'Control'} plus Shift plus Left or Right Arrow moves it.
      </span>
      {/* biome-ignore lint/a11y/useFocusableInteractive: grid row; focus goes to the header buttons (roving tab stop) */}
      <div ref={rowRef} className={clsx(styles.header, drag?.active && styles.headerDragging)} role="row" aria-rowindex={1}>
        {columns.map((col, i) => {
          const def = columnDef(col.id);
          const sortable = isSortable(col.id);
          const dir = sort?.column === col.id ? sort.direction : null;
          const dragging = drag?.active && drag.id === col.id;
          return (
            // biome-ignore lint/a11y/useFocusableInteractive: the button inside takes focus (roving tab stop)
            <div
              key={col.id}
              role="columnheader"
              aria-sort={sortable ? ariaSort(sort, col.id) : undefined}
              aria-label={col.id === 'indicator' ? 'Markers' : undefined}
              data-col={col.id}
              className={clsx(
                styles.headerCell,
                def.align === 'end' && styles.end,
                i === columns.length - 1 && styles.headerCellLast,
                dragging && styles.headerCellDragging,
                resizing === col.id && styles.headerCellResizing,
              )}
              style={dragging ? { transform: `translateX(${drag.dx}px)` } : undefined}
            >
              {col.id !== 'indicator' && (
                <button
                  type="button"
                  tabIndex={col.id === tabbable ? 0 : -1}
                  aria-describedby={HELP_ID}
                  className={clsx(styles.headerButton, dir && styles.headerSorted)}
                  onFocus={() => setFocusCol(col.id)}
                  onKeyDown={(e) => onHeaderKeyDown(e, col)}
                  onPointerDown={(e) => onHeaderPointerDown(e, col.id)}
                  onPointerMove={onHeaderPointerMove}
                  onPointerUp={onHeaderPointerUp}
                  onPointerCancel={() => update(null)}
                  onClick={() => {
                    if (suppressClick.current) {
                      suppressClick.current = false;
                      return;
                    }
                    if (sortable) onSort(col.id);
                  }}
                >
                  {isHeaderColumn(col.id) && (
                    <span className={styles.headerSide} title={parseHeaderColumn(col.id).side === 'request' ? 'Request header' : 'Response header'}>
                      {parseHeaderColumn(col.id).side === 'request' ? '↑' : '↓'}
                    </span>
                  )}
                  <span className={styles.headerLabel}>{def.label}</span>
                  {dir === 'asc' && <ArrowUp size={11} strokeWidth={2.25} className={styles.sortIcon} aria-hidden />}
                  {dir === 'desc' && <ArrowDown size={11} strokeWidth={2.25} className={styles.sortIcon} aria-hidden />}
                </button>
              )}
              {col.id !== 'indicator' && (
                <div
                  className={styles.resizer}
                  role="separator"
                  aria-orientation="vertical"
                  aria-label={`Resize ${def.label} column`}
                  onPointerDown={(e) => onResizePointerDown(e, col)}
                  onDoubleClick={() => onResize(col.id, def.defaultWidth, true)}
                />
              )}
            </div>
          );
        })}
        {dropX != null && <span className={styles.dropIndicator} style={{ transform: `translateX(${dropX - 1}px)` }} aria-hidden />}
      </div>
    </div>
  );
});
