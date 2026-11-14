import clsx from 'clsx';
import { ArrowDown, ArrowUp, FlaskConical, PencilLine, Pin, StickyNote, Workflow } from 'lucide-react';
import type { ReactNode } from 'react';
import type { ExchangeDetail, ExchangeSummary } from '../../../../api/types';
import { AppIcon, ColorDot, MethodBadge, StatusBadge, Tooltip } from '../../../../components/ui';
import { durationSeverity, formatBytes, formatDuration, formatElapsed } from '../../../../lib/format';
import { useAppStore } from '../../../../state/app';
import { revealRule } from '../../../../state/intents';
import { hasOriginal, headsFor, protocolLabel, sizeInfo, tlsLabel } from '../lib/detail';
import { ruleChipLabel } from '../../../../lib/ruleFromExchange';
import { findItem } from '../../../../lib/rules/tree';
import { useNow } from '../hooks/misc';
import { GrpcCallChips } from './GrpcCall';
import { HeaderActions } from './HeaderActions';
import { openNoteDialog } from '../../dialogs/TrafficDialogs';
import { UrlTitle } from './UrlTitle';
import styles from './ExchangeHeader.module.css';

const PENDING = new Set(['requesting', 'waiting', 'responding']);

export interface ExchangeHeaderProps {
  /** Live summary of the shown exchange. */
  summary: ExchangeSummary;
  detail: ExchangeDetail | null;
  showOriginal: boolean;
  onToggleOriginal: () => void;
  /** Banners (paused / error) rendered inside the sticky header. */
  children?: ReactNode;
  /** `connection` for TLS failures / tunnels (authority title, reduced actions). */
  variant?: 'http' | 'connection';
}

/** Sticky detail header (DESIGN.md §6.4): title line, meta chips, actions, note. */
export function ExchangeHeader({ summary, detail, showOriginal, onToggleOriginal, children, variant = 'http' }: ExchangeHeaderProps) {
  const heads = detail ? headsFor(detail, showOriginal) : null;
  const request = heads?.request;
  const status = heads?.response?.status ?? summary.status;
  const statusMessage = heads?.response?.statusMessage;

  return (
    <header className={styles.header}>
      <div className={styles.top}>
        <UrlTitle
          scheme={request?.scheme ?? summary.scheme}
          host={request?.host ?? summary.host}
          port={request?.port ?? summary.port}
          path={request?.path ?? summary.path}
          resetKey={summary.id}
          plain={variant === 'connection' ? `${summary.host.includes(':') ? `[${summary.host}]` : summary.host}:${summary.port}` : undefined}
          leading={
            <>
              <MethodBadge method={request?.method ?? summary.method} variant="pill" />
              <StatusBadge
                status={status}
                statusMessage={statusMessage}
                error={summary.error}
                state={summary.state}
                kind={summary.kind}
                paused={summary.paused}
                showText
              />
            </>
          }
        />
        <HeaderActions
          summary={summary}
          requestBodySize={detail?.requestBody.wireSize ?? summary.requestSize}
          responseBodySize={detail?.responseBody.wireSize ?? summary.responseSize}
          note={detail?.note}
          onEditNote={() => openNoteDialog(summary.id)}
          variant={variant}
        />
      </div>
      <MetaLine summary={summary} detail={detail} showOriginal={showOriginal} onToggleOriginal={onToggleOriginal} />
      {detail?.note && (
        <button type="button" className={styles.note} onClick={() => openNoteDialog(summary.id)} title="Edit note">
          <StickyNote size={13} strokeWidth={1.75} aria-hidden />
          <span className={styles.noteText}>{detail.note}</span>
        </button>
      )}
      {children}
    </header>
  );
}

interface MetaLineProps {
  summary: ExchangeSummary;
  detail: ExchangeDetail | null;
  showOriginal: boolean;
  onToggleOriginal: () => void;
}

function MetaLine({ summary, detail, showOriginal, onToggleOriginal }: MetaLineProps) {
  const pending = PENDING.has(summary.state);
  const now = useNow(250, pending);
  const rules = useAppStore((s) => s.rules);

  const res = sizeInfo(detail?.responseBody, summary.responseSize);
  const req = sizeInfo(detail?.requestBody, summary.requestSize);
  const tls = tlsLabel(detail?.client.tls?.version);
  const process = detail?.client.process;
  const sourceName = summary.source ?? process?.name ?? detail?.client.interceptor;
  const matched = detail?.matchedRules?.length ? detail.matchedRules : summary.rule ? [summary.rule] : [];
  const ruleItem = matched[0] ? findItem(rules, matched[0]) : undefined;
  const stream = summary.kind === 'web-socket' || summary.kind === 'event-stream';
  const connection = summary.kind === 'tunnel' || summary.kind === 'tls-failure';
  const pausedMs = detail?.timing.pausedDuration ?? 0;
  // Time spent paused at a breakpoint is the user's, not the server's: don't colour it as slow.
  const severity = durationSeverity(summary.duration != null ? summary.duration - pausedMs : undefined);
  const ttfb = detail?.timing.responseHeaders;
  const durationTip = [
    stream ? 'Connection lifetime' : 'Total time',
    ttfb != null && !stream ? `first byte after ${formatDuration(ttfb)}` : null,
    pausedMs > 0 ? `includes ${formatDuration(pausedMs)} paused at a breakpoint` : null,
  ]
    .filter(Boolean)
    .join(' · ');

  const open = summary.state === 'streaming';
  const nowOpen = useNow(1000, open);
  const hasResponse = detail?.response != null || summary.status != null;

  return (
    <div className={styles.meta}>
      {summary.paused ? null : pending || open ? (
        <Chip>
          <Tooltip content={open ? (stream ? 'Connection is open' : 'Tunnel is open') : 'Still in progress'}>
            <span className={clsx(styles.live, 'tnum')}>
              <span className={clsx(styles.liveDot, open && styles.liveDotOpen)} aria-hidden />
              {open ? `Open ${formatElapsed(summary.startedAt, nowOpen)}` : formatElapsed(summary.startedAt, now)}
            </span>
          </Tooltip>
        </Chip>
      ) : (
        <Chip>
          <Tooltip content={durationTip}>
            <span className={clsx('tnum', !stream && !connection && severity === 'slow' && styles.warn, !stream && !connection && severity === 'very-slow' && styles.bad)}>{formatDuration(summary.duration)}</span>
          </Tooltip>
        </Chip>
      )}
      {req.wire > 0 && (
        <Chip>
          <Tooltip content={sizeTooltip('Request', req)}>
            <span className={clsx(styles.size, 'tnum')}>
              <ArrowUp size={11} strokeWidth={2} aria-hidden className={styles.dirIcon} />
              {formatBytes(req.wire)}
            </span>
          </Tooltip>
        </Chip>
      )}
      {(res.wire > 0 || (hasResponse && !connection)) && (
      <Chip>
        <Tooltip content={sizeTooltip(summary.kind === 'tunnel' ? 'Received' : 'Response', res)}>
          <span className={clsx(styles.size, 'tnum')}>
            <ArrowDown size={11} strokeWidth={2} aria-hidden className={styles.dirIcon} />
            {formatBytes(res.wire)}
            {res.decoded != null && (
              <span className={styles.decoded}>
                {' → '}
                {formatBytes(res.decoded)}
              </span>
            )}
          </span>
        </Tooltip>
      </Chip>
      )}
      {!connection && <Chip>{protocolLabel(summary.kind, summary.httpVersion)}</Chip>}
      {!connection && <GrpcCallChips summary={summary} detail={detail} />}
      {tls && (
        <Chip>
          <Tooltip content={[detail?.client.tls?.cipher, detail?.client.tls?.alpn && `ALPN ${detail.client.tls.alpn}`].filter(Boolean).join(' · ') || 'Client TLS'}>
            <span>{tls}</span>
          </Tooltip>
        </Chip>
      )}
      {sourceName && (
        <Chip>
          <Tooltip content={process ? `${process.path ?? process.name} · pid ${process.pid}` : `Source: ${sourceName}`}>
            <span className={styles.source}>
              <AppIcon name={detail?.client.interceptor ?? sourceName} size={13} />
              <span className="truncate">{sourceName}</span>
            </span>
          </Tooltip>
        </Chip>
      )}
      {detail?.client.remoteAddr && (
        <Chip>
          <Tooltip content="Client address">
            <span className={clsx(styles.mono, 'selectable')}>{detail.client.remoteAddr}</span>
          </Tooltip>
        </Chip>
      )}
      {matched[0] && (
        <Chip>
          <Tooltip content={matched.length > 1 ? `Matched ${matched.length} rules — open the first` : 'Open rule'}>
            <button type="button" className={clsx(styles.pill, styles.rulePill)} onClick={() => revealRule(matched[0] as string)}>
              <Workflow size={11} strokeWidth={2} aria-hidden />
              <span className={styles.pillText}>{ruleChipLabel(ruleItem)}</span>
              {matched.length > 1 && <span className={styles.more}>+{matched.length - 1}</span>}
            </button>
          </Tooltip>
        </Chip>
      )}
      {summary.mocked && (
        <Chip>
          <span className={clsx(styles.pill, styles.mockPill)}>
            <FlaskConical size={11} strokeWidth={2} aria-hidden />
            Mocked
          </span>
        </Chip>
      )}
      {detail && hasOriginal(detail) && (
        <Chip>
          <Tooltip content={showOriginal ? 'Showing the original exchange — click for the modified one' : 'Modified by a rule or breakpoint — click to see the original'}>
            <button type="button" className={clsx(styles.pill, styles.modPill)} aria-pressed={showOriginal} onClick={onToggleOriginal}>
              <PencilLine size={11} strokeWidth={2} aria-hidden />
              {showOriginal ? 'Original' : 'Modified'}
            </button>
          </Tooltip>
        </Chip>
      )}
      {summary.color && (
        <Chip>
          <Tooltip content={`Highlighted ${summary.color}`}>
            <span className={styles.dotWrap}>
              <ColorDot color={summary.color} size={8} label={`Highlighted ${summary.color}`} />
            </span>
          </Tooltip>
        </Chip>
      )}
      {summary.pinned && (
        <Chip>
          <span className={styles.pinned}>
            <Pin size={11} strokeWidth={2} aria-hidden />
            Pinned
          </span>
        </Chip>
      )}
    </div>
  );
}

function sizeTooltip(label: string, info: ReturnType<typeof sizeInfo>): string {
  if (info.decoded == null) return `${label}: ${formatBytes(info.wire)} on the wire`;
  return `${label}: ${formatBytes(info.wire)} on the wire${info.encodings.length ? ` (${info.encodings.join(', ')})` : ''}, ${formatBytes(info.decoded)} decoded`;
}

function Chip({ children }: { children: ReactNode }) {
  return <span className={styles.chip}>{children}</span>;
}
