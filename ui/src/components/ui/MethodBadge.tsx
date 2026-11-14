import clsx from 'clsx';
import type { CSSProperties } from 'react';
import { methodColorVar } from '../../lib/http';
import styles from './MethodBadge.module.css';

export interface MethodBadgeProps {
  method: string;
  /** Fixed 56px width for list columns (DESIGN.md §5). */
  fixedWidth?: boolean;
  /** `text` (lists) or `pill` (soft tinted background, headers/rules). */
  variant?: 'text' | 'pill';
  size?: 'sm' | 'md';
  className?: string;
}

/** HTTP method in its method colour (mono, semibold). */
export function MethodBadge({ method, fixedWidth, variant = 'text', size = 'md', className }: MethodBadgeProps) {
  const upper = method.toUpperCase();
  return (
    <span
      className={clsx(styles.method, styles[variant], styles[size], fixedWidth && styles.fixed, className)}
      style={{ '--method-color': methodColorVar(upper) } as CSSProperties}
      title={upper.length > 7 ? upper : undefined}
    >
      {upper}
    </span>
  );
}
