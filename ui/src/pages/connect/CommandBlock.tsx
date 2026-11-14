import clsx from 'clsx';
import type { ReactNode } from 'react';
import { CopyButton } from '../../components/ui';
import styles from './CommandBlock.module.css';

export interface CommandBlockProps {
  /** The text to show and copy. */
  code: string;
  /** What Copy puts on the clipboard when it differs from what's shown (a masked secret). */
  copyValue?: string;
  /** Optional label above the block. */
  title?: ReactNode;
  /** Muted placeholder look while the real command loads. */
  placeholder?: boolean;
  /** Toast shown on copy (default: none, the button flips to ✓). */
  copyToast?: string;
  /** Called after a successful copy click (e.g. "copying counts as started"). */
  onCopy?: () => void;
  className?: string;
  /** Wrap long lines (default true; turn off for code where indentation matters). */
  wrap?: boolean;
  'aria-label'?: string;
}

/** Monospace, selectable command/code block with a copy button. */
export function CommandBlock({ code, copyValue, title, placeholder, copyToast, onCopy, className, wrap = true, ...aria }: CommandBlockProps) {
  return (
    <div className={clsx(styles.root, className)}>
      {title && <div className={styles.title}>{title}</div>}
      <div className={clsx(styles.block, placeholder && styles.placeholder)} onCopy={onCopy}>
        {/* biome-ignore lint/a11y/noNoninteractiveTabindex: scrollable region: focusable so it can be scrolled with the keyboard */}
        <pre className={clsx(styles.code, wrap && styles.wrap)} role="region" aria-label={aria['aria-label']} tabIndex={0}>
          {code}
        </pre>
        <span className={styles.copy} onClickCapture={onCopy}>
          <CopyButton value={copyValue ?? code} label="Copy" toastMessage={copyToast} disabled={placeholder} tooltipSide="left" />
        </span>
      </div>
    </div>
  );
}
