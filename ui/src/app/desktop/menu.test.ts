import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { acceleratorOf, MENU_ACCELERATORS, MENU_IDS, MenuKeyDedupe, SAME_KEYPRESS_MS } from './menu';

const key = (code: string, mods: Partial<Record<'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey', boolean>> = {}) => ({ code, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods });

describe('menu accelerators', () => {
  test('keydowns as accelerators, from the physical key', () => {
    expect(acceleratorOf(key('KeyS', { metaKey: true }), true)).toBe('mod+s');
    expect(acceleratorOf(key('KeyE', { metaKey: true, shiftKey: true }), true)).toBe('mod+shift+e');
    expect(acceleratorOf(key('Digit2', { metaKey: true }), true)).toBe('mod+2');
    expect(acceleratorOf(key('Comma', { metaKey: true }), true)).toBe('mod+,');
    expect(acceleratorOf(key('KeyS', { ctrlKey: true }), false)).toBe('mod+s');
    // Not the platform's command key, or with Option.
    expect(acceleratorOf(key('KeyS', { ctrlKey: true }), true)).toBeNull();
    expect(acceleratorOf(key('KeyS', { metaKey: true, altKey: true }), true)).toBeNull();
    expect(acceleratorOf(key('KeyS'), true)).toBeNull();
  });

  test('the page handled the key: the menu event for it is dropped', () => {
    const d = new MenuKeyDedupe();
    expect(d.keyDown('mod+s', 1000, () => true)).toBe(false);
    expect(d.shouldRunMenu('save-session', 1050)).toBe(false);
    // The next keypress, unhandled, goes to the menu.
    expect(d.keyDown('mod+s', 3000, () => false)).toBe(false);
    expect(d.shouldRunMenu('save-session', 3040)).toBe(true);
  });

  test('the menu ran first: the keydown that follows is swallowed', () => {
    const d = new MenuKeyDedupe();
    expect(d.shouldRunMenu('toggle-recording', 1000)).toBe(true);
    expect(d.keyDown('mod+shift+r', 1030, () => false)).toBe(true);
    // Much later it's a new keypress.
    expect(d.keyDown('mod+shift+r', 1000 + SAME_KEYPRESS_MS + 1, () => false)).toBe(false);
  });

  test('a click on a menu item always runs', () => {
    const d = new MenuKeyDedupe();
    expect(d.shouldRunMenu('import', 5000)).toBe(true);
    expect(d.shouldRunMenu('shortcuts', 5001)).toBe(true);
    // A keypress handled long ago doesn't block a later click.
    d.keyDown('mod+o', 0, () => true);
    expect(d.shouldRunMenu('import', SAME_KEYPRESS_MS * 3)).toBe(true);
  });

  test('only menu accelerators are tracked', () => {
    const d = new MenuKeyDedupe();
    expect(d.covers('mod+s')).toBe(true);
    expect(d.covers('mod+j')).toBe(false);
    expect(d.covers(null)).toBe(false);
  });
});

describe('the desktop shell menu (desktop/src-tauri/src/lib.rs)', () => {
  const rust = readFileSync(join(import.meta.dir, '../../../../desktop/src-tauri/src/lib.rs'), 'utf8');
  const menu = rust.slice(rust.indexOf('fn build_menu'), rust.indexOf('fn handle_menu'));
  const handled = rust.slice(rust.indexOf('fn handle_menu'), rust.indexOf('fn open_url'));
  const items = [...menu.matchAll(/item\(\s*app,\s*"([a-z0-9-]+)",\s*"[^"]*",\s*(None|Some\("([^"]+)"\))/g)].map((m) => ({ id: m[1] as string, accel: m[3] }));
  // Ids the shell handles itself (zoom, reload, docs…); everything else is emitted to the UI.
  const native = new Set([...handled.matchAll(/"([a-z0-9-]+)" =>/g)].map((m) => m[1]));

  // `MENU_COMMANDS` (desktopIntegration.ts) is typed to have a command for every `MENU_IDS` entry.
  test('every item the shell forwards is handled', () => {
    const forwarded = items.filter((i) => !native.has(i.id)).map((i) => i.id);
    expect(forwarded.length).toBeGreaterThan(10);
    expect([...forwarded].sort()).toEqual([...MENU_IDS].sort());
  });

  test('accelerators match the menu', () => {
    for (const { id, accel } of items) {
      if (native.has(id)) continue;
      const expected = accel ? accel.replace('CmdOrCtrl', 'mod').toLowerCase() : undefined;
      expect([id, MENU_ACCELERATORS[id]]).toEqual([id, expected]);
    }
  });
});
