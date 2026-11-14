import clsx from 'clsx';
import { ArrowRight, CirclePause, Ellipsis, History, MessageSquareReply, Play, RotateCcw, Unplug, Zap } from 'lucide-react';
import type { BreakpointStage } from '../../../../api/types';
import { afterMenuClose, Button, MenuButton, Tooltip } from '../../../../components/ui';
import { formatElapsed, pluralize } from '../../../../lib/format';
import { PAUSE_WARN_MS } from '../lib/detail';
import { useNow } from '../hooks/misc';
import styles from './Banners.module.css';

export interface PausedBannerProps {
  stage: BreakpointStage;
  /** When the exchange started waiting (unix ms). */
  since: number;
  changes: number;
  /** A validation problem blocks resuming. */
  invalid: string | null;
  busy: 'resume' | 'close' | 'reset' | null;
  onResume: () => void;
  onRespond?: () => void;
  onClose: () => void;
  onReset: () => void;
  onRevert: () => void;
  /** Other exchanges paused right now (jump to the next one). */
  othersPaused?: number;
  onNextPaused?: () => void;
}

/**
 * Amber banner for an exchange paused at a breakpoint (DESIGN.md §6.4/§6.6): what is waiting,
 * a live timer (warning once clients are likely to time out), Resume (⌘↵), Respond…, Close, Reset.
 */
export function PausedBanner({ stage, since, changes, invalid, busy, onResume, onRespond, onClose, onReset, onRevert, othersPaused = 0, onNextPaused }: PausedBannerProps) {
  const now = useNow(1000);
  const waited = Math.max(0, now - since);
  const late = waited >= PAUSE_WARN_MS;
  const what = stage === 'request' ? 'Paused before the request is sent' : 'Paused before the response is returned';
  const hint =
    stage === 'request'
      ? 'Edit the request below, then resume.'
      : 'Edit the response below, then resume.';

  return (
    <div className={clsx(styles.banner, styles.paused)} role="status" aria-live="polite">
      <CirclePause size={16} strokeWidth={2} className={styles.icon} aria-hidden />
      <div className={styles.text}>
        <div className={styles.title}>
          <span>{what}</span>
          <Tooltip content={late ? 'Many clients give up after ~30 s — resume soon' : 'Time the client has been waiting'}>
            <span className={clsx(styles.timer, 'tnum', late && styles.timerLate)}>waiting {formatElapsed(since, now)}</span>
          </Tooltip>
        </div>
        <div className={styles.sub}>
          {changes > 0 ? (
            <>
              <span className={styles.changes}>{pluralize(changes, 'change')}</span>
              <button type="button" className={styles.link} onClick={onRevert}>
                <RotateCcw size={11} strokeWidth={2} aria-hidden />
                Revert
              </button>
            </>
          ) : (
            <span>{hint}</span>
          )}
          {invalid && <span className={styles.invalid}>{invalid}</span>}
          {othersPaused > 0 && onNextPaused && (
            <button type="button" className={styles.link} onClick={onNextPaused}>
              {othersPaused} more paused
              <ArrowRight size={11} strokeWidth={2} aria-hidden />
            </button>
          )}
        </div>
      </div>
      <div className={styles.actions}>
        {onRespond && (
          <Tooltip content="Answer the client yourself — the request is never sent upstream">
            <Button size="sm" icon={MessageSquareReply} onClick={onRespond} disabled={busy != null} className={styles.wideOnly}>
              Respond…
            </Button>
          </Tooltip>
        )}
        <Tooltip content="Close the client connection cleanly, without a response">
          <Button size="sm" variant="ghost" icon={Unplug} onClick={onClose} loading={busy === 'close'} disabled={busy != null && busy !== 'close'} className={styles.wideOnly}>
            Close
          </Button>
        </Tooltip>
        <Tooltip content="Reset the connection (TCP RST) — simulates a network failure">
          <Button size="sm" variant="ghost" icon={Zap} onClick={onReset} loading={busy === 'reset'} disabled={busy != null && busy !== 'reset'} className={styles.wideOnly}>
            Reset
          </Button>
        </Tooltip>
        <span className={styles.narrowOnly}>
          <MenuButton
            label="More breakpoint actions"
            icon={Ellipsis}
            items={[
              onRespond && { label: 'Respond…', icon: MessageSquareReply, onSelect: afterMenuClose(onRespond), hint: 'answer without forwarding' },
              { label: 'Close connection', icon: Unplug, onSelect: onClose },
              { label: 'Reset connection', icon: Zap, onSelect: onReset },
            ]}
          />
        </span>
        <Tooltip content={invalid ? `Can't resume: ${invalid}` : changes > 0 ? `Resume with ${pluralize(changes, 'change')}` : stage === 'request' ? 'Send the request upstream' : 'Return the response to the client'}>
          <Button size="sm" variant="primary" icon={Play} kbd="mod+enter" onClick={onResume} loading={busy === 'resume'} disabled={(busy != null && busy !== 'resume') || invalid != null}>
            Resume
          </Button>
        </Tooltip>
      </div>
    </div>
  );
}

/** Shown while the header's "Modified" toggle displays the original exchange. */
export function OriginalBanner({ onShowModified }: { onShowModified: () => void }) {
  return (
    <div className={clsx(styles.banner, styles.warnSoft)} role="status">
      <History size={15} strokeWidth={2} className={styles.icon} aria-hidden />
      <div className={styles.text}>
        <div className={styles.titleRegular}>Viewing the original exchange — before rules and breakpoint edits changed it.</div>
      </div>
      <div className={styles.actions}>
        <Button size="sm" variant="ghost" onClick={onShowModified}>
          Show modified
        </Button>
      </div>
    </div>
  );
}
