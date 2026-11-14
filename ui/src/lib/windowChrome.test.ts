import { describe, expect, test } from 'bun:test';
import type { AkevDesktopGlobals } from '../api/types';
import { customTitleBarAvailable, isWindowDragTarget, parseDecorationLayout } from './windowChrome';

describe('parseDecorationLayout', () => {
  test('splits buttons by side and drops other entries', () => {
    expect(parseDecorationLayout('appmenu:minimize,maximize,close')).toEqual({ left: [], right: ['minimize', 'maximize', 'close'] });
    expect(parseDecorationLayout(':close')).toEqual({ left: [], right: ['close'] });
    expect(parseDecorationLayout('close,minimize,maximize:appmenu')).toEqual({ left: ['close', 'minimize', 'maximize'], right: [] });
    expect(parseDecorationLayout('icon , menu: close , spacer, minimize')).toEqual({ left: [], right: ['close', 'minimize'] });
  });

  test('no colon means everything is on the left; unknown layout gets all three on the right', () => {
    expect(parseDecorationLayout('minimize,maximize,close')).toEqual({ left: ['minimize', 'maximize', 'close'], right: [] });
    expect(parseDecorationLayout(null)).toEqual({ left: [], right: ['minimize', 'maximize', 'close'] });
    expect(parseDecorationLayout('')).toEqual({ left: [], right: [] });
  });
});

describe('isWindowDragTarget', () => {
  test('is false outside the DOM', () => {
    expect(isWindowDragTarget(null)).toBe(false);
    expect(isWindowDragTarget({} as EventTarget)).toBe(false);
  });
});

describe('customTitleBarAvailable', () => {
  const withGlobals = (g: AkevDesktopGlobals | undefined, run: () => void) => {
    const had = 'window' in globalThis;
    const prev = (globalThis as { window?: unknown }).window;
    (globalThis as { window?: unknown }).window = { __AKEV__: g };
    try {
      run();
    } finally {
      if (had) (globalThis as { window?: unknown }).window = prev;
      else delete (globalThis as { window?: unknown }).window;
    }
  };

  test('Windows desktop app: always (no layout is reported)', () => {
    withGlobals({ desktop: true, platform: 'windows', windowChrome: { custom: true } }, () => expect(customTitleBarAvailable()).toBe(true));
    withGlobals({ desktop: true, platform: 'windows', windowChrome: { custom: false } }, () => expect(customTitleBarAvailable()).toBe(true));
  });

  test('Linux desktop app: only with the buttons on the right', () => {
    withGlobals({ desktop: true, platform: 'linux', windowChrome: { custom: true, layout: 'appmenu:minimize,maximize,close' } }, () => expect(customTitleBarAvailable()).toBe(true));
    withGlobals({ desktop: true, platform: 'linux', windowChrome: { custom: false, layout: 'close,minimize,maximize:' } }, () => expect(customTitleBarAvailable()).toBe(false));
  });

  test('macOS and the browser: never', () => {
    withGlobals({ desktop: true, platform: 'macos', windowChrome: { custom: false } }, () => expect(customTitleBarAvailable()).toBe(false));
    withGlobals(undefined, () => expect(customTitleBarAvailable()).toBe(false));
  });
});
