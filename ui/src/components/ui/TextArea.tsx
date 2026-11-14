import clsx from 'clsx';
import { useCallback, useLayoutEffect, useRef, type ComponentProps, type Ref } from 'react';
import styles from './Input.module.css';

export interface TextAreaProps extends ComponentProps<'textarea'> {
  invalid?: boolean;
  mono?: boolean;
  /** Grow with content between `minRows` and `maxRows`. */
  autoResize?: boolean;
  minRows?: number;
  maxRows?: number;
}

function assignRef<T>(ref: Ref<T> | undefined, value: T | null): void {
  if (typeof ref === 'function') ref(value);
  else if (ref) (ref as { current: T | null }).current = value;
}

/** Multi-line text input with optional auto-resize. */
export function TextArea({ invalid, mono, autoResize, minRows = 3, maxRows = 12, className, ref, value, ...rest }: TextAreaProps) {
  const inner = useRef<HTMLTextAreaElement | null>(null);
  const setRef = useCallback(
    (el: HTMLTextAreaElement | null) => {
      inner.current = el;
      assignRef(ref, el);
    },
    [ref],
  );

  useLayoutEffect(() => {
    const el = inner.current;
    if (!autoResize || !el) return;
    const style = getComputedStyle(el);
    const line = parseFloat(style.lineHeight) || 18;
    const pad = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom) + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth);
    el.style.height = 'auto';
    const min = minRows * line + pad;
    const max = maxRows * line + pad;
    el.style.height = `${Math.min(max, Math.max(min, el.scrollHeight + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth)))}px`;
  }, [autoResize, minRows, maxRows, value]);

  return (
    <textarea
      ref={setRef}
      value={value}
      className={clsx(styles.textarea, mono && styles.mono, autoResize && styles.autoResize, className)}
      aria-invalid={invalid || undefined}
      rows={autoResize ? minRows : rest.rows}
      spellCheck={false}
      {...rest}
    />
  );
}
