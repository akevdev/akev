import { CircleAlert } from 'lucide-react';
import { createContext, useContext, useEffect, useId, type ReactNode } from 'react';
import type { ValidationIssue } from '../../../lib/rules/validate';
import styles from './Editor.module.css';

/**
 * Validation plumbing for the rule editor: structural issues (from `validateRule`) plus
 * field-local errors that only exist as text (JSON being typed, a port list…). Required-
 * field messages stay hidden until the field has been touched, so freshly added conditions
 * don't shout at the user.
 */
export interface EditorValidation {
  issues: readonly ValidationIssue[];
  /** Report (or clear with `null`) an error that lives in a field's text. */
  setFieldError(key: string, message: string | null): void;
  touch(path: string): void;
  isTouched(path: string): boolean;
}

export const EditorValidationContext = createContext<EditorValidation>({
  issues: [],
  setFieldError: () => {},
  touch: () => {},
  isTouched: () => true,
});

const REQUIRED = /^(Enter |Add at least|Select at least|Pick )/;

/** The error to show for `path` (exact match), respecting the touched state for required fields. */
export function useIssue(path: string, deep = false): string | undefined {
  const v = useContext(EditorValidationContext);
  const issue = v.issues.find((i) => i.path === path || (deep && i.path.startsWith(`${path}.`)));
  if (!issue) return undefined;
  if (REQUIRED.test(issue.message) && !v.isTouched(issue.path)) return undefined;
  return issue.message;
}

export function useTouch(): (path: string) => void {
  return useContext(EditorValidationContext).touch;
}

/** Register a text-level error for as long as the component is mounted. */
export function useFieldError(message: string | null): void {
  const { setFieldError } = useContext(EditorValidationContext);
  const key = useId();
  useEffect(() => {
    setFieldError(key, message);
  }, [key, message, setFieldError]);
  useEffect(() => () => setFieldError(key, null), [key, setFieldError]);
}

export function FieldError({ children, id }: { children?: ReactNode; id?: string }) {
  if (!children) return null;
  return (
    <div className={styles.fieldError} id={id} role="alert">
      <CircleAlert size={12} strokeWidth={2.25} aria-hidden />
      <span>{children}</span>
    </div>
  );
}

/** Small caps label for a form block. */
export function BlockLabel({ children, hint, htmlFor }: { children: ReactNode; hint?: ReactNode; htmlFor?: string }) {
  return (
    <div className={styles.blockLabel}>
      <label htmlFor={htmlFor}>{children}</label>
      {hint && <span className={styles.blockHint}>{hint}</span>}
    </div>
  );
}

/** A labelled block in an action form. */
export function Block({ label, hint, error, children, htmlFor, className }: { label?: ReactNode; hint?: ReactNode; error?: ReactNode; children: ReactNode; htmlFor?: string; className?: string }) {
  return (
    <div className={className ? `${styles.block} ${className}` : styles.block}>
      {label && <BlockLabel hint={hint} htmlFor={htmlFor}>{label}</BlockLabel>}
      {children}
      <FieldError>{error}</FieldError>
    </div>
  );
}

/** Explanatory text under a form. */
export function Explain({ children, icon }: { children: ReactNode; icon?: ReactNode }) {
  return (
    <p className={styles.explain}>
      {icon}
      <span>{children}</span>
    </p>
  );
}
