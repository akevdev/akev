import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { ArrowUpRight, Copy, History as HistoryIcon, Save, SearchX, SquarePlus, Trash2 } from 'lucide-react';
import { memo, useMemo, useRef, type CSSProperties, type KeyboardEvent } from 'react';
import { navigate } from '../../app/router';
import { ContextMenu, EmptyState, MethodBadge, toast } from '../../components/ui';
import { formatDateTime, formatDuration } from '../../lib/format';
import { errorInfo, statusClass } from '../../lib/http';
import { copyText } from '../../lib/platform';
import type { ErrorKind } from '../../api/types';
import { filterHistory, groupHistory, historyTimeLabel, type HistoryRow } from './library';
import type { HistoryEntry } from './model';
import { useNow } from './hooks';
import styles from './Sidebar.module.css';

const DAY_H = 28;
const ENTRY_H = 44;

const ERROR_KINDS = new Set<string>([
  'client-aborted',
  'dns-failed',
  'connection-refused',
  'connection-reset',
  'host-unreachable',
  'timeout',
  'upstream-cert-invalid',
  'upstream-tls-failed',
  'upstream-protocol-error',
  'client-protocol-error',
  'client-tls-rejected',
  'rule-closed',
  'rule-reset',
  'rule-timeout',
  'breakpoint-aborted',
  'upstream-proxy-failed',
  'internal',
]);

/** Short label for a history entry's outcome. */
function outcome(e: HistoryEntry): { text: string; color: string } | null {
  if (e.status != null) return { text: String(e.status), color: `var(--s-${statusClass(e.status)})` };
  if (e.error) return { text: ERROR_KINDS.has(e.error) ? errorInfo(e.error as ErrorKind).short : e.error, color: e.error === 'Cancelled' ? 'var(--text-subtle)' : 'var(--danger)' };
  return null;
}

export interface HistoryListProps {
  history: readonly HistoryEntry[];
  query: string;
  onOpen: (entry: HistoryEntry, newTab?: boolean) => void;
  onSave: (entry: HistoryEntry) => void;
  onRemove: (entry: HistoryEntry) => void;
}

/** Virtualised request history grouped by day. */
export const HistoryList = memo(function HistoryList({ history, query, onOpen, onSave, onRemove }: HistoryListProps) {
  const now = useNow(30_000);
  const filtered = useMemo(() => filterHistory(history, query), [history, query]);
  const newest = history[0]?.at ?? 0;
  const rows = useMemo(() => groupHistory(filtered, Math.max(now, newest)), [filtered, now, newest]);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (rows[i]?.type === 'day' ? DAY_H : ENTRY_H),
    overscan: 12,
    getItemKey: (i) => rows[i]?.key ?? i,
  });

  if (history.length === 0) {
    return <EmptyState size="sm" icon={HistoryIcon} title="No history yet" description="Every request you send lands here — click one to reopen it." className={styles.empty} />;
  }
  if (rows.length === 0) {
    return <EmptyState size="sm" icon={SearchX} title="No matches" description={`Nothing in history matches “${query}”.`} className={styles.empty} />;
  }

  const focusEntry = (index: number, dir: 1 | -1) => {
    let i = index;
    while (rows[i] && rows[i]?.type !== 'entry') i += dir;
    if (!rows[i]) return;
    virtualizer.scrollToIndex(i, { align: 'auto' });
    requestAnimationFrame(() => scrollRef.current?.querySelector<HTMLElement>(`[data-index="${i}"]`)?.focus());
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, index: number, entry: HistoryEntry) => {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      focusEntry(index + 1, 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      focusEntry(index - 1, -1);
    } else if (e.key === 'Enter') {
      e.preventDefault();
      onOpen(entry, e.metaKey || e.ctrlKey);
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      onRemove(entry);
      focusEntry(index + 1, 1);
    }
  };

  return (
    <div ref={scrollRef} className={styles.scroll}>
      <div role="list" aria-label="History" className={styles.virtual} style={{ height: virtualizer.getTotalSize() }}>
        {virtualizer.getVirtualItems().map((v) => {
          const row = rows[v.index] as HistoryRow;
          const style: CSSProperties = { transform: `translateY(${v.start}px)`, height: v.size };
          if (row.type === 'day') {
            return (
              <div key={row.key} className={styles.dayRow} style={style} role="presentation">
                {row.label}
              </div>
            );
          }
          return <HistoryItem key={row.key} entry={row.entry} index={v.index} style={style} now={now} onOpen={onOpen} onSave={onSave} onRemove={onRemove} onKeyDown={onKeyDown} />;
        })}
      </div>
    </div>
  );
});

interface HistoryItemProps {
  entry: HistoryEntry;
  index: number;
  style: CSSProperties;
  now: number;
  onOpen: (entry: HistoryEntry, newTab?: boolean) => void;
  onSave: (entry: HistoryEntry) => void;
  onRemove: (entry: HistoryEntry) => void;
  onKeyDown: (e: KeyboardEvent<HTMLDivElement>, index: number, entry: HistoryEntry) => void;
}

const HistoryItem = memo(function HistoryItem({ entry, index, style, now, onOpen, onSave, onRemove, onKeyDown }: HistoryItemProps) {
  const out = outcome(entry);
  const url = entry.host ? `${entry.host}${entry.path}` : entry.draft.url;
  return (
    <ContextMenu
      items={() => [
        { label: 'Open', icon: SquarePlus, onSelect: () => onOpen(entry) },
        { label: 'Open in new tab', icon: SquarePlus, onSelect: () => onOpen(entry, true) },
        entry.exchangeId != null && { label: 'Open in Traffic', icon: ArrowUpRight, onSelect: () => navigate(`/traffic/${entry.exchangeId}`) },
        { type: 'separator' },
        {
          label: 'Copy URL',
          icon: Copy,
          onSelect: () => {
            void copyText(entry.draft.url).then((ok) => ok && toast.success('URL copied'));
          },
        },
        { label: 'Save…', icon: Save, onSelect: () => setTimeout(() => onSave(entry), 0) },
        { type: 'separator' },
        { label: 'Remove from history', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => onRemove(entry) },
      ]}
    >
      <div
        role="listitem"
        // biome-ignore lint/a11y/noNoninteractiveTabindex: list row that opens on Enter and has a context menu; the list moves focus between rows
        tabIndex={0}
        data-index={index}
        className={styles.historyRow}
        style={style}
        title={`${entry.method} ${entry.draft.url}\n${formatDateTime(entry.at)}${entry.bodyOmitted ? '\n(large body not kept in history)' : ''}`}
        onClick={(e) => onOpen(entry, e.metaKey || e.ctrlKey)}
        onAuxClick={(e) => e.button === 1 && onOpen(entry, true)}
        onKeyDown={(e) => onKeyDown(e, index, entry)}
      >
        <div className={styles.historyLine}>
          <MethodBadge method={entry.method} size="sm" className={styles.historyMethod} />
          <span className={styles.historyPath}>{entry.path || '/'}</span>
          {out && (
            <span className={clsx(styles.historyStatus, 'tnum')} style={{ color: out.color }}>
              {out.text}
            </span>
          )}
        </div>
        <div className={clsx(styles.historyLine, styles.historyMeta)}>
          <span className={styles.historyHost}>{entry.host || url}</span>
          <span className={clsx(styles.historyTime, 'tnum')}>
            {entry.durationMs != null && <>{formatDuration(entry.durationMs)} · </>}
            {historyTimeLabel(entry.at, now)}
          </span>
        </div>
      </div>
    </ContextMenu>
  );
});
