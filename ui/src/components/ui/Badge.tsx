import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';
import styles from './Badge.module.css';

export type BadgeVariant = 'neutral' | 'accent' | 'success' | 'warning' | 'danger' | 'info';

export interface BadgeProps extends Omit<ComponentProps<'span'>, 'children'> {
  variant?: BadgeVariant;
  size?: 'sm' | 'md';
  icon?: LucideIcon;
  /** Leading status dot. */
  dot?: boolean;
  /** Pulsing dot (live/active states). */
  pulse?: boolean;
  /** Outline instead of soft fill. */
  outline?: boolean;
  mono?: boolean;
  children?: ReactNode;
}

/** Tiny rounded label/pill for states and counts. */
export function Badge({ variant = 'neutral', size = 'md', icon: Icon, dot, pulse, outline, mono, className, children, ...rest }: BadgeProps) {
  return (
    <span className={clsx(styles.badge, styles[variant], styles[size], outline && styles.outline, mono && styles.mono, className)} {...rest}>
      {(dot || pulse) && <span className={clsx(styles.dot, pulse && styles.pulse)} aria-hidden />}
      {Icon && <Icon size={size === 'sm' ? 11 : 12} strokeWidth={2} aria-hidden />}
      {children}
    </span>
  );
}
