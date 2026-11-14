/**
 * Context menus of the traffic list (DESIGN.md §6.3): single row, multi-row, empty area and
 * column header. Built on open from the store's current state. The Highlight and Create rule
 * items are shared with the detail pane.
 */

import {
  ArrowDownNarrowWide,
  ArrowLeftRight,
  ArrowUpNarrowWide,
  Ban,
  Braces,
  CheckSquare,
  CirclePause,
  Columns3,
  Copy,
  Download,
  FileDown,
  Filter,
  FolderOpen,
  Highlighter,
  Link,
  PenLine,
  Pin,
  PinOff,
  Play,
  Plus,
  Repeat,
  RotateCcw,
  Send,
  StickyNote,
  Terminal,
  Trash2,
  Workflow,
  X,
} from 'lucide-react';
import type { ExchangeId, ExchangeSummary, TagColor } from '../../../api/types';
import { TAG_COLORS } from '../../../api/types';
import { ColorDot, type MenuItems, type MenuItemSpec } from '../../../components/ui';
import { pluralize } from '../../../lib/format';
import type { RuleTemplate } from '../../../lib/ruleFromExchange';
import { trafficStore } from '../../../state/traffic';
import { isHeaderColumn, MAX_HEADER_COLUMNS, TRAFFIC_COLUMNS, useUiStore, type HeaderColumnId, type TrafficColumnId } from '../../../state/ui';
import { columnLabel, isSortable } from '../list/columns';
import { openCompareDialog, openHeaderColumnDialog, openNoteDialog } from '../dialogs/TrafficDialogs';
import { trafficView } from '../trafficView';
import { requestClear } from './clear';
import { removeHeaderColumn } from './headerColumns';
import { openRepeatDialog } from '../../../components/repeat/repeat';
import { summaryUrl } from '../../../lib/http';
import { isPinnedHost, togglePinnedHost } from '../sidebar/sidebarState';
import { codegenTargets } from './codegenTargets';
import {
  copyAsSnippet,
  copyBody,
  copyUrls,
  createRule,
  deleteExchanges,
  downloadBody,
  exportHar,
  filterBy,
  openInComposeFor,
  replay,
  setHighlight,
  togglePin,
} from './exchangeActions';
import styles from './menus.module.css';

const COLOR_LABELS: Record<TagColor, string> = {
  red: 'Red',
  orange: 'Orange',
  yellow: 'Yellow',
  green: 'Green',
  blue: 'Blue',
  purple: 'Purple',
  grey: 'Grey',
};

/**
 * Highlight colours as a radio group (`'mixed'` = several colours, none checked). Shared by
 * the row menus, the detail header and the multi-selection pane.
 */
export function highlightItems(current: TagColor | null | undefined | 'mixed', onPick: (color: TagColor | null) => void): MenuItems {
  return [
    {
      type: 'radio',
      key: 'highlight',
      value: current === 'mixed' ? '' : (current ?? 'none'),
      onValueChange: (v) => onPick(v === 'none' ? null : (v as TagColor)),
      options: [
        { value: 'none', label: <span className={styles.colorItem}>No highlight</span> },
        ...TAG_COLORS.map((c) => ({
          value: c,
          label: (
            <span className={styles.colorItem}>
              <ColorDot color={c} size={10} />
              {COLOR_LABELS[c]}
            </span>
          ),
        })),
      ],
    },
  ];
}

function copyAsItems(id: ExchangeId): MenuItems {
  const targets = codegenTargets().filter((t) => t.id !== 'curl');
  if (targets.length === 0) return [{ label: 'Loading targets…', disabled: true }];
  const out: MenuItemSpec[] = [];
  let group: string | null = null;
  for (const t of targets) {
    if (t.group !== group) {
      group = t.group;
      out.push({ type: 'label', key: `g-${group}`, label: group });
    }
    out.push({ key: t.id, label: t.name, onSelect: () => void copyAsSnippet([id], t.id) });
  }
  return out;
}

const RULE_ITEMS: readonly { template: RuleTemplate; label: string; icon: typeof Play; http: boolean }[] = [
  { template: 'mock', label: 'Mock this response', icon: Braces, http: true },
  { template: 'breakpoint-request', label: 'Breakpoint on request', icon: CirclePause, http: true },
  { template: 'breakpoint-response', label: 'Breakpoint on response', icon: CirclePause, http: true },
  { template: 'map-local', label: 'Map Local', icon: FolderOpen, http: true },
  { template: 'rewrite', label: 'Rewrite…', icon: PenLine, http: true },
  { template: 'block-host', label: 'Block this host', icon: Ban, http: false },
];

/**
 * "Create rule ▸" items for one exchange (row menu and detail header). Connection-level
 * exchanges (tunnels, TLS failures) only offer the host rules.
 */
export function createRuleItems(s: ExchangeSummary, options: { httpLike?: boolean } = {}): MenuItems {
  const httpLike = options.httpLike ?? (s.kind !== 'tunnel' && s.kind !== 'tls-failure');
  const hasResponse = s.status != null;
  return RULE_ITEMS.filter((r) => httpLike || !r.http).map((r) => ({
    key: r.template,
    label: r.label,
    icon: r.icon,
    disabled: r.template === 'mock' && !hasResponse,
    hint: r.template === 'mock' && !hasResponse ? 'No response yet' : r.template === 'block-host' ? s.host : undefined,
    onSelect: () => void createRule(s.id, r.template),
  }));
}

/** Menu for exactly one exchange. */
export function singleRowMenu(s: ExchangeSummary): MenuItems {
  const id = s.id;
  const httpLike = s.kind !== 'tunnel' && s.kind !== 'tls-failure';
  const hasResponse = s.status != null;
  return [
    { key: 'copy-url', label: 'Copy URL', icon: Link, shortcut: 'mod+c', onSelect: () => void copyUrls([id]) },
    { key: 'copy-curl', label: 'Copy as cURL', icon: Terminal, shortcut: 'mod+shift+c', disabled: !httpLike, onSelect: () => void copyAsSnippet([id], 'curl') },
    { type: 'submenu', key: 'copy-as', label: 'Copy as…', icon: Copy, disabled: !httpLike, items: copyAsItems(id) },
    {
      type: 'submenu',
      key: 'copy-body',
      label: 'Copy body',
      icon: Copy,
      disabled: !httpLike,
      items: [
        { key: 'req', label: 'Request body', disabled: s.requestSize === 0, hint: s.requestSize === 0 ? 'Empty' : undefined, onSelect: () => void copyBody(id, 'request') },
        { key: 'res', label: 'Response body', disabled: !hasResponse, hint: !hasResponse ? 'No response' : undefined, onSelect: () => void copyBody(id, 'response') },
      ],
    },
    { type: 'separator', key: 's1' },
    { key: 'replay', label: 'Replay', icon: Play, shortcut: 'mod+r', disabled: !httpLike, onSelect: () => void replay([id]) },
    { key: 'repeat', label: 'Repeat…', icon: Repeat, disabled: !httpLike, hint: 'Send many times', onSelect: () => openRepeatDialog({ kind: 'exchange', id, label: `${s.method} ${summaryUrl(s)}` }) },
    { key: 'compose', label: 'Edit in Compose', icon: Send, shortcut: 'mod+e', disabled: !httpLike, onSelect: () => void openInComposeFor(id) },
    { type: 'separator', key: 's2' },
    { type: 'submenu', key: 'rule', label: 'Create rule', icon: Workflow, items: createRuleItems(s) },
    { type: 'separator', key: 's3' },
    {
      type: 'submenu',
      key: 'filter',
      label: 'Filter',
      icon: Filter,
      items: [
        { key: 'host', label: 'Same host', hint: s.host, onSelect: () => filterBy(id, 'host') },
        { key: 'path', label: 'Same path', disabled: !httpLike, onSelect: () => filterBy(id, 'path') },
        { key: 'exclude', label: 'Exclude host', icon: X, onSelect: () => filterBy(id, 'exclude-host') },
        { key: 'source', label: 'Same source', disabled: !s.source, hint: s.source, onSelect: () => filterBy(id, 'source') },
      ],
    },
    { type: 'submenu', key: 'highlight', label: 'Highlight', icon: Highlighter, items: highlightItems(s.color, (c) => void setHighlight([id], c)) },
    { key: 'pin-host', label: isPinnedHost(s.host) ? 'Unpin host from the sidebar' : 'Pin host to the sidebar', icon: isPinnedHost(s.host) ? PinOff : Pin, hint: s.host, disabled: !s.host, onSelect: () => togglePinnedHost(s.host) },
    { key: 'pin', label: s.pinned ? 'Unpin' : 'Pin', icon: s.pinned ? PinOff : Pin, shortcut: 'mod+p', onSelect: () => void togglePin([id]) },
    { key: 'note', label: s.hasNote ? 'Edit note…' : 'Add note…', icon: StickyNote, onSelect: () => openNoteDialog(id) },
    { type: 'separator', key: 's4' },
    { key: 'har', label: 'Export as HAR', icon: FileDown, onSelect: () => exportHar('selected', [id]) },
    { key: 'download', label: 'Download response body', icon: Download, disabled: !httpLike || !hasResponse || s.responseSize === 0, onSelect: () => void downloadBody(id, 'response') },
    { type: 'separator', key: 's5' },
    { key: 'delete', label: 'Delete', icon: Trash2, shortcut: 'delete', destructive: true, onSelect: () => deleteExchanges([id]) },
  ];
}

/** Menu for a multi-selection (ids in display order). */
export function multiRowMenu(ids: readonly ExchangeId[]): MenuItems {
  const rows = ids.map((id) => trafficStore.get(id)).filter((s): s is ExchangeSummary => s != null);
  const n = rows.length;
  const allPinned = rows.every((s) => s.pinned);
  const colors = new Set(rows.map((s) => s.color));
  const color = colors.size === 1 ? rows[0]?.color : 'mixed';
  return [
    { type: 'label', key: 'title', label: `${pluralize(n, 'exchange')} selected` },
    { key: 'copy-urls', label: 'Copy URLs', icon: Link, shortcut: 'mod+c', onSelect: () => void copyUrls(ids) },
    { key: 'copy-curl', label: 'Copy as cURL (all)', icon: Terminal, shortcut: 'mod+shift+c', onSelect: () => void copyAsSnippet(ids, 'curl') },
    { key: 'har', label: 'Export selected as HAR', icon: FileDown, onSelect: () => exportHar('selected', ids) },
    { type: 'separator', key: 's1' },
    { key: 'replay', label: `Replay ${n}`, icon: Play, shortcut: 'mod+r', onSelect: () => void replay(ids) },
    { key: 'pin', label: allPinned ? 'Unpin all' : 'Pin all', icon: allPinned ? PinOff : Pin, shortcut: 'mod+p', onSelect: () => void togglePin(ids) },
    { type: 'submenu', key: 'highlight', label: 'Highlight', icon: Highlighter, items: highlightItems(color, (c) => void setHighlight(ids, c)) },
    {
      key: 'compare',
      label: 'Compare',
      icon: ArrowLeftRight,
      disabled: n !== 2,
      hint: n !== 2 ? 'Select exactly 2' : undefined,
      onSelect: () => n === 2 && openCompareDialog((rows[0] as ExchangeSummary).id, (rows[1] as ExchangeSummary).id),
    },
    { type: 'separator', key: 's2' },
    { key: 'delete', label: `Delete ${pluralize(n, 'exchange')}`, icon: Trash2, shortcut: 'delete', destructive: true, onSelect: () => deleteExchanges(ids) },
  ];
}

/** Column visibility + sort items (header context menu & More ▸ Columns). */
export function columnItems(clicked?: TrafficColumnId): MenuItems {
  const ui = useUiStore.getState();
  const items: MenuItemSpec[] = [];
  if (clicked && isSortable(clicked)) {
    const sort = ui.sort;
    const active = sort?.column === clicked;
    items.push(
      { type: 'label', key: 'sort-label', label: `Sort by ${columnLabel(clicked)}` },
      { key: 'asc', label: 'Ascending', icon: ArrowUpNarrowWide, hint: active && sort?.direction === 'asc' ? 'Current' : undefined, onSelect: () => ui.setSort({ column: clicked, direction: 'asc' }) },
      { key: 'desc', label: 'Descending', icon: ArrowDownNarrowWide, hint: active && sort?.direction === 'desc' ? 'Current' : undefined, onSelect: () => ui.setSort({ column: clicked, direction: 'desc' }) },
      { key: 'none', label: 'Default order', icon: RotateCcw, disabled: !sort, onSelect: () => ui.setSort(null) },
      { type: 'separator', key: 's-sort' },
    );
  }
  if (clicked && isHeaderColumn(clicked)) {
    items.push({ key: 'remove-header', label: `Remove ${columnLabel(clicked)}`, icon: X, onSelect: () => removeHeaderColumn(clicked) }, { type: 'separator', key: 's-remove' });
  }
  items.push({ type: 'label', key: 'cols', label: 'Columns' });
  for (const def of TRAFFIC_COLUMNS) {
    if (def.fixed) continue;
    const cfg = ui.columns.find((c) => c.id === def.id);
    items.push({
      type: 'checkbox',
      key: `col-${def.id}`,
      label: columnLabel(def.id),
      checked: cfg?.visible ?? def.defaultVisible,
      onCheckedChange: (v) => useUiStore.getState().setColumnVisible(def.id, v),
    });
  }
  const headerCols = ui.columns.filter((c) => isHeaderColumn(c.id));
  if (headerCols.length) items.push({ type: 'label', key: 'header-cols', label: 'Header columns' });
  for (const c of headerCols) {
    items.push({ type: 'checkbox', key: `col-${c.id}`, label: columnLabel(c.id), checked: true, onCheckedChange: (v) => !v && removeHeaderColumn(c.id as HeaderColumnId) });
  }
  items.push(
    { key: 'add-header', label: 'Add header column…', icon: Plus, disabled: headerCols.length >= MAX_HEADER_COLUMNS, hint: headerCols.length >= MAX_HEADER_COLUMNS ? `${MAX_HEADER_COLUMNS} at most` : undefined, onSelect: openHeaderColumnDialog },
    { type: 'separator', key: 's-reset' },
    { key: 'reset', label: 'Reset columns', icon: Columns3, onSelect: () => useUiStore.getState().resetColumns() },
  );
  return items;
}

/** Right-click on the empty part of the list. */
export function listMenu(): MenuItems {
  const snap = trafficStore.getSnapshot();
  const list = trafficView.getList();
  return [
    { key: 'select-all', label: 'Select all', icon: CheckSquare, shortcut: 'mod+a', disabled: list.length === 0, onSelect: () => trafficStore.selectAll(trafficView.listArg()) },
    { key: 'export', label: 'Export visible as HAR', icon: FileDown, disabled: list.length === 0, onSelect: () => exportHar('visible') },
    { type: 'separator', key: 's1' },
    { type: 'submenu', key: 'columns', label: 'Columns', icon: Columns3, items: columnItems() },
    { type: 'separator', key: 's2' },
    { key: 'clear', label: 'Clear traffic', icon: Trash2, shortcut: 'mod+shift+x', destructive: true, disabled: snap.order.length === 0, onSelect: () => void requestClear() },
  ];
}
