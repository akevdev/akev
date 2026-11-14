/**
 * Session token & base-URL resolution (HTTP API reference § Authentication).
 *
 * Priority:
 * 1. Desktop shell: `window.__AKEV__.token` / `.apiUrl`.
 * 2. `?token=` in the page URL (from `akev start`'s link) → remembered in localStorage for this
 *    daemon origin and stripped from the address bar with `history.replaceState`, so it doesn't
 *    leak into screenshots, bookmarks or Referer headers. The UI never puts it back into the
 *    page URL.
 * 3. The token remembered for this daemon origin (any tab, bookmarks, reloads).
 *
 * A remembered token goes stale when the daemon restarts with a new one: `main.tsx` checks it
 * before connecting ({@link checkStoredToken}), forgets it when Akev rejects it, and the
 * "not authorized" screen explains why ({@link storedTokenWasStale}).
 *
 * The `?token=` link is the only browser login: the daemon never accepts the token from a
 * cookie (cookies aren't isolated by port). Without a token every API request is rejected
 * and the UI asks for the link again.
 */

import { storageGet, storageRemove, storageSet } from '../lib/storage';
import type { AkevDesktopGlobals } from './types';

/** Prefix of the localStorage key the token is remembered under ({@link tokenStorageKey}). */
const TOKEN_STORAGE_KEY = 'akev.token';

/** localStorage key of the token remembered for one daemon origin (`http://127.0.0.1:45456`). */
export function tokenStorageKey(apiOrigin: string): string {
  return `${TOKEN_STORAGE_KEY}:${apiOrigin}`;
}

export interface ClientConfig {
  /** Absolute API origin (`http://127.0.0.1:45456`) or `''` for same-origin. No trailing slash. */
  baseUrl: string;
  /** Session token (sent as `Authorization: Bearer`), or `null` when none was provided. */
  token: string | null;
}

/** Where the token came from. */
export type TokenSource = 'desktop' | 'url' | 'stored' | null;

export interface ResolvedAuth {
  config: ClientConfig;
  source: TokenSource;
  /** localStorage key the token is remembered under (per daemon origin). */
  storageKey: string;
}

export interface AuthEnvironment {
  location: Pick<Location, 'href' | 'search' | 'origin'>;
  history?: Pick<History, 'replaceState' | 'state'>;
  desktop?: AkevDesktopGlobals;
  /** `import.meta.env.VITE_AKEV_URL` (dev override). */
  envUrl?: string;
  /** localStorage access (injectable for tests). */
  getStored?: (key: string) => string | null;
  setStored?: (key: string, value: string) => void;
}

function trimSlash(url: string): string {
  return url.replace(/\/+$/, '');
}

/** URL with the `token` query parameter removed (hash and other params preserved). */
export function stripTokenFromUrl(href: string): string {
  const url = new URL(href);
  url.searchParams.delete('token');
  // URLSearchParams re-serialises `?mock` as `?mock=`; keep flag-style params tidy.
  const search = url.searchParams
    .toString()
    .replace(/=(?=&|$)/g, '');
  return `${url.origin}${url.pathname}${search ? `?${search}` : ''}${url.hash}`;
}

/** The daemon's origin: the API base URL's, else the page's own (same-origin UI). */
export function apiOrigin(baseUrl: string, pageOrigin: string): string {
  if (!baseUrl) return pageOrigin;
  try {
    return new URL(baseUrl).origin;
  } catch {
    return baseUrl;
  }
}

/**
 * Resolve the client configuration: remembers a URL token for this daemon origin and removes it
 * from the address bar. Safe to call more than once.
 */
export function resolveAuth(env: AuthEnvironment): ResolvedAuth {
  const getStored = env.getStored ?? ((k: string) => storageGet(k));
  const setStored = env.setStored ?? ((k: string, v: string) => void storageSet(k, v));

  const baseUrl = trimSlash(env.desktop?.apiUrl || env.envUrl || '');
  const storageKey = tokenStorageKey(apiOrigin(baseUrl, env.location.origin));

  if (env.desktop?.token) return { config: { baseUrl, token: env.desktop.token }, source: 'desktop', storageKey };

  let urlToken: string | null = null;
  try {
    urlToken = new URLSearchParams(env.location.search).get('token');
  } catch {
    urlToken = null;
  }
  if (urlToken) {
    setStored(storageKey, urlToken);
    try {
      env.history?.replaceState(env.history.state, '', stripTokenFromUrl(env.location.href));
    } catch {
      // replaceState can throw for opaque origins; the token still works.
    }
    return { config: { baseUrl, token: urlToken }, source: 'url', storageKey };
  }

  const stored = getStored(storageKey);
  return { config: { baseUrl, token: stored || null }, source: stored ? 'stored' : null, storageKey };
}

/** Resolve from the real browser environment. */
export function resolveBrowserAuth(): ResolvedAuth {
  return resolveAuth({
    location: window.location,
    history: window.history,
    desktop: window.__AKEV__,
    envUrl: import.meta.env.VITE_AKEV_URL as string | undefined,
  });
}

// =====================================================================================
// Stale remembered tokens
// =====================================================================================

let staleToken = false;

/** Whether the remembered token was rejected at startup (Akev restarted with a new one). */
export function storedTokenWasStale(): boolean {
  return staleToken;
}

/** The outcome of checking a remembered token. */
export type TokenCheck = 'valid' | 'stale' | 'unknown';

/**
 * Check a remembered token before connecting. `probe` performs an authenticated request and
 * resolves `true` when it succeeds, `false` when Akev rejects the token (401), and throws for
 * anything else (Akev not running yet, network trouble): the token is kept then, and the usual
 * connection flow takes over. A rejected token is forgotten.
 */
export async function checkStoredToken(auth: ResolvedAuth, probe: () => Promise<boolean>, removeStored: (key: string) => void = (k) => void storageRemove(k)): Promise<TokenCheck> {
  if (auth.source !== 'stored' || !auth.config.token) return 'unknown';
  let ok: boolean;
  try {
    ok = await probe();
  } catch {
    return 'unknown';
  }
  if (ok) return 'valid';
  removeStored(auth.storageKey);
  staleToken = true;
  return 'stale';
}
