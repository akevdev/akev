import * as RadixTooltip from '@radix-ui/react-tooltip';
import clsx from 'clsx';
import type { ReactElement, ReactNode } from 'react';
import { Kbd } from './Kbd';
import styles from './Tooltip.module.css';

/** Mount once at the app root (400 ms delay, 100 ms skip delay — DESIGN.md §5). */
export function TooltipProvider({ children }: { children: ReactNode }) {
  return (
    <RadixTooltip.Provider delayDuration={400} skipDelayDuration={100}>
      {children}
    </RadixTooltip.Provider>
  );
}

export interface TooltipProps {
  /** Tooltip text/content. When empty, the child is rendered without a tooltip. */
  content: ReactNode;
  /** Shortcut combo rendered with `Kbd` after the content. */
  shortcut?: string;
  side?: 'top' | 'right' | 'bottom' | 'left';
  align?: 'start' | 'center' | 'end';
  /** Override the provider delay (ms). */
  delay?: number;
  /** Controlled open state (rarely needed). */
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Must be a single element that forwards its ref (all primitives do). */
  children: ReactElement;
  className?: string;
  /** Allow multi-line content up to 280px wide. */
  multiline?: boolean;
}

/**
 * Hover/focus tooltip. Wrap any focusable element:
 * `<Tooltip content="Copy URL" shortcut="mod+c"><IconButton … /></Tooltip>`.
 */
export function Tooltip({ content, shortcut, side = 'top', align = 'center', delay, open, onOpenChange, children, className, multiline }: TooltipProps) {
  if (content == null || content === '' || content === false) return children;
  return (
    <RadixTooltip.Root delayDuration={delay} open={open} onOpenChange={onOpenChange}>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <RadixTooltip.Portal>
        <RadixTooltip.Content
          side={side}
          align={align}
          sideOffset={6}
          collisionPadding={8}
          className={clsx(styles.content, multiline && styles.multiline, className)}
        >
          <span className={styles.text}>{content}</span>
          {shortcut && <Kbd combo={shortcut} size="sm" className={styles.kbd} />}
        </RadixTooltip.Content>
      </RadixTooltip.Portal>
    </RadixTooltip.Root>
  );
}
