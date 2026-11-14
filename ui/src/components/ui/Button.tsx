import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';
import { Kbd } from './Kbd';
import { Spinner } from './Spinner';
import styles from './Button.module.css';

export type ButtonVariant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'subtle-danger';
export type ControlSize = 'sm' | 'md' | 'lg';

export interface ButtonProps extends Omit<ComponentProps<'button'>, 'children'> {
  variant?: ButtonVariant;
  size?: ControlSize;
  /** Leading icon (lucide component). */
  icon?: LucideIcon;
  /** Trailing icon (e.g. a chevron for menu buttons). */
  iconRight?: LucideIcon;
  /** Shows a spinner and disables interaction while keeping the width stable. */
  loading?: boolean;
  /** Shortcut hint rendered at the end (`mod+enter`). */
  kbd?: string;
  /** Stretch to the container width. */
  block?: boolean;
  children?: ReactNode;
}

const ICON_SIZE: Record<ControlSize, number> = { sm: 14, md: 15, lg: 16 };

/**
 * The standard button. Variants: `primary` (accent), `secondary` (default), `ghost`,
 * `danger`, `subtle-danger`. Defaults to `type="button"`.
 */
export function Button({
  variant = 'secondary',
  size = 'md',
  icon: Icon,
  iconRight: IconRight,
  loading = false,
  kbd,
  block,
  className,
  children,
  disabled,
  type = 'button',
  ...rest
}: ButtonProps) {
  const iconSize = ICON_SIZE[size];
  return (
    <button
      type={type}
      className={clsx(styles.button, styles[variant], styles[size], block && styles.block, loading && styles.loading, !children && styles.iconOnly, className)}
      disabled={disabled || loading}
      aria-busy={loading || undefined}
      {...rest}
    >
      <span className={styles.inner}>
        {Icon && <Icon size={iconSize} strokeWidth={1.75} aria-hidden className={styles.icon} />}
        {children != null && children !== false && <span className={styles.label}>{children}</span>}
        {IconRight && <IconRight size={iconSize - 1} strokeWidth={1.75} aria-hidden className={styles.iconRight} />}
        {kbd && <Kbd combo={kbd} size="sm" variant={variant === 'primary' || variant === 'danger' ? 'inverse' : 'default'} className={styles.kbd} />}
      </span>
      {loading && (
        <span className={styles.spinner}>
          <Spinner size={iconSize} />
        </span>
      )}
    </button>
  );
}
