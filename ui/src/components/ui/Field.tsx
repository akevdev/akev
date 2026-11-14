import clsx from 'clsx';
import { CircleAlert } from 'lucide-react';
import { cloneElement, isValidElement, useId, type ReactElement, type ReactNode } from 'react';
import styles from './Field.module.css';

export interface FieldProps {
  label: ReactNode;
  /** Help text under the control. */
  hint?: ReactNode;
  /** Inline error (replaces the hint, marks the control invalid). */
  error?: ReactNode;
  /** `stacked` (label above) or `inline` (label left, control right — settings rows). */
  layout?: 'stacked' | 'inline';
  /** Mark as required (asterisk). */
  required?: boolean;
  /** The control. A single element receives `id`, `aria-describedby` and `invalid`. */
  children: ReactNode;
  className?: string;
  /** Explicit control id (otherwise generated). */
  htmlFor?: string;
}

/** Label + control + hint/error, wired for accessibility. */
export function Field({ label, hint, error, layout = 'stacked', required, children, className, htmlFor }: FieldProps) {
  const autoId = useId();
  const id = htmlFor ?? autoId;
  const descId = `${id}-desc`;
  const describedBy = error || hint ? descId : undefined;
  const control = isValidElement(children)
    ? cloneElement(children as ReactElement<Record<string, unknown>>, {
        id: (children.props as Record<string, unknown>).id ?? id,
        'aria-describedby': describedBy,
        ...(error ? { invalid: true } : {}),
      })
    : children;
  return (
    <div className={clsx(styles.field, styles[layout], className)}>
      <div className={styles.labelCol}>
        <label htmlFor={id} className={styles.label}>
          {label}
          {required && <span className={styles.required} aria-hidden> *</span>}
        </label>
        {layout === 'inline' && (error || hint) && (
          <p id={descId} className={clsx(styles.hint, error && styles.error)}>
            {error && <CircleAlert size={12} strokeWidth={2} aria-hidden />}
            {error ?? hint}
          </p>
        )}
      </div>
      <div className={styles.control}>{control}</div>
      {layout === 'stacked' && (error || hint) && (
        <p id={descId} className={clsx(styles.hint, error && styles.error)} role={error ? 'alert' : undefined}>
          {error && <CircleAlert size={12} strokeWidth={2} aria-hidden />}
          {error ?? hint}
        </p>
      )}
    </div>
  );
}
