import clsx from 'clsx';
import { lazy, Suspense, type Ref } from 'react';
import type { EditorView } from '@codemirror/view';
import type { CodeLanguage } from './codemirror/languages';
import { Skeleton } from './Skeleton';
import styles from './CodeEditor.module.css';

export type { CodeLanguage } from './codemirror/languages';
export { normalizeLanguage } from './codemirror/languages';

/** Imperative handle (`ref`) of a CodeEditor. */
export interface CodeEditorHandle {
  readonly view: EditorView | null;
  focus(): void;
  /** Open the styled search panel (also ⌘F / Ctrl+F inside the editor). */
  openSearch(): void;
  foldAll(): void;
  unfoldAll(): void;
  getValue(): string;
}

export interface CodeEditorProps {
  value: string;
  onChange?: (value: string) => void;
  /** A {@link CodeLanguage} or a loose name (`py`, `curl`, `json-lines`…). Lazy-loaded. */
  language?: CodeLanguage | string;
  readOnly?: boolean;
  /** Soft-wrap long lines. */
  wrap?: boolean;
  /** Line-number gutter (default true). */
  lineNumbers?: boolean;
  /** Fold gutter (default: on for structured languages). */
  folding?: boolean;
  placeholder?: string;
  /** Fill the parent's height (parent must have a definite height). */
  fill?: boolean;
  minHeight?: number | string;
  maxHeight?: number | string;
  /** Transparent background (inherit the surface). */
  transparent?: boolean;
  autoFocus?: boolean;
  /**
   * Identity of the logical document. When it changes, selection and scroll reset — use the
   * exchange id + side so switching exchanges doesn't keep the previous scroll position.
   */
  contentId?: string | number;
  /** ⌘/Ctrl+Enter inside the editor. */
  onSubmit?: () => void;
  /** ⌘/Ctrl+S inside the editor. */
  onSave?: () => void;
  className?: string;
  'aria-label'?: string;
  ref?: Ref<CodeEditorHandle>;
}

const Impl = lazy(() => import('./codemirror/CodeEditorImpl'));

/**
 * CodeMirror 6 editor/viewer themed by tokens, with lazy language loading, search panel,
 * folding, wrap and placeholder. CodeMirror itself is code-split: the first editor on screen
 * loads it (~a few frames), showing a skeleton meanwhile.
 */
export function CodeEditor(props: CodeEditorProps) {
  const { className, fill, minHeight } = props;
  return (
    <Suspense
      fallback={
        <div className={clsx(styles.fallback, fill && styles.fillFallback, className)} style={{ minHeight: minHeight ?? 64 }}>
          <Skeleton lines={3} height={10} />
        </div>
      }
    >
      <Impl {...props} className={clsx(styles.editor, className)} />
    </Suspense>
  );
}
