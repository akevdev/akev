/**
 * Traffic sidebar state shared with the rest of the page (kept small: the sidebar itself is
 * lazy): whether it's open, the pinned hosts (both persisted UI prefs), and applying a sidebar
 * item as a clause of the traffic filter.
 */

import { trafficStore } from '../../../state/traffic';
import { usePagePref, useUiStore } from '../../../state/ui';
import { appendClause } from '../filter/filterEdit';
import { hasClause, removeClause } from './clauses';

const OPEN_PREF = 'traffic.sidebar';
const PINNED_PREF = 'traffic.pinnedHosts';

export function useSidebarOpen(): [boolean, (open: boolean) => void] {
  const [open, setOpen] = usePagePref<boolean>(OPEN_PREF, false);
  return [open === true, setOpen];
}

export function toggleSidebar(): void {
  const ui = useUiStore.getState();
  ui.setPagePref(OPEN_PREF, ui.pages[OPEN_PREF] !== true);
}

function readPinned(v: unknown): string[] {
  return Array.isArray(v) ? v.filter((h): h is string => typeof h === 'string' && h.length > 0) : [];
}

export function pinnedHosts(): string[] {
  return readPinned(useUiStore.getState().pages[PINNED_PREF]);
}

export function usePinnedHosts(): string[] {
  const raw = useUiStore((s) => s.pages[PINNED_PREF]);
  return readPinned(raw);
}

export function isPinnedHost(host: string): boolean {
  return pinnedHosts().includes(host);
}

/** Pin a host to the top of the sidebar, or unpin it. */
export function togglePinnedHost(host: string): void {
  const list = pinnedHosts();
  const next = list.includes(host) ? list.filter((h) => h !== host) : [...list, host];
  useUiStore.getState().setPagePref(PINNED_PREF, next);
}

/** The clause the sidebar added last (replaced when another item is picked). */
let current: string | null = null;

/**
 * Narrow the traffic list to a sidebar item: its clause is ANDed onto the user's filter text
 * (replacing the item picked before), or removed when it's already there.
 */
export function toggleScope(itemClause: string): void {
  const text = trafficStore.getSnapshot().filter.text;
  let next: string;
  if (hasClause(text, itemClause)) {
    next = removeClause(text, itemClause);
    if (current === itemClause) current = null;
  } else {
    const base = current && current !== itemClause && hasClause(text, current) ? removeClause(text, current) : text;
    next = appendClause(base, itemClause);
    current = itemClause;
  }
  trafficStore.setFilterText(next);
  trafficStore.flushFilter();
  if (next.trim()) useUiStore.getState().pushRecentFilter(next);
}

/** Remove a sidebar clause from the filter (the ✕ on the scope bar). */
export function clearScope(itemClause: string): void {
  const text = trafficStore.getSnapshot().filter.text;
  if (!hasClause(text, itemClause)) return;
  if (current === itemClause) current = null;
  trafficStore.setFilterText(removeClause(text, itemClause));
  trafficStore.flushFilter();
}
