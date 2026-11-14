import clsx from 'clsx';
import type { ComponentProps, ReactNode } from 'react';
import type { ControlSize } from './Button';
import styles from './Input.module.css';

export interface InputProps extends Omit<ComponentProps<'input'>, 'size' | 'prefix'> {
  size?: ControlSize;
  /** Red border + `aria-invalid`. */
  invalid?: boolean;
  /** Element before the input (icon, prefix text). */
  leading?: ReactNode;
  /** Element after the input (unit, button, kbd hint). */
  trailing?: ReactNode;
  /** Monospace text (URLs, header values, code). */
  mono?: boolean;
  /** Class for the inner `<input>` (`className` styles the wrapper). */
  inputClassName?: string;
}

/**
 * Single-line text input. `ref` points at the `<input>`; `className` styles the wrapper,
 * which shows the focus ring.
 */
export function Input({ size = 'md', invalid, leading, trailing, mono, className, inputClassName, disabled, readOnly, ...rest }: InputProps) {
  return (
    <div
      className={clsx(styles.wrap, styles[size], mono && styles.mono, className)}
      data-invalid={invalid || undefined}
      data-disabled={disabled || undefined}
      data-readonly={readOnly || undefined}
    >
      {leading != null && <span className={styles.adornment}>{leading}</span>}
      <input
        className={clsx(styles.input, inputClassName)}
        aria-invalid={invalid || undefined}
        disabled={disabled}
        readOnly={readOnly}
        spellCheck={false}
        autoComplete="off"
        {...rest}
      />
      {trailing != null && <span className={styles.adornment}>{trailing}</span>}
    </div>
  );
}
