/**
 * Platform detection and environment integrations (clipboard, downloads, files, links).
 *
 * In the desktop app, Tauri's IPC provides native implementations ({@link tauriBridge}); in a
 * plain browser we use web APIs. Pages should always go through these helpers rather than
 * calling `navigator.clipboard` / creating `<a download>` themselves.
 */

import type { AkevDesktopBridge } from '../api/types';
import { toast } from '../components/ui/toast';

interface NavigatorUAData {
  platform?: string;
}

function detectMac(): boolean {
  const uaData = (navigator as Navigator & { userAgentData?: NavigatorUAData }).userAgentData;
  const platform = uaData?.platform ?? navigator.platform ?? '';
  if (/mac|iphone|ipad|ipod/i.test(platform)) return true;
  return /Mac OS X|Macintosh/.test(navigator.userAgent ?? '');
}

/** True on macOS / iOS: `Mod` means ⌘ instead of Ctrl. */
export const isMac: boolean = detectMac();

/** Running inside the Akev desktop shell. */
export function isDesktop(): boolean {
  return window.__AKEV__?.desktop === true || '__TAURI__' in window;
}

// -------------------------------------------------------------------------------------
// The desktop app: native integrations over Tauri's IPC
// -------------------------------------------------------------------------------------

type TauriInvoke = (cmd: string, args?: unknown, options?: { headers?: Record<string, string> }) => Promise<unknown>;

/** Tauri's `invoke` (`window.__TAURI__.core` with `withGlobalTauri`, or the internals). */
function tauriInvoke(): TauriInvoke | undefined {
  const w = window as { __TAURI__?: { core?: { invoke?: TauriInvoke } }; __TAURI_INTERNALS__?: { invoke?: TauriInvoke } };
  return w.__TAURI__?.core?.invoke ?? w.__TAURI_INTERNALS__?.invoke;
}

/** Header carrying the suggested name for the desktop shell's `save_file` command. */
export const FILE_NAME_HEADER = 'x-akev-file-name';

let tauriBridgeCache: AkevDesktopBridge | null = null;

/**
 * Native integrations in the desktop app: the webview drops `window.open` and browser-style
 * downloads, so links open through the opener plugin, files are saved through the shell's
 * `save_file` command (it shows the Save dialog itself) and notifications use the
 * notification plugin. File pickers keep using `<input type=file>`: the webview opens the
 * native chooser for those.
 */
export function tauriBridge(): AkevDesktopBridge | undefined {
  const invoke = tauriInvoke();
  if (!invoke) return undefined;
  if (tauriBridgeCache) return tauriBridgeCache;
  tauriBridgeCache = {
    openExternal: async (url) => {
      await invoke('plugin:opener|open_url', { url });
    },
    saveFile: async ({ suggestedName, url, data }) => {
      let bytes: Uint8Array;
      if (data != null) {
        bytes = typeof data === 'string' ? new TextEncoder().encode(data) : data;
      } else if (url) {
        const res = await fetch(url);
        if (!res.ok) throw new Error(`Download failed (HTTP ${res.status})`);
        bytes = new Uint8Array(await res.arrayBuffer());
      } else {
        return false;
      }
      const saved = await invoke('save_file', bytes, { headers: { [FILE_NAME_HEADER]: encodeURIComponent(suggestedName) } });
      return saved === true;
    },
    notify: (title, body) => {
      invoke('plugin:notification|notify', { options: { title, body } }).catch(() => {});
    },
    installUpdate: async () => (await invoke('install_update')) === true,
  };
  return tauriBridgeCache;
}

/** Test hook: forget the cached bridge (tests swap `window.__TAURI_INTERNALS__`). */
export function resetTauriBridgeForTests(): void {
  tauriBridgeCache = null;
}

/** A link that should open outside the app (another site or a mail link), or `null`. */
export function externalLinkTarget(href: string, base: string): string | null {
  let url: URL;
  try {
    url = new URL(href, base);
  } catch {
    return null;
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:' && url.protocol !== 'mailto:') return null;
  if (url.protocol !== 'mailto:' && url.origin === new URL(base).origin) return null;
  return url.href;
}

/**
 * In the desktop app, clicking a link to another site opens it in the default browser: the
 * webview would otherwise drop it (`target=_blank`) or navigate the app away. Links that
 * handle their own clicks (`preventDefault`) are left alone.
 */
export function installExternalLinkHandling(): void {
  if (!isDesktop()) return;
  const onClick = (e: MouseEvent) => {
    if (e.defaultPrevented || (e.button !== 0 && e.button !== 1)) return;
    const a = e.target instanceof Element ? e.target.closest('a[href]') : null;
    if (!(a instanceof HTMLAnchorElement)) return;
    const target = externalLinkTarget(a.getAttribute('href') ?? '', window.location.href);
    if (!target) return;
    e.preventDefault();
    openExternal(target);
  };
  document.addEventListener('click', onClick);
  document.addEventListener('auxclick', onClick);
}

/**
 * Copy text to the clipboard. Resolves `true` on success. Falls back to a hidden textarea +
 * `execCommand('copy')` where the async Clipboard API is unavailable (insecure contexts).
 */
export async function copyText(text: string): Promise<boolean> {
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
  } catch {
    // fall through to legacy path
  }
  try {
    const ta = document.createElement('textarea');
    ta.value = text;
    ta.setAttribute('readonly', '');
    ta.style.position = 'fixed';
    ta.style.opacity = '0';
    ta.style.pointerEvents = 'none';
    document.body.appendChild(ta);
    ta.select();
    const ok = document.execCommand('copy');
    ta.remove();
    return ok;
  } catch {
    return false;
  }
}

/**
 * Download a URL (e.g. an `/api/export/har?token=…` link). The desktop bridge gets a native
 * save dialog; browsers use a temporary `<a download>`.
 */
/** Save through the desktop app; a failure is shown (callers fire and forget). */
async function nativeSave(b: AkevDesktopBridge, options: Parameters<NonNullable<AkevDesktopBridge['saveFile']>>[0]): Promise<boolean> {
  try {
    return (await b.saveFile?.(options)) === true;
  } catch (e) {
    toast.error(`Couldn't save ${options.suggestedName}`, { description: e instanceof Error ? e.message : String(e) });
    return false;
  }
}

export async function downloadUrl(url: string, suggestedName: string): Promise<boolean> {
  const b = tauriBridge();
  if (b?.saveFile) return nativeSave(b, { suggestedName, url });
  const a = document.createElement('a');
  a.href = await withDownloadTicket(url);
  a.download = suggestedName;
  a.rel = 'noopener';
  document.body.appendChild(a);
  a.click();
  a.remove();
  return true;
}

/**
 * Swap the API token in a download URL for a one-time ticket (`POST /api/download-ticket`),
 * so the token never ends up in the browser's download history. Falls back to the URL as it
 * is when the daemon can't issue one.
 */
export async function withDownloadTicket(url: string): Promise<string> {
  let parsed: URL;
  try {
    parsed = new URL(url, window.location.href);
  } catch {
    return url;
  }
  const token = parsed.searchParams.get('token');
  if (!token || !parsed.pathname.startsWith('/api/')) return url;
  try {
    const res = await fetch(new URL('/api/download-ticket', parsed).toString(), {
      method: 'POST',
      headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' },
      body: JSON.stringify({ path: parsed.pathname }),
    });
    if (!res.ok) return url;
    const { ticket } = (await res.json()) as { ticket?: string };
    if (!ticket) return url;
    parsed.searchParams.delete('token');
    parsed.searchParams.set('ticket', ticket);
    return parsed.toString();
  } catch {
    return url;
  }
}

/** Save generated data (text or bytes) as a file. */
export async function saveData(data: string | Uint8Array, suggestedName: string, mime = 'application/octet-stream'): Promise<boolean> {
  const b = tauriBridge();
  if (b?.saveFile) return nativeSave(b, { suggestedName, data });
  const part: BlobPart = typeof data === 'string' ? data : (data.slice().buffer as ArrayBuffer);
  const url = URL.createObjectURL(new Blob([part], { type: mime }));
  try {
    return await downloadUrl(url, suggestedName);
  } finally {
    setTimeout(() => URL.revokeObjectURL(url), 10_000);
  }
}

/** A file picked by the user. */
export interface PickedFile {
  name: string;
  data: Uint8Array;
}

/**
 * Let the user pick file(s). `accept` takes extensions/mime types (`['.har', 'application/json']`).
 * Resolves `null` when cancelled.
 */
export function pickFiles(options: { accept?: string[]; multiple?: boolean } = {}): Promise<PickedFile[] | null> {
  return new Promise((resolve) => {
    const input = document.createElement('input');
    input.type = 'file';
    if (options.accept) input.accept = options.accept.join(',');
    input.multiple = options.multiple ?? false;
    input.style.display = 'none';
    let settled = false;
    const finish = (value: PickedFile[] | null) => {
      if (settled) return;
      settled = true;
      input.remove();
      resolve(value);
    };
    input.addEventListener('change', async () => {
      const files = Array.from(input.files ?? []);
      if (files.length === 0) return finish(null);
      const picked = await Promise.all(
        files.map(async (f) => ({ name: f.name, data: new Uint8Array(await f.arrayBuffer()) })),
      );
      finish(picked);
    });
    input.addEventListener('cancel', () => finish(null));
    document.body.appendChild(input);
    input.click();
  });
}

/** Open a link in the system browser (desktop) or a new tab. */
export function openExternal(url: string): void {
  const b = tauriBridge();
  if (b?.openExternal) {
    Promise.resolve(b.openExternal(url)).catch((e: unknown) => console.warn(`[akev] couldn't open ${url}`, e));
    return;
  }
  window.open(url, '_blank', 'noopener,noreferrer');
}

/** Show an OS notification when the desktop bridge supports it (no-op in browsers). */
export function notifyNative(title: string, body: string): void {
  tauriBridge()?.notify?.(title, body);
}

/**
 * Whether an OS notification is worth showing: the window is hidden, minimised or simply
 * not the focused one (the user is working in another app).
 */
export function windowInBackground(doc: Pick<Document, 'visibilityState' | 'hasFocus'> = document): boolean {
  return doc.visibilityState === 'hidden' || !doc.hasFocus();
}

// -------------------------------------------------------------------------------------
// The desktop app: native file pickers, window and events
// -------------------------------------------------------------------------------------

/** Whether this window can show a native file / folder picker that returns a path. */
export function canPickNativePath(): boolean {
  return tauriInvoke() != null;
}

/**
 * Ask for a file or folder with the native dialog and return its absolute path (desktop app
 * only: browsers never reveal paths). `null` when cancelled or unavailable.
 */
export async function pickNativePath(options: { directory: boolean; title?: string }): Promise<string | null> {
  const invoke = tauriInvoke();
  if (!invoke) return null;
  const picked = await invoke('plugin:dialog|open', { options: { directory: options.directory, multiple: false, ...(options.title ? { title: options.title } : {}) } });
  if (typeof picked === 'string') return picked;
  if (Array.isArray(picked) && typeof picked[0] === 'string') return picked[0];
  return null;
}

interface TauriGlobals {
  event?: { listen?: (event: string, handler: (e: { payload: unknown }) => void) => Promise<() => void> };
  window?: { getCurrentWindow?: () => { setTitle?: (title: string) => Promise<void> } };
}

function tauriGlobals(): TauriGlobals | undefined {
  return (window as { __TAURI__?: TauriGlobals }).__TAURI__;
}

/**
 * Listen to an event the desktop shell emits (`akev://menu`, `akev://imported`). A no-op in
 * browsers. Returns the unsubscribe function.
 */
export function listenDesktopEvent(event: string, handler: (payload: unknown) => void): () => void {
  const listen = tauriGlobals()?.event?.listen;
  if (!listen) return () => {};
  let unlisten: (() => void) | null = null;
  let disposed = false;
  listen(event, (e) => handler(e.payload)).then(
    (u) => {
      if (disposed) u();
      else unlisten = u;
    },
    (e: unknown) => console.warn(`[akev] couldn't listen to ${event}`, e),
  );
  return () => {
    disposed = true;
    unlisten?.();
  };
}

/** Set the desktop window's title (the taskbar, Alt-Tab and the Window menu show it). */
export function setDesktopWindowTitle(title: string): void {
  const win = tauriGlobals()?.window?.getCurrentWindow?.();
  win?.setTitle?.(title).catch((e: unknown) => console.warn("[akev] couldn't set the window title", e));
}
