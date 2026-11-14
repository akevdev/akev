/**
 * Pure helpers for the Compose sidebar: request history and the saved-requests tree
 * (folders one level deep, drag & drop projection, keyboard moves).
 */

import { formatRelativeTime, formatTime } from '../../lib/format';
import type { ImportedCollection } from './importers';
import { HISTORY_LIMIT, type HistoryEntry, type SavedFolder, type SavedNode, type SavedRequest } from './model';

// =====================================================================================
// History
// =====================================================================================

export function addHistory(history: readonly HistoryEntry[], entry: HistoryEntry, limit = HISTORY_LIMIT): HistoryEntry[] {
  return [entry, ...history.filter((h) => h.id !== entry.id)].slice(0, limit);
}

/** Case-insensitive match on method, host, path, URL and status (all words must match). */
export function filterHistory(history: readonly HistoryEntry[], query: string): readonly HistoryEntry[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  if (!words.length) return history;
  return history.filter((h) => {
    const hay = `${h.method} ${h.host}${h.path} ${h.draft.url} ${h.status ?? ''} ${h.error ?? ''}`.toLowerCase();
    return words.every((w) => hay.includes(w));
  });
}

export type HistoryRow = { type: 'day'; key: string; label: string } | { type: 'entry'; key: string; entry: HistoryEntry };

function dayStart(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** Day label: Today, Yesterday, weekday (this week), or a date. */
export function dayLabel(ms: number, now: number = Date.now()): string {
  const diffDays = Math.round((dayStart(now) - dayStart(ms)) / 86_400_000);
  if (diffDays <= 0) return 'Today';
  if (diffDays === 1) return 'Yesterday';
  const d = new Date(ms);
  if (diffDays < 7) return d.toLocaleDateString(undefined, { weekday: 'long' });
  const sameYear = d.getFullYear() === new Date(now).getFullYear();
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric', ...(sameYear ? {} : { year: 'numeric' }) });
}

/**
 * Time shown on a history row: relative within today ("3m ago" — never in the future), the
 * clock time ("18:05") under older day headers so it can't contradict them.
 */
export function historyTimeLabel(at: number, now: number = Date.now()): string {
  const clock = Math.max(now, at);
  if (dayStart(at) === dayStart(clock)) return formatRelativeTime(at, clock);
  return formatTime(at, false).slice(0, 5);
}

/** Flatten history (newest first) into day headers + entries for a virtual list. */
export function groupHistory(entries: readonly HistoryEntry[], now: number = Date.now()): HistoryRow[] {
  const rows: HistoryRow[] = [];
  let currentDay = -1;
  for (const e of entries) {
    const day = dayStart(e.at);
    if (day !== currentDay) {
      currentDay = day;
      rows.push({ type: 'day', key: `d${day}`, label: dayLabel(e.at, now) });
    }
    rows.push({ type: 'entry', key: e.id, entry: e });
  }
  return rows;
}

// =====================================================================================
// Saved tree
// =====================================================================================

export function findSaved(nodes: readonly SavedNode[], id: string): SavedRequest | undefined {
  for (const n of nodes) {
    if (n.type === 'request' && n.id === id) return n;
    if (n.type === 'folder') {
      const c = n.children.find((r) => r.id === id);
      if (c) return c;
    }
  }
  return undefined;
}

export function findNode(nodes: readonly SavedNode[], id: string): SavedNode | undefined {
  for (const n of nodes) {
    if (n.id === id) return n;
    if (n.type === 'folder') {
      const c = n.children.find((r) => r.id === id);
      if (c) return c;
    }
  }
  return undefined;
}

/** Folder id containing a request (null = top level, undefined = not found). */
export function parentOf(nodes: readonly SavedNode[], id: string): string | null | undefined {
  for (const n of nodes) {
    if (n.id === id) return null;
    if (n.type === 'folder' && n.children.some((c) => c.id === id)) return n.id;
  }
  return undefined;
}

export function folders(nodes: readonly SavedNode[]): SavedFolder[] {
  return nodes.filter((n): n is SavedFolder => n.type === 'folder');
}

/** Add a request at the end of a folder (or the top level). */
export function addSaved(nodes: readonly SavedNode[], request: SavedRequest, folderId: string | null = null): SavedNode[] {
  if (folderId) {
    let found = false;
    const next = nodes.map((n) => {
      if (n.type === 'folder' && n.id === folderId) {
        found = true;
        return { ...n, collapsed: false, children: [...n.children, request] };
      }
      return n;
    });
    if (found) return next;
  }
  return [...nodes, request];
}

export function addFolder(nodes: readonly SavedNode[], folder: SavedFolder): SavedNode[] {
  return [...nodes, folder];
}

function mapRequests(nodes: readonly SavedNode[], fn: (r: SavedRequest) => SavedRequest): SavedNode[] {
  return nodes.map((n) => (n.type === 'folder' ? { ...n, children: n.children.map(fn) } : fn(n)));
}

export function updateSaved(nodes: readonly SavedNode[], id: string, patch: Partial<Omit<SavedRequest, 'type' | 'id'>>): SavedNode[] {
  return mapRequests(nodes, (r) => (r.id === id ? { ...r, ...patch } : r));
}

export function renameNode(nodes: readonly SavedNode[], id: string, name: string): SavedNode[] {
  const clean = name.trim();
  if (!clean) return nodes as SavedNode[];
  return nodes.map((n) => {
    if (n.id === id) return { ...n, name: clean };
    if (n.type === 'folder' && n.children.some((c) => c.id === id)) return { ...n, children: n.children.map((c) => (c.id === id ? { ...c, name: clean } : c)) };
    return n;
  });
}

export function setFolderCollapsed(nodes: readonly SavedNode[], id: string, collapsed: boolean): SavedNode[] {
  return nodes.map((n) => (n.type === 'folder' && n.id === id ? { ...n, collapsed } : n));
}

/** Remove a node (a folder takes its requests with it). */
export function removeNode(nodes: readonly SavedNode[], id: string): SavedNode[] {
  return nodes
    .filter((n) => n.id !== id)
    .map((n) => (n.type === 'folder' && n.children.some((c) => c.id === id) ? { ...n, children: n.children.filter((c) => c.id !== id) } : n));
}

export interface SavedRow {
  node: SavedNode;
  depth: 0 | 1;
  parentId: string | null;
  /** Folder rows: number of children. */
  count?: number;
}

/**
 * Visible rows: folders followed by their children (unless collapsed). With a query, only
 * matching requests (and folders whose name matches, or that contain matches) are shown,
 * expanded.
 */
export function flattenSaved(nodes: readonly SavedNode[], query = '', hideChildrenOf?: string | null): SavedRow[] {
  const q = query.trim().toLowerCase();
  const matches = (r: SavedRequest) => !q || `${r.name} ${r.draft.method} ${r.draft.url}`.toLowerCase().includes(q);
  const rows: SavedRow[] = [];
  for (const n of nodes) {
    if (n.type === 'request') {
      if (matches(n)) rows.push({ node: n, depth: 0, parentId: null });
      continue;
    }
    const folderMatches = !!q && n.name.toLowerCase().includes(q);
    const kids = q && !folderMatches ? n.children.filter(matches) : n.children;
    if (q && !folderMatches && kids.length === 0) continue;
    rows.push({ node: n, depth: 0, parentId: null, count: n.children.length });
    if ((!n.collapsed || q) && n.id !== hideChildrenOf) for (const c of kids) rows.push({ node: c, depth: 1, parentId: n.id });
  }
  return rows;
}

export interface DropTarget {
  parentId: string | null;
  /** Insert after this sibling (null = first). */
  afterId: string | null;
}

/** Horizontal drag distance (px) that moves a request out of its folder. */
export const OUTDENT_PX = 24;

/**
 * Where a dragged node lands, given the visible rows (as rendered during the drag), the row it
 * is over and the horizontal drag offset (dragging left out-dents the last child of a folder).
 */
export function projectDrop(rows: readonly SavedRow[], activeId: string, overId: string, deltaX = 0): DropTarget | null {
  const from = rows.findIndex((r) => r.node.id === activeId);
  const to = rows.findIndex((r) => r.node.id === overId);
  if (from < 0 || to < 0) return null;
  const moved = [...rows];
  const [active] = moved.splice(from, 1);
  if (!active) return null;
  moved.splice(to, 0, active);
  const above = moved[to - 1];
  const below = moved[to + 1];

  if (active.node.type === 'folder') {
    if (!above) return { parentId: null, afterId: null };
    return { parentId: null, afterId: above.parentId ?? above.node.id };
  }
  if (!above) return { parentId: null, afterId: null };
  if (above.node.type === 'folder') {
    const expanded = !above.node.collapsed;
    return expanded ? { parentId: above.node.id, afterId: null } : { parentId: null, afterId: above.node.id };
  }
  if (above.parentId) {
    const lastChild = !below || below.parentId !== above.parentId;
    if (lastChild && deltaX < -OUTDENT_PX) return { parentId: null, afterId: above.parentId };
    return { parentId: above.parentId, afterId: above.node.id };
  }
  return { parentId: null, afterId: above.node.id };
}

/** Move a node to a drop target. Folders can only live at the top level. */
export function moveNode(nodes: readonly SavedNode[], id: string, target: DropTarget): SavedNode[] {
  const node = findNode(nodes, id);
  if (!node || target.afterId === id) return nodes as SavedNode[];
  if (node.type === 'folder' && target.parentId) return nodes as SavedNode[];
  const without = removeNode(nodes, id);
  if (target.parentId) {
    if (node.type !== 'request') return nodes as SavedNode[];
    return without.map((n) => {
      if (n.type !== 'folder' || n.id !== target.parentId) return n;
      const children = [...n.children];
      const at = target.afterId ? children.findIndex((c) => c.id === target.afterId) + 1 : 0;
      children.splice(at, 0, node);
      return { ...n, collapsed: false, children };
    });
  }
  const out = [...without];
  const at = target.afterId ? out.findIndex((n) => n.id === target.afterId) + 1 : 0;
  out.splice(at, 0, node);
  return out;
}

/** Keyboard reorder within the same parent (−1 up, +1 down). */
export function moveWithinParent(nodes: readonly SavedNode[], id: string, delta: -1 | 1): SavedNode[] {
  const parent = parentOf(nodes, id);
  if (parent === undefined) return nodes as SavedNode[];
  const swap = <T extends { id: string }>(list: readonly T[]): T[] => {
    const i = list.findIndex((x) => x.id === id);
    const j = i + delta;
    if (i < 0 || j < 0 || j >= list.length) return list as T[];
    const out = [...list];
    [out[i], out[j]] = [out[j] as T, out[i] as T];
    return out;
  };
  if (parent === null) return swap(nodes);
  return nodes.map((n) => (n.type === 'folder' && n.id === parent ? { ...n, children: swap(n.children) } : n));
}

/** Move a request into a folder (append) or to the top level (null). */
export function moveToFolder(nodes: readonly SavedNode[], id: string, folderId: string | null): SavedNode[] {
  const node = findNode(nodes, id);
  if (!node || node.type !== 'request' || parentOf(nodes, id) === folderId) return nodes as SavedNode[];
  if (folderId) {
    const folder = nodes.find((n) => n.id === folderId);
    if (!folder || folder.type !== 'folder') return nodes as SavedNode[];
    return moveNode(nodes, id, { parentId: folderId, afterId: folder.children[folder.children.length - 1]?.id ?? null });
  }
  return moveNode(nodes, id, { parentId: null, afterId: nodes[nodes.length - 1]?.id ?? null });
}

/** A unique name ("Folder", "Folder 2", …). */
export function uniqueName(base: string, existing: readonly string[]): string {
  if (!existing.includes(base)) return base;
  for (let i = 2; ; i++) if (!existing.includes(`${base} ${i}`)) return `${base} ${i}`;
}

// =====================================================================================
// Imports
// =====================================================================================

/**
 * Saved folders for an imported collection: one folder per collection folder (named
 * `Collection / Folder`), plus one named after the collection for its top-level requests.
 * Folder names are made unique against the existing tree.
 */
export function collectionFolders(c: ImportedCollection, existing: readonly SavedNode[], makeId: (prefix: string) => string, now = Date.now()): SavedFolder[] {
  const taken = folders(existing).map((f) => f.name);
  const out: SavedFolder[] = [];
  const add = (name: string, requests: ImportedCollection['requests']) => {
    if (!requests.length) return;
    const unique = uniqueName(name, taken);
    taken.push(unique);
    out.push({
      type: 'folder',
      id: makeId('f'),
      name: unique,
      collapsed: false,
      children: requests.map((r) => ({ type: 'request', id: makeId('s'), name: r.name, draft: r.draft, updatedAt: now })),
    });
  };
  add(c.name, c.requests);
  for (const f of c.folders) add(`${c.name} / ${f.name}`, f.requests);
  return out;
}
