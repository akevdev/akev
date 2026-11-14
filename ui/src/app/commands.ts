/**
 * Command palette registry.
 *
 * Pages register commands for as long as they are mounted:
 *
 * ```tsx
 * useCommands([
 *   { id: 'traffic.clear', title: 'Clear traffic', group: 'Traffic', icon: Trash2,
 *     shortcut: 'mod+shift+x', run: clearTraffic },
 * ]);
 * ```
 *
 * Commands with a `shortcut` only *display* it — bind the key itself with `useShortcut`.
 * For results that depend on what the user types (e.g. "jump to exchange #123"), register a
 * {@link CommandProvider} with `useCommandProvider`.
 */

import type { LucideIcon } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useSyncExternalStore, type ReactNode } from 'react';

export interface Command {
  /** Unique id (`<area>.<action>`). Later registrations with the same id replace earlier ones. */
  id: string;
  title: string;
  /** Palette section: Navigation, Traffic, Rules, Compose, Connect, Appearance, ... */
  group: string;
  /** Secondary text shown muted after the title. */
  subtitle?: string;
  icon?: LucideIcon;
  /** Custom leading visual (e.g. an AppIcon) — takes precedence over `icon`. */
  iconNode?: ReactNode;
  /** Display-only shortcut hint (`mod+shift+r`). */
  shortcut?: string;
  /** Extra search terms. */
  keywords?: string[];
  /** Hidden when this returns false. */
  when?: () => boolean;
  /** Disabled (shown dimmed) when true. */
  disabled?: boolean;
  /** Keep the palette open after running (e.g. toggles). */
  keepOpen?: boolean;
  run: () => void | Promise<void>;
}

/** Produces commands from the current palette search text. */
export interface CommandProvider {
  id: string;
  /** Return at most a handful of results; called on every keystroke. */
  provide(search: string): Command[];
}

/** Display order of groups in the palette; unknown groups go last, alphabetically. */
export const GROUP_ORDER = ['Exchanges', 'Navigation', 'Traffic', 'Rules', 'Compose', 'Connect', 'Breakpoints', 'Appearance', 'Help', 'General'];

let nextOwner = 1;
const commandSets = new Map<number, Command[]>();
const providers = new Map<number, CommandProvider>();
let snapshot: Command[] = [];
let providerSnapshot: CommandProvider[] = [];
const listeners = new Set<() => void>();

function rebuild(): void {
  const byId = new Map<string, Command>();
  for (const cmds of commandSets.values()) for (const c of cmds) byId.set(c.id, c);
  snapshot = [...byId.values()];
  providerSnapshot = [...providers.values()];
  for (const l of listeners) l();
}

/** Imperatively register commands; returns an unregister function. */
export function registerCommands(commands: Command[]): () => void {
  const owner = nextOwner++;
  commandSets.set(owner, commands);
  rebuild();
  return () => {
    commandSets.delete(owner);
    rebuild();
  };
}

/** Register a dynamic provider; returns an unregister function. */
export function registerCommandProvider(provider: CommandProvider): () => void {
  const owner = nextOwner++;
  providers.set(owner, provider);
  rebuild();
  return () => {
    providers.delete(owner);
    rebuild();
  };
}

/**
 * Structural identity of a command list: what the palette displays. Handlers (`run`, `when`)
 * are excluded — they are forwarded to the latest render instead (see {@link useCommands}).
 */
export function commandSignature(commands: readonly Command[]): string {
  return JSON.stringify(
    commands.map((c) => [c.id, c.title, c.group, c.subtitle, c.shortcut, c.keywords, c.disabled, c.keepOpen, !!c.when, c.icon?.displayName ?? !!c.icon]),
  );
}

/**
 * Register commands while the component is mounted.
 *
 * ```tsx
 * useCommands([
 *   { id: 'traffic.clear', title: 'Clear traffic', group: 'Traffic', icon: Trash2, shortcut: 'mod+shift+x', run: clear },
 * ]);
 * // or, to avoid rebuilding an expensive list every render:
 * useCommands(() => buildCommands(interceptors), [interceptors]);
 * ```
 *
 * The palette registry is only rebuilt when what it *displays* changes (ids, titles,
 * subtitles, shortcuts, disabled…), so passing a fresh array literal every render is cheap.
 * `run` and `when` always call the handler from the latest render — no stale closures.
 */
export function useCommands(commands: readonly Command[] | (() => readonly Command[]), deps?: readonly unknown[]): void {
  // A factory with deps is re-evaluated only when deps change; anything else per render.
  const list = useMemo(
    () => (typeof commands === 'function' ? commands() : commands),
    // biome-ignore lint/correctness/useExhaustiveDependencies: caller-provided deps
    deps ?? [commands],
  );

  // Latest handlers by id, refreshed every commit (before the registration effect runs).
  const latest = useRef(new Map<string, Command>());
  useLayoutEffect(() => {
    latest.current = new Map(list.map((c) => [c.id, c]));
  });

  const signature = commandSignature(list);
  useEffect(() => {
    const forwarded = [...latest.current.values()].map<Command>((c) => ({
      ...c,
      run: () => (latest.current.get(c.id) ?? c).run(),
      when: c.when ? () => (latest.current.get(c.id)?.when ?? c.when ?? (() => true))() : undefined,
    }));
    return registerCommands(forwarded);
  }, [signature]);
}

/** Register a dynamic provider while mounted. */
export function useCommandProvider(provider: CommandProvider): void {
  const ref = useRef(provider);
  useEffect(() => {
    ref.current = provider;
  });
  const id = provider.id;
  useEffect(
    () => registerCommandProvider({ id, provide: (search) => ref.current.provide(search) }),
    [id],
  );
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** All registered static commands (latest registration per id). */
export function getCommands(): Command[] {
  return snapshot;
}

/** All registered static commands (reactive). */
export function useCommandList(): Command[] {
  return useSyncExternalStore(subscribe, getCommands, getCommands);
}

/** All registered providers (reactive). */
export function useCommandProviders(): CommandProvider[] {
  return useSyncExternalStore(subscribe, () => providerSnapshot, () => providerSnapshot);
}

// =====================================================================================
// Palette open state
// =====================================================================================

let paletteOpen = false;
let paletteInitial = '';
const paletteListeners = new Set<() => void>();

function emitPalette(): void {
  for (const l of paletteListeners) l();
}

/** Open the palette, optionally pre-filled (e.g. `#` to jump to an exchange). */
export function openPalette(initialSearch = ''): void {
  paletteInitial = initialSearch;
  paletteOpen = true;
  emitPalette();
}

export function closePalette(): void {
  if (!paletteOpen) return;
  paletteOpen = false;
  emitPalette();
}

export function togglePalette(): void {
  if (paletteOpen) closePalette();
  else openPalette();
}

export function usePaletteState(): { open: boolean; initialSearch: string } {
  const open = useSyncExternalStore(
    (l) => {
      paletteListeners.add(l);
      return () => paletteListeners.delete(l);
    },
    () => paletteOpen,
    () => paletteOpen,
  );
  return { open, initialSearch: paletteInitial };
}
