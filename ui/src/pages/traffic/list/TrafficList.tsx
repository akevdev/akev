import clsx from 'clsx';
import { ArrowDown, ArrowUp } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent, type ReactElement } from 'react';
import { flushSync } from 'react-dom';
import type { ExchangeId } from '../../../api/types';
import { useShortcut, useShortcutDocs } from '../../../app/shortcuts';
import { ContextMenu, Skeleton, type MenuItems } from '../../../components/ui';
import { formatNumber } from '../../../lib/format';
import { isMac } from '../../../lib/platform';
import { indexInList, trafficStore, useTraffic } from '../../../state/traffic';
import { useRowHeight, useUiStore, type TrafficColumnId } from '../../../state/ui';
import { filterHidden, useHidden } from '../actions/hiddenRows';
import { loadCodegenTargets } from '../actions/codegenTargets';
import { copyAsSnippet, copyUrls, deleteExchanges, openInComposeFor, replay, selectedIds, togglePin } from '../actions/exchangeActions';
import { columnItems, listMenu, multiRowMenu, singleRowMenu } from '../actions/menus';
import { EmptyTraffic, NoMatches } from '../empty/EmptyStates';
import { trafficView } from '../trafficView';
import { fitColumns, gridTemplate, isNaturalOrder, isNewestFirst, moveColumnTo, nextSort, totalWidth, visibleColumns } from './columns';
import { GridTooltip } from './GridTooltip';
import { setHeaderColumns } from './HeaderCell';
import { ListHeader } from './ListHeader';
import { countNewer } from './liveTail';
import { TrafficRow } from './TrafficRow';
import { useDisplayList } from './useDisplayList';
import { canvasHeight, firstVisibleRow, rowWindow, slotPool, windowCovers, type RowWindow } from './virtual';
import { smoothScroll } from '../../../lib/motion';
import styles from './TrafficList.module.css';

/** Column header height (px). */
const HEADER_H = 28;
/** The indicator column renders wide enough for the pin / pause glyph. */
const INDICATOR_W = 20;
const OVERSCAN = 20;

type MenuTarget = { kind: 'rows' } | { kind: 'header'; column: TrafficColumnId } | { kind: 'list' };

/** Focus is in the list or nowhere in particular (so list shortcuts may act). */
function focusInListOrBody(scroller: HTMLElement | null): boolean {
  const a = document.activeElement;
  return !a || a === document.body || (scroller != null && scroller.contains(a));
}

function hasTextSelection(): boolean {
  const sel = window.getSelection();
  return sel != null && !sel.isCollapsed && sel.toString().length > 0;
}

/**
 * The virtualised traffic list (DESIGN.md §6.3): sticky sortable/resizable/reorderable
 * header, memoised rows, keyboard selection, context menus, live tail with a "N new" pill,
 * and the empty states.
 */
export function TrafficList() {
  const { list, natural } = useDisplayList();
  const rowH = useRowHeight();
  const columns = useUiStore((s) => s.columns);
  const sort = useUiStore((s) => s.sort);
  const monoPaths = useUiStore((s) => s.monoPaths);
  const loaded = useTraffic((s) => s.loaded);
  const filterActive = useTraffic((s) => s.filter.active);
  const activeId = useTraffic((s) => s.selection.active);
  const hidden = useHidden();
  // Total exchanges not hidden for undo (decides "no traffic" vs "no matches").
  const order = useTraffic((s) => (hidden.entries.length ? s.order : null));
  const orderLength = useTraffic((s) => s.order.length);
  const total = useMemo(() => (order ? filterHidden(order, hidden.entries, (id) => trafficStore.get(id)).length : orderLength), [order, orderLength, hidden]);

  const ascending = isNaturalOrder(sort);
  const tailMode: 'bottom' | 'top' | null = ascending ? 'bottom' : isNewestFirst(sort) ? 'top' : null;

  // ---- columns (live widths while dragging an edge; fitted to the pane width)
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [liveWidths, setLiveWidths] = useState<Partial<Record<TrafficColumnId, number>>>({});
  const [paneWidth, setPaneWidth] = useState(0);
  const [viewportH, setViewportH] = useState(0);
  const viewportHRef = useRef(0);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const read = () => {
      viewportHRef.current = el.clientHeight;
      setPaneWidth(el.clientWidth);
      setViewportH(el.clientHeight);
    };
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  const visibleCols = useMemo(() => {
    const cols = visibleColumns(columns).map((c) => {
      const w = c.id === 'indicator' ? Math.max(c.width, INDICATOR_W) : (liveWidths[c.id] ?? c.width);
      return w === c.width ? c : { ...c, width: w };
    });
    // 2px of slack: WebKitGTK rounds sub-pixel column widths up and can report the vertical
    // scrollbar late, which would otherwise produce a 1–2px horizontal scrollbar.
    return fitColumns(cols, paneWidth > 0 ? paneWidth - 2 : paneWidth);
  }, [columns, liveWidths, paneWidth]);
  // Custom header columns: fetch only the headers that are shown.
  useEffect(() => setHeaderColumns(visibleColumns(columns).map((c) => c.id)), [columns]);
  const idsKey = visibleCols.map((c) => c.id).join(',');
  const columnIds = useMemo(() => idsKey.split(',') as TrafficColumnId[], [idsKey]);
  const template = gridTemplate(visibleCols);
  const contentWidth = totalWidth(visibleCols);

  const onSort = useCallback((id: TrafficColumnId) => {
    const ui = useUiStore.getState();
    ui.setSort(nextSort(ui.sort, id));
  }, []);
  const onResize = useCallback((id: TrafficColumnId, width: number, commit: boolean) => {
    if (commit) {
      useUiStore.getState().setColumnWidth(id, width);
      setLiveWidths((w) => {
        const { [id]: _, ...rest } = w;
        return rest;
      });
    } else setLiveWidths((w) => (w[id] === width ? w : { ...w, [id]: width }));
  }, []);
  const onReorder = useCallback((from: TrafficColumnId, to: TrafficColumnId, place: 'before' | 'after') => {
    const ui = useUiStore.getState();
    const next = moveColumnTo(ui.columns, from, to, place);
    if (next !== ui.columns) ui.setColumns([...next]);
  }, []);

  // ---- virtualisation (fixed row height: pure arithmetic, rows recycled through slots)
  const listRef = useRef(list);
  listRef.current = list;
  const rowHRef = useRef(rowH);
  rowHRef.current = rowH;
  const tailModeRef = useRef(tailMode);
  tailModeRef.current = tailMode;
  const [first, setFirst] = useState(0);
  const firstRef = useRef(0);
  const win = rowWindow(first, viewportH, rowH, HEADER_H, list.length, OVERSCAN);
  const winRef = useRef<RowWindow>(win);
  winRef.current = win;
  const pool = slotPool(viewportH, rowH, HEADER_H, OVERSCAN);
  const totalH = canvasHeight(list.length, rowH, HEADER_H);
  const totalHRef = useRef(totalH);
  totalHRef.current = totalH;

  /**
   * Re-derive the first visible row from the scroll position. Outside effects, a jump past
   * the rendered window renders synchronously (no blank frame); small moves are batched.
   */
  const syncFirst = useCallback((inEffect: boolean) => {
    const el = scrollRef.current;
    if (!el) return;
    const f = firstVisibleRow(el.scrollTop, rowHRef.current);
    if (f === firstRef.current) return;
    firstRef.current = f;
    if (inEffect || windowCovers(winRef.current, f, viewportHRef.current, rowHRef.current, HEADER_H, listRef.current.length)) setFirst(f);
    else flushSync(() => setFirst(f));
  }, []);

  useLayoutEffect(() => trafficView.setList(list, natural), [list, natural]);

  // ---- live tail & scroll anchoring
  const atTail = useRef(true);
  const anchor = useRef<{ id: ExchangeId; offset: number } | null>(null);
  const [leftAtMaxId, setLeftAtMaxId] = useState<number | null>(null);

  const newestId = (): number => {
    const o = trafficStore.getSnapshot().order;
    return (o[o.length - 1] as number | undefined) ?? 0;
  };

  const setTail = useCallback((value: boolean) => {
    if (atTail.current === value) return;
    atTail.current = value;
    setLeftAtMaxId(value ? null : newestId());
  }, []);

  const updateAnchor = () => {
    const el = scrollRef.current;
    if (!el) return;
    const i = Math.max(0, Math.floor(el.scrollTop / rowH));
    const id = listRef.current[i];
    anchor.current = id == null ? null : { id, offset: el.scrollTop - i * rowH };
  };

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el) return;
    // Arithmetic instead of scrollHeight/clientHeight: no forced layout per scroll event.
    const top = el.scrollTop;
    if (tailMode === 'bottom') setTail(totalHRef.current - top - viewportHRef.current <= rowH / 2);
    else if (tailMode === 'top') setTail(top <= rowH / 2);
    else setTail(false);
    updateAnchor();
    syncFirst(false);
  };

  const onScrollRef = useRef(onScroll);
  onScrollRef.current = onScroll;

  const scrollToTail = useCallback(
    (smooth = false) => {
      const el = scrollRef.current;
      if (!el) return;
      const top = tailMode === 'top' ? 0 : totalHRef.current;
      if (smooth) el.scrollTo({ top, behavior: smoothScroll() });
      else {
        el.scrollTop = top;
        syncFirst(false);
      }
      atTail.current = true;
      setLeftAtMaxId(null);
    },
    [tailMode, syncFirst],
  );

  // After every list change: stick to the live edge, or keep the top row where it was.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    if (atTail.current && tailMode) {
      el.scrollTop = tailMode === 'top' ? 0 : totalHRef.current;
      syncFirst(true);
      return;
    }
    const a = anchor.current;
    if (a) {
      const idx = indexInList(list, a.id, ascending);
      if (idx >= 0) {
        const target = idx * rowH + a.offset;
        if (Math.abs(el.scrollTop - target) >= 1) el.scrollTop = target;
      }
    }
    syncFirst(true);
  }, [list, tailMode, ascending, rowH, syncFirst]);

  // A new sort direction starts at its live edge.
  useEffect(() => {
    atTail.current = tailMode != null;
    setLeftAtMaxId(null);
    if (tailMode) requestAnimationFrame(() => scrollToTail());
    else scrollRef.current?.scrollTo({ top: 0 });
  }, [tailMode, sort?.column, scrollToTail]);

  // When a filter result lands, keep the focused row in view (or show the newest matches).
  const filterSettled = useTraffic((s) => (s.filter.pending ? -1 : s.filter.seq));
  const firstSettle = useRef(true);
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs only once the filter settles
  useEffect(() => {
    if (filterSettled < 0) return;
    if (firstSettle.current) {
      firstSettle.current = false;
      return;
    }
    const raf = requestAnimationFrame(() => {
      const active = trafficStore.getSnapshot().selection.active;
      if (active != null && indexInList(listRef.current, active, ascending) >= 0) trafficView.scrollToId(active, 'center');
      else if (tailMode) scrollToTail();
    });
    return () => cancelAnimationFrame(raf);
  }, [filterSettled]);

  const newCount = useMemo(() => (leftAtMaxId == null || !tailMode ? 0 : countNewer(list, leftAtMaxId, tailMode === 'top')), [list, leftAtMaxId, tailMode]);

  // ---- imperative handle for the page
  useEffect(() => {
    trafficView.registerList({
      // Fixed row heights make this exact (and synchronous — no retry loop that could fight
      // the live tail).
      scrollToId(id, align = 'auto') {
        const el = scrollRef.current;
        const idx = indexInList(listRef.current, id, isNaturalOrder(useUiStore.getState().sort));
        if (!el || idx < 0) return;
        const h = rowHRef.current;
        const rowTop = HEADER_H + idx * h;
        const viewTop = el.scrollTop + HEADER_H;
        const vh = viewportHRef.current || el.clientHeight;
        const viewBottom = el.scrollTop + vh;
        let next: number | null = null;
        if (align === 'center') next = rowTop - HEADER_H - (vh - HEADER_H - h) / 2;
        else if (rowTop < viewTop) next = rowTop - HEADER_H;
        else if (rowTop + h > viewBottom) next = rowTop + h - vh;
        if (next == null) return;
        el.scrollTop = Math.max(0, Math.round(next));
        onScrollRef.current();
      },
      reveal(id) {
        const list = listRef.current;
        const mode = tailModeRef.current;
        const edge = mode === 'bottom' ? list[list.length - 1] : mode === 'top' ? list[0] : undefined;
        if (mode && id !== edge) setTail(false);
        trafficView.scrollToId(id);
      },
      focus() {
        scrollRef.current?.focus({ preventScroll: true });
      },
    });
    return () => trafficView.registerList(null);
  }, [setTail]);

  useEffect(() => {
    loadCodegenTargets().catch(() => undefined);
  }, []);

  // ---- keyboard
  const pageRows = () => Math.max(1, Math.floor(((viewportHRef.current || 400) - HEADER_H) / rowH) - 1);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.target !== e.currentTarget && (e.target as HTMLElement).closest('button, input')) return;
    const mod = isMac ? e.metaKey : e.ctrlKey;
    let handled = true;
    switch (e.key) {
      case 'ArrowDown':
        trafficView.move(1, e.shiftKey);
        break;
      case 'ArrowUp':
        trafficView.move(-1, e.shiftKey);
        break;
      case 'Home':
        trafficView.move(-Infinity, e.shiftKey);
        break;
      case 'End':
        trafficView.move(Infinity, e.shiftKey);
        break;
      case 'PageDown':
        trafficView.move(pageRows(), e.shiftKey);
        break;
      case 'PageUp':
        trafficView.move(-pageRows(), e.shiftKey);
        break;
      case 'Enter':
        if (trafficStore.getSnapshot().selection.ids.size > 0) trafficView.focusDetail();
        else handled = false;
        break;
      case 'a':
      case 'A':
        if (mod && !e.altKey && !e.shiftKey) trafficStore.selectAll(trafficView.listArg());
        else handled = false;
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  // Shortcuts that act on the selection from anywhere on the page (not while typing).
  const withSelection = (fn: (ids: ExchangeId[]) => void) => () => {
    const ids = selectedIds();
    if (ids.length === 0) return false;
    fn(ids);
    return undefined;
  };
  useShortcut('j', () => trafficView.move(1), { description: 'Next exchange', group: 'Traffic list' });
  useShortcut('k', () => trafficView.move(-1), { description: 'Previous exchange', group: 'Traffic list' });
  useShortcut('shift+j', () => trafficView.move(1, true));
  useShortcut('shift+k', () => trafficView.move(-1, true));
  useShortcut('mod+a', () => {
    if (!focusInListOrBody(scrollRef.current)) return false;
    trafficStore.selectAll(trafficView.listArg());
    return undefined;
  });
  useShortcut(
    'escape',
    () => {
      if (trafficStore.getSnapshot().selection.ids.size === 0) return false;
      trafficStore.clearSelection();
      return undefined;
    },
    { description: 'Deselect', group: 'Traffic list' },
  );
  useShortcut('delete', () => (focusInListOrBody(scrollRef.current) ? withSelection(deleteExchanges)() : false), { description: 'Delete selected (undoable)', group: 'Traffic list' });
  useShortcut('backspace', () => (focusInListOrBody(scrollRef.current) ? withSelection(deleteExchanges)() : false));
  useShortcut('mod+p', withSelection((ids) => void togglePin(ids)), { description: 'Pin / unpin', group: 'Traffic list' });
  useShortcut('mod+r', withSelection((ids) => void replay(ids)), { description: 'Replay', group: 'Traffic list' });
  useShortcut(
    'mod+e',
    withSelection((ids) => void openInComposeFor(ids[0] as number)),
    { description: 'Edit in Compose', group: 'Traffic list' },
  );
  useShortcut('mod+shift+c', withSelection((ids) => void copyAsSnippet(ids, 'curl')), { description: 'Copy as cURL', group: 'Traffic list' });
  useShortcut(
    'mod+c',
    () => {
      if (hasTextSelection()) return false;
      return withSelection((ids) => void copyUrls(ids))();
    },
    { description: 'Copy URL(s)', group: 'Traffic list' },
  );
  useShortcutDocs([
    { combo: 'arrowdown or arrowup', description: 'Move selection', group: 'Traffic list' },
    { combo: 'shift+arrowdown or shift+arrowup', description: 'Extend selection', group: 'Traffic list' },
    { combo: 'home or end', description: 'First / last exchange', group: 'Traffic list' },
    { combo: 'pageup or pagedown', description: 'Page up / down', group: 'Traffic list' },
    { combo: 'mod+a', description: 'Select all visible', group: 'Traffic list' },
    { combo: 'enter', description: 'Focus the detail pane', group: 'Traffic list' },
    { combo: 'shift+arrowleft or shift+arrowright', description: 'Column header: narrow / widen the column', group: 'Traffic list' },
    { combo: 'mod+shift+arrowleft or mod+shift+arrowright', description: 'Column header: move the column', group: 'Traffic list' },
  ]);

  // ---- mouse
  const onMouseDown = (e: MouseEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    const row = (e.target as Element).closest<HTMLElement>('[data-row-id]');
    if (!row) return;
    const id = Number(row.dataset.rowId);
    const mod = isMac ? e.metaKey : e.ctrlKey;
    if (e.shiftKey) {
      e.preventDefault(); // no text selection; keep focus handling below
      scrollRef.current?.focus({ preventScroll: true });
      trafficStore.extendTo(id, trafficView.listArg());
    } else if (mod) trafficStore.toggle(id);
    else trafficStore.select(id);
    trafficView.reveal(id);
  };

  const menuTarget = useRef<MenuTarget>({ kind: 'list' });
  const onContextMenuCapture = (e: MouseEvent<HTMLDivElement>) => {
    const target = e.target as Element;
    const col = target.closest<HTMLElement>('[data-col]');
    if (col) {
      menuTarget.current = { kind: 'header', column: col.dataset.col as TrafficColumnId };
      return;
    }
    const row = target.closest<HTMLElement>('[data-row-id]');
    if (row) {
      const id = Number(row.dataset.rowId);
      if (!trafficStore.getSnapshot().selection.ids.has(id)) trafficStore.select(id);
      // Keep the row still under the open menu (stop following live traffic).
      trafficView.reveal(id);
      menuTarget.current = { kind: 'rows' };
      return;
    }
    // Keyboard context menu (Shift+F10 / Menu key) on the focused grid acts on the selection.
    const keyboard = target === e.currentTarget && trafficStore.getSnapshot().selection.ids.size > 0;
    menuTarget.current = keyboard ? { kind: 'rows' } : { kind: 'list' };
  };
  const menuItems = (): MenuItems => {
    const t = menuTarget.current;
    if (t.kind === 'header') return columnItems(t.column);
    if (t.kind === 'rows') {
      const ids = selectedIds();
      const first = ids.length === 1 ? trafficStore.get(ids[0] as number) : undefined;
      if (first) return singleRowMenu(first);
      if (ids.length > 1) return multiRowMenu(ids);
    }
    return listMenu();
  };

  // ---- render
  const activeIdx = activeId == null ? -1 : indexInList(list, activeId, ascending);
  const activeRendered = activeIdx >= win.start && activeIdx < win.end;
  const rows: ReactElement[] = [];
  if (loaded) {
    for (let i = win.start; i < win.end; i++) {
      const id = list[i] as number;
      // Keyed by slot, not id: scrolling patches recycled rows in place (no DOM churn).
      rows.push(<TrafficRow key={i % pool} id={id} index={i} top={HEADER_H + i * rowH} columns={columnIds} monoPaths={monoPaths} />);
    }
  }
  const empty = loaded && list.length === 0;
  const noTraffic = empty && total === 0;

  return (
    <div className={styles.root}>
      <ContextMenu items={menuItems}>
        <div
          ref={scrollRef}
          className={clsx(styles.scroller, noTraffic && styles.scrollerEmpty)}
          role="grid"
          aria-label="Captured exchanges"
          aria-rowcount={list.length + 1}
          aria-colcount={visibleCols.length}
          aria-multiselectable
          aria-activedescendant={activeRendered ? `traffic-row-${activeId}` : undefined}
          tabIndex={0}
          onKeyDown={onKeyDown}
          onMouseDown={onMouseDown}
          onDoubleClick={(e) => {
            if ((e.target as Element).closest('[data-row-id]')) trafficView.focusDetail();
          }}
          onContextMenuCapture={onContextMenuCapture}
          onScroll={onScroll}
          onWheel={(e) => {
            // Leaving the live edge must win over a batch arriving in the same frame.
            if ((tailMode === 'bottom' && e.deltaY < 0) || (tailMode === 'top' && e.deltaY > 0)) setTail(false);
          }}
        >
          {noTraffic ? (
            <EmptyTraffic />
          ) : (
            <div
              className={styles.canvas}
              style={{ height: loaded && !empty ? totalH : '100%', minWidth: contentWidth, ['--cols' as string]: template }}
            >
              <ListHeader columns={visibleCols} sort={sort} onSort={onSort} onResize={onResize} onReorder={onReorder} />
              {!loaded ? (
                <LoadingRows rowH={rowH} />
              ) : (
                <div role="rowgroup">{rows}</div>
              )}
              {empty && filterActive && (
                <div className={styles.emptyOverlay}>
                  <NoMatches total={total} />
                </div>
              )}
            </div>
          )}
        </div>
      </ContextMenu>
      {newCount > 0 && (
        <button type="button" className={clsx(styles.newPill, tailMode === 'top' && styles.newPillTop)} onClick={() => scrollToTail(true)}>
          {tailMode === 'top' ? <ArrowUp size={13} strokeWidth={2.25} aria-hidden /> : <ArrowDown size={13} strokeWidth={2.25} aria-hidden />}
          {formatNumber(newCount)} new
        </button>
      )}
      <GridTooltip containerRef={scrollRef} />
    </div>
  );
}

function LoadingRows({ rowH }: { rowH: number }) {
  const widths = useMemo(() => Array.from({ length: 14 }, (_, i) => 30 + ((i * 37) % 55)), []);
  return (
    <div className={styles.loading} role="status" aria-busy="true" aria-label="Loading traffic">
      {widths.map((w, i) => (
        <div key={i} className={styles.loadingRow} style={{ height: rowH }}>
          <Skeleton width={36} height={10} />
          <Skeleton width={28} height={10} />
          <Skeleton width={`${Math.round(w * 0.6)}%`} height={10} />
        </div>
      ))}
    </div>
  );
}
