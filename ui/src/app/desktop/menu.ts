/**
 * The macOS menu bar of the desktop app (`desktop/src-tauri/src/lib.rs`, `build_menu`):
 * the shell emits `akev://menu` with the item id, and the UI runs the matching command.
 *
 * Menu accelerators overlap with UI shortcuts (⌘K, ⌘1–4, ⌘⇧R…), and some keys mean something
 * else on a page: ⌘S saves the request in Compose but is "Save Session…" in the menu, ⌘T opens
 * a Compose tab. WebKit gives a key to the page first and only lets the menu have it when the
 * page didn't handle it, but to be safe either way a keypress runs exactly one of them:
 *
 * - The page handled the key (`defaultPrevented`): a menu event for the same accelerator
 *   arriving right after it is dropped.
 * - The menu ran first: the page's keydown for the same accelerator arriving right after it
 *   is swallowed before the shortcut registry sees it.
 *
 * Clicking a menu item never involves a keypress, so it always runs.
 */

/** The ids the shell emits that the UI handles (zoom, reload, docs… are handled natively). */
export const MENU_IDS = [
  'settings',
  'compose-new',
  'import',
  'export-har',
  'save-session',
  'nav-traffic',
  'nav-rules',
  'nav-compose',
  'nav-connect',
  'command-palette',
  'toggle-recording',
  'clear-traffic',
  'shortcuts',
] as const;

export type MenuId = (typeof MENU_IDS)[number];

/** Menu item → its accelerator in shortcut-registry syntax (`CmdOrCtrl` is `mod`). */
export const MENU_ACCELERATORS: Readonly<Partial<Record<string, string>>> = {
  settings: 'mod+,',
  'compose-new': 'mod+t',
  import: 'mod+o',
  'export-har': 'mod+shift+e',
  'save-session': 'mod+s',
  'nav-traffic': 'mod+1',
  'nav-rules': 'mod+2',
  'nav-compose': 'mod+3',
  'nav-connect': 'mod+4',
  'command-palette': 'mod+k',
  'toggle-recording': 'mod+shift+r',
  'clear-traffic': 'mod+shift+x',
};

/** Within this long, a keypress and a menu event for the same accelerator are one action. */
export const SAME_KEYPRESS_MS = 600;

const SHORTCUT_KEYS: Readonly<Record<string, string>> = { Comma: ',', Minus: '-', Equal: '=' };

/**
 * A keydown as an accelerator (`mod+shift+e`), from its physical key so layouts and Shift
 * don't change it. `null` without the platform's command modifier.
 */
export function acceleratorOf(e: Pick<KeyboardEvent, 'code' | 'metaKey' | 'ctrlKey' | 'altKey' | 'shiftKey'>, mac: boolean): string | null {
  const mod = mac ? e.metaKey && !e.ctrlKey : e.ctrlKey && !e.metaKey;
  if (!mod || e.altKey) return null;
  let key: string | undefined;
  if (e.code.startsWith('Key') && e.code.length === 4) key = e.code.slice(3).toLowerCase();
  else if (e.code.startsWith('Digit') && e.code.length === 6) key = e.code.slice(5);
  else key = SHORTCUT_KEYS[e.code];
  if (!key) return null;
  return `mod+${e.shiftKey ? 'shift+' : ''}${key}`;
}

/** Remembers which side (page or menu) acted on an accelerator, and when. */
export class MenuKeyDedupe {
  /** The latest keydown per accelerator; `handled()` reads whether the page acted on it. */
  private readonly keys = new Map<string, { at: number; handled: () => boolean }>();
  private readonly menuRan = new Map<string, number>();
  private readonly accelerators: ReadonlySet<string>;

  constructor(accelerators: Iterable<string> = Object.values(MENU_ACCELERATORS).filter((a): a is string => a != null)) {
    this.accelerators = new Set(accelerators);
  }

  /** Whether `accel` belongs to a menu item. */
  covers(accel: string | null): accel is string {
    return accel != null && this.accelerators.has(accel);
  }

  /**
   * A keydown for `accel` arrives. Returns `true` when it must be swallowed (the menu just ran
   * the same accelerator); otherwise remembers it, and `handled` tells later whether the page
   * acted on it (read when the menu event comes in, after the key's dispatch finished).
   */
  keyDown(accel: string, now: number, handled: () => boolean): boolean {
    const ran = this.menuRan.get(accel);
    if (ran != null && now - ran < SAME_KEYPRESS_MS) return true;
    this.keys.set(accel, { at: now, handled });
    return false;
  }

  /** A menu event arrives: run it unless the page just handled a keypress of its accelerator. */
  shouldRunMenu(id: string, now: number): boolean {
    const accel = MENU_ACCELERATORS[id];
    if (!accel) return true;
    const key = this.keys.get(accel);
    if (key && now - key.at < SAME_KEYPRESS_MS && key.handled()) {
      this.keys.delete(accel);
      return false;
    }
    this.menuRan.set(accel, now);
    return true;
  }
}
