import clsx from 'clsx';
import type { CSSProperties } from 'react';
import { tagColorVar } from '../../lib/http';
import styles from './ColorDot.module.css';

export interface ColorDotProps {
  /** A tag colour name (`red`…`grey`) or any CSS colour / `var(--token)`. */
  color: string;
  size?: number;
  /** Selection ring (colour pickers). */
  selected?: boolean;
  className?: string;
  /** Accessible name; omit for decorative dots. */
  label?: string;
}

const TAG_NAMES = new Set(['red', 'orange', 'yellow', 'green', 'blue', 'purple', 'grey']);

/** Small colour indicator (user tags, legends, status dots). */
export function ColorDot({ color, size = 8, selected, className, label }: ColorDotProps) {
  const c = TAG_NAMES.has(color) ? tagColorVar(color) : color;
  return (
    // biome-ignore lint/a11y/useAriaPropsSupportedByRole: role="img" is set whenever aria-label is
    <span
      className={clsx(styles.dot, selected && styles.selected, className)}
      style={{ '--dot-color': c, width: size, height: size } as CSSProperties}
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    />
  );
}
