/**
 * The desktop window's title bar on Linux and Windows.
 *
 * The system title bar doesn't match the app (GTK even draws a plain one inside the window
 * under Wayland). So the desktop shell creates the window without system decorations (unless
 * Settings → Title bar says "System") and the UI draws the window buttons at the right end of
 * the top toolbar, with empty toolbar space (`data-window-drag`) dragging the window and
 * double-click maximising it. On Linux they follow the desktop's `gtk-decoration-layout`, and
 * a layout with buttons on the left keeps the system title bar: Akev only places them on the
 * right. Windows gets minimise, maximise, close (no layout is reported). macOS keeps its own.
 *
 * The shell reports how it created the window in `window.__AKEV__.windowChrome`.
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import { useAppStore } from '../state/app';
import { useUiStore } from '../state/ui';

export type WindowButton = 'minimize' | 'maximize' | 'close';

const BUTTONS: readonly string[] = ['minimize', 'maximize', 'close'];

/** The window buttons of a `gtk-decoration-layout` by side (`left:right`; other names dropped). */
export function parseDecorationLayout(layout: string | null | undefined): { left: WindowButton[]; right: WindowButton[] } {
  const pick = (part: string) => part.split(',').map((b) => b.trim()).filter((b): b is WindowButton => BUTTONS.includes(b));
  if (layout == null) return { left: [], right: ['minimize', 'maximize', 'close'] };
  const colon = layout.indexOf(':');
  // No colon: everything is on the left.
  if (colon < 0) return { left: pick(layout), right: [] };
  return { left: pick(layout.slice(0, colon)), right: pick(layout.slice(colon + 1)) };
}

/** The parts of Tauri's window API used here (`window.__TAURI__.window`, `withGlobalTauri`). */
export interface DesktopWindow {
  minimize(): Promise<void>;
  toggleMaximize(): Promise<void>;
  close(): Promise<void>;
  startDragging(): Promise<void>;
  isMaximized(): Promise<boolean>;
  setDecorations(decorations: boolean): Promise<void>;
  onResized(handler: () => void): Promise<() => void>;
}

export function desktopWindow(): DesktopWindow | null {
  const tauri = (window as { __TAURI__?: { window?: { getCurrentWindow?: () => DesktopWindow } } }).__TAURI__;
  try {
    return tauri?.window?.getCurrentWindow?.() ?? null;
  } catch {
    return null;
  }
}

function startup() {
  return window.__AKEV__;
}

/**
 * Whether this window can use Akev's title bar: the Windows desktop app, or the Linux one with
 * the desktop's window buttons on the right.
 */
export function customTitleBarAvailable(): boolean {
  const g = startup();
  if (!g?.windowChrome) return false;
  if (g.platform === 'windows') return true;
  return g.platform === 'linux' && parseDecorationLayout(g.windowChrome.layout).left.length === 0;
}

/** Whether the window currently has no system decorations (so Akev draws the buttons). */
const useChromeState = create<{ custom: boolean }>(() => ({ custom: startup()?.windowChrome?.custom === true }));

/** The buttons to draw, in order, or `null` while the system title bar is in use. */
export function useWindowButtons(): WindowButton[] | null {
  const custom = useChromeState((s) => s.custom);
  return custom ? parseDecorationLayout(startup()?.windowChrome?.layout).right : null;
}

/**
 * Apply Settings → Title bar to the window. Waits for the daemon's copy of the preferences
 * (`hello`): the shell created the window from that copy, and the local one may be stale.
 */
export function useTitleBarSync(): void {
  const pref = useUiStore((s) => s.titleBar);
  const connected = useAppStore((s) => s.status != null);
  useEffect(() => {
    if (!connected || !customTitleBarAvailable()) return;
    const want = pref === 'akev';
    if (useChromeState.getState().custom === want) return;
    const win = desktopWindow();
    if (!win) return;
    win.setDecorations(!want).then(
      () => useChromeState.setState({ custom: want }),
      (e: unknown) => console.warn("[akev] couldn't change the window decorations", e),
    );
  }, [pref, connected]);
}

/** Never start a window drag from these (inside a `data-window-drag` area). */
const INTERACTIVE = [
  'button',
  'a[href]',
  'input',
  'textarea',
  'select',
  'label',
  '[role="button"]',
  '[role="tab"]',
  '[role="menuitem"]',
  '[role="option"]',
  '[role="combobox"]',
  '[role="switch"]',
  '[role="checkbox"]',
  '[role="radio"]',
  '[role="slider"]',
  '[role="textbox"]',
  '[role="separator"][aria-orientation="vertical"][tabindex]',
  '[contenteditable]:not([contenteditable="false"])',
  '[data-no-window-drag]',
].join(',');

/** Whether a mouse event hit the empty part of a `data-window-drag` area. */
export function isWindowDragTarget(target: EventTarget | null): boolean {
  if (typeof Element === 'undefined' || !(target instanceof Element)) return false;
  const zone = target.closest('[data-window-drag]');
  if (!zone) return false;
  const control = target.closest(INTERACTIVE);
  return !(control && zone.contains(control));
}

/**
 * Drag the window from `data-window-drag` areas and maximise it on double-click. The drag
 * starts once the pointer moves a few pixels, so a double-click still reaches `dblclick`.
 */
export function useWindowDrag(enabled: boolean): void {
  useEffect(() => {
    if (!enabled) return;
    const win = desktopWindow();
    if (!win) return;
    let pressed: { x: number; y: number } | null = null;
    const down = (e: MouseEvent) => {
      if (e.button !== 0 || !isWindowDragTarget(e.target)) return;
      e.preventDefault(); // no text selection or focus change from the title area
      pressed = { x: e.screenX, y: e.screenY };
    };
    const move = (e: MouseEvent) => {
      if (!pressed) return;
      if ((e.buttons & 1) === 0) {
        pressed = null;
        return;
      }
      if (Math.abs(e.screenX - pressed.x) + Math.abs(e.screenY - pressed.y) < 4) return;
      pressed = null;
      void win.startDragging().catch(() => {});
    };
    const up = () => {
      pressed = null;
    };
    const dbl = (e: MouseEvent) => {
      if (e.button !== 0 || !isWindowDragTarget(e.target)) return;
      e.preventDefault();
      void win.toggleMaximize().catch(() => {});
    };
    document.addEventListener('mousedown', down, true);
    document.addEventListener('mousemove', move, true);
    document.addEventListener('mouseup', up, true);
    document.addEventListener('dblclick', dbl, true);
    return () => {
      document.removeEventListener('mousedown', down, true);
      document.removeEventListener('mousemove', move, true);
      document.removeEventListener('mouseup', up, true);
      document.removeEventListener('dblclick', dbl, true);
    };
  }, [enabled]);
}
