// biome-ignore-all lint/a11y/useFocusableInteractive: the grid keeps focus and points at the active row with aria-activedescendant; cells take none
import clsx from 'clsx';
import { ArrowDown, CirclePause, Pin, Sparkles, StickyNote } from 'lucide-react';
import { memo, type CSSProperties, type ReactNode } from 'react';
import type { ExchangeSummary, HttpVersion } from '../../../api/types';
import { ProviderMark } from '../../../components/exchange/llm/ProviderMark';
import { AppIcon, MethodBadge, StatusBadge } from '../../../components/ui';
import { providerInfo } from '../../../lib/llm/providers';
import { durationSeverity, formatBytes, formatDuration, formatElapsed, formatNumber, formatTime } from '../../../lib/format';
import { contentKindColorVar, contentKindLabel, errorInfo, formatAuthority, statusText, tagColorVar } from '../../../lib/http';
import { useAppStore } from '../../../state/app';
import { useExchangeSummary, useIsSelected, useTraffic } from '../../../state/traffic';
import type { TrafficColumnId } from '../../../state/ui';
import { useNow } from './clock';
import { HeaderCell } from './HeaderCell';
import { kindIcon } from './kindIcon';
import { ruleTitles } from './useDisplayList';
import styles from './TrafficList.module.css';

export interface TrafficRowProps {
  id: number;
  /** 0-based position in the display list (for `aria-rowindex`). */
  index: number;
  /** Pixel offset from the top of the canvas. */
  top: number;
  columns: readonly TrafficColumnId[];
  monoPaths: boolean;
}

const PENDING: ReadonlySet<string> = new Set(['requesting', 'waiting', 'responding']);

const TLS_FAILURE_TEXT = 'TLS handshake failed — client rejected certificate';

/**
 * Upstream address, when the daemon includes one in the summary. `ExchangeSummary` has no
 * such field yet (the address lives in `ExchangeDetail.upstream`), so this reads the likely
 * names defensively and the column shows "—" until then.
 */
function remoteAddress(s: ExchangeSummary): string | undefined {
  const loose = s as ExchangeSummary & { remoteAddress?: unknown; remoteIp?: unknown };
  const v = loose.remoteAddress ?? loose.remoteIp;
  return typeof v === 'string' && v ? v : undefined;
}

function isPending(s: ExchangeSummary): boolean {
  return PENDING.has(s.state) && s.status == null && !s.error;
}

function protocolLabel(s: ExchangeSummary): string {
  if (s.kind === 'web-socket') return s.scheme === 'wss' ? 'wss' : 'ws';
  if (s.kind === 'event-stream') return 'sse';
  const v: HttpVersion = s.httpVersion;
  if (v === 'HTTP/2') return 'h2';
  if (v === 'HTTP/3') return 'h3';
  if (v === 'HTTP/1.0' || v === 'HTTP/0.9') return 'h1.0';
  return 'h1';
}

function statusTip(s: ExchangeSummary): string {
  if (s.paused) return s.paused === 'request' ? 'Paused before the request is sent' : 'Paused before the response is returned';
  if (s.kind === 'tls-failure') return TLS_FAILURE_TEXT;
  if (s.error && s.status == null) return errorInfo(s.error).title.replaceAll('{host}', s.host || 'the server');
  if (s.kind === 'tunnel' && s.status == null) return 'Tunnelled connection (not intercepted)';
  if (s.status == null) return s.state === 'responding' ? 'Receiving response…' : 'Waiting for response…';
  const text = statusText(s.status);
  let tip = `${s.status}${text ? ` ${text}` : ''}`;
  if (s.error) tip += ` · ${errorInfo(s.error).short}`;
  if (s.mocked) tip += ' · Mocked by a rule';
  else if (s.modified) tip += ' · Modified by a rule or breakpoint';
  return tip;
}

/** Live elapsed time for in-flight rows (shared clock). */
function LiveElapsed({ since }: { since: number }) {
  const now = useNow();
  return <>{formatElapsed(since, now)}</>;
}

function RuleCell({ rule }: { rule: string }) {
  const rules = useAppStore((s) => s.rules);
  const title = ruleTitles(rules).get(rule) ?? 'Deleted rule';
  return (
    <span className={clsx(styles.ruleChip, 'truncate')} data-tip={title} data-tip-overflow="">
      {title}
    </span>
  );
}

function PathText({ s }: { s: ExchangeSummary }) {
  if (s.kind === 'tls-failure') {
    return <span className={styles.failText}>{TLS_FAILURE_TEXT}</span>;
  }
  if (s.kind === 'tunnel') {
    return <span className={styles.dimText}>Tunnel to {formatAuthority(s.scheme, s.host, s.port)} — not intercepted</span>;
  }
  const q = s.path.indexOf('?');
  if (q < 0) return <>{s.path || '/'}</>;
  return (
    <>
      {s.path.slice(0, q)}
      <span className={styles.query}>{s.path.slice(q)}</span>
    </>
  );
}

function cell(column: TrafficColumnId, s: ExchangeSummary, monoPaths: boolean): ReactNode {
  switch (column) {
    case 'indicator': {
      const glyph = s.paused ? (
        <CirclePause size={12} strokeWidth={2.25} className={styles.pausedGlyph} aria-label="Paused" />
      ) : s.pinned ? (
        <Pin size={11} strokeWidth={2.25} className={styles.pinGlyph} aria-label="Pinned" />
      ) : null;
      return (
        <div role="gridcell" key={column} className={styles.indicator} data-tip={s.paused ? 'Paused at a breakpoint' : s.pinned ? 'Pinned' : undefined}>
          {s.color && <span className={styles.stripe} role="img" aria-label={`${s.color} highlight`} />}
          {glyph}
        </div>
      );
    }
    case 'id':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.end, styles.idCell, 'tnum')}>
          {s.id}
        </div>
      );
    case 'method':
      return (
        <div role="gridcell" key={column} className={styles.cell}>
          <MethodBadge method={s.method} />
        </div>
      );
    case 'status':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.statusCell)} data-tip={statusTip(s)}>
          <StatusBadge summary={s} tooltip={false} />
          {s.mocked ? (
            <span className={clsx(styles.marker, styles.mocked)} role="img" aria-label="Mocked">
              M
            </span>
          ) : s.modified ? (
            <span className={clsx(styles.marker, styles.modified)} role="img" aria-label="Modified">
              ✎
            </span>
          ) : null}
        </div>
      );
    case 'host': {
      const authority = formatAuthority(s.scheme, s.host, s.port);
      const portIdx = authority.length > s.host.length && !authority.startsWith('[') ? s.host.length : -1;
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.hostCell)}>
          <span className="truncate" data-tip={authority} data-tip-overflow="">
            {portIdx > 0 ? (
              <>
                {s.host}
                <span className={styles.port}>{authority.slice(portIdx)}</span>
              </>
            ) : (
              authority
            )}
          </span>
        </div>
      );
    }
    case 'path':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.pathCell, monoPaths && styles.mono)}>
          <span className="truncate" data-tip={s.kind === 'tls-failure' ? TLS_FAILURE_TEXT : s.kind === 'tunnel' ? undefined : s.path} data-tip-overflow="">
            <PathText s={s} />
          </span>
          {s.messageCount ? (
            <span className={styles.msgCount} data-tip={`${formatNumber(s.messageCount)} ${s.kind === 'event-stream' ? 'events' : 'messages'}`}>
              {formatNumber(s.messageCount)}
            </span>
          ) : null}
          {s.hasNote && <StickyNote size={12} strokeWidth={2} className={styles.noteGlyph} data-tip="Has a note" aria-label="Has a note" />}
        </div>
      );
    case 'type': {
      if (s.llm) {
        const tip = [`${providerInfo(s.llm.provider).name} LLM call`, s.llm.model, s.contentType].filter(Boolean).join(' · ');
        return (
          <div role="gridcell" key={column} className={clsx(styles.cell, styles.typeCell, styles.llmType)} data-tip={tip}>
            <Sparkles size={12} strokeWidth={2} className={styles.kindIcon} aria-hidden />
            <span className="truncate">LLM</span>
          </div>
        );
      }
      const Icon = kindIcon(s.contentKind);
      if (s.contentKind === 'unknown' || (s.status == null && s.responseSize === 0)) {
        return (
          <div role="gridcell" key={column} className={clsx(styles.cell, styles.subtleText)}>
            —
          </div>
        );
      }
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.typeCell)} style={{ '--kind': contentKindColorVar(s.contentKind) } as CSSProperties} data-tip={s.contentType}>
          {Icon && <Icon size={12} strokeWidth={2} className={styles.kindIcon} aria-hidden />}
          <span className="truncate">{contentKindLabel(s.contentKind)}</span>
        </div>
      );
    }
    case 'model':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.modelCell)}>
          {s.llm ? (
            <>
              <ProviderMark provider={s.llm.provider} size={15} />
              <span className="truncate" data-tip={`${providerInfo(s.llm.provider).name}${s.llm.model ? ` · ${s.llm.model}` : ''}`} data-tip-overflow="">
                {s.llm.model ?? providerInfo(s.llm.provider).name}
              </span>
            </>
          ) : (
            <span className={styles.subtleText}>—</span>
          )}
        </div>
      );
    case 'size': {
      if (s.kind === 'tunnel') {
        return (
          <div role="gridcell" key={column} className={clsx(styles.cell, styles.end, styles.numCell, styles.tunnelBytes, 'tnum')} data-tip={`Tunnel · sent ${formatBytes(s.requestSize)} · received ${formatBytes(s.responseSize)}`}>
            <ArrowDown size={10} strokeWidth={2.25} aria-hidden />
            {formatBytes(s.responseSize)}
          </div>
        );
      }
      const empty = !s.responseSize;
      const none = s.kind === 'tls-failure' || (empty && s.status == null);
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.end, styles.numCell, empty && styles.subtleText, 'tnum')}>
          {none ? '—' : formatBytes(s.responseSize ?? 0)}
        </div>
      );
    }
    case 'time': {
      if (isPending(s) && !s.paused) {
        return (
          <div role="gridcell" key={column} className={clsx(styles.cell, styles.end, styles.numCell, styles.pendingTime, 'tnum')} data-tip="In flight">
            <LiveElapsed since={s.startedAt} />
          </div>
        );
      }
      const sev = durationSeverity(s.duration);
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.end, styles.numCell, sev === 'slow' && styles.slow, sev === 'very-slow' && styles.verySlow, s.duration == null && styles.subtleText, 'tnum')}>
          {s.duration == null ? '—' : formatDuration(s.duration)}
        </div>
      );
    }
    case 'source':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.sourceCell)}>
          {s.source ? (
            <>
              <AppIcon name={s.source} size={13} />
              <span className="truncate" data-tip={s.source} data-tip-overflow="">
                {s.source}
              </span>
            </>
          ) : (
            <span className={styles.subtleText}>—</span>
          )}
        </div>
      );
    case 'started':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.numCell, 'tnum')}>
          {formatTime(s.startedAt)}
        </div>
      );
    case 'protocol':
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, styles.protoCell)} data-tip={s.httpVersion}>
          {protocolLabel(s)}
        </div>
      );
    case 'remoteIp': {
      const addr = remoteAddress(s);
      return (
        <div role="gridcell" key={column} className={clsx(styles.cell, addr ? styles.protoCell : styles.subtleText)}>
          {addr ? (
            <span className="truncate" data-tip={addr} data-tip-overflow="">
              {addr}
            </span>
          ) : (
            '—'
          )}
        </div>
      );
    }
    case 'rule':
      return (
        <div role="gridcell" key={column} className={styles.cell}>
          {s.rule ? <RuleCell rule={s.rule} /> : <span className={styles.subtleText}>—</span>}
        </div>
      );
    default:
      return <HeaderCell key={column} s={s} column={column} />;
  }
}

/**
 * One traffic row. Memoised on its props; reads its summary, selection and focus through
 * cheap per-row store subscriptions, so a new batch re-renders only the rows that changed.
 */
export const TrafficRow = memo(function TrafficRow({ id, index, top, columns, monoPaths }: TrafficRowProps) {
  const s = useExchangeSummary(id);
  const selected = useIsSelected(id);
  const active = useTraffic((snap) => snap.selection.active === id);
  const style = { transform: `translateY(${top}px)` } as CSSProperties;
  if (!s) return <div role="row" aria-rowindex={index + 2} className={styles.row} style={style} />;
  const failed = s.state === 'aborted' || (s.error != null && s.status == null) || s.kind === 'tls-failure';
  if (s.color) (style as Record<string, string>)['--tag'] = tagColorVar(s.color);
  return (
    <div
      role="row"
      id={`traffic-row-${id}`}
      data-row-id={id}
      aria-rowindex={index + 2}
      aria-selected={selected}
      className={clsx(styles.row, selected && styles.selected, active && styles.active, failed && styles.failed, s.paused && styles.paused, s.color && styles.tagged)}
      style={style}
    >
      {columns.map((c) => cell(c, s, monoPaths))}
    </div>
  );
});
