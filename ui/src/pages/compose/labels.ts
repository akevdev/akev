/**
 * Short, human labels for requests (tab titles, toasts) and the keyboard shortcuts Compose
 * advertises. Pure — no React.
 */

/** `api.example.com/v1/users` for a URL (scheme, query and fragment dropped). */
export function urlLabel(url: string): string {
  const u = url.trim().replace(/^[a-z][a-z0-9+.-]*:\/\//i, '');
  const cut = u.search(/[?#]/);
  return cut >= 0 ? u.slice(0, cut) : u;
}

/** Longest path label before it is shortened to its last segments. */
export const PATH_LABEL_MAX = 17;

/**
 * The most distinguishing part of a URL for a narrow tab: the path (`/v1/users`) — shortened
 * from the left to its last segments (`…/akev/issues`) when long — or the host when the path
 * is empty. Works with `{{variables}}` and scheme-less URLs.
 */
export function shortUrlLabel(url: string, max = PATH_LABEL_MAX): string {
  const noQuery = urlLabel(url);
  if (!noQuery) return '';
  const slash = noQuery.indexOf('/');
  const host = slash >= 0 ? noQuery.slice(0, slash) : noQuery;
  const path = slash >= 0 ? noQuery.slice(slash).replace(/\/+$/, '') : '';
  if (!path) return host;
  if (path.length <= max) return path;
  const segments = path.split('/').filter(Boolean);
  let out = '';
  for (let i = segments.length - 1; i >= 0; i--) {
    const next = `/${segments[i]}${out}`;
    if (out && next.length > max) break;
    out = next;
  }
  return `…${out}`;
}

/**
 * Default name for a saved request: its path (`/v1/users`), or the host when there is none.
 * The method isn't repeated — lists and tabs show it as a badge.
 */
export function suggestedRequestName(url: string): string {
  const noScheme = url.trim().replace(/^[a-z][a-z0-9+.-]*:\/\//i, '');
  const slash = noScheme.indexOf('/');
  const path = (slash >= 0 ? noScheme.slice(slash) : '').split(/[?#]/)[0] ?? '';
  const label = path && path !== '/' ? path : noScheme.split(/[?#/]/)[0] || 'Untitled request';
  return label.slice(0, 80);
}

export interface ComposeKeys {
  newTab: string;
  closeTab: string;
  reopenTab: string;
}

/**
 * Browsers reserve Ctrl/⌘ T, W and ⇧T for their own tabs, so the web UI advertises Alt
 * variants; the desktop shell gets the familiar ones. Both are always bound.
 */
export function composeKeys(desktop: boolean): ComposeKeys {
  return desktop ? { newTab: 'mod+t', closeTab: 'mod+w', reopenTab: 'mod+shift+t' } : { newTab: 'alt+t', closeTab: 'alt+w', reopenTab: 'alt+shift+t' };
}
