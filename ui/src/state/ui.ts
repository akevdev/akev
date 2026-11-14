/**
 * Persisted UI preferences (theme, density, scale, columns, split ratios, last tabs, saved
 * filters, per-page prefs).
 *
 * Persistence is two-tier:
 * 1. `localStorage` (`akev.ui.v1`) — synchronous, read before first paint
 *    (`public/theme-boot.js`), so the theme never flashes.
 * 2. The daemon's `settings.ui` via `PATCH /api/settings/ui` (debounced) — shared between
 *    the browser UI and the desktop app. Conflicts resolve by `updatedAt` (newest wins).
 *
 * Page engineers: store page-specific preferences with {@link usePagePref} (namespaced under
 * `pages`) instead of adding top-level fields, unless several pages share them.
 */

import { create } from 'zustand';
import { api } from '../api/client';
import { storageGetJson, storageSetJson } from '../lib/storage';
import { debounce } from '../lib/timing';

export const UI_STORAGE_KEY = 'akev.ui.v1';

export type ThemePref = 'system' | 'dark' | 'light' | 'midnight' | 'contrast';
export type ResolvedTheme = Exclude<ThemePref, 'system'>;
export type Density = 'compact' | 'default' | 'comfortable';
export type StartPage = 'traffic' | 'rules' | 'compose' | 'connect';
export type DetailPosition = 'auto' | 'right' | 'bottom';
export type TitleBarPref = 'akev' | 'system';

export const THEMES: readonly { id: ThemePref; label: string }[] = [
  { id: 'system', label: 'System' },
  { id: 'dark', label: 'Dark' },
  { id: 'light', label: 'Light' },
  { id: 'midnight', label: 'Midnight' },
  { id: 'contrast', label: 'High contrast' },
];

export const DENSITIES: readonly { id: Density; label: string; rowHeight: number }[] = [
  { id: 'compact', label: 'Compact', rowHeight: 26 },
  { id: 'default', label: 'Default', rowHeight: 30 },
  { id: 'comfortable', label: 'Comfortable', rowHeight: 34 },
];

export const UI_SCALES = [0.9, 1, 1.1, 1.2] as const;
export type UiScale = (typeof UI_SCALES)[number];

/** Traffic list columns (DESIGN.md §6.3). */
export type BuiltinColumnId =
  | 'indicator'
  | 'id'
  | 'method'
  | 'status'
  | 'host'
  | 'path'
  | 'type'
  | 'model'
  | 'size'
  | 'time'
  | 'source'
  | 'started'
  | 'protocol'
  | 'remoteIp'
  | 'rule';

/** A custom column showing the value of one request (`req:`) or response (`res:`) header. */
export type HeaderColumnId = `req:${string}` | `res:${string}`;

export type TrafficColumnId = BuiltinColumnId | HeaderColumnId;

/** Most custom header columns at once. */
export const MAX_HEADER_COLUMNS = 8;

const HEADER_COLUMN = /^(req|res):([!#$%&'*+.^_`|~0-9a-z-]{1,128})$/;

/** Whether `id` names a (well-formed) custom header column. */
export function isHeaderColumn(id: string): id is HeaderColumnId {
  return HEADER_COLUMN.test(id);
}

/** The column id for a header (names are case-insensitive: stored lowercase). */
export function headerColumnId(side: 'request' | 'response', name: string): HeaderColumnId {
  return `${side === 'request' ? 'req' : 'res'}:${name.trim().toLowerCase()}`;
}

/** Side and lowercase name of a header column. */
export function parseHeaderColumn(id: HeaderColumnId): { side: 'request' | 'response'; name: string } {
  return { side: id.startsWith('req:') ? 'request' : 'response', name: id.slice(4) };
}

/** `x-request-id` → `X-Request-Id` (how header names are usually written). */
export function headerDisplayName(name: string): string {
  return name.replace(/(^|-)([a-z])/g, (_, dash: string, c: string) => dash + c.toUpperCase());
}

/** The definition of a custom header column. */
export function headerColumnDef(id: HeaderColumnId): ColumnDef {
  return { id, label: headerDisplayName(parseHeaderColumn(id).name), defaultWidth: 140, minWidth: 60, defaultVisible: true };
}

/** Definition of any column, built-in or custom (`undefined` for unknown ids). */
export function columnDefOf(id: string): ColumnDef | undefined {
  return TRAFFIC_COLUMNS.find((c) => c.id === id) ?? (isHeaderColumn(id) ? headerColumnDef(id) : undefined);
}

export interface ColumnConfig {
  id: TrafficColumnId;
  visible: boolean;
  /** Width in px; `path` is flexible and uses this as its minimum. */
  width: number;
}

export interface ColumnDef {
  id: TrafficColumnId;
  label: string;
  defaultWidth: number;
  minWidth: number;
  defaultVisible: boolean;
  /** Can't be hidden or reordered. */
  fixed?: boolean;
  align?: 'start' | 'end';
}

export const TRAFFIC_COLUMNS: readonly ColumnDef[] = [
  { id: 'indicator', label: '', defaultWidth: 6, minWidth: 6, defaultVisible: true, fixed: true },
  { id: 'id', label: '#', defaultWidth: 56, minWidth: 40, defaultVisible: false, align: 'end' },
  { id: 'method', label: 'Method', defaultWidth: 64, minWidth: 52, defaultVisible: true },
  { id: 'status', label: 'Status', defaultWidth: 72, minWidth: 52, defaultVisible: true },
  { id: 'host', label: 'Host', defaultWidth: 200, minWidth: 80, defaultVisible: true },
  { id: 'path', label: 'Path', defaultWidth: 240, minWidth: 120, defaultVisible: true },
  { id: 'type', label: 'Type', defaultWidth: 76, minWidth: 56, defaultVisible: true },
  { id: 'model', label: 'Model', defaultWidth: 168, minWidth: 80, defaultVisible: false },
  { id: 'size', label: 'Size', defaultWidth: 76, minWidth: 56, defaultVisible: true, align: 'end' },
  { id: 'time', label: 'Time', defaultWidth: 76, minWidth: 56, defaultVisible: true, align: 'end' },
  { id: 'source', label: 'Source', defaultWidth: 120, minWidth: 64, defaultVisible: true },
  { id: 'started', label: 'Started', defaultWidth: 104, minWidth: 80, defaultVisible: false },
  { id: 'protocol', label: 'Protocol', defaultWidth: 64, minWidth: 48, defaultVisible: false },
  { id: 'remoteIp', label: 'Remote IP', defaultWidth: 128, minWidth: 80, defaultVisible: false },
  { id: 'rule', label: 'Rule', defaultWidth: 120, minWidth: 64, defaultVisible: false },
];

export interface SortState {
  column: TrafficColumnId;
  direction: 'asc' | 'desc';
}

export interface SavedFilter {
  name: string;
  query: string;
}

export interface UiPrefs {
  version: 1;
  /** Unix ms of the last local change (conflict resolution with the daemon copy). */
  updatedAt: number;
  theme: ThemePref;
  density: Density;
  scale: UiScale;
  /** Render traffic paths in the monospace font. */
  monoPaths: boolean;
  startPage: StartPage;
  detailPosition: DetailPosition;
  /** Desktop app on Linux: Akev's own window buttons in the toolbar, or the system title bar. */
  titleBar: TitleBarPref;
  columns: ColumnConfig[];
  sort: SortState | null;
  /** SplitPane ratios by pane id (0..1 = size of the first pane). */
  splitRatios: Record<string, number>;
  /** Last selected tab per tab-group id (e.g. `traffic.request`). */
  lastTabs: Record<string, string>;
  /** Most recent filter queries, newest first (max 20). */
  recentFilters: string[];
  savedFilters: SavedFilter[];
  /** Rebound keyboard shortcuts: binding id (`app/shortcuts.ts` `bindingId`) → combo. */
  shortcuts: Record<string, string>;
  /** Free-form per-page preferences (see {@link usePagePref}). */
  pages: Record<string, unknown>;
}

export function defaultColumns(): ColumnConfig[] {
  return TRAFFIC_COLUMNS.map((c) => ({ id: c.id, visible: c.defaultVisible, width: c.defaultWidth }));
}

export function defaultPrefs(): UiPrefs {
  return {
    version: 1,
    updatedAt: 0,
    theme: 'system',
    density: 'default',
    scale: 1,
    monoPaths: false,
    startPage: 'traffic',
    detailPosition: 'auto',
    titleBar: 'akev',
    columns: defaultColumns(),
    sort: null,
    splitRatios: {},
    lastTabs: {},
    recentFilters: [],
    savedFilters: [],
    shortcuts: {},
    pages: {},
  };
}

// =====================================================================================
// Sanitising (prefs come from localStorage / the daemon and may be old, new or corrupt)
// =====================================================================================

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null && !Array.isArray(v);
}

function oneOf<T extends string | number>(v: unknown, allowed: readonly T[], fallback: T): T {
  return allowed.includes(v as T) ? (v as T) : fallback;
}

function sanitizeColumns(v: unknown): ColumnConfig[] {
  const out: ColumnConfig[] = [];
  const seen = new Set<TrafficColumnId>();
  let headerColumns = 0;
  if (Array.isArray(v)) {
    for (const raw of v) {
      if (!isRecord(raw) || typeof raw.id !== 'string') continue;
      const def = columnDefOf(raw.id);
      if (!def || seen.has(def.id)) continue;
      if (isHeaderColumn(def.id)) {
        // Custom columns exist while shown; at most a handful.
        if (raw.visible === false || headerColumns >= MAX_HEADER_COLUMNS) continue;
        headerColumns++;
      }
      seen.add(def.id);
      const width = typeof raw.width === 'number' && Number.isFinite(raw.width) ? Math.max(def.minWidth, Math.min(2000, Math.round(raw.width))) : def.defaultWidth;
      out.push({ id: def.id, visible: def.fixed ? true : typeof raw.visible === 'boolean' ? raw.visible : def.defaultVisible, width });
    }
  }
  // Columns added in newer versions appear at their default position.
  TRAFFIC_COLUMNS.forEach((def, index) => {
    if (seen.has(def.id)) return;
    out.splice(Math.min(index, out.length), 0, { id: def.id, visible: def.defaultVisible, width: def.defaultWidth });
  });
  // The indicator is always first.
  const ind = out.findIndex((c) => c.id === 'indicator');
  if (ind > 0) out.unshift(...out.splice(ind, 1));
  return out;
}

function sanitizeNumberRecord(v: unknown, min: number, max: number): Record<string, number> {
  const out: Record<string, number> = {};
  if (isRecord(v)) for (const [k, n] of Object.entries(v)) if (typeof n === 'number' && n >= min && n <= max) out[k] = n;
  return out;
}

function sanitizeStringRecord(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  if (isRecord(v)) for (const [k, s] of Object.entries(v)) if (typeof s === 'string') out[k] = s;
  return out;
}

/** Shortcut bindings: short, non-empty combos (the registry ignores ones that don't parse). */
function sanitizeShortcuts(v: unknown): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [k, s] of Object.entries(sanitizeStringRecord(v))) if (k.includes('::') && s.trim() && s.length <= 64) out[k] = s.trim();
  return out;
}

/** Turn arbitrary JSON into valid prefs, filling defaults for anything missing/invalid. */
export function sanitizePrefs(input: unknown): UiPrefs {
  const d = defaultPrefs();
  if (!isRecord(input)) return d;
  const sort = isRecord(input.sort) && TRAFFIC_COLUMNS.some((c) => c.id === (input.sort as Record<string, unknown>).column)
    ? { column: (input.sort as Record<string, unknown>).column as TrafficColumnId, direction: oneOf((input.sort as Record<string, unknown>).direction, ['asc', 'desc'] as const, 'asc') }
    : null;
  return {
    version: 1,
    updatedAt: typeof input.updatedAt === 'number' && Number.isFinite(input.updatedAt) ? input.updatedAt : 0,
    theme: oneOf(input.theme, ['system', 'dark', 'light', 'midnight', 'contrast'] as const, d.theme),
    density: oneOf(input.density, ['compact', 'default', 'comfortable'] as const, d.density),
    scale: oneOf(input.scale, UI_SCALES, d.scale),
    monoPaths: typeof input.monoPaths === 'boolean' ? input.monoPaths : d.monoPaths,
    startPage: oneOf(input.startPage, ['traffic', 'rules', 'compose', 'connect'] as const, d.startPage),
    detailPosition: oneOf(input.detailPosition, ['auto', 'right', 'bottom'] as const, d.detailPosition),
    titleBar: oneOf(input.titleBar, ['akev', 'system'] as const, d.titleBar),
    columns: sanitizeColumns(input.columns),
    sort,
    splitRatios: sanitizeNumberRecord(input.splitRatios, 0, 1),
    lastTabs: sanitizeStringRecord(input.lastTabs),
    recentFilters: Array.isArray(input.recentFilters)
      ? input.recentFilters.filter((q): q is string => typeof q === 'string' && q.length > 0).slice(0, 20)
      : [],
    savedFilters: Array.isArray(input.savedFilters)
      ? input.savedFilters
          .filter(isRecord)
          .filter((f) => typeof f.name === 'string' && typeof f.query === 'string')
          .map((f) => ({ name: f.name as string, query: f.query as string }))
      : [],
    shortcuts: sanitizeShortcuts(input.shortcuts),
    pages: isRecord(input.pages) ? { ...input.pages } : {},
  };
}

// =====================================================================================
// Store
// =====================================================================================

export interface UiActions {
  setTheme(theme: ThemePref): void;
  /** Cycles system → dark → light → system (nav rail toggle). */
  cycleTheme(): void;
  setDensity(density: Density): void;
  setScale(scale: UiScale): void;
  setMonoPaths(on: boolean): void;
  setStartPage(page: StartPage): void;
  setDetailPosition(position: DetailPosition): void;
  setTitleBar(titleBar: TitleBarPref): void;
  setColumns(columns: ColumnConfig[]): void;
  setColumnWidth(id: TrafficColumnId, width: number): void;
  setColumnVisible(id: TrafficColumnId, visible: boolean): void;
  /** Add a custom column showing a header's value. `false` when the name is invalid or there are already {@link MAX_HEADER_COLUMNS}. */
  addHeaderColumn(side: 'request' | 'response', name: string): boolean;
  /** Move a column from one index to another (indicator stays first). */
  moveColumn(from: number, to: number): void;
  resetColumns(): void;
  setSort(sort: SortState | null): void;
  setSplitRatio(id: string, ratio: number): void;
  setLastTab(group: string, tab: string): void;
  pushRecentFilter(query: string): void;
  saveFilter(filter: SavedFilter): void;
  removeSavedFilter(name: string): void;
  /** Rebind a shortcut (`null` restores its default). */
  setShortcut(binding: string, combo: string | null): void;
  /** Restore every shortcut's default keys. */
  resetShortcuts(): void;
  setPagePref(key: string, value: unknown): void;
  /** Apply prefs received from the daemon (hello / settings broadcast) when newer. */
  applyRemote(raw: unknown): void;
}

export type UiState = UiPrefs & UiActions;

let applyingRemote = false;

function prefsOf(state: UiState): UiPrefs {
  const {
    version,
    updatedAt,
    theme,
    density,
    scale,
    monoPaths,
    startPage,
    detailPosition,
    titleBar,
    columns,
    sort,
    splitRatios,
    lastTabs,
    recentFilters,
    savedFilters,
    shortcuts,
    pages,
  } = state;
  return { version, updatedAt, theme, density, scale, monoPaths, startPage, detailPosition, titleBar, columns, sort, splitRatios, lastTabs, recentFilters, savedFilters, shortcuts, pages };
}

const initial = sanitizePrefs(storageGetJson(UI_STORAGE_KEY));

export const useUiStore = create<UiState>()((set, get) => {
  /** Apply a local change and bump `updatedAt`. */
  const update = (partial: Partial<UiPrefs>) => set({ ...partial, updatedAt: Date.now() });
  return {
    ...initial,
    setTheme: (theme) => update({ theme }),
    cycleTheme: () => {
      const order: ThemePref[] = ['system', 'dark', 'light'];
      const i = order.indexOf(get().theme);
      update({ theme: order[(i + 1) % order.length] ?? 'system' });
    },
    setDensity: (density) => update({ density }),
    setScale: (scale) => update({ scale }),
    setMonoPaths: (monoPaths) => update({ monoPaths }),
    setStartPage: (startPage) => update({ startPage }),
    setDetailPosition: (detailPosition) => update({ detailPosition }),
    setTitleBar: (titleBar) => update({ titleBar }),
    setColumns: (columns) => update({ columns: sanitizeColumns(columns) }),
    setColumnWidth: (id, width) => {
      const def = columnDefOf(id);
      if (!def) return;
      const w = Math.max(def.minWidth, Math.min(2000, Math.round(width)));
      update({ columns: get().columns.map((c) => (c.id === id ? { ...c, width: w } : c)) });
    },
    setColumnVisible: (id, visible) => {
      if (TRAFFIC_COLUMNS.find((c) => c.id === id)?.fixed) return;
      // A hidden custom column is removed (it's added again from "Add header column…").
      if (isHeaderColumn(id) && !visible) return update({ columns: get().columns.filter((c) => c.id !== id) });
      update({ columns: get().columns.map((c) => (c.id === id ? { ...c, visible } : c)) });
    },
    addHeaderColumn: (side, name) => {
      const id = headerColumnId(side, name);
      if (!isHeaderColumn(id)) return false;
      const cols = get().columns;
      if (cols.some((c) => c.id === id)) return true;
      if (cols.filter((c) => isHeaderColumn(c.id)).length >= MAX_HEADER_COLUMNS) return false;
      update({ columns: [...cols, { id, visible: true, width: headerColumnDef(id).defaultWidth }] });
      return true;
    },
    moveColumn: (from, to) => {
      const cols = [...get().columns];
      if (from <= 0 || to <= 0 || from >= cols.length || to >= cols.length || from === to) return;
      const [moved] = cols.splice(from, 1);
      if (!moved) return;
      cols.splice(to, 0, moved);
      update({ columns: cols });
    },
    resetColumns: () => update({ columns: defaultColumns() }),
    setSort: (sort) => update({ sort }),
    setSplitRatio: (id, ratio) => {
      if (!Number.isFinite(ratio)) return;
      update({ splitRatios: { ...get().splitRatios, [id]: Math.max(0, Math.min(1, ratio)) } });
    },
    setLastTab: (group, tab) => {
      if (get().lastTabs[group] === tab) return;
      update({ lastTabs: { ...get().lastTabs, [group]: tab } });
    },
    pushRecentFilter: (query) => {
      const q = query.trim();
      if (!q) return;
      const recent = [q, ...get().recentFilters.filter((r) => r !== q)].slice(0, 20);
      update({ recentFilters: recent });
    },
    saveFilter: (filter) => {
      const others = get().savedFilters.filter((f) => f.name !== filter.name);
      update({ savedFilters: [...others, filter] });
    },
    removeSavedFilter: (name) => update({ savedFilters: get().savedFilters.filter((f) => f.name !== name) }),
    setShortcut: (binding, combo) => {
      const { [binding]: _, ...rest } = get().shortcuts;
      update({ shortcuts: combo ? { ...rest, [binding]: combo } : rest });
    },
    resetShortcuts: () => update({ shortcuts: {} }),
    setPagePref: (key, value) => update({ pages: { ...get().pages, [key]: value } }),
    applyRemote: (raw) => {
      if (raw == null || (isRecord(raw) && Object.keys(raw).length === 0)) {
        // Daemon has no prefs yet: seed it with ours.
        if (get().updatedAt > 0) pushRemote(prefsOf(get()));
        return;
      }
      const remote = sanitizePrefs(raw);
      const local = get();
      if (remote.updatedAt > local.updatedAt) {
        applyingRemote = true;
        try {
          set(remote);
        } finally {
          applyingRemote = false;
        }
      } else if (remote.updatedAt < local.updatedAt) {
        pushRemote(prefsOf(local));
      }
    },
  };
});

// =====================================================================================
// Persistence wiring
// =====================================================================================

let remoteEnabled = false;

const pushRemote = debounce((prefs: UiPrefs) => {
  if (!remoteEnabled) return;
  api.settings.patchUi(prefs).catch((e: unknown) => {
    // Offline or daemon restarting: the next change (or reconnect) will sync again.
    console.warn('[akev] failed to sync UI preferences', e);
  });
}, 600);

const writeLocal = debounce((prefs: UiPrefs) => storageSetJson(UI_STORAGE_KEY, prefs), 100);

useUiStore.subscribe((state, prev) => {
  if (state.updatedAt === prev.updatedAt && !applyingRemote) return;
  const prefs = prefsOf(state);
  writeLocal(prefs);
  if (!applyingRemote) pushRemote(prefs);
});

/** Enable syncing to the daemon (called once the stream delivers `hello`). */
export function enableRemotePrefsSync(enabled: boolean): void {
  remoteEnabled = enabled;
}

/** Flush pending writes (before unload). */
export function flushPrefs(): void {
  writeLocal.flush();
  pushRemote.flush();
}

window.addEventListener('pagehide', flushPrefs);

// =====================================================================================
// Hooks
// =====================================================================================

/** Row height in px for the current density (virtual lists need it in JS). */
export function useRowHeight(): number {
  const density = useUiStore((s) => s.density);
  return DENSITIES.find((d) => d.id === density)?.rowHeight ?? 30;
}

/**
 * A namespaced, persisted page preference.
 *
 * ```ts
 * const [layout, setLayout] = usePagePref<'list' | 'grid'>('connect.layout', 'grid');
 * ```
 * The value must be JSON-serialisable. Validate shape when reading complex values.
 */
export function usePagePref<T>(key: string, defaultValue: T): [T, (value: T) => void] {
  const stored = useUiStore((s) => s.pages[key]);
  const setPagePref = useUiStore((s) => s.setPagePref);
  const value = stored === undefined ? defaultValue : (stored as T);
  return [value, (v: T) => setPagePref(key, v)];
}

/** Last selected tab for a tab group, with a setter (for Tabs `value`/`onValueChange`). */
export function useLastTab(group: string, defaultTab: string): [string, (tab: string) => void] {
  const tab = useUiStore((s) => s.lastTabs[group]) ?? defaultTab;
  const setLastTab = useUiStore((s) => s.setLastTab);
  return [tab, (t: string) => setLastTab(group, t)];
}
