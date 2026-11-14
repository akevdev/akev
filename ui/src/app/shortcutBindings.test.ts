import { afterEach, describe, expect, test } from 'bun:test';
import {
  bindingDefault,
  bindingId,
  comboFromEvent,
  dispatchShortcutEvent,
  displayCombo,
  getShortcutDocs,
  normalizeCombo,
  platformCombo,
  registerShortcut,
  setShortcutOverrides,
} from './shortcuts';

function press(init: Partial<KeyboardEvent>): KeyboardEvent & { prevented: boolean } {
  const e = {
    key: '',
    code: '',
    ctrlKey: false,
    metaKey: false,
    altKey: false,
    shiftKey: false,
    isComposing: false,
    defaultPrevented: false,
    target: null,
    prevented: false,
    preventDefault() {
      e.prevented = true;
      e.defaultPrevented = true;
    },
    ...init,
  };
  return e as unknown as KeyboardEvent & { prevented: boolean };
}

afterEach(() => setShortcutOverrides({}));

describe('shortcut bindings', () => {
  test('combos normalise and resolve per platform', () => {
    expect(normalizeCombo('Shift+Mod+K')).toBe('mod+shift+k');
    expect(normalizeCombo('mod+space')).toBe('mod+space');
    expect(platformCombo('mod+k', false)).toBe('ctrl+k');
    expect(platformCombo('mod+k', true)).toBe('meta+k');
    expect(platformCombo('ctrl+k', false)).toBe(platformCombo('mod+k', false));
    // Shifted symbols match whatever Shift says.
    expect(platformCombo('shift+?', false)).toBe(platformCombo('?', false));
    const id = bindingId('Traffic', 'Clear traffic', 'Shift+Mod+X');
    expect(id).toBe('Traffic::Clear traffic::mod+shift+x');
    expect(bindingDefault(id)).toBe('mod+shift+x');
  });

  test('recording a key press', () => {
    expect(comboFromEvent(press({ key: 'k', code: 'KeyK', ctrlKey: true }), false)).toBe('mod+k');
    expect(comboFromEvent(press({ key: 'k', code: 'KeyK', metaKey: true }), true)).toBe('mod+k');
    expect(comboFromEvent(press({ key: 'K', code: 'KeyK', ctrlKey: true, shiftKey: true }), false)).toBe('mod+shift+k');
    expect(comboFromEvent(press({ key: 'Control', code: 'ControlLeft', ctrlKey: true }), false)).toBeNull();
    expect(comboFromEvent(press({ key: 'F2', code: 'F2' }), false)).toBe('f2');
    expect(comboFromEvent(press({ key: ' ', code: 'Space', altKey: true }), false)).toBe('alt+space');
    // Option on macOS changes the character: the physical key is used.
    expect(comboFromEvent(press({ key: 'ñ', code: 'KeyN', altKey: true }), true)).toBe('alt+n');
    // Non-Latin layouts too.
    expect(comboFromEvent(press({ key: 'л', code: 'KeyK', ctrlKey: true }), false)).toBe('mod+k');
    // What's recorded fires again.
    expect(normalizeCombo(comboFromEvent(press({ key: '?', code: 'Slash', shiftKey: true }), false) ?? '')).toBe('shift+?');
  });

  test('a rebound shortcut fires on its new keys only and documents them', () => {
    let fired = 0;
    const off = registerShortcut('mod+alt+shift+f11', () => void fired++, { description: 'Rebind me', group: 'Test' });
    const id = bindingId('Test', 'Rebind me', 'mod+alt+shift+f11');
    const old = () => press({ key: 'F11', code: 'F11', ctrlKey: true, altKey: true, shiftKey: true });
    const next = () => press({ key: 'F10', code: 'F10', ctrlKey: true, altKey: true });
    dispatchShortcutEvent(old());
    expect(fired).toBe(1);
    setShortcutOverrides({ [id]: 'mod+alt+f10' });
    dispatchShortcutEvent(old());
    expect(fired).toBe(1);
    const e = next();
    dispatchShortcutEvent(e);
    expect(fired).toBe(2);
    expect(e.prevented).toBe(true);
    expect(getShortcutDocs().find((d) => d.description === 'Rebind me')).toMatchObject({ combo: 'mod+alt+f10', defaultCombo: 'mod+alt+shift+f11', binding: id });
    // Hints written with the default combo show the new keys.
    expect(displayCombo('mod+alt+shift+f11')).toBe('mod+alt+f10');
    expect(displayCombo('mod+k')).toBe('mod+k');
    // Resetting restores the default.
    setShortcutOverrides({});
    dispatchShortcutEvent(old());
    expect(fired).toBe(3);
    off();
  });

  test('a shortcut registered after the rebinding uses it; invalid overrides fall back', () => {
    const id = bindingId('Test', 'Later', 'mod+alt+f9');
    setShortcutOverrides({ [id]: 'mod+alt+f8', [bindingId('Test', 'Broken', 'mod+alt+f7')]: 'mod+shift' });
    let later = 0;
    let broken = 0;
    const offA = registerShortcut('mod+alt+f9', () => void later++, { description: 'Later', group: 'Test' });
    const offB = registerShortcut('mod+alt+f7', () => void broken++, { description: 'Broken', group: 'Test' });
    dispatchShortcutEvent(press({ key: 'F8', code: 'F8', ctrlKey: true, altKey: true }));
    dispatchShortcutEvent(press({ key: 'F7', code: 'F7', ctrlKey: true, altKey: true }));
    expect([later, broken]).toEqual([1, 1]);
    offA();
    offB();
    // With nothing registered, the hint follows the one override for that default.
    expect(displayCombo('mod+alt+f9')).toBe('mod+alt+f8');
  });

  test('shortcuts without a description keep their keys', () => {
    let n = 0;
    const off = registerShortcut('mod+alt+f6', () => void n++);
    setShortcutOverrides({ [bindingId('General', '', 'mod+alt+f6')]: 'mod+alt+f5' });
    dispatchShortcutEvent(press({ key: 'F6', code: 'F6', ctrlKey: true, altKey: true }));
    expect(n).toBe(1);
    off();
  });
});
