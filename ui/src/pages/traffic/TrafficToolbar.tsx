import clsx from 'clsx';
import {
  Archive,
  ArrowDownToLine,
  ChartGantt,
  ChevronDown,
  Columns3,
  EyeOff,
  FileDown,
  FileUp,
  Keyboard,
  List,
  MoreVertical,
  PanelBottom,
  PanelLeft,
  PanelLeftClose,
  PanelRight,
  Rows3,
  Save,
  Trash2,
  Type,
} from 'lucide-react';
import { useLayoutEffect, useRef, useState } from 'react';
import { importForeign } from '../../app/sessions/actions';
import { openSessionsDialog } from '../../app/sessions/sessions';
import { setShortcutsOverlayOpen } from '../../app/shortcutsOverlay';
import { Button, IconButton, MenuButton, SegmentedControl, Toolbar, ToolbarSeparator, type MenuItems } from '../../components/ui';
import { redactExports, setRedactExports } from '../../lib/exportRedaction';
import { formatNumber } from '../../lib/format';
import { TYPE_CHIPS, type TypeChip } from '../../lib/http';
import { useAppStore } from '../../state/app';
import { trafficStore, useTraffic } from '../../state/traffic';
import { DENSITIES, useUiStore, type DetailPosition } from '../../state/ui';
import { requestClear } from './actions/clear';
import { exportHar, importFile, saveSession, selectedIds, toggleRecording } from './actions/exchangeActions';
import { columnItems } from './actions/menus';
import { FilterBar } from './filter/FilterBar';
import { toggleSidebar, useSidebarOpen } from './sidebar/sidebarState';
import { toggleTrafficView, useTrafficViewMode } from './timeline/timelineState';
import { trafficView } from './trafficView';
import styles from './TrafficPage.module.css';

/** Below this middle-section width the type chips collapse into a dropdown. */
const CHIPS_COLLAPSE_WIDTH = 925;

function TypeChips({ collapsed }: { collapsed: boolean }) {
  const chip = useTraffic((s) => s.filter.chip);
  const set = (v: TypeChip) => trafficStore.setTypeChip(v);
  if (collapsed) {
    const current = TYPE_CHIPS.find((c) => c.id === chip);
    return (
      <MenuButton
        label="Content type"
        align="start"
        trigger={
          <Button variant="secondary" iconRight={ChevronDown} className={clsx(styles.chipButton, chip !== 'all' && styles.chipButtonActive)} aria-label={`Content type: ${current?.label ?? 'All'}`}>
            {chip === 'all' ? 'All types' : current?.label}
          </Button>
        }
        items={[{ type: 'radio', key: 'types', value: chip, onValueChange: (v) => set(v as TypeChip), options: TYPE_CHIPS.map((c) => ({ value: c.id, label: c.label })) }]}
      />
    );
  }
  return (
    <SegmentedControl<TypeChip>
      value={chip}
      onValueChange={set}
      items={TYPE_CHIPS.map((c) => ({ value: c.id, label: c.label }))}
      aria-label="Content type"
      className={styles.chips}
    />
  );
}

function TimelineButton() {
  const timeline = useTrafficViewMode() === 'timeline';
  return <IconButton label={timeline ? 'Show the traffic list' : 'Show the timeline'} icon={timeline ? List : ChartGantt} shortcut="t" active={timeline} aria-pressed={timeline} onClick={toggleTrafficView} />;
}

function SidebarButton() {
  const [open] = useSidebarOpen();
  return <IconButton label={open ? 'Hide the hosts sidebar' : 'Show the hosts sidebar'} icon={open ? PanelLeftClose : PanelLeft} shortcut="mod+b" active={open} aria-pressed={open} onClick={toggleSidebar} />;
}

function RecordButton() {
  const recording = useAppStore((s) => s.recording);
  const label = recording ? 'Pause recording' : 'Resume recording';
  return (
    <IconButton label={label} shortcut="mod+shift+r" onClick={toggleRecording} className={clsx(styles.record, !recording && styles.recordPaused)} aria-pressed={recording}>
      <span className={clsx(styles.recordDot, recording && styles.recordDotOn)} aria-hidden />
    </IconButton>
  );
}

export interface TrafficToolbarProps {
  /** Effective detail-pane orientation (for the More menu). */
  orientation: 'right' | 'bottom';
  onToggleOrientation: () => void;
}

/** Filter bar, type chips and the page actions (DESIGN.md §6.2). */
export function TrafficToolbar({ orientation, onToggleOrientation }: TrafficToolbarProps) {
  const middleRef = useRef<HTMLDivElement | null>(null);
  const [collapsed, setCollapsed] = useState(false);
  const total = useTraffic((s) => s.order.length);

  useLayoutEffect(() => {
    const el = middleRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      if (entry) setCollapsed(entry.contentRect.width < CHIPS_COLLAPSE_WIDTH);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const transferItems = (): MenuItems => {
    const visible = trafficView.getList().length;
    const selected = selectedIds();
    return [
      { type: 'label', key: 'import-label', label: 'Import' },
      { key: 'import-har', label: 'Import HAR…', icon: FileUp, onSelect: () => void importFile('har') },
      { key: 'import-akev', label: 'Import .akev session…', icon: FileUp, onSelect: () => void importFile('session') },
      { key: 'import-charles', label: 'Import Charles session (.chlsj)…', icon: FileUp, onSelect: () => void importForeign('charles') },
      { key: 'import-mitmproxy', label: 'Import mitmproxy flows…', icon: FileUp, onSelect: () => void importForeign('mitmproxy') },
      { key: 'import-fiddler', label: 'Import Fiddler sessions (.saz)…', icon: FileUp, onSelect: () => void importForeign('fiddler') },
      { type: 'separator', key: 's1' },
      { type: 'label', key: 'export-label', label: 'Export' },
      { key: 'export-all', label: 'Export all as HAR', icon: FileDown, hint: formatNumber(total), disabled: total === 0, onSelect: () => exportHar('all') },
      { key: 'export-visible', label: 'Export visible as HAR', icon: FileDown, hint: formatNumber(visible), disabled: visible === 0, onSelect: () => exportHar('visible') },
      { key: 'export-selected', label: 'Export selected as HAR', icon: FileDown, hint: selected.length ? formatNumber(selected.length) : undefined, disabled: selected.length === 0, onSelect: () => exportHar('selected', selected) },
      { type: 'checkbox', key: 'redact-har', label: 'Redact secrets in HAR files', icon: EyeOff, checked: redactExports('har'), onCheckedChange: (v) => setRedactExports('har', v) },
      { type: 'separator', key: 's2' },
      { type: 'label', key: 'sessions-label', label: 'Sessions' },
      { key: 'save-as-session', label: 'Save as session…', icon: Save, disabled: total === 0, onSelect: () => openSessionsDialog({ save: true }) },
      { key: 'sessions', label: 'Saved sessions…', icon: Archive, onSelect: () => openSessionsDialog() },
      { key: 'session', label: 'Download .akev file', icon: FileDown, disabled: total === 0, onSelect: () => saveSession() },
      { type: 'checkbox', key: 'redact-session', label: 'Redact secrets in .akev downloads', icon: EyeOff, checked: redactExports('session'), onCheckedChange: (v) => setRedactExports('session', v) },
    ];
  };

  const moreItems = (): MenuItems => {
    const ui = useUiStore.getState();
    return [
      { type: 'label', key: 'detail-label', label: 'Detail pane' },
      {
        type: 'radio',
        key: 'detail',
        value: ui.detailPosition,
        onValueChange: (v) => useUiStore.getState().setDetailPosition(v as DetailPosition),
        options: [
          { value: 'auto', label: 'Automatic' },
          { value: 'right', label: 'Right' },
          { value: 'bottom', label: 'Bottom' },
        ],
      },
      { key: 'toggle-detail', label: orientation === 'right' ? 'Move to bottom' : 'Move to right', icon: orientation === 'right' ? PanelBottom : PanelRight, shortcut: 'mod+shift+d', onSelect: onToggleOrientation },
      { type: 'separator', key: 's1' },
      {
        type: 'submenu',
        key: 'density',
        label: 'Density',
        icon: Rows3,
        items: [{ type: 'radio', key: 'd', value: ui.density, onValueChange: (v) => useUiStore.getState().setDensity(v as (typeof DENSITIES)[number]['id']), options: DENSITIES.map((d) => ({ value: d.id, label: `${d.label} · ${d.rowHeight}px` })) }],
      },
      { type: 'submenu', key: 'columns', label: 'Columns', icon: Columns3, items: columnItems() },
      { type: 'checkbox', key: 'mono', label: 'Monospace paths', icon: Type, checked: ui.monoPaths, onCheckedChange: (v) => useUiStore.getState().setMonoPaths(v) },
      { type: 'separator', key: 's2' },
      { key: 'shortcuts', label: 'Keyboard shortcuts', icon: Keyboard, shortcut: 'shift+?', onSelect: () => setShortcutsOverlayOpen(true) },
    ];
  };

  return (
    <Toolbar
      titleBar
      className={styles.toolbar}
      aria-label="Traffic"
      right={
        <>
          <TimelineButton />
          <ToolbarSeparator />
          <RecordButton />
          <IconButton label="Clear traffic" icon={Trash2} shortcut="mod+shift+x" variant="danger" disabled={total === 0} onClick={() => void requestClear()} />
          <MenuButton label="Import / export" icon={ArrowDownToLine} items={transferItems} menuClassName={styles.menu} />
          <MenuButton label="View options" icon={MoreVertical} items={moreItems} menuClassName={styles.menu} />
        </>
      }
    >
      <div ref={middleRef} className={styles.middle}>
        <SidebarButton />
        <FilterBar />
        <TypeChips collapsed={collapsed} />
      </div>
    </Toolbar>
  );
}
