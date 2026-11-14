/**
 * Tiny hash router (`#/traffic`, `#/traffic/123`, `#/rules/<id>`, `#/compose`, `#/connect`,
 * `#/settings/<section>`). Works identically in browsers and the Tauri webview.
 *
 * ```ts
 * const route = useRoute();              // { page: 'traffic', params: { id: '123' }, ... }
 * navigate('/rules/abc');                // push
 * navigate('/traffic/124', { replace: true }); // replace (coalesced, see below)
 * <a href={href('/connect')}>Connect</a>
 * ```
 *
 * Replace-navigations (e.g. selection changes while arrowing through the list) update the
 * in-memory route immediately but write `history.replaceState` at most every 200 ms — Chrome
 * rate-limits history calls and logs warnings when a list is scrubbed quickly.
 */

import { useSyncExternalStore } from 'react';

/** Top-level pages. `kitchen-sink` is the internal primitives showcase. */
export type PageId = 'traffic' | 'rules' | 'compose' | 'connect' | 'settings' | 'kitchen-sink' | 'not-found';

export interface RouteDef {
  page: PageId;
  /** Pattern with `:param` segments, e.g. `/traffic/:id`. */
  pattern: string;
}

/** Route table, most specific first. */
export const ROUTES: readonly RouteDef[] = [
  { page: 'traffic', pattern: '/traffic/:id' },
  { page: 'traffic', pattern: '/traffic' },
  { page: 'rules', pattern: '/rules/:id' },
  { page: 'rules', pattern: '/rules' },
  { page: 'compose', pattern: '/compose/:tab' },
  { page: 'compose', pattern: '/compose' },
  { page: 'connect', pattern: '/connect/:interceptor' },
  { page: 'connect', pattern: '/connect' },
  { page: 'settings', pattern: '/settings/:section' },
  { page: 'settings', pattern: '/settings' },
  { page: 'kitchen-sink', pattern: '/_kitchen-sink' },
];

export interface Route {
  page: PageId;
  /** Normalised path without the leading `#`, e.g. `/traffic/123`. */
  path: string;
  /** Decoded `:param` values. */
  params: Readonly<Record<string, string>>;
  /** Query parameters after `?` inside the hash (`#/traffic?filter=x`). */
  query: URLSearchParams;
  /** The full hash string (for identity comparisons). */
  hash: string;
}

/** Match a path against a `:param` pattern. Returns params or `null`. */
export function matchPattern(pattern: string, path: string): Record<string, string> | null {
  const p = pattern.split('/').filter(Boolean);
  const s = path.split('/').filter(Boolean);
  if (p.length !== s.length) return null;
  const params: Record<string, string> = {};
  for (let i = 0; i < p.length; i++) {
    const pp = p[i] as string;
    const ss = s[i] as string;
    if (pp.startsWith(':')) {
      try {
        params[pp.slice(1)] = decodeURIComponent(ss);
      } catch {
        params[pp.slice(1)] = ss;
      }
    } else if (pp !== ss) return null;
  }
  return params;
}

/** Parse a location hash (`#/traffic/12?x=1`) into a {@link Route}. */
export function parseHash(hash: string, startPage: string = 'traffic'): Route {
  const raw = hash.startsWith('#') ? hash.slice(1) : hash;
  const qi = raw.indexOf('?');
  let path = qi >= 0 ? raw.slice(0, qi) : raw;
  const query = new URLSearchParams(qi >= 0 ? raw.slice(qi + 1) : '');
  path = `/${path.split('/').filter(Boolean).join('/')}`;
  if (path === '/') path = `/${startPage}`;
  for (const def of ROUTES) {
    const params = matchPattern(def.pattern, path);
    if (params) return { page: def.page, path, params, query, hash: `#${raw}` };
  }
  return { page: 'not-found', path, params: {}, query, hash: `#${raw}` };
}

/** Build an `href` for a path (`/rules/x` → `#/rules/x`). */
export function href(path: string): string {
  return `#${path.startsWith('/') ? path : `/${path}`}`;
}

// =====================================================================================
// Location store
// =====================================================================================

let startPage = 'traffic';
let currentHash = window.location.hash;
let currentRoute: Route = parseHash(currentHash, startPage);
const listeners = new Set<() => void>();
let pendingReplace: string | null = null;
let replaceTimer: ReturnType<typeof setTimeout> | null = null;
let lastReplaceAt = 0;
const REPLACE_INTERVAL = 200;

function emit(hash: string): void {
  if (hash === currentHash) return;
  currentHash = hash;
  currentRoute = parseHash(hash, startPage);
  for (const l of listeners) l();
}

function writeReplace(hash: string): void {
  pendingReplace = null;
  lastReplaceAt = Date.now();
  try {
    const url = `${window.location.pathname}${window.location.search}${hash}`;
    window.history.replaceState(window.history.state, '', url);
  } catch {
    // ignore (opaque origins)
  }
}

function flushPendingReplace(): void {
  if (replaceTimer != null) {
    clearTimeout(replaceTimer);
    replaceTimer = null;
  }
  if (pendingReplace != null) writeReplace(pendingReplace);
}

window.addEventListener('hashchange', () => {
  pendingReplace = null;
  if (replaceTimer != null) {
    clearTimeout(replaceTimer);
    replaceTimer = null;
  }
  emit(window.location.hash);
});

/** Set the page `/` redirects to (from UI prefs). */
export function setStartPage(page: string): void {
  if (page === startPage) return;
  startPage = page;
  currentRoute = parseHash(currentHash, startPage);
  for (const l of listeners) l();
}

/**
 * Navigate to a path (`/traffic/12`, optionally with `?query`).
 * `replace` swaps the current history entry (coalesced writes).
 */
export function navigate(path: string, options: { replace?: boolean } = {}): void {
  const hash = href(path);
  if (hash === currentHash) return;
  if (options.replace) {
    emit(hash);
    pendingReplace = hash;
    const wait = REPLACE_INTERVAL - (Date.now() - lastReplaceAt);
    if (wait <= 0) flushPendingReplace();
    else if (replaceTimer == null) replaceTimer = setTimeout(() => {
      replaceTimer = null;
      if (pendingReplace != null) writeReplace(pendingReplace);
    }, wait);
    return;
  }
  flushPendingReplace();
  emit(hash);
  window.location.hash = hash;
}

/** Current route (outside React). */
export function getRoute(): Route {
  return currentRoute;
}

export function subscribeRoute(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Current route; re-renders on navigation. */
export function useRoute(): Route {
  return useSyncExternalStore(subscribeRoute, getRoute, getRoute);
}

/** Re-renders only when the selected part of the route changes. */
export function useRouteParam(name: string): string | undefined {
  return useSyncExternalStore(
    subscribeRoute,
    () => currentRoute.params[name],
    () => currentRoute.params[name],
  );
}

/** Current page id; re-renders only on page changes. */
export function usePage(): PageId {
  return useSyncExternalStore(
    subscribeRoute,
    () => currentRoute.page,
    () => currentRoute.page,
  );
}
