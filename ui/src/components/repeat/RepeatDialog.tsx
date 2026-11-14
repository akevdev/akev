import clsx from 'clsx';
import { ExternalLink, Repeat, RotateCcw, Square } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { api, errorMessage, isApiError } from '../../api/client';
import { REPEAT_LIMITS, type RepeatParams, type RepeatResult } from '../../api/types';
import { navigate } from '../../app/router';
import { formatDuration, formatNumber, pluralize } from '../../lib/format';
import { errorInfo, statusColorVar, statusText } from '../../lib/http';
import { Button, Dialog, Field, NumberInput, ProgressBar } from '../ui';
import type { RepeatTarget } from './repeat';
import { repeatStats } from './repeatStats';
import styles from './RepeatDialog.module.css';

type Phase = { kind: 'setup' } | { kind: 'running' } | { kind: 'done'; sent: number; cancelled: boolean; error?: string };

/** How often progress repaints while results stream in (ms). */
const REPAINT_MS = 100;

function Stat({ label, value }: { label: string; value: number | null }) {
  return (
    <div className={styles.stat}>
      <span className={styles.statLabel}>{label}</span>
      <span className={clsx(styles.statValue, 'tnum')}>{value == null ? '—' : formatDuration(value)}</span>
    </div>
  );
}

/**
 * Repeat a request N times with a chosen concurrency and pause, through
 * `POST /api/send/repeat`: live progress, status counts and min / avg / p50 / p95 / max latency,
 * cancellable (closing the dialog cancels too).
 */
export function RepeatDialog({ target, onClose }: { target: RepeatTarget; onClose: () => void }) {
  const [count, setCount] = useState<number | null>(10);
  const [concurrency, setConcurrency] = useState<number | null>(1);
  const [delay, setDelay] = useState<number | null>(0);
  const [phase, setPhase] = useState<Phase>({ kind: 'setup' });
  const [results, setResults] = useState<readonly RepeatResult[]>([]);
  const controller = useRef<AbortController | null>(null);
  const total = Math.max(1, count ?? 1);

  // Closing (or unmounting) stops a run.
  useEffect(() => () => controller.current?.abort(), []);

  const start = async () => {
    const params: RepeatParams = {
      ...(target.kind === 'request' ? { request: target.request, options: target.options } : { exchangeId: target.id }),
      count: total,
      concurrency: Math.min(total, Math.max(1, concurrency ?? 1)),
      delayMs: Math.max(0, delay ?? 0),
    };
    const ctrl = new AbortController();
    controller.current = ctrl;
    const buffer: RepeatResult[] = [];
    let timer: ReturnType<typeof setTimeout> | null = null;
    const paint = () => {
      timer = null;
      setResults(buffer.slice());
    };
    setResults([]);
    setPhase({ kind: 'running' });
    try {
      const done = await api.repeat(
        params,
        (r) => {
          buffer.push(r);
          if (timer == null) timer = setTimeout(paint, REPAINT_MS);
        },
        ctrl.signal,
      );
      setPhase({ kind: 'done', sent: done.sent, cancelled: false });
    } catch (e) {
      const cancelled = isApiError(e) && e.code === 'aborted';
      setPhase({ kind: 'done', sent: buffer.length, cancelled, error: cancelled ? undefined : errorMessage(e) });
    } finally {
      if (timer != null) clearTimeout(timer);
      paint();
      if (controller.current === ctrl) controller.current = null;
    }
  };

  const stats = useMemo(() => repeatStats(results), [results]);
  const running = phase.kind === 'running';
  const progress = results.length / total;

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={
        <span className={styles.title}>
          <Repeat size={16} strokeWidth={1.75} aria-hidden /> Repeat request
        </span>
      }
      description={<span className={styles.target}>{target.label}</span>}
      width={560}
      footer={
        phase.kind === 'setup' ? (
          <>
            <Button variant="ghost" onClick={onClose}>
              Cancel
            </Button>
            <Button variant="primary" icon={Repeat} onClick={() => void start()} disabled={count == null || count < 1}>
              Send {pluralize(total, 'time')}
            </Button>
          </>
        ) : running ? (
          <Button variant="secondary" icon={Square} onClick={() => controller.current?.abort()}>
            Stop
          </Button>
        ) : (
          <>
            {stats.slowest != null && (
              <Button
                variant="ghost"
                icon={ExternalLink}
                className={styles.footerStart}
                onClick={() => {
                  onClose();
                  navigate(`/traffic/${stats.slowest}`);
                }}
              >
                Open the slowest
              </Button>
            )}
            <Button variant="secondary" icon={RotateCcw} onClick={() => setPhase({ kind: 'setup' })}>
              Change and run again
            </Button>
            <Button variant="primary" onClick={onClose}>
              Done
            </Button>
          </>
        )
      }
    >
      {phase.kind === 'setup' ? (
        <div className={styles.form}>
          <Field label="Times" hint={`Up to ${formatNumber(REPEAT_LIMITS.count)}`}>
            <NumberInput value={count} onValueChange={setCount} min={1} max={REPEAT_LIMITS.count} stepper autoFocus />
          </Field>
          <Field label="At once" hint={`Concurrency, up to ${REPEAT_LIMITS.concurrency}`}>
            <NumberInput value={concurrency} onValueChange={setConcurrency} min={1} max={REPEAT_LIMITS.concurrency} stepper />
          </Field>
          <Field label="Pause" hint="Between each worker's requests">
            <NumberInput value={delay} onValueChange={setDelay} min={0} max={REPEAT_LIMITS.delayMs} step={50} unit="ms" stepper />
          </Field>
          <p className={styles.note}>Each request goes through Akev like a Compose send and shows up in the traffic list (unless recording is paused).</p>
        </div>
      ) : (
        <div className={styles.run} aria-live="polite">
          <div className={styles.progressRow}>
            <span className={clsx(styles.progressText, 'tnum')}>
              {formatNumber(results.length)} of {formatNumber(total)}
              {phase.kind === 'done' && (phase.cancelled ? ' · stopped' : phase.error ? ' · interrupted' : ' · done')}
            </span>
            <ProgressBar value={progress} variant={phase.kind === 'done' && (phase.cancelled || phase.error) ? 'warning' : stats.errors.length || stats.classes['5xx'] ? 'warning' : 'accent'} aria-label="Requests finished" className={styles.progress} />
          </div>
          {phase.kind === 'done' && phase.error && <p className={styles.error}>Couldn't finish the run: {phase.error}</p>}
          <div className={styles.stats}>
            <Stat label="Min" value={stats.min} />
            <Stat label="Average" value={stats.avg} />
            <Stat label="p50" value={stats.p50} />
            <Stat label="p95" value={stats.p95} />
            <Stat label="Max" value={stats.max} />
          </div>
          <div className={styles.codes} aria-label="Results">
            {stats.statuses.map(([code, n]) => (
              <span key={code} className={styles.code} style={{ '--c': statusColorVar(code) } as CSSProperties} title={statusText(code)}>
                <span className={clsx(styles.codeValue, 'tnum')}>{code}</span>
                <span className="tnum">×{formatNumber(n)}</span>
              </span>
            ))}
            {stats.errors.map(([kind, n]) => (
              <span key={kind} className={clsx(styles.code, styles.codeError)} title={errorInfo(kind).title.replaceAll('{host}', 'the server')}>
                <span className={styles.codeValue}>{errorInfo(kind).short}</span>
                <span className="tnum">×{formatNumber(n)}</span>
              </span>
            ))}
            {results.length === 0 && <span className={styles.waiting}>Waiting for the first response…</span>}
          </div>
        </div>
      )}
    </Dialog>
  );
}

export default RepeatDialog;
