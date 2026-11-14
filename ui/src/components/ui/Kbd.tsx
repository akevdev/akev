import clsx from 'clsx';
import type { ReactNode } from 'react';
import { displayCombo, formatShortcut, useShortcutBindingsVersion } from '../../app/shortcuts';
import styles from './Kbd.module.css';

export interface KbdProps {
  /**
   * A shortcut combo (`mod+shift+k`) rendered as platform-specific keys. It's a default combo
   * from code: when the user rebound that shortcut, the new keys are shown (see `literal`).
   */
  combo?: string;
  /** Show `combo` as given, not the keys it was rebound to (shortcut settings and lists). */
  literal?: boolean;
  /** Literal key content (used when `combo` is not given). */
  children?: ReactNode;
  size?: 'sm' | 'md';
  /** `subtle` drops the border/background (for use inside buttons and menus). */
  variant?: 'default' | 'subtle' | 'inverse';
  className?: string;
}

/** Keyboard key(s): `<Kbd combo="mod+k" />` → ⌘ K on macOS, Ctrl K elsewhere. */
export function Kbd({ combo, literal = false, children, size = 'md', variant = 'default', className }: KbdProps) {
  // Re-render when shortcuts are rebound.
  useShortcutBindingsVersion();
  if (combo) {
    const shown = literal ? combo : displayCombo(combo);
    let keys: string[];
    try {
      keys = formatShortcut(shown);
    } catch {
      keys = [shown];
    }
    return (
      <span className={clsx(styles.group, className)}>
        <span className="sr-only">{keys.join(' ')}</span>
        {keys.map((k, i) => (
          // biome-ignore lint/suspicious/noArrayIndexKey: a combo can repeat a key; order is the identity
          <kbd key={i} className={clsx(styles.kbd, styles[size], styles[variant])} aria-hidden>
            {k}
          </kbd>
        ))}
      </span>
    );
  }
  return <kbd className={clsx(styles.kbd, styles[size], styles[variant], className)}>{children}</kbd>;
}
