/**
 * Global keyboard shortcut registry (platform-aware `Mod` = ⌘ on macOS, Ctrl elsewhere).
 *
 * ```tsx
 * useShortcut('mod+k', () => openPalette(), { description: 'Command palette', group: 'General' });
 * useShortcut('j', next, { description: 'Next exchange', group: 'Traffic' });
 * useShortcut('mod+enter', resume, { allowInInputs: true });
 * ```
 *
 * Rules:
 * - Combos are `+`-joined: modifiers `mod`, `ctrl`, `alt`/`option`, `shift`, `meta`/`cmd`,
 *   then one key (`k`, `1`, `,`, `/`, `?`, `enter`, `escape`, `arrowup`, `delete`, `f2`...).
 * - Shortcuts never fire while typing in an input/textarea/contenteditable unless
 *   `allowInInputs` is set, and never inside open dialogs/menus unless `allowInOverlays`.
 * - Events a component already handled (`event.defaultPrevented`) are ignored.
 * - The most recently registered handler for a combo wins; returning `false` from a handler
 *   passes the event on to the next one (so pages can override app-level shortcuts).
 * - Registered descriptions power the `?` overlay and the command palette hints.
 * - Shortcuts with a description can be rebound in Settings › Shortcuts. A binding is identified
 *   by {@link bindingId} (group, description, default combo); the user's overrides live in the UI
 *   prefs and are applied with {@link setShortcutOverrides}. `<Kbd combo>` shows the effective
 *   keys for a default combo ({@link displayCombo}), so tooltips, menus and the palette follow.
 */

import { useEffect, useLayoutEffect, useRef, useSyncExternalStore } from 'react';
import { isMac } from '../lib/platform';

export interface ShortcutOptions {
  /** Fire even when focus is in a text field. */
  allowInInputs?: boolean;
  /** Fire even when focus is inside an open dialog / menu / listbox. */
  allowInOverlays?: boolean;
  /** Default `true`. */
  preventDefault?: boolean;
  /** Disabled shortcuts stay registered for the overlay but don't fire. */
  enabled?: boolean;
  /** Shown in the shortcuts overlay. Omit to hide the entry. */
  description?: string;
  /** Overlay section, e.g. `General`, `Navigation`, `Traffic`. */
  group?: string;
}

/** Return `false` to let the next handler for the same combo run. */
export type ShortcutHandler = (event: KeyboardEvent) => void | boolean;

interface ParsedCombo {
  key: string;
  mod: boolean;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
}

interface Registration {
  id: number;
  /** Effective combo (the user's override, else the default). */
  combo: string;
  parsed: ParsedCombo;
  /** The combo the code registered. */
  defaultCombo: string;
  /** Normalised {@link defaultCombo}. */
  defaultKey: string;
  /** {@link bindingId} for shortcuts with a description (rebindable), else `null`. */
  binding: string | null;
  handler: ShortcutHandler;
  options: ShortcutOptions;
}

/** A documented shortcut (registered handler or documentation-only entry). */
export interface ShortcutDoc {
  /** Combo; documentation-only entries may list alternatives separated by ` or ` (`j or arrowdown`). */
  combo: string;
  description: string;
  group: string;
  /** Rebindable shortcuts: their {@link bindingId}. */
  binding?: string;
  /** Rebindable shortcuts: the combo the code registered (`combo` is the effective one). */
  defaultCombo?: string;
}

const KEY_ALIASES: Record<string, string> = {
  esc: 'escape',
  return: 'enter',
  space: ' ',
  spacebar: ' ',
  up: 'arrowup',
  down: 'arrowdown',
  left: 'arrowleft',
  right: 'arrowright',
  del: 'delete',
  plus: '+',
  comma: ',',
  period: '.',
  slash: '/',
};

/** Parse `mod+shift+k` into its parts. Throws on an empty key. */
export function parseCombo(combo: string): ParsedCombo {
  const parts = combo
    .toLowerCase()
    .split(/\+(?!$)/)
    .map((p) => p.trim())
    .filter(Boolean);
  const out: ParsedCombo = { key: '', mod: false, ctrl: false, alt: false, shift: false, meta: false };
  for (const part of parts) {
    switch (part) {
      case 'mod':
        out.mod = true;
        break;
      case 'ctrl':
      case 'control':
        out.ctrl = true;
        break;
      case 'alt':
      case 'option':
      case 'opt':
        out.alt = true;
        break;
      case 'shift':
        out.shift = true;
        break;
      case 'meta':
      case 'cmd':
      case 'command':
        out.meta = true;
        break;
      default:
        out.key = KEY_ALIASES[part] ?? part;
    }
  }
  if (!out.key) throw new Error(`Invalid shortcut "${combo}": missing key`);
  return out;
}

/**
 * Canonical form of a combo (`Shift+Mod+K` → `mod+shift+k`), for comparing bindings. Throws on
 * an invalid combo, like {@link parseCombo}.
 */
export function normalizeCombo(combo: string): string {
  const p = parseCombo(combo);
  const parts: string[] = [];
  if (p.mod) parts.push('mod');
  if (p.ctrl) parts.push('ctrl');
  if (p.alt) parts.push('alt');
  if (p.shift) parts.push('shift');
  if (p.meta) parts.push('meta');
  parts.push(p.key === ' ' ? 'space' : p.key);
  return parts.join('+');
}

/**
 * The combo as it fires on a platform (`mod` resolved to `ctrl`, or `meta` on macOS), so
 * `mod+k` and `ctrl+k` compare equal on Linux and Windows.
 */
export function platformCombo(combo: string, mac: boolean = isMac): string {
  const p = parseCombo(combo);
  const parts: string[] = [];
  if (p.ctrl || (p.mod && !mac)) parts.push('ctrl');
  if (p.alt) parts.push('alt');
  if (p.shift && !SHIFTED_SYMBOLS.has(p.key)) parts.push('shift');
  if (p.meta || (p.mod && mac)) parts.push('meta');
  parts.push(p.key === ' ' ? 'space' : p.key);
  return parts.join('+');
}

/** Stable id of a rebindable shortcut: its group, description and default combo. */
export function bindingId(group: string, description: string, defaultCombo: string): string {
  let key = defaultCombo;
  try {
    key = normalizeCombo(defaultCombo);
  } catch {
    // keep the raw text
  }
  return `${group}::${description}::${key}`;
}

/** The default combo a {@link bindingId} was made from. */
export function bindingDefault(binding: string): string {
  return binding.slice(binding.lastIndexOf('::') + 2);
}

const MODIFIER_KEYS = new Set(['shift', 'control', 'alt', 'meta', 'os', 'altgraph', 'capslock', 'fn', 'hyper', 'super']);

/**
 * The combo a key press makes, for recording a binding (`mod` for Ctrl, or ⌘ on macOS).
 * `null` while only modifiers are held and for keys that can't be bound.
 */
export function comboFromEvent(e: Pick<KeyboardEvent, 'key' | 'code' | 'ctrlKey' | 'metaKey' | 'altKey' | 'shiftKey'>, mac: boolean = isMac): string | null {
  let key = e.key.toLowerCase();
  if (MODIFIER_KEYS.has(key) || key === 'unidentified' || key === 'process') return null;
  if (key === ' ' || key === 'spacebar') key = 'space';
  // Alt (macOS option) and non-Latin layouts change the character: use the physical key.
  if (key === 'dead' || (key.length === 1 && (e.altKey || !/^[\x20-\x7e]$/.test(key)))) {
    const fallback = codeKey(e as KeyboardEvent);
    if (fallback) key = fallback;
    else if (key === 'dead') return null;
  }
  if (key === '+') key = 'plus';
  const parts: string[] = [];
  const mod = mac ? e.metaKey : e.ctrlKey;
  if (mod) parts.push('mod');
  if (mac ? e.ctrlKey : e.metaKey) parts.push(mac ? 'ctrl' : 'meta');
  if (e.altKey) parts.push('alt');
  if (e.shiftKey) parts.push('shift');
  parts.push(key);
  return parts.join('+');
}

/** Characters produced with Shift on US layouts; matching them ignores the Shift state. */
const SHIFTED_SYMBOLS = new Set(['?', '!', '@', '#', '$', '%', '^', '&', '*', '(', ')', '_', '+', '{', '}', '|', ':', '"', '<', '>', '~']);

function eventKey(e: Pick<KeyboardEvent, 'key'>): string {
  return e.key.toLowerCase();
}

/** Fallback key from `event.code` for non-Latin layouts (`KeyK` → `k`, `Digit1` → `1`). */
function codeKey(e: Pick<KeyboardEvent, 'code'>): string | null {
  const code = e.code;
  if (code.startsWith('Key') && code.length === 4) return code.slice(3).toLowerCase();
  if (code.startsWith('Digit') && code.length === 6) return code.slice(5);
  const map: Record<string, string> = { Comma: ',', Period: '.', Slash: '/', Minus: '-', Equal: '=', BracketLeft: '[', BracketRight: ']', Backslash: '\\', Semicolon: ';', Quote: "'", Backquote: '`' };
  return map[code] ?? null;
}

/** Whether a keyboard event matches a parsed combo on this platform. */
export function matchesCombo(e: KeyboardEvent, c: ParsedCombo, mac: boolean = isMac): boolean {
  const wantCtrl = c.ctrl || (c.mod && !mac);
  const wantMeta = c.meta || (c.mod && mac);
  if (e.ctrlKey !== wantCtrl || e.metaKey !== wantMeta || e.altKey !== c.alt) return false;
  const key = eventKey(e);
  if (SHIFTED_SYMBOLS.has(c.key)) return key === c.key;
  if (e.shiftKey !== c.shift) return false;
  if (key === c.key) return true;
  // With Alt (macOS option) or a non-Latin layout, `key` may be a different character.
  const fallback = codeKey(e);
  return fallback !== null && fallback === c.key && (e.altKey || !/^[a-z0-9]$/.test(key));
}

/** True when the event target is a text-editing element. */
export function isEditableTarget(target: EventTarget | null): boolean {
  if (typeof HTMLElement === 'undefined' || !(target instanceof HTMLElement)) return false;
  if (target.isContentEditable) return true;
  const tag = target.tagName;
  if (tag === 'TEXTAREA' || tag === 'SELECT') return true;
  if (tag === 'INPUT') {
    const type = (target as HTMLInputElement).type;
    return !['checkbox', 'radio', 'button', 'submit', 'reset', 'range', 'color', 'file'].includes(type);
  }
  return false;
}

function isInOverlay(target: EventTarget | null): boolean {
  if (typeof Element === 'undefined' || !(target instanceof Element)) return false;
  return target.closest('[role="dialog"], [role="alertdialog"], [role="menu"], [role="listbox"], [cmdk-root]') !== null;
}

// =====================================================================================
// Registry
// =====================================================================================

let nextId = 1;
const registrations: Registration[] = [];
const docOnly = new Map<number, ShortcutDoc[]>();
let docsSnapshot: ShortcutDoc[] = [];
const docListeners = new Set<() => void>();

/** The user's rebindings: {@link bindingId} → combo. */
let overrides: Readonly<Record<string, string>> = {};
let bindingsVersion = 0;
const bindingListeners = new Set<() => void>();

function effectiveCombo(binding: string | null, defaultCombo: string): string {
  const o = binding ? overrides[binding] : undefined;
  if (o) {
    try {
      parseCombo(o);
      return o;
    } catch {
      // An invalid stored combo falls back to the default.
    }
  }
  return defaultCombo;
}

function hasOverrides(): boolean {
  for (const _ in overrides) return true;
  return false;
}

function notifyBindings(): void {
  bindingsVersion++;
  for (const l of bindingListeners) l();
}

/**
 * Apply the user's rebindings (Settings › Shortcuts, persisted in the UI prefs). Registered
 * shortcuts switch keys immediately; later registrations pick them up too.
 */
export function setShortcutOverrides(next: Readonly<Record<string, string>>): void {
  if (next === overrides) return;
  overrides = next;
  for (const r of registrations) {
    const combo = effectiveCombo(r.binding, r.defaultCombo);
    if (combo !== r.combo) {
      r.combo = combo;
      r.parsed = parseCombo(combo);
    }
  }
  rebuildDocs();
  notifyBindings();
}

/**
 * The keys that currently trigger the shortcut registered as `combo` (a default combo from
 * code: a command's `shortcut`, an `IconButton` hint…): the user's rebinding when there is one.
 * The most recently registered, enabled handler wins, like key handling; without one, an
 * override whose default is `combo` is used when there's exactly one.
 */
export function displayCombo(combo: string): string {
  if (!hasOverrides()) return combo;
  let key: string;
  try {
    key = normalizeCombo(combo);
  } catch {
    return combo;
  }
  for (let i = registrations.length - 1; i >= 0; i--) {
    const r = registrations[i] as Registration;
    if (r.binding && r.defaultKey === key && r.options.enabled !== false) return r.combo;
  }
  const matches = Object.entries(overrides).filter(([id]) => bindingDefault(id) === key);
  return matches.length === 1 ? (matches[0] as [string, string])[1] : combo;
}

/** Re-render when bindings change (for components that display shortcuts). */
export function useShortcutBindingsVersion(): number {
  return useSyncExternalStore(
    (l) => {
      bindingListeners.add(l);
      return () => bindingListeners.delete(l);
    },
    () => bindingsVersion,
    () => bindingsVersion,
  );
}

function rebuildDocs(): void {
  const seen = new Set<string>();
  const out: ShortcutDoc[] = [];
  const push = (d: ShortcutDoc) => {
    const key = `${d.group}|${d.combo}|${d.description}`;
    if (seen.has(key)) return;
    seen.add(key);
    out.push(d);
  };
  for (const r of registrations) {
    if (r.options.description) {
      push({ combo: r.combo, description: r.options.description, group: r.options.group ?? 'General', binding: r.binding ?? undefined, defaultCombo: r.defaultCombo });
    }
  }
  for (const docs of docOnly.values()) docs.forEach(push);
  docsSnapshot = out;
  for (const l of docListeners) l();
}

function onKeyDown(e: KeyboardEvent): void {
  if (e.defaultPrevented || e.isComposing || e.key === 'Unidentified' || e.key === 'Dead') return;
  const editable = isEditableTarget(e.target);
  const overlay = isInOverlay(e.target);
  for (let i = registrations.length - 1; i >= 0; i--) {
    const r = registrations[i] as Registration;
    if (r.options.enabled === false) continue;
    if (editable && !r.options.allowInInputs) continue;
    if (overlay && !r.options.allowInOverlays) continue;
    if (!matchesCombo(e, r.parsed)) continue;
    const result = r.handler(e);
    if (result === false) continue;
    if (r.options.preventDefault !== false) e.preventDefault();
    return;
  }
}

/** Run the registry on a key event (what the window listener does; exported for tests). */
export function dispatchShortcutEvent(e: KeyboardEvent): void {
  onKeyDown(e);
}

/** The documented shortcuts right now (non-reactive; see {@link useShortcutDocList}). */
export function getShortcutDocs(): readonly ShortcutDoc[] {
  return docsSnapshot;
}

let installed = false;
function ensureInstalled(): void {
  if (installed) return;
  installed = true;
  window.addEventListener('keydown', onKeyDown);
}

/** Imperatively register a shortcut. Returns an unregister function. */
export function registerShortcut(combo: string, handler: ShortcutHandler, options: ShortcutOptions = {}): () => void {
  ensureInstalled();
  const defaultKey = normalizeCombo(combo);
  const binding = options.description ? bindingId(options.group ?? 'General', options.description, combo) : null;
  const effective = effectiveCombo(binding, combo);
  const reg: Registration = { id: nextId++, combo: effective, parsed: parseCombo(effective), defaultCombo: combo, defaultKey, binding, handler, options };
  registrations.push(reg);
  if (options.description) rebuildDocs();
  if (binding && hasOverrides()) notifyBindings();
  return () => {
    const i = registrations.indexOf(reg);
    if (i >= 0) registrations.splice(i, 1);
    if (options.description) rebuildDocs();
    if (binding && hasOverrides()) notifyBindings();
  };
}

/**
 * Register a shortcut for the lifetime of the component. The handler may change between
 * renders without re-registering.
 */
export function useShortcut(combo: string, handler: ShortcutHandler, options: ShortcutOptions = {}): void {
  const handlerRef = useRef(handler);
  useLayoutEffect(() => {
    handlerRef.current = handler;
  });
  const { allowInInputs, allowInOverlays, preventDefault, enabled, description, group } = options;
  useEffect(
    () =>
      registerShortcut(combo, (e) => handlerRef.current(e), {
        allowInInputs,
        allowInOverlays,
        preventDefault,
        enabled,
        description,
        group,
      }),
    [combo, allowInInputs, allowInOverlays, preventDefault, enabled, description, group],
  );
}

/**
 * Document shortcuts handled locally by a component (e.g. list arrow keys) so they appear in
 * the `?` overlay while the component is mounted.
 */
export function useShortcutDocs(docs: readonly ShortcutDoc[]): void {
  const key = JSON.stringify(docs);
  useEffect(() => {
    const id = nextId++;
    docOnly.set(id, JSON.parse(key) as ShortcutDoc[]);
    rebuildDocs();
    return () => {
      docOnly.delete(id);
      rebuildDocs();
    };
  }, [key]);
}

/** All documented shortcuts (registered + documentation-only), reactive. */
export function useShortcutDocList(): ShortcutDoc[] {
  return useSyncExternalStore(
    (l) => {
      docListeners.add(l);
      return () => docListeners.delete(l);
    },
    () => docsSnapshot,
    () => docsSnapshot,
  );
}

// =====================================================================================
// Display
// =====================================================================================

const MAC_SYMBOLS: Record<string, string> = { mod: '⌘', meta: '⌘', cmd: '⌘', ctrl: '⌃', alt: '⌥', option: '⌥', shift: '⇧' };
const PC_NAMES: Record<string, string> = { mod: 'Ctrl', meta: 'Win', cmd: 'Win', ctrl: 'Ctrl', alt: 'Alt', option: 'Alt', shift: 'Shift' };
const KEY_NAMES: Record<string, string> = {
  escape: 'Esc',
  enter: '↵',
  ' ': 'Space',
  arrowup: '↑',
  arrowdown: '↓',
  arrowleft: '←',
  arrowright: '→',
  backspace: '⌫',
  delete: 'Del',
  tab: 'Tab',
  pageup: 'PgUp',
  pagedown: 'PgDn',
  home: 'Home',
  end: 'End',
};

/**
 * Split a combo into display keys: `mod+shift+k` → `['⌘','⇧','K']` on macOS,
 * `['Ctrl','Shift','K']` elsewhere.
 */
export function formatShortcut(combo: string, mac: boolean = isMac): string[] {
  const parsed = parseCombo(combo);
  const keys: string[] = [];
  const names = mac ? MAC_SYMBOLS : PC_NAMES;
  // Conventional order: Ctrl, Alt, Shift, Cmd (mac: ⌃⌥⇧⌘).
  if (parsed.ctrl) keys.push(names.ctrl as string);
  if (parsed.mod && !mac) keys.push(names.mod as string);
  if (parsed.alt) keys.push(names.alt as string);
  if (parsed.shift && !SHIFTED_SYMBOLS.has(parsed.key)) keys.push(names.shift as string);
  if (parsed.meta || (parsed.mod && mac)) keys.push(names.mod as string);
  const k = parsed.key;
  keys.push(KEY_NAMES[k] ?? (k.length === 1 ? k.toUpperCase() : k.charAt(0).toUpperCase() + k.slice(1)));
  return keys;
}

/** Single-string label: `⌘K` on macOS, `Ctrl+K` elsewhere. */
export function shortcutLabel(combo: string, mac: boolean = isMac): string {
  return formatShortcut(combo, mac).join(mac ? '' : '+');
}
