import { describe, expect, test } from 'bun:test';
import { commandSignature, getCommands, registerCommands, type Command } from './commands';
import { scoreCommand } from './commandScore';
import { href, matchPattern, parseHash } from './router';
import { formatShortcut, matchesCombo, parseCombo, registerShortcut, shortcutLabel } from './shortcuts';
import { resolveTheme } from './theme';

describe('router', () => {
  test('matchPattern extracts decoded params', () => {
    expect(matchPattern('/traffic/:id', '/traffic/42')).toEqual({ id: '42' });
    expect(matchPattern('/rules/:id', '/rules/a%2Fb')).toEqual({ id: 'a/b' });
    expect(matchPattern('/rules/:id', '/rules/%E0%A4%A')).toEqual({ id: '%E0%A4%A' });
    expect(matchPattern('/traffic/:id', '/traffic')).toBeNull();
    expect(matchPattern('/traffic', '/rules')).toBeNull();
  });

  test('parseHash resolves pages, params and hash queries', () => {
    expect(parseHash('#/traffic/12')).toMatchObject({ page: 'traffic', path: '/traffic/12', params: { id: '12' } });
    expect(parseHash('#/settings/proxy').params).toEqual({ section: 'proxy' });
    expect(parseHash('#/connect/android-adb').params).toEqual({ interceptor: 'android-adb' });
    expect(parseHash('#/_kitchen-sink').page).toBe('kitchen-sink');
    const r = parseHash('#/traffic?filter=method%3Apost');
    expect(r.page).toBe('traffic');
    expect(r.query.get('filter')).toBe('method:post');
    // Trailing / duplicate slashes are normalised.
    expect(parseHash('#//rules///abc/').path).toBe('/rules/abc');
  });

  test('empty hash goes to the start page; unknown paths are not-found', () => {
    expect(parseHash('').page).toBe('traffic');
    expect(parseHash('#/', 'connect').page).toBe('connect');
    expect(parseHash('#/nope/what').page).toBe('not-found');
  });

  test('href', () => {
    expect(href('/rules/x')).toBe('#/rules/x');
    expect(href('compose')).toBe('#/compose');
  });
});

function key(init: Partial<KeyboardEvent>): KeyboardEvent {
  return { key: '', code: '', ctrlKey: false, metaKey: false, altKey: false, shiftKey: false, ...init } as KeyboardEvent;
}

describe('shortcuts', () => {
  test('parseCombo', () => {
    expect(parseCombo('mod+shift+K')).toEqual({ key: 'k', mod: true, ctrl: false, alt: false, shift: true, meta: false });
    expect(parseCombo('esc').key).toBe('escape');
    expect(parseCombo('mod+,').key).toBe(',');
    expect(parseCombo('ctrl++').key).toBe('+');
    expect(() => parseCombo('mod+shift')).toThrow();
  });

  test('mod is ⌘ on macOS and Ctrl elsewhere', () => {
    const c = parseCombo('mod+k');
    expect(matchesCombo(key({ key: 'k', code: 'KeyK', ctrlKey: true }), c, false)).toBe(true);
    expect(matchesCombo(key({ key: 'k', code: 'KeyK', metaKey: true }), c, false)).toBe(false);
    expect(matchesCombo(key({ key: 'k', code: 'KeyK', metaKey: true }), c, true)).toBe(true);
    expect(matchesCombo(key({ key: 'k', code: 'KeyK', ctrlKey: true }), c, true)).toBe(false);
  });

  test('modifiers must match exactly', () => {
    expect(matchesCombo(key({ key: 'K', code: 'KeyK', ctrlKey: true, shiftKey: true }), parseCombo('mod+k'), false)).toBe(false);
    expect(matchesCombo(key({ key: 'R', code: 'KeyR', ctrlKey: true, shiftKey: true }), parseCombo('mod+shift+r'), false)).toBe(true);
    expect(matchesCombo(key({ key: 'j' }), parseCombo('j'), false)).toBe(true);
    expect(matchesCombo(key({ key: 'j', altKey: true }), parseCombo('j'), false)).toBe(false);
  });

  test('shifted symbols match regardless of Shift (keyboard layouts)', () => {
    const c = parseCombo('shift+?');
    expect(matchesCombo(key({ key: '?', code: 'Slash', shiftKey: true }), c, false)).toBe(true);
    expect(matchesCombo(key({ key: '?', code: 'Minus', shiftKey: true }), parseCombo('?'), false)).toBe(true);
  });

  test('non-Latin layouts and macOS Option fall back to the physical key', () => {
    // Russian layout: Ctrl+К is physically K.
    expect(matchesCombo(key({ key: 'к', code: 'KeyK', ctrlKey: true }), parseCombo('mod+k'), false)).toBe(true);
    // macOS Option+R types "®".
    expect(matchesCombo(key({ key: '®', code: 'KeyR', altKey: true }), parseCombo('alt+r'), true)).toBe(true);
    // A Latin key that differs from the physical key must not match (AZERTY "a" on KeyQ).
    expect(matchesCombo(key({ key: 'a', code: 'KeyQ', ctrlKey: true }), parseCombo('mod+q'), false)).toBe(false);
  });

  test('display labels', () => {
    expect(formatShortcut('mod+shift+k', true)).toEqual(['⇧', '⌘', 'K']);
    expect(formatShortcut('mod+shift+k', false)).toEqual(['Ctrl', 'Shift', 'K']);
    expect(shortcutLabel('mod+k', true)).toBe('⌘K');
    expect(shortcutLabel('mod+k', false)).toBe('Ctrl+K');
    expect(shortcutLabel('shift+?', false)).toBe('?');
    expect(shortcutLabel('mod+enter', false)).toBe('Ctrl+↵');
    expect(shortcutLabel('escape', true)).toBe('Esc');
  });

  test('registerShortcut returns an unregister function', () => {
    const off = registerShortcut('mod+alt+shift+f12', () => undefined, { description: 'Test', group: 'Test' });
    expect(typeof off).toBe('function');
    off();
  });
});

describe('command palette ranking', () => {
  test('title matches beat secondary text, which beats fuzzy', () => {
    const theme = scoreCommand('theme', 'Theme: Dark', ['Appearance', 'color']);
    const fuzzy = scoreCommand('theme', 'Launch Edge', ['Connect', 'Microsoft Edge is not installed']);
    const secondary = scoreCommand('theme', 'Dark mode', ['Appearance', 'theme']);
    expect(theme).toBeGreaterThan(secondary);
    expect(secondary).toBeGreaterThan(fuzzy);
    expect(fuzzy).toBe(0); // "theme" is not a subsequence of "launch edge"
  });

  test('prefix > word prefix > substring > subsequence', () => {
    const prefix = scoreCommand('go', 'Go to Rules');
    const word = scoreCommand('rul', 'Go to Rules');
    const sub = scoreCommand('ule', 'Go to Rules');
    const subseq = scoreCommand('gtr', 'Go to Rules');
    expect(prefix).toBeGreaterThan(word);
    expect(word).toBeGreaterThan(sub);
    expect(sub).toBeGreaterThan(subseq);
    expect(subseq).toBeGreaterThan(0);
  });

  test('every token must match somewhere', () => {
    expect(scoreCommand('dark theme', 'Theme: Dark')).toBeGreaterThan(0.8);
    expect(scoreCommand('dark banana', 'Theme: Dark')).toBe(0);
    expect(scoreCommand('', 'Anything')).toBe(1);
  });

  test('single-character tokens are not fuzzy-matched', () => {
    expect(scoreCommand('x', 'Go to Rules')).toBe(0);
  });
});

describe('command registry', () => {
  const find = (id: string) => getCommands().find((c) => c.id === id);

  test('register and unregister', () => {
    const off = registerCommands([{ id: 'test.run', title: 'Run', group: 'Test', run: () => undefined }]);
    expect(find('test.run')?.title).toBe('Run');
    off();
    expect(find('test.run')).toBeUndefined();
  });

  test('later registrations with the same id win', () => {
    const a = registerCommands([{ id: 'test.dup', title: 'A', group: 'Test', run: () => undefined }]);
    const b = registerCommands([{ id: 'test.dup', title: 'B', group: 'Test', run: () => undefined }]);
    expect(find('test.dup')?.title).toBe('B');
    b();
    expect(find('test.dup')?.title).toBe('A');
    a();
    expect(find('test.dup')).toBeUndefined();
  });

  test('signature ignores handler identity but tracks what is displayed', () => {
    const base: Command = { id: 'x', title: 'X', group: 'G', run: () => undefined };
    expect(commandSignature([base])).toBe(commandSignature([{ ...base, run: () => undefined }]));
    expect(commandSignature([base])).not.toBe(commandSignature([{ ...base, title: 'Y' }]));
    expect(commandSignature([base])).not.toBe(commandSignature([{ ...base, disabled: true }]));
  });
});

describe('theme', () => {
  test('system follows prefers-color-scheme', () => {
    expect(resolveTheme('system', true)).toBe('light');
    expect(resolveTheme('system', false)).toBe('dark');
    expect(resolveTheme('midnight', true)).toBe('midnight');
  });
});
