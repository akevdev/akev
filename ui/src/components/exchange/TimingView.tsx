// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { CirclePause, Info, Repeat, Timer, Wand2, XCircle } from 'lucide-react';
import { useEffect, useMemo, useState, type CSSProperties } from 'react';
import type { ErrorKind, ExchangeDetail } from '../../api/types';
import { durationSeverity, formatBytes, formatDate, formatDuration, formatTime } from '../../lib/format';
import { EmptyState, Tooltip } from '../ui';
import { buildWaterfall, niceTicks, PHASE_COLORS, tickLabel } from './model/timing';
import styles from './TimingView.module.css';

export interface TimingViewProps {
  detail: ExchangeDetail;
  className?: string;
}

const SETUP_ERRORS: ReadonlySet<ErrorKind> = new Set<ErrorKind>(['dns-failed', 'connection-refused', 'host-unreachable', 'upstream-tls-failed', 'upstream-cert-invalid', 'upstream-proxy-failed']);

const LIVE_STATES = new Set(['requesting', 'waiting', 'responding', 'paused', 'streaming']);

/**
 * Timing waterfall (DESIGN.md §6.4): DNS, connect, TLS, request sent, waiting (TTFB) and download
 * on a shared ms scale with tick labels; total, absolute start, connection-reuse note; paused
 * breakpoint time cut out of the timeline and reported separately. Live while in progress.
 */
export function TimingView({ detail, className }: TimingViewProps) {
  const state = detail.summary.state;
  const live = LIVE_STATES.has(state) && detail.timing.responseBody == null && detail.timing.aborted == null;
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const id = window.setInterval(() => setNow(Date.now()), 250);
    return () => window.clearInterval(id);
  }, [live]);

  const w = useMemo(() => buildWaterfall(detail.timing, { state, now: live ? now : undefined }), [detail.timing, state, live, now]);
  const ticks = useMemo(() => niceTicks(w.end || 1), [w.end]);
  const scale = ticks[ticks.length - 1] || 1;
  const pct = (ms: number) => `${Math.min(100, Math.max(0, (ms / scale) * 100))}%`;
  const trackStyle = { backgroundSize: `${100 / (ticks.length - 1)}% 100%` } as CSSProperties;

  // A connection that failed to open can't have been reused (guards contradictory data).
  const reused = detail.upstream?.reused === true && !(detail.error && SETUP_ERRORS.has(detail.error.kind));
  const mocked = detail.summary.mocked === true || (detail.upstream == null && detail.error == null && detail.response != null);
  const download = w.phases.find((p) => p.id === 'download');
  const total = w.total ?? (live ? Math.max(0, now - w.startedAt - w.paused) : null);
  const sev = durationSeverity(total);

  if (w.phases.length === 0 && total == null) {
    return (
      <div className={clsx(styles.root, className)}>
        <div className={styles.empty}>
          <EmptyState icon={Timer} size="sm" title="No timing yet" description="Phase timings appear as the exchange progresses." />
        </div>
      </div>
    );
  }

  return (
    <div className={clsx(styles.root, className)}>
      <div className={styles.stats}>
        <div className={styles.stat}>
          <span className={styles.statLabel}>{live ? 'Elapsed' : 'Total'}</span>
          <span className={clsx(styles.statValue, 'tnum', sev === 'slow' && styles.slow, sev === 'very-slow' && styles.verySlow)}>{total != null ? formatDuration(total) : '—'}</span>
          <span className={styles.statSub}>{w.paused > 0 ? `+ ${formatDuration(w.paused)} paused` : live ? 'In progress' : detail.timing.aborted != null ? 'Aborted' : 'Complete'}</span>
        </div>
        <div className={styles.stat}>
          <span className={styles.statLabel} title="Time to first byte: from the request being sent until the response headers arrived">
            TTFB
          </span>
          <span className={clsx(styles.statValue, 'tnum')}>{w.ttfb != null ? formatDuration(w.ttfb) : '—'}</span>
          <span className={styles.statSub}>{w.ttfb != null && total ? `${Math.round((w.ttfb / Math.max(total, 1)) * 100)}% of total` : 'No response yet'}</span>
        </div>
        <div className={styles.stat}>
          <span className={styles.statLabel}>Download</span>
          <span className={clsx(styles.statValue, 'tnum')}>{download ? formatDuration(download.duration) : '—'}</span>
          <span className={clsx(styles.statSub, 'tnum')}>
            {formatBytes(detail.responseBody.wireSize)}
            {download && download.duration > 0 && detail.responseBody.wireSize > 0 ? ` · ${formatBytes((detail.responseBody.wireSize / download.duration) * 1000)}/s` : ''}
          </span>
        </div>
        <div className={styles.stat}>
          <span className={styles.statLabel}>Started</span>
          <span className={clsx(styles.statValue, styles.small, 'tnum')}>{formatTime(w.startedAt)}</span>
          <span className={clsx(styles.statSub, 'tnum')}>{formatDate(w.startedAt)}</span>
        </div>
      </div>

      {(reused || mocked || w.paused > 0 || w.abortedAt != null || w.approximate) && (
        <div className={styles.notes}>
          {reused && (
            <div className={clsx(styles.note, styles.noteInfo)}>
              <Repeat size={14} aria-hidden />
              <span>
                <strong>Connection reused</strong> — no DNS, connect or TLS time for this exchange.
              </span>
            </div>
          )}
          {mocked && !reused && (
            <div className={clsx(styles.note, styles.noteInfo)}>
              <Wand2 size={14} aria-hidden />
              <span>
                <strong>Answered by Akev</strong> — no upstream connection was made.
              </span>
            </div>
          )}
          {w.paused > 0 && (
            <div className={clsx(styles.note, styles.notePaused)}>
              <CirclePause size={14} aria-hidden />
              <span>
                <strong>Paused at a breakpoint for {formatDuration(w.paused)}</strong> — excluded from the timeline and the total.
              </span>
            </div>
          )}
          {w.approximate && (
            <div className={clsx(styles.note, styles.noteInfo)}>
              <Info size={14} aria-hidden />
              <span>
                <strong>Connection setup overlapped other phases</strong> — its bars are scaled to fit before the request was sent; durations are as measured.
              </span>
            </div>
          )}
          {w.abortedAt != null && (
            <div className={clsx(styles.note, styles.noteDanger)}>
              <XCircle size={14} aria-hidden />
              <span>
                <strong>Aborted after {formatDuration(w.abortedAt)}</strong>
                {detail.error ? ` — ${detail.error.message}` : ''}
              </span>
            </div>
          )}
        </div>
      )}

      <div className={styles.waterfall} role="table" aria-label="Timing waterfall">
        <div className={clsx(styles.row, styles.head)} role="row">
          <span className={styles.headLabel} role="columnheader">
            Phase
          </span>
          <div className={styles.track} role="columnheader" aria-label="Timeline">
            {ticks.map((t) => (
              <span key={t} className={clsx(styles.tick, 'tnum')} style={{ left: pct(t) }}>
                {tickLabel(t)}
              </span>
            ))}
          </div>
          <span className={clsx(styles.headLabel, styles.value)} role="columnheader">
            Duration
          </span>
        </div>
        {w.phases.map((p) => (
          <div key={p.id} className={styles.row} role="row">
            <span className={styles.label} role="rowheader">
              <span className={styles.swatch} style={{ background: PHASE_COLORS[p.id] }} aria-hidden />
              <span>{p.label}</span>
            </span>
            <div className={styles.track} style={trackStyle} role="cell">
              <Tooltip content={`${p.description} · starts at ${formatDuration(p.start)}, lasts ${formatDuration(p.duration)}`} multiline>
                <span
                  className={clsx(styles.bar, p.id === 'pending' && styles.pending)}
                  style={{ left: pct(p.start), width: `max(2px, ${pct(p.span)})`, background: PHASE_COLORS[p.id] }}
                  role="img"
                  aria-label={`${p.label}: ${formatDuration(p.duration)}`}
                />
              </Tooltip>
              {w.pauseAt != null && <span className={styles.marker} style={{ left: pct(w.pauseAt) }} aria-hidden />}
              {w.abortedAt != null && <span className={clsx(styles.marker, styles.abort)} style={{ left: pct(w.abortedAt) }} aria-hidden />}
            </div>
            <span className={clsx(styles.value, 'tnum')} role="cell">
              {formatDuration(p.duration)}
            </span>
          </div>
        ))}
        <div className={clsx(styles.row, styles.total)} role="row">
          <span className={styles.label} role="rowheader">
            {live ? 'Elapsed' : 'Total'}
          </span>
          <div className={styles.track} style={trackStyle} role="cell">
            {total != null && <span className={styles.totalBar} style={{ width: pct(total) }} aria-hidden />}
            {w.pauseAt != null && (
              <span className={clsx(styles.markerLabel, styles.paused)} style={{ left: pct(w.pauseAt) }}>
                <CirclePause size={10} strokeWidth={2.5} aria-hidden />
                {formatDuration(w.paused)}
              </span>
            )}
            {w.abortedAt != null && (
              <span className={clsx(styles.markerLabel, styles.abort)} style={{ left: pct(w.abortedAt) }}>
                Aborted
              </span>
            )}
          </div>
          <span className={clsx(styles.value, 'tnum')} role="cell">
            {total != null ? formatDuration(total) : '—'}
          </span>
        </div>
      </div>
    </div>
  );
}
