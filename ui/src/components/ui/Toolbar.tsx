import clsx from 'clsx';
import type { ComponentProps, ReactNode } from 'react';
import styles from './Toolbar.module.css';

export interface ToolbarProps extends Omit<ComponentProps<'div'>, 'children'> {
  /** Left-aligned content. */
  left?: ReactNode;
  /** Right-aligned content. */
  right?: ReactNode;
  /** Free-form content (rendered between left and right, flexible). */
  children?: ReactNode;
  /** `subtle` (default, --bg-subtle), `panel`, or `transparent`. */
  tone?: 'subtle' | 'panel' | 'transparent';
  /** Bottom border (default true). */
  bordered?: boolean;
  /** 32px compact height (section toolbars). */
  compact?: boolean;
  /**
   * A page's top toolbar: in the Linux desktop app it doubles as the title bar (empty space
   * drags the window; content stays clear of the window buttons, see `lib/windowChrome.ts`).
   */
  titleBar?: boolean;
}

/** 40px action row with left/right slots and 6px gaps (DESIGN.md §5). */
export function Toolbar({ left, right, children, tone = 'subtle', bordered = true, compact, titleBar, className, ...rest }: ToolbarProps) {
  return (
    <div
      role="toolbar"
      className={clsx(styles.toolbar, styles[tone], bordered && styles.bordered, compact && styles.compact, titleBar && styles.titleBar, className)}
      data-window-drag={titleBar || undefined}
      {...rest}
    >
      {left != null && <div className={styles.group}>{left}</div>}
      {children != null && <div className={styles.middle}>{children}</div>}
      {children == null && <div className={styles.spacer} />}
      {right != null && <div className={clsx(styles.group, styles.right)}>{right}</div>}
    </div>
  );
}

/** Thin vertical divider between toolbar groups. */
export function ToolbarSeparator() {
  return <span className={styles.separator} role="separator" aria-orientation="vertical" />;
}
