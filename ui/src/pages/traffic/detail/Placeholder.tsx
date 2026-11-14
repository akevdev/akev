import clsx from 'clsx';
import { ArrowRight, CirclePause, MousePointerClick } from 'lucide-react';
import { useMemo } from 'react';
import { MethodBadge, Kbd } from '../../../components/ui';
import { formatBytes, formatCompactNumber, formatNumber } from '../../../lib/format';
import { useAppStore } from '../../../state/app';
import { trafficStore, useTraffic } from '../../../state/traffic';
import { openExchange } from '../actions/exchangeActions';
import { useNow } from './hooks/misc';
import styles from './Placeholder.module.css';

const HINTS: readonly { combo: string; alt?: string; label: string }[] = [
  { combo: 'arrowdown', alt: 'j', label: 'Next exchange' },
  { combo: 'arrowup', alt: 'k', label: 'Previous exchange' },
  { combo: 'shift+arrowdown', label: 'Extend selection' },
  { combo: 'mod+a', label: 'Select all visible' },
  { combo: 'mod+f', alt: '/', label: 'Filter traffic' },
  { combo: 'mod+k', label: 'Command palette' },
  { combo: 'mod+shift+r', label: 'Toggle recording' },
  { combo: 'shift+?', label: 'All shortcuts' },
];

interface LiveStats {
  total: number;
  visible: number;
  perSecond: number;
  failed: number;
  bytes: number;
}

/** Stats over the captured traffic, recomputed once a second (not per frame). */
function useLiveStats(): LiveStats {
  const now = useNow(1000);
  const loaded = useTraffic((s) => s.loaded);
  return useMemo(() => {
    const snap = trafficStore.getSnapshot();
    let failed = 0;
    let bytes = 0;
    for (const s of snap.summaries.values()) {
      if (s.error || s.kind === 'tls-failure' || (s.status != null && s.status >= 500)) failed++;
      bytes += (s.requestSize || 0) + (s.responseSize || 0);
    }
    // Exchanges started in the last 10 s (order is ascending by id ≈ start time).
    let recent = 0;
    const since = now - 10_000;
    for (let i = snap.order.length - 1; i >= 0; i--) {
      const s = snap.summaries.get(snap.order[i] as number);
      if (!s) continue;
      if (s.startedAt < since) break;
      recent++;
    }
    return { total: snap.order.length, visible: snap.visible.length, perSecond: recent / 10, failed, bytes };
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [now, loaded]);
}

/** Nothing selected (DESIGN.md §6.1): what to do next, live stats and shortcut hints. */
export function Placeholder() {
  const stats = useLiveStats();
  const paused = useAppStore((s) => s.paused);
  const firstPaused = paused[0];
  const pausedSummary = useTraffic((s) => (firstPaused ? s.summaries.get(firstPaused.id) : undefined));
  // Unlisted (not recorded) pauses still carry their request.
  const pausedLine = pausedSummary
    ? { method: pausedSummary.method, where: `${pausedSummary.host}${pausedSummary.path}` }
    : firstPaused
      ? { method: firstPaused.request.method, where: firstPaused.request.url.replace(/^[a-z][a-z0-9+.-]*:\/\//i, '') }
      : null;
  const filtered = stats.visible !== stats.total;

  return (
    <div className={styles.root}>
      <div className={styles.inner}>
        <div className={styles.hero}>
          <span className={styles.icon} aria-hidden>
            <MousePointerClick size={22} strokeWidth={1.6} />
          </span>
          <h2 className={styles.title}>{stats.total === 0 ? 'Nothing captured yet' : 'Select an exchange'}</h2>
          <p className={styles.description}>
            {stats.total === 0
              ? 'Requests appear in the list the moment a client sends them through the proxy.'
              : 'Pick a row to inspect its request, response, timing and TLS details. Select several to compare, export or bulk-edit them.'}
          </p>
        </div>

        {firstPaused && (
          <button type="button" className={styles.paused} onClick={() => openExchange(firstPaused.id)}>
            <CirclePause size={16} strokeWidth={2} className={styles.pausedIcon} aria-hidden />
            <span className={styles.pausedText}>
              <span className={styles.pausedTitle}>
                {paused.length === 1 ? '1 exchange is paused at a breakpoint' : `${paused.length} exchanges are paused at breakpoints`}
              </span>
              {pausedLine && (
                <span className={styles.pausedReq}>
                  <MethodBadge method={pausedLine.method} size="sm" />
                  <span className="truncate">{pausedLine.where}</span>
                </span>
              )}
            </span>
            <span className={styles.pausedCta}>
              Open
              <ArrowRight size={13} strokeWidth={2} aria-hidden />
            </span>
          </button>
        )}

        <dl className={styles.stats} aria-label="Live traffic statistics">
          <Stat label="Captured" value={formatNumber(stats.total)} hint={filtered ? `${formatNumber(stats.visible)} shown` : undefined} />
          <Stat label="Per second" value={stats.perSecond >= 10 ? formatCompactNumber(Math.round(stats.perSecond)) : stats.perSecond.toFixed(1)} hint="last 10 s" live={stats.perSecond > 0} />
          <Stat label="Failed" value={formatNumber(stats.failed)} hint="errors & 5xx" tone={stats.failed > 0 ? 'danger' : undefined} />
          <Stat label="Transferred" value={formatBytes(stats.bytes)} hint="sent + received" />
        </dl>

        <div className={styles.hints}>
          <h3 className={styles.hintsTitle}>Keyboard</h3>
          <ul className={styles.hintList}>
            {HINTS.map((h) => (
              <li key={h.combo} className={styles.hint}>
                <span className={styles.keys}>
                  <Kbd combo={h.combo} size="sm" />
                  {h.alt && (
                    <>
                      <span className={styles.or}>or</span>
                      <Kbd combo={h.alt} size="sm" />
                    </>
                  )}
                </span>
                <span className={styles.hintLabel}>{h.label}</span>
              </li>
            ))}
          </ul>
        </div>
      </div>
    </div>
  );
}

function Stat({ label, value, hint, tone, live }: { label: string; value: string; hint?: string; tone?: 'danger'; live?: boolean }) {
  return (
    <div className={styles.stat}>
      <dt className={styles.statLabel}>
        {label}
        {live && <span className={styles.liveDot} aria-hidden />}
      </dt>
      <dd className={clsx(styles.statValue, 'tnum', tone === 'danger' && styles.danger)}>{value}</dd>
      {hint && <dd className={styles.statHint}>{hint}</dd>}
    </div>
  );
}
