import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import type { ComponentProps, ReactNode } from 'react';
import type { ControlSize } from './Button';
import { Spinner } from './Spinner';
import { Tooltip } from './Tooltip';
import styles from './IconButton.module.css';

export interface IconButtonProps extends Omit<ComponentProps<'button'>, 'children'> {
  /** Required: used for the tooltip and `aria-label`. */
  label: string;
  icon?: LucideIcon;
  /** Custom content instead of `icon` (e.g. a dot + icon). */
  children?: ReactNode;
  /** Shortcut shown in the tooltip. */
  shortcut?: string;
  /** Toggle/pressed state (accent tint). Sets `aria-pressed` when a boolean is given. */
  active?: boolean;
  size?: ControlSize;
  /** `ghost` (default), `subtle` (bordered), `danger` (red on hover). */
  variant?: 'ghost' | 'subtle' | 'danger';
  tooltipSide?: 'top' | 'right' | 'bottom' | 'left';
  /** Set `false` to suppress the tooltip (e.g. when wrapped by a menu trigger with its own). */
  tooltip?: boolean;
  loading?: boolean;
}

const SIZES: Record<ControlSize, number> = { sm: 14, md: 16, lg: 18 };

/** Square icon-only button with a mandatory accessible label and tooltip. */
export function IconButton({
  label,
  icon: Icon,
  children,
  shortcut,
  active,
  size = 'md',
  variant = 'ghost',
  tooltipSide = 'top',
  tooltip = true,
  loading,
  className,
  type = 'button',
  disabled,
  ...rest
}: IconButtonProps) {
  const button = (
    <button
      type={type}
      aria-label={label}
      aria-pressed={typeof active === 'boolean' ? active : undefined}
      data-active={active || undefined}
      disabled={disabled || loading}
      className={clsx(styles.button, styles[size], styles[variant], className)}
      {...rest}
    >
      {loading ? <Spinner size={SIZES[size] - 2} /> : Icon ? <Icon size={SIZES[size]} strokeWidth={1.75} aria-hidden /> : children}
    </button>
  );
  if (!tooltip) return button;
  return (
    <Tooltip content={label} shortcut={shortcut} side={tooltipSide}>
      {button}
    </Tooltip>
  );
}
