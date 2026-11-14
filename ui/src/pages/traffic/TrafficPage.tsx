import { ArrowDownNarrowWide, ArrowUpNarrowWide, ChartGantt, CheckSquare, Columns3, FileDown, FilterX, PanelBottom, PanelLeft, PanelRight, List, Repeat, RotateCcw, Search, Star, Trash2, Type } from 'lucide-react';
import { lazy, Suspense, useEffect, useRef, useSyncExternalStore } from 'react';
import { useCommands, type Command } from '../../app/commands';
import { getRoute, navigate, useRouteParam } from '../../app/router';
import { useShortcut } from '../../app/shortcuts';
import { SplitPane, toast } from '../../components/ui';
import { TYPE_CHIPS } from '../../lib/http';
import { useAppStore } from '../../state/app';
import { indexInList, trafficStore, useSingleSelectedId, useTraffic } from '../../state/traffic';
import { isHeaderColumn, TRAFFIC_COLUMNS, useUiStore } from '../../state/ui';
import { requestClear } from './actions/clear';
import { exportHar, selectedIds } from './actions/exchangeActions';
import { removeHeaderColumn } from './actions/headerColumns';
import { DetailPane } from './detail/DetailPane';
import { CaptureDropOverlay, useCaptureDrop } from './drop/CaptureDrop';
import { closeFirstRun, FIRST_RUN_DONE_PREF, useFirstRunOpen } from './first-run/firstRun';
import { openHeaderColumnDialog, TrafficDialogs } from './dialogs/TrafficDialogs';
import { openRepeatDialog, RepeatHost } from '../../components/repeat/repeat';
import { summaryUrl } from '../../lib/http';
import { columnLabel, isNaturalOrder, isSortable } from './list/columns';
import { TrafficList } from './list/TrafficList';
import { TrafficToolbar } from './TrafficToolbar';
import { trafficView } from './trafficView';
import { showUnrecordedPause, useUnrecordedPause } from './unrecordedPause';
import { toggleSidebar, useSidebarOpen } from './sidebar/sidebarState';
import { toggleTrafficView, useTrafficViewMode } from './timeline/timelineState';
import styles from './TrafficPage.module.css';

/** The hosts / apps sidebar and the timeline load the first time they're opened. */
const TrafficSidebar = lazy(() => import('./sidebar/TrafficSidebar'));
const TrafficTimeline = lazy(() => import('./timeline/TrafficTimeline'));

// Stress-test helpers (`__akevTraffic.bulk(100_000)`): dev server, or a production build made
// with `VITE_AKEV_STRESS=1` for realistic profiling. Never part of a normal build.
if (import.meta.env.DEV || import.meta.env.VITE_AKEV_STRESS === '1') void import('./dev/bulkLoad');

/** The first-run setup, shown in place of the list until it's finished or skipped. */
const loadFirstRun = () => import('./first-run/FirstRunFlow');
const FirstRunFlow = lazy(loadFirstRun);

/** The detail pane docks right by default on windows at least this wide (DESIGN.md §6.1). */
const WIDE_WINDOW = 1100;

function subscribeResize(cb: () => void): () => void {
  window.addEventListener('resize', cb);
  return () => window.removeEventListener('resize', cb);
}

function useWideWindow(): boolean {
  return useSyncExternalStore(
    subscribeResize,
    () => window.innerWidth >= WIDE_WINDOW,
    () => true,
  );
}

/** Wait (briefly) for an exchange to arrive in the store — e.g. a just-replayed request. */
function whenKnown(id: number, timeoutMs = 4000): Promise<boolean> {
  if (trafficStore.get(id)) return Promise.resolve(true);
  return new Promise((resolve) => {
    const timer = setTimeout(() => {
      unsub();
      resolve(false);
    }, timeoutMs);
    const unsub = trafficStore.subscribe(() => {
      if (!trafficStore.get(id)) return;
      clearTimeout(timer);
      unsub();
      resolve(true);
    });
  });
}

const isPaused = (id: number): boolean => useAppStore.getState().paused.some((p) => p.id === id);

/**
 * Two-way sync between the selection and `#/traffic/<id>`: selecting one exchange updates the
 * URL (replace, coalesced by the router); deep links and jumps select and reveal the row.
 *
 * A paused exchange that isn't in the list (recording is paused, the capture settings ignore
 * its host, it was cleared) can't be selected: the detail pane shows it from its breakpoint
 * snapshot instead (`unrecordedPause.ts`), and switches to the full view if it turns up after all.
 */
function useRouteSelectionSync(): void {
  const routeId = useRouteParam('id');
  const loaded = useTraffic((s) => s.loaded);
  const selection = useTraffic((s) => s.selection.ids);
  const pendingDeepLink = useRef<number | null>(null);

  // URL → selection.
  useEffect(() => {
    if (!loaded || routeId == null) {
      showUnrecordedPause(null);
      return;
    }
    const id = Number(routeId);
    if (!Number.isInteger(id)) return;
    if (useUnrecordedPause.getState().id !== id) showUnrecordedPause(null);
    const sel = trafficStore.getSnapshot().selection.ids;
    if (sel.size === 1 && sel.has(id)) {
      // Already selected (e.g. "View" on a toast): just make sure the row is on screen. Next
      // frame: scrolling may render synchronously, which React can't do inside an effect.
      const raf = requestAnimationFrame(() => trafficView.scrollToId(id));
      return () => cancelAnimationFrame(raf);
    }
    let cancelled = false;
    if (!trafficStore.get(id) && isPaused(id)) {
      // Show it right away from the breakpoint; keep waiting in case its summary is in flight.
      showUnrecordedPause(id);
      trafficStore.clearSelection();
    } else {
      pendingDeepLink.current = id;
    }
    void whenKnown(id).then((known) => {
      if (cancelled) return;
      if (pendingDeepLink.current === id) pendingDeepLink.current = null;
      if (!known) {
        if (useUnrecordedPause.getState().id === id) return;
        if (isPaused(id)) {
          showUnrecordedPause(id);
          trafficStore.clearSelection();
          return;
        }
        toast.error(`Exchange #${id} isn't in this session`, { description: 'It may have been deleted or cleared.' });
        navigate('/traffic', { replace: true });
        return;
      }
      showUnrecordedPause(null);
      trafficStore.select(id);
      requestAnimationFrame(() => {
        if (cancelled) return;
        const ascending = isNaturalOrder(useUiStore.getState().sort);
        if (indexInList(trafficView.getList(), id, ascending) >= 0) trafficView.scrollToId(id, 'center');
        else if (trafficStore.getSnapshot().filter.active) {
          toast(`Exchange #${id} is hidden by the filter`, { action: { label: 'Clear filter', onClick: () => trafficStore.clearFilter() } });
        }
      });
    });
    return () => {
      cancelled = true;
      if (pendingDeepLink.current === id) pendingDeepLink.current = null;
    };
  }, [routeId, loaded]);

  // Selection → URL (reads the live selection so a just-applied deep link isn't undone).
  useEffect(() => {
    if (!loaded || getRoute().page !== 'traffic' || pendingDeepLink.current != null) return;
    const ids = trafficStore.getSnapshot().selection.ids;
    // Nothing selected because the pane shows an unlisted paused exchange: keep its URL.
    if (ids.size === 0 && useUnrecordedPause.getState().id != null) return;
    const single = ids.size === 1 ? (ids.values().next().value as number) : null;
    navigate(single != null ? `/traffic/${single}` : '/traffic', { replace: true });
  }, [selection, loaded]);
}

/** The Traffic page (DESIGN.md §6): toolbar + list | detail split with right/bottom docking. */
export function TrafficPage() {
  const detailPosition = useUiStore((s) => s.detailPosition);
  const wide = useWideWindow();
  const orientation: 'right' | 'bottom' = detailPosition === 'auto' ? (wide ? 'right' : 'bottom') : detailPosition;
  const detailRef = useRef<HTMLElement | null>(null);

  const toggleOrientation = () => useUiStore.getState().setDetailPosition(orientation === 'right' ? 'bottom' : 'right');

  useRouteSelectionSync();
  const firstRun = useFirstRunOpen();
  const pageRef = useRef<HTMLDivElement | null>(null);
  // Dropped capture files are imported; during the setup that means the user has traffic to look at.
  const dropping = useCaptureDrop(pageRef, () => {
    if (firstRun) closeFirstRun();
  });
  // Likely a first launch: fetch the setup while the first snapshot loads.
  useEffect(() => {
    if (useUiStore.getState().pages[FIRST_RUN_DONE_PREF] !== true) void loadFirstRun();
  }, []);

  useEffect(() => {
    trafficView.registerDetail(detailRef.current);
    return () => trafficView.registerDetail(null);
  }, [orientation, firstRun]);

  // ---- shortcuts
  useShortcut('mod+f', () => trafficView.focusFilter(), { allowInInputs: true, description: 'Focus the filter', group: 'Traffic' });
  useShortcut('/', () => trafficView.focusFilter(), { description: 'Focus the filter', group: 'Traffic' });
  useShortcut('mod+shift+x', () => void requestClear(), { allowInInputs: true, description: 'Clear traffic (undoable)', group: 'Traffic' });
  useShortcut('mod+shift+d', toggleOrientation, { allowInInputs: true, description: 'Toggle detail pane position', group: 'Traffic' });
  useShortcut('mod+b', toggleSidebar, { allowInInputs: true, description: 'Show or hide the sidebar', group: 'Traffic' });
  useShortcut('t', toggleTrafficView, { description: 'Switch between list and timeline', group: 'Traffic' });
  const [sidebarOpen] = useSidebarOpen();
  const viewMode = useTrafficViewMode();

  // ---- palette
  const empty = useTraffic((s) => s.order.length === 0);
  const filterActive = useTraffic((s) => s.filter.active);
  const chip = useTraffic((s) => s.filter.chip);
  const noSelection = useTraffic((s) => s.selection.ids.size === 0);
  const singleId = useSingleSelectedId();
  const columns = useUiStore((s) => s.columns);
  const sort = useUiStore((s) => s.sort);
  const monoPaths = useUiStore((s) => s.monoPaths);
  const savedFilters = useUiStore((s) => s.savedFilters);

  useCommands(() => {
    const cmds: Command[] = [
      { id: 'traffic.clear', title: 'Clear traffic', subtitle: 'Undoable', group: 'Traffic', icon: Trash2, shortcut: 'mod+shift+x', keywords: ['delete', 'empty', 'reset'], disabled: empty, run: () => void requestClear() },
      {
        id: 'traffic.toggle-detail',
        title: orientation === 'right' ? 'Move detail pane to the bottom' : 'Move detail pane to the right',
        group: 'Traffic',
        icon: orientation === 'right' ? PanelBottom : PanelRight,
        shortcut: 'mod+shift+d',
        keywords: ['layout', 'dock', 'split'],
        run: () => useUiStore.getState().setDetailPosition(orientation === 'right' ? 'bottom' : 'right'),
      },
      { id: 'traffic.sidebar', title: sidebarOpen ? 'Hide the hosts sidebar' : 'Show the hosts sidebar', subtitle: 'Hosts by domain and apps, with counts', group: 'Traffic', icon: PanelLeft, shortcut: 'mod+b', keywords: ['sidebar', 'domains', 'hosts', 'apps', 'tree', 'pinned'], run: toggleSidebar },
      { id: 'traffic.timeline', title: viewMode === 'timeline' ? 'Show the traffic list' : 'Show the timeline', subtitle: viewMode === 'timeline' ? undefined : 'Visible or selected exchanges on one time axis', group: 'Traffic', icon: viewMode === 'timeline' ? List : ChartGantt, shortcut: 't', keywords: ['waterfall', 'timeline', 'timing', 'gantt', 'list'], run: toggleTrafficView },
      { id: 'traffic.focus-filter', title: 'Filter traffic…', group: 'Traffic', icon: Search, shortcut: 'mod+f', keywords: ['search', 'find', 'query'], run: () => trafficView.focusFilter() },
      { id: 'traffic.clear-filter', title: 'Clear filter', group: 'Traffic', icon: FilterX, disabled: !filterActive, run: () => trafficStore.clearFilter() },
      { id: 'traffic.export-visible', title: 'Export visible as HAR', group: 'Traffic', icon: FileDown, keywords: ['download', 'save', 'filtered'], disabled: empty, run: () => exportHar('visible') },
      {
        id: 'traffic.repeat',
        title: 'Repeat the selected request…',
        subtitle: 'Send it many times, several at once',
        group: 'Traffic',
        icon: Repeat,
        keywords: ['replay', 'load', 'benchmark', 'concurrency', 'n times'],
        disabled: singleId == null,
        run: () => {
          const s = singleId != null ? trafficStore.get(singleId) : undefined;
          if (s) openRepeatDialog({ kind: 'exchange', id: s.id, label: `${s.method} ${summaryUrl(s)}` });
        },
      },
      { id: 'traffic.export-selected', title: 'Export selected as HAR', group: 'Traffic', icon: FileDown, keywords: ['download', 'save'], disabled: noSelection, run: () => exportHar('selected', selectedIds()) },
      { id: 'traffic.select-all', title: 'Select all visible exchanges', group: 'Traffic', icon: CheckSquare, shortcut: 'mod+a', disabled: empty, run: () => trafficStore.selectAll(trafficView.listArg()) },
      { id: 'traffic.mono-paths', title: monoPaths ? 'Show paths in the UI font' : 'Show paths in monospace', group: 'Traffic', icon: Type, keywords: ['font', 'path', 'mono'], run: () => useUiStore.getState().setMonoPaths(!monoPaths) },
      { id: 'traffic.reset-columns', title: 'Reset traffic columns', group: 'Traffic', icon: Columns3, keywords: ['width', 'order'], run: () => useUiStore.getState().resetColumns() },
      { id: 'traffic.add-header-column', title: 'Add header column…', subtitle: 'Show a request or response header in the list', group: 'Traffic', icon: Columns3, keywords: ['column', 'header', 'x-request-id', 'cache-control', 'custom'], run: openHeaderColumnDialog },
    ];
    for (const def of TRAFFIC_COLUMNS) {
      if (def.fixed) continue;
      const visible = columns.find((c) => c.id === def.id)?.visible ?? def.defaultVisible;
      cmds.push({
        id: `traffic.column.${def.id}`,
        title: `${visible ? 'Hide' : 'Show'} column: ${columnLabel(def.id)}`,
        group: 'Traffic',
        icon: Columns3,
        keywords: ['columns', 'list'],
        run: () => useUiStore.getState().setColumnVisible(def.id, !visible),
      });
    }
    for (const col of columns) {
      if (!isHeaderColumn(col.id)) continue;
      const id = col.id;
      cmds.push({ id: `traffic.column.${id}`, title: `Remove column: ${columnLabel(id)}`, group: 'Traffic', icon: Columns3, keywords: ['columns', 'header'], run: () => removeHeaderColumn(id) });
    }
    // Sorting (header clicks are mouse-only; the palette makes it keyboard-accessible).
    for (const col of columns) {
      if (!col.visible || !isSortable(col.id)) continue;
      const current = sort?.column === col.id ? sort.direction : null;
      const next = current === 'asc' ? 'desc' : 'asc';
      cmds.push({
        id: `traffic.sort.${col.id}`,
        title: `Sort by ${columnLabel(col.id)}${current ? (next === 'desc' ? ' (descending)' : ' (ascending)') : ''}`,
        subtitle: current ? `Currently ${current === 'asc' ? 'ascending' : 'descending'}` : undefined,
        group: 'Traffic',
        icon: next === 'asc' ? ArrowUpNarrowWide : ArrowDownNarrowWide,
        keywords: ['sort', 'order', 'column'],
        run: () => useUiStore.getState().setSort({ column: col.id, direction: next }),
      });
    }
    if (sort) {
      cmds.push({ id: 'traffic.sort.reset', title: 'Restore default order', subtitle: 'Oldest first, newest at the bottom', group: 'Traffic', icon: RotateCcw, keywords: ['sort', 'order', 'unsort'], run: () => useUiStore.getState().setSort(null) });
    }
    for (const c of TYPE_CHIPS) {
      cmds.push({
        id: `traffic.type.${c.id}`,
        title: c.id === 'all' ? 'Show all content types' : `Show only ${c.label}`,
        subtitle: chip === c.id ? 'Current' : undefined,
        group: 'Traffic',
        icon: FilterX,
        keywords: ['type', 'filter', 'chip'],
        disabled: chip === c.id,
        run: () => trafficStore.setTypeChip(c.id),
      });
    }
    for (const f of savedFilters) {
      cmds.push({
        id: `traffic.saved-filter.${f.name}`,
        title: `Apply saved filter: ${f.name}`,
        subtitle: f.query,
        group: 'Traffic',
        icon: Star,
        keywords: ['filter', 'saved'],
        run: () => {
          trafficStore.setFilterText(f.query);
          trafficStore.flushFilter();
          useUiStore.getState().pushRecentFilter(f.query);
        },
      });
    }
    return cmds;
  }, [empty, orientation, filterActive, noSelection, monoPaths, columns, sort, chip, savedFilters, sidebarOpen, viewMode, singleId]);

  return (
    <div ref={pageRef} className={styles.page}>
      <TrafficToolbar orientation={orientation} onToggleOrientation={toggleOrientation} />
      <div className={styles.body}>
        {sidebarOpen && !firstRun && (
          <Suspense fallback={<div className={styles.sidebarFallback} />}>
            <TrafficSidebar />
          </Suspense>
        )}
        {firstRun ? (
          <Suspense fallback={null}>
            <FirstRunFlow />
          </Suspense>
        ) : (
          <SplitPane
            key={orientation}
            id={`traffic.detail.${orientation}`}
            direction={orientation === 'right' ? 'horizontal' : 'vertical'}
            defaultRatio={orientation === 'right' ? 0.58 : 0.5}
            minFirst={orientation === 'right' ? 420 : 150}
            minSecond={orientation === 'right' ? 360 : 200}
            dividerLabel="Resize the detail pane"
          >
            {viewMode === 'timeline' ? (
              <Suspense fallback={<div className={styles.timelineFallback} />}>
                <TrafficTimeline />
              </Suspense>
            ) : (
              <TrafficList />
            )}
            <section ref={detailRef} className={styles.detail} tabIndex={-1} aria-label="Exchange details">
              <DetailPane orientation={orientation} />
            </section>
          </SplitPane>
        )}
        <CaptureDropOverlay visible={dropping} />
      </div>
      <TrafficDialogs />
      <RepeatHost />
    </div>
  );
}

export default TrafficPage;
