import clsx from 'clsx';
import { Check, CircleAlert } from 'lucide-react';
import { Spinner, Tooltip } from '../../components/ui';
import type { SyncPhase } from './settingsSync';
import styles from './SaveIndicator.module.css';

/** Subtle "Saving… / Saved / Not saved" status for the Settings toolbar (fixed width, no layout shift). */
export function SaveIndicator({ phase, error }: { phase: SyncPhase; error: string | null }) {
  const content =
    phase === 'pending' || phase === 'saving' ? (
      <>
        <Spinner size={12} tone="muted" />
        <span>Saving…</span>
      </>
    ) : phase === 'saved' ? (
      <>
        <Check size={13} strokeWidth={2.2} aria-hidden />
        <span>Saved</span>
      </>
    ) : phase === 'error' ? (
      <>
        <CircleAlert size={13} strokeWidth={2} aria-hidden />
        <span>Not saved</span>
      </>
    ) : null;
  const node = (
    <span className={clsx(styles.indicator, styles[phase])} role="status" aria-live="polite" tabIndex={phase === 'error' ? 0 : undefined}>
      {content}
    </span>
  );
  return phase === 'error' && error ? (
    <Tooltip content={error} multiline side="bottom">
      {node}
    </Tooltip>
  ) : (
    node
  );
}
