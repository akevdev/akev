import clsx from 'clsx';
import styles from './ProgressBar.module.css';

export interface ProgressBarProps {
  /** 0..1; omit for an indeterminate bar. */
  value?: number;
  variant?: 'accent' | 'success' | 'warning' | 'danger';
  size?: 'sm' | 'md';
  className?: string;
  'aria-label'?: string;
}

/** Thin determinate/indeterminate progress bar. */
export function ProgressBar({ value, variant = 'accent', size = 'md', className, ...aria }: ProgressBarProps) {
  const indeterminate = value == null || !Number.isFinite(value);
  const pct = indeterminate ? 0 : Math.max(0, Math.min(1, value)) * 100;
  return (
    <div
      className={clsx(styles.track, styles[size], className)}
      role="progressbar"
      aria-label={aria['aria-label']}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={indeterminate ? undefined : Math.round(pct)}
    >
      <div className={clsx(styles.bar, styles[variant], indeterminate && styles.indeterminate)} style={indeterminate ? undefined : { width: `${pct}%` }} />
    </div>
  );
}
