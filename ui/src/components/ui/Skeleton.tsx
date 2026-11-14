import clsx from 'clsx';
import type { CSSProperties } from 'react';
import styles from './Skeleton.module.css';

export interface SkeletonProps {
  width?: number | string;
  height?: number | string;
  /** Render N stacked text lines (last one shorter). */
  lines?: number;
  radius?: number | string;
  circle?: boolean;
  className?: string;
}

/** Shimmering placeholder while content loads. */
export function Skeleton({ width, height = 12, lines, radius, circle, className }: SkeletonProps) {
  if (lines && lines > 1) {
    return (
      <div className={clsx(styles.lines, className)} aria-hidden>
        {Array.from({ length: lines }, (_, i) => (
          <span key={i} className={styles.skeleton} style={{ height, width: i === lines - 1 ? '60%' : '100%', borderRadius: radius }} />
        ))}
      </div>
    );
  }
  const style: CSSProperties = {
    width: circle ? height : width ?? '100%',
    height,
    borderRadius: circle ? '999px' : radius,
  };
  return <span className={clsx(styles.skeleton, className)} style={style} aria-hidden />;
}
