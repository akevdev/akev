import * as RadixPopover from '@radix-ui/react-popover';
import clsx from 'clsx';
import type { ComponentProps } from 'react';
import styles from './Popover.module.css';

/** Popover root (Radix). Compose with `PopoverTrigger` + `PopoverContent`. */
export const Popover = RadixPopover.Root;
export const PopoverTrigger = RadixPopover.Trigger;
export const PopoverAnchor = RadixPopover.Anchor;
export const PopoverClose = RadixPopover.Close;

export interface PopoverContentProps extends ComponentProps<typeof RadixPopover.Content> {
  /** Remove the default padding (for lists/menus inside). */
  flush?: boolean;
  /** Fixed width in px. */
  width?: number;
}

/** Styled, portalled popover surface. */
export function PopoverContent({ className, flush, width, sideOffset = 6, collisionPadding = 8, style, ...rest }: PopoverContentProps) {
  return (
    <RadixPopover.Portal>
      <RadixPopover.Content
        sideOffset={sideOffset}
        collisionPadding={collisionPadding}
        className={clsx(styles.content, flush && styles.flush, className)}
        // An explicit width also lifts the default 480px cap (still clamped to the viewport).
        style={width ? { width, maxWidth: `min(${width}px, calc(100vw - 16px))`, ...style } : style}
        {...rest}
      />
    </RadixPopover.Portal>
  );
}
