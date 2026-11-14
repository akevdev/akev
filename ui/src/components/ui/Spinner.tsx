import clsx from 'clsx';
import styles from './Spinner.module.css';

export interface SpinnerProps {
  /** Diameter in px (default 14). */
  size?: number;
  className?: string;
  /** Accessible label; omit when the surrounding control already conveys loading. */
  label?: string;
  /** Inherit the current text colour (default) or use the accent colour. */
  tone?: 'current' | 'accent' | 'muted';
}

/** Indeterminate circular spinner (SVG arc, GPU-friendly rotation). */
export function Spinner({ size = 14, className, label, tone = 'current' }: SpinnerProps) {
  const stroke = size <= 14 ? 1.75 : size <= 24 ? 2 : 2.5;
  const r = (size - stroke) / 2;
  const c = 2 * Math.PI * r;
  return (
    <svg
      className={clsx(styles.spinner, styles[tone], className)}
      width={size}
      height={size}
      viewBox={`0 0 ${size} ${size}`}
      role={label ? 'status' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <circle className={styles.track} cx={size / 2} cy={size / 2} r={r} strokeWidth={stroke} fill="none" />
      <circle
        className={styles.arc}
        cx={size / 2}
        cy={size / 2}
        r={r}
        strokeWidth={stroke}
        fill="none"
        strokeLinecap="round"
        strokeDasharray={`${c * 0.28} ${c}`}
      />
    </svg>
  );
}
