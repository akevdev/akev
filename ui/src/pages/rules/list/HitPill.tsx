import clsx from 'clsx';
import { useEffect, useRef, useState } from 'react';
import type { RuleId } from '../../../api/types';
import { Tooltip } from '../../../components/ui';
import { formatDateTime, formatNumber, formatRelativeTime } from '../../../lib/format';
import { formatHits, limitState } from '../../../lib/rules/stats';
import { useAppStore } from '../../../state/app';
import styles from './RuleTree.module.css';

/** Evaluated when the tooltip opens, so the relative time is always fresh. */
function LastHit({ at }: { at: number | undefined }) {
  if (!at) return <>Never matched yet</>;
  return (
    <>
      Last match {formatRelativeTime(at)} · {formatDateTime(at)}
    </>
  );
}

export interface HitPillProps {
  ruleId: RuleId;
  maxMatches: number | undefined;
}

/**
 * Live hit counter (`12×`) that flashes on increment. With a match limit it reads `2/3` and
 * dims once exhausted. Subscribes to this rule's stats only.
 */
export function HitPill({ ruleId, maxMatches }: HitPillProps) {
  const stats = useAppStore((s) => s.ruleStats[ruleId]);
  const hits = stats?.matches ?? 0;
  const limit = limitState({ maxMatches }, stats);
  const label = limit ? `${limit.used}/${limit.limit}` : formatHits(hits);
  const prev = useRef({ hits, label });
  const [flash, setFlash] = useState(0);

  // Flash when the counter visibly moves (an exhausted `3/3` doesn't flash again).
  useEffect(() => {
    if (hits > prev.current.hits && label !== prev.current.label) setFlash((f) => f + 1);
    prev.current = { hits, label };
  }, [hits, label]);

  if (hits === 0 && !limit) return <span className={styles.pillSlot} />;

  const tooltip = (
    <span className={styles.pillTip}>
      <span>
        {formatNumber(hits)} {hits === 1 ? 'match' : 'matches'}
        {limit ? ` · ${limit.exhausted ? 'limit reached — the rule is skipped now' : `${limit.remaining} left of ${limit.limit}`}` : ''}
      </span>
      <span className={styles.pillTipMuted}>
        <LastHit at={stats?.lastMatchedAt} />
      </span>
    </span>
  );
  return (
    <span className={styles.pillSlot}>
      <Tooltip content={tooltip} multiline side="left">
        <span
          key={flash}
          className={clsx('tnum', styles.pill, flash > 0 && styles.pillFlash, limit?.exhausted && styles.pillExhausted)}
          role="img"
          aria-label={`${hits} matches`}
          tabIndex={-1}
        >
          {label}
        </span>
      </Tooltip>
    </span>
  );
}
