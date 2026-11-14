import clsx from 'clsx';
import {
  Ban,
  CircleOff,
  CirclePause,
  Clock,
  FileWarning,
  Globe,
  LockKeyhole,
  ShieldAlert,
  Unplug,
  Zap,
  type LucideIcon,
} from 'lucide-react';
import type { CSSProperties, ReactNode } from 'react';
import type { BreakpointStage, ErrorKind, ExchangeKind, ExchangeState, ExchangeSummary } from '../../api/types';
import { errorInfo, statusClass, statusText } from '../../lib/http';
import { Tooltip } from './Tooltip';
import styles from './StatusBadge.module.css';

export interface StatusBadgeProps {
  status?: number;
  /** Reason phrase (falls back to the canonical one). */
  statusMessage?: string;
  error?: ErrorKind;
  state?: ExchangeState;
  kind?: ExchangeKind;
  paused?: BreakpointStage;
  /** Convenience: take everything from a summary row. */
  summary?: Pick<ExchangeSummary, 'status' | 'error' | 'state' | 'kind' | 'paused'>;
  /** Append the reason phrase after the code (detail headers). */
  showText?: boolean;
  /** Disable the tooltip (when the parent already explains). */
  tooltip?: boolean;
  className?: string;
}

const ERROR_ICONS: Partial<Record<ErrorKind, LucideIcon>> = {
  'client-aborted': CircleOff,
  'dns-failed': Globe,
  'connection-refused': Ban,
  'connection-reset': Zap,
  'host-unreachable': Unplug,
  timeout: Clock,
  'upstream-cert-invalid': ShieldAlert,
  'upstream-tls-failed': LockKeyhole,
  'client-tls-rejected': LockKeyhole,
  'upstream-protocol-error': FileWarning,
  'client-protocol-error': FileWarning,
  'rule-closed': Ban,
  'rule-reset': Zap,
  'rule-timeout': Clock,
  'breakpoint-aborted': CircleOff,
  'upstream-proxy-failed': Unplug,
};

const PENDING_STATES: ReadonlySet<ExchangeState> = new Set(['requesting', 'waiting', 'responding']);

/**
 * Response status coloured by class, with states: pending (animated dot), paused (amber),
 * errors (icon + short label, e.g. "Refused", "TLS", "Timeout"), WebSocket (`WS`).
 * Colour is never the only signal: errors and pauses carry icons/labels.
 */
export function StatusBadge(props: StatusBadgeProps) {
  const s = props.summary;
  const status = props.status ?? s?.status;
  const error = props.error ?? s?.error;
  const state = props.state ?? s?.state;
  const kind = props.kind ?? s?.kind;
  const paused = props.paused ?? s?.paused;
  const { showText, tooltip = true, className, statusMessage } = props;

  let content: ReactNode;
  let tip: string;
  let cls: string | undefined;
  let color: string | undefined;

  if (paused) {
    content = (
      <>
        <CirclePause size={13} strokeWidth={2} aria-hidden />
        <span>Paused</span>
      </>
    );
    tip = paused === 'request' ? 'Paused at a breakpoint before the request is sent' : 'Paused at a breakpoint before the response is returned';
    cls = styles.paused;
  } else if (error && status == null) {
    const info = errorInfo(error);
    const Icon = ERROR_ICONS[error] ?? Ban;
    content = (
      <>
        <Icon size={12} strokeWidth={2} aria-hidden />
        <span>{info.short}</span>
      </>
    );
    tip = info.title.replaceAll('{host}', 'the server');
    cls = styles.error;
  } else if (kind === 'tls-failure') {
    content = (
      <>
        <LockKeyhole size={12} strokeWidth={2} aria-hidden />
        <span>TLS</span>
      </>
    );
    tip = 'TLS handshake failed';
    cls = styles.error;
  } else if (kind === 'tunnel' && status == null) {
    content = <span>Tunnel</span>;
    tip = 'Tunnelled connection (not intercepted)';
    cls = styles.tunnel;
  } else if (status == null) {
    if (state && PENDING_STATES.has(state)) {
      content = <span className={styles.pendingDot} aria-hidden />;
      tip = state === 'responding' ? 'Receiving response…' : 'Waiting for response…';
      cls = styles.pending;
    } else {
      content = <span>—</span>;
      tip = 'No response';
      cls = styles.none;
    }
  } else {
    const isWs = (kind === 'web-socket' && status === 101) || (kind === 'web-socket' && state === 'streaming');
    const text = statusMessage || statusText(status);
    color = `var(--s-${statusClass(status)})`;
    content = (
      <>
        <span className="tnum">{isWs ? 'WS' : status}</span>
        {isWs && state === 'streaming' && <span className={styles.liveDot} aria-hidden />}
        {showText && text && <span className={styles.text}>{text}</span>}
      </>
    );
    tip = `${status}${text ? ` ${text}` : ''}${error ? ` · ${errorInfo(error).short}` : ''}`;
    cls = clsx(styles.code, error && styles.withError);
  }

  const badge = (
    <span
      className={clsx(styles.badge, cls, className)}
      style={color ? ({ '--status-color': color } as CSSProperties) : undefined}
      aria-label={tip}
      role="img"
    >
      {content}
    </span>
  );
  return tooltip && !showText ? <Tooltip content={tip}>{badge}</Tooltip> : badge;
}
