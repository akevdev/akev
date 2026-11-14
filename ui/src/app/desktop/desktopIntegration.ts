/**
 * Desktop-app integration that lives for the whole session (mounted once by the app shell):
 *
 * - `akev://menu` (macOS menu bar) → the matching command, deduplicated against UI shortcuts
 *   for the same accelerator (`menu.ts`);
 * - `akev://imported` (a capture file opened through a file association) → a toast with Show,
 *   or the error;
 * - the window title follows `document.title` (taskbar, Alt-Tab, the Window menu).
 *
 * Everything is a no-op in a browser.
 */

import { useEffect } from 'react';
import { requestClear } from '../../pages/traffic/actions/clear';
import { exportHar, toggleRecording } from '../../pages/traffic/actions/exchangeActions';
import { isDesktop, isMac, listenDesktopEvent, setDesktopWindowTitle } from '../../lib/platform';
import { openInCompose } from '../../state/intents';
import { togglePalette } from '../commands';
import { navigate } from '../router';
import { openCaptureFiles } from '../sessions/actions';
import { openSessionsDialog } from '../sessions/sessions';
import { setShortcutsOverlayOpen } from '../shortcutsOverlay';
import { announceImported } from './imported';
import { acceleratorOf, MenuKeyDedupe, type MenuId } from './menu';

/** What each menu item does. */
export const MENU_COMMANDS: Readonly<Record<MenuId, () => void>> = {
  'nav-traffic': () => navigate('/traffic'),
  'nav-rules': () => navigate('/rules'),
  'nav-compose': () => navigate('/compose'),
  'nav-connect': () => navigate('/connect'),
  settings: () => navigate('/settings'),
  import: () => void openCaptureFiles(),
  'export-har': () => exportHar('all'),
  'save-session': () => openSessionsDialog({ save: true }),
  'command-palette': () => togglePalette(),
  'toggle-recording': () => toggleRecording(),
  'clear-traffic': () => void requestClear(),
  'compose-new': () => openInCompose({ method: 'GET', url: '', headers: [], body: '' }),
  shortcuts: () => setShortcutsOverlayOpen(true),
};

/** Run a menu item's command. Returns whether the id was known. */
export function runMenuCommand(id: string): boolean {
  const run = (MENU_COMMANDS as Readonly<Record<string, (() => void) | undefined>>)[id];
  if (!run) return false;
  run();
  return true;
}

/** Mirror `document.title` into the window title. Returns the cleanup. */
function syncWindowTitle(): () => void {
  let last = '';
  const apply = () => {
    const title = document.title;
    if (!title || title === last) return;
    last = title;
    setDesktopWindowTitle(title);
  };
  apply();
  const observer = new MutationObserver(apply);
  observer.observe(document.head, { subtree: true, childList: true, characterData: true });
  return () => observer.disconnect();
}

/** Start listening (once, from the app shell). Returns the cleanup. */
export function startDesktopIntegration(): () => void {
  const dedupe = new MenuKeyDedupe();
  const cleanups: (() => void)[] = [];

  if (isMac) {
    // Capture phase: runs before the shortcut registry. Whether a page shortcut handled the
    // key (`defaultPrevented`) is read when the menu event comes in, after the dispatch.
    const onKeyDown = (e: KeyboardEvent) => {
      const accel = acceleratorOf(e, true);
      if (!dedupe.covers(accel)) return;
      if (dedupe.keyDown(accel, performance.now(), () => e.defaultPrevented)) {
        e.preventDefault();
        e.stopImmediatePropagation();
      }
    };
    window.addEventListener('keydown', onKeyDown, true);
    cleanups.push(() => window.removeEventListener('keydown', onKeyDown, true));
  }

  cleanups.push(
    listenDesktopEvent('akev://menu', (payload) => {
      if (typeof payload !== 'string' || !dedupe.shouldRunMenu(payload, performance.now())) return;
      runMenuCommand(payload);
    }),
    listenDesktopEvent('akev://imported', announceImported),
    syncWindowTitle(),
  );
  return () => {
    for (const c of cleanups) c();
  };
}

/** Mount once (app shell); a no-op outside the desktop app. */
export function useDesktopIntegration(): void {
  useEffect(() => (isDesktop() ? startDesktopIntegration() : undefined), []);
}
