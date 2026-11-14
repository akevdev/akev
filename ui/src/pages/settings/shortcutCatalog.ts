/**
 * Settings → Shortcuts: the live shortcut registry (global + currently mounted) merged with a
 * reference list of page shortcuts (pages that aren't mounted while Settings is open don't
 * register theirs), grouped per action, plus the conflict check used when rebinding.
 */

import { bindingId, normalizeCombo, platformCombo, type ShortcutDoc } from '../../app/shortcuts';
import { composeKeys } from '../compose/labels';

interface ReferenceShortcut {
  combo: string;
  description: string;
  group: string;
  /** Handled by the component itself (list keys…): documented, not rebindable. */
  fixed?: boolean;
}

/**
 * Page shortcuts (DESIGN.md §4, §6–8). Rebindable entries mirror the `useShortcut` registrations
 * exactly (group, description and combo make the binding id); keep them in sync.
 */
function referenceShortcuts(desktop: boolean): ReferenceShortcut[] {
  const keys = composeKeys(desktop);
  return [
    { combo: 'mod+f', description: 'Focus the filter', group: 'Traffic' },
    { combo: '/', description: 'Focus the filter', group: 'Traffic' },
    { combo: 'mod+shift+x', description: 'Clear traffic (undoable)', group: 'Traffic' },
    { combo: 'mod+shift+d', description: 'Toggle detail pane position', group: 'Traffic' },
    { combo: 'mod+b', description: 'Show or hide the sidebar', group: 'Traffic' },
    { combo: 't', description: 'Switch between list and timeline', group: 'Traffic' },
    { combo: 'j', description: 'Next exchange', group: 'Traffic list' },
    { combo: 'k', description: 'Previous exchange', group: 'Traffic list' },
    { combo: 'arrowdown or arrowup', description: 'Move selection', group: 'Traffic list', fixed: true },
    { combo: 'shift+arrowdown or shift+arrowup', description: 'Extend selection', group: 'Traffic list', fixed: true },
    { combo: 'home or end', description: 'First / last exchange', group: 'Traffic list', fixed: true },
    { combo: 'pageup or pagedown', description: 'Page up / down', group: 'Traffic list', fixed: true },
    { combo: 'mod+a', description: 'Select all visible', group: 'Traffic list', fixed: true },
    { combo: 'enter', description: 'Focus the detail pane', group: 'Traffic list', fixed: true },
    { combo: 'escape', description: 'Deselect', group: 'Traffic list' },
    { combo: 'delete', description: 'Delete selected (undoable)', group: 'Traffic list' },
    { combo: 'mod+p', description: 'Pin / unpin', group: 'Traffic list' },
    { combo: 'mod+r', description: 'Replay', group: 'Traffic list' },
    { combo: 'mod+e', description: 'Edit in Compose', group: 'Traffic list' },
    { combo: 'mod+c', description: 'Copy URL(s)', group: 'Traffic list' },
    { combo: 'mod+shift+c', description: 'Copy as cURL', group: 'Traffic list' },
    { combo: 'mod+enter', description: 'Resume paused exchange', group: 'Breakpoints' },
    { combo: 'mod+enter', description: 'Forward the paused WebSocket message', group: 'Breakpoints' },
    { combo: 'alt+z', description: 'Toggle line wrap', group: 'Body viewer' },
    { combo: 'mod+f', description: 'Search in body', group: 'Body viewer' },
    { combo: 'mod+s', description: 'Download body', group: 'Body viewer' },
    { combo: 'n', description: 'New rule (last template)', group: 'Rules' },
    { combo: 'alt+n', description: 'New rule from a template…', group: 'Rules' },
    { combo: 'shift+n', description: 'New group', group: 'Rules' },
    { combo: 'mod+f', description: 'Search rules', group: 'Rules' },
    { combo: 'mod+z', description: 'Undo', group: 'Rules' },
    { combo: 'mod+shift+z', description: 'Redo', group: 'Rules' },
    { combo: 'mod+d', description: 'Duplicate rule', group: 'Rules' },
    { combo: 'mod+enter', description: 'Send request', group: 'Compose' },
    { combo: 'escape', description: 'Cancel request', group: 'Compose' },
    { combo: keys.newTab, description: 'New request tab', group: 'Compose' },
    { combo: keys.closeTab, description: 'Close tab', group: 'Compose' },
    { combo: keys.reopenTab, description: 'Reopen closed tab', group: 'Compose' },
    { combo: 'mod+s', description: 'Save request', group: 'Compose' },
    { combo: 'mod+shift+s', description: 'Save as…', group: 'Compose' },
    { combo: 'mod+l', description: 'Focus the URL', group: 'Compose' },
    { combo: 'mod+b', description: 'Toggle sidebar', group: 'Compose' },
    { combo: 'mod+shift+d', description: 'Response below / right', group: 'Compose' },
    { combo: 'ctrl+tab', description: 'Next tab', group: 'Compose' },
    { combo: 'ctrl+shift+tab', description: 'Previous tab', group: 'Compose' },
    { combo: 'shift+alt+f', description: 'Format JSON body', group: 'Compose' },
    { combo: 'mod+f', description: 'Search interceptors', group: 'Connect' },
  ];
}

/** Reference shortcuts as docs, with the user's rebindings applied. */
export function referenceDocs(overrides: Readonly<Record<string, string>>, desktop: boolean): ShortcutDoc[] {
  return referenceShortcuts(desktop).map((r) => {
    if (r.fixed || /\s+or\s+/.test(r.combo)) return { combo: r.combo, description: r.description, group: r.group };
    const binding = bindingId(r.group, r.description, r.combo);
    return { combo: overrides[binding] ?? r.combo, description: r.description, group: r.group, binding, defaultCombo: r.combo };
  });
}

export const SHORTCUT_GROUP_ORDER = ['General', 'Navigation', 'Traffic', 'Traffic list', 'Detail', 'Breakpoints', 'Body viewer', 'Code', 'Diff', 'Compare', 'Rules', 'Compose', 'Connect', 'Settings', 'Appearance'];

/** One key (or alternative keys) of an action. */
export interface CatalogBinding {
  /** Effective combo; documentation-only entries may list alternatives (`j or arrowdown`). */
  combo: string;
  /** Set for rebindable shortcuts. */
  binding?: string;
  defaultCombo?: string;
}

/** One action and its keys. */
export interface CatalogEntry {
  group: string;
  description: string;
  bindings: CatalogBinding[];
}

export interface ShortcutGroup {
  group: string;
  items: CatalogEntry[];
}

function comboKey(combo: string): string {
  return combo
    .split(/\s+or\s+/)
    .map((c) => {
      try {
        return normalizeCombo(c);
      } catch {
        return c.trim().toLowerCase();
      }
    })
    .join('|');
}

function docKey(d: ShortcutDoc): string {
  return d.binding ?? `${d.group}|${d.description}|${comboKey(d.combo)}`;
}

/**
 * Merge live + reference docs (live wins on the same binding, or group + combo + description for
 * documentation-only entries), filter by a query matching the description, group or key names,
 * and group per action in display order.
 */
export function groupShortcuts(live: readonly ShortcutDoc[], reference: readonly ShortcutDoc[], query = '', keyLabel: (combo: string) => string = (c) => c): ShortcutGroup[] {
  const merged = new Map<string, ShortcutDoc>();
  for (const d of live) merged.set(docKey(d), d);
  for (const d of reference) if (!merged.has(docKey(d))) merged.set(docKey(d), d);
  const q = query.trim().toLowerCase();
  const groups = new Map<string, Map<string, CatalogEntry>>();
  for (const d of merged.values()) {
    if (q) {
      const hay = `${d.description} ${d.group} ${d.combo} ${d.combo.split(/\s+or\s+/).map(keyLabel).join(' ')}`.toLowerCase();
      if (!q.split(/\s+/).every((t) => hay.includes(t))) continue;
    }
    const entries = groups.get(d.group) ?? new Map<string, CatalogEntry>();
    const entry = entries.get(d.description) ?? { group: d.group, description: d.description, bindings: [] };
    entry.bindings.push({ combo: d.combo, binding: d.binding, defaultCombo: d.defaultCombo });
    entries.set(d.description, entry);
    groups.set(d.group, entries);
  }
  return [...groups.entries()]
    .sort(([a], [b]) => {
      const ia = SHORTCUT_GROUP_ORDER.indexOf(a);
      const ib = SHORTCUT_GROUP_ORDER.indexOf(b);
      return (ia < 0 ? 99 : ia) - (ib < 0 ? 99 : ib) || a.localeCompare(b);
    })
    .map(([group, items]) => ({ group, items: [...items.values()] }));
}

// =====================================================================================
// Conflicts
// =====================================================================================

type Scope = 'global' | 'traffic' | 'viewer' | 'rules' | 'compose' | 'connect' | 'settings';

/** Where a group's shortcuts are active: global ones everywhere, page ones on their page. */
const SCOPES: Readonly<Record<string, Scope>> = {
  General: 'global',
  Navigation: 'global',
  Appearance: 'global',
  Traffic: 'traffic',
  'Traffic list': 'traffic',
  Detail: 'traffic',
  Breakpoints: 'traffic',
  'Body viewer': 'viewer',
  Code: 'viewer',
  Diff: 'viewer',
  Compare: 'viewer',
  Rules: 'rules',
  Compose: 'compose',
  Connect: 'connect',
  Settings: 'settings',
};

/** Body/code/diff viewers appear on the Traffic and Compose pages. */
function scopesOverlap(a: Scope, b: Scope): boolean {
  if (a === 'global' || b === 'global' || a === b) return true;
  const viewerPage = (s: Scope) => s === 'traffic' || s === 'compose';
  return (a === 'viewer' && viewerPage(b)) || (b === 'viewer' && viewerPage(a));
}

function scopeOf(group: string): Scope {
  return SCOPES[group] ?? 'global';
}

function samePress(a: string, b: string, mac: boolean): boolean {
  try {
    return platformCombo(a, mac) === platformCombo(b, mac);
  } catch {
    return false;
  }
}

/**
 * Actions that `combo` would collide with if `binding` (in `group`) were rebound to it: other
 * shortcuts with the same keys that can be active at the same time (global ones anywhere, page
 * ones on the same page).
 */
export function findConflicts(combo: string, binding: string, group: string, entries: readonly CatalogEntry[], mac: boolean): CatalogEntry[] {
  const scope = scopeOf(group);
  return entries.filter(
    (e) =>
      scopesOverlap(scope, scopeOf(e.group)) &&
      e.bindings.some((b) => b.binding !== binding && b.combo.split(/\s+or\s+/).some((c) => samePress(c, combo, mac))),
  );
}
