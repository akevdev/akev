/**
 * Pure reducers for the Compose tab strip (browser-like tabs). The store wraps these; tests
 * exercise them directly.
 */

import { isPristineBlank, newTab, type ComposeTab, type RequestDraft } from './model';

export interface TabsState {
  tabs: ComposeTab[];
  activeId: string;
  /** Recently closed tabs, most recent last (for "Reopen closed tab"). In memory only. */
  closed: ClosedTab[];
}

export interface ClosedTab {
  tab: ComposeTab;
  index: number;
}

export const CLOSED_LIMIT = 25;

export function initialTabs(tabs: ComposeTab[], activeId?: string | null): TabsState {
  const list = tabs.length ? tabs : [newTab()];
  const active = list.find((t) => t.id === activeId) ?? list[list.length - 1];
  return { tabs: list, activeId: (active as ComposeTab).id, closed: [] };
}

export function activeTab(s: TabsState): ComposeTab {
  return s.tabs.find((t) => t.id === s.activeId) ?? (s.tabs[0] as ComposeTab);
}

export interface OpenOptions {
  /** Replace the active tab when it's a pristine blank one (browser "new tab page" behaviour). */
  replacePristine?: boolean;
  /** Insert right after the active tab instead of at the end. */
  afterActive?: boolean;
  /** Don't select the new tab. */
  background?: boolean;
}

export function openTab(s: TabsState, tab: ComposeTab, opts: OpenOptions = {}): TabsState {
  const current = activeTab(s);
  if (opts.replacePristine && current && isPristineBlank(current)) {
    return { ...s, tabs: s.tabs.map((t) => (t.id === current.id ? tab : t)), activeId: opts.background ? s.activeId : tab.id };
  }
  const tabs = [...s.tabs];
  const at = opts.afterActive ? tabs.findIndex((t) => t.id === s.activeId) + 1 : tabs.length;
  tabs.splice(at <= 0 ? tabs.length : at, 0, tab);
  return { ...s, tabs, activeId: opts.background ? s.activeId : tab.id };
}

/** Close a tab. The last tab is replaced by a fresh blank one; focus moves right, else left. */
export function closeTab(s: TabsState, id: string): TabsState {
  const index = s.tabs.findIndex((t) => t.id === id);
  if (index < 0) return s;
  const tab = s.tabs[index] as ComposeTab;
  const closed = isPristineBlank(tab) ? s.closed : [...s.closed, { tab, index }].slice(-CLOSED_LIMIT);
  const tabs = s.tabs.filter((t) => t.id !== id);
  if (tabs.length === 0) {
    const fresh = newTab();
    return { tabs: [fresh], activeId: fresh.id, closed };
  }
  let activeId = s.activeId;
  if (s.activeId === id) activeId = (tabs[index] ?? tabs[index - 1] ?? tabs[0] as ComposeTab).id;
  return { tabs, activeId, closed };
}

/** Close several tabs at once (close others / to the right). */
export function closeTabs(s: TabsState, ids: readonly string[]): TabsState {
  let next = s;
  for (const id of ids) next = closeTab(next, id);
  return next;
}

export function closeOthers(s: TabsState, keepId: string): TabsState {
  const next = closeTabs(s, s.tabs.filter((t) => t.id !== keepId).map((t) => t.id));
  return { ...next, activeId: keepId };
}

export function closeToRight(s: TabsState, id: string): TabsState {
  const i = s.tabs.findIndex((t) => t.id === id);
  if (i < 0) return s;
  const next = closeTabs(s, s.tabs.slice(i + 1).map((t) => t.id));
  return next.tabs.some((t) => t.id === next.activeId) ? next : { ...next, activeId: id };
}

/** Reopen the most recently closed tab at its old position. */
export function reopenClosed(s: TabsState): TabsState {
  const last = s.closed[s.closed.length - 1];
  if (!last) return s;
  const closed = s.closed.slice(0, -1);
  // The id may have been reused (e.g. tab restored twice) — keep ids unique.
  const tab = s.tabs.some((t) => t.id === last.tab.id) ? { ...last.tab, id: `${last.tab.id}r` } : last.tab;
  const current = activeTab(s);
  let tabs: ComposeTab[];
  if (s.tabs.length === 1 && current && isPristineBlank(current)) tabs = [tab];
  else {
    tabs = [...s.tabs];
    tabs.splice(Math.min(last.index, tabs.length), 0, tab);
  }
  return { tabs, activeId: tab.id, closed };
}

export function moveTab(s: TabsState, from: number, to: number): TabsState {
  if (from === to || from < 0 || to < 0 || from >= s.tabs.length || to >= s.tabs.length) return s;
  const tabs = [...s.tabs];
  const [moved] = tabs.splice(from, 1);
  if (!moved) return s;
  tabs.splice(to, 0, moved);
  return { ...s, tabs };
}

export function selectTab(s: TabsState, id: string): TabsState {
  if (s.activeId === id || !s.tabs.some((t) => t.id === id)) return s;
  return { ...s, activeId: id };
}

/** Select the tab `delta` positions away (wraps around). */
export function selectRelative(s: TabsState, delta: number): TabsState {
  const n = s.tabs.length;
  if (n < 2) return s;
  const i = s.tabs.findIndex((t) => t.id === s.activeId);
  const next = s.tabs[(((i + delta) % n) + n) % n] as ComposeTab;
  return { ...s, activeId: next.id };
}

/** Select by position (1-based; 9 = last, like browsers). */
export function selectIndex(s: TabsState, position: number): TabsState {
  const tab = position >= 9 ? s.tabs[s.tabs.length - 1] : s.tabs[position - 1];
  return tab ? { ...s, activeId: tab.id } : s;
}

export function patchTab(s: TabsState, id: string, patch: Partial<ComposeTab> | ((t: ComposeTab) => Partial<ComposeTab>)): TabsState {
  let changed = false;
  const tabs = s.tabs.map((t) => {
    if (t.id !== id) return t;
    const p = typeof patch === 'function' ? patch(t) : patch;
    changed = true;
    return { ...t, ...p };
  });
  return changed ? { ...s, tabs } : s;
}

export function updateDraft(s: TabsState, id: string, update: (d: RequestDraft) => RequestDraft): TabsState {
  return patchTab(s, id, (t) => ({ draft: update(t.draft) }));
}

export function duplicateTab(s: TabsState, id: string): TabsState {
  const i = s.tabs.findIndex((t) => t.id === id);
  const src = s.tabs[i];
  if (!src) return s;
  const copy = newTab(src.draft, { title: src.title ? `${src.title} copy` : undefined, requestTab: src.requestTab, responseTab: src.responseTab });
  const tabs = [...s.tabs];
  tabs.splice(i + 1, 0, copy);
  return { ...s, tabs, activeId: copy.id };
}
