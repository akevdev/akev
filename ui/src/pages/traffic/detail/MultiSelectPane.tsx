import clsx from 'clsx';
import { ChartGantt, Copy, FileDown, GitCompareArrows, Highlighter, Layers, Link, Pin, PinOff, Trash2, X } from 'lucide-react';
import { useMemo, useState, type CSSProperties } from 'react';
import type { ExchangeId, ExchangeSummary } from '../../../api/types';
import { useCommands } from '../../../app/commands';
import { ExchangeCompare } from '../../../components/exchange';
import { Button, Dialog, DropdownMenu, DropdownMenuContent, DropdownMenuItems, DropdownMenuTrigger, Kbd, MethodBadge, Tooltip } from '../../../components/ui';
import { formatBytes, formatDuration, formatNumber, formatPercent, formatTime, pluralize } from '../../../lib/format';
import { trafficStore, useTraffic } from '../../../state/traffic';
import { copyAsSnippet, copyUrls, deleteExchanges, exportHar, setHighlight, setPinned } from '../actions/exchangeActions';
import { highlightItems } from '../actions/menus';
import { useNow } from './hooks/misc';
import { BUCKET_LABELS, computeMultiStats, type StatusBucket } from './lib/multiStats';
import styles from './MultiSelectPane.module.css';
import { setTrafficView, useTrafficViewMode } from '../timeline/timelineState';

const BUCKET_COLOR: Record<StatusBucket, string> = {
  '1xx': 'var(--s-1xx)',
  '2xx': 'var(--s-2xx)',
  '3xx': 'var(--s-3xx)',
  '4xx': 'var(--s-4xx)',
  '5xx': 'var(--s-5xx)',
  err: 'var(--s-err)',
  pending: 'var(--text-subtle)',
  other: 'var(--border-strong)',
};

/**
 * Selected summaries in id order. Recomputed when the selection changes, and refreshed once a
 * second so live rows (pending → complete) update the stats without per-frame work.
 */
function useSelectedSummaries(): { ids: ExchangeId[]; summaries: ExchangeSummary[] } {
  const selection = useTraffic((s) => s.selection.ids);
  const tick = useNow(1000);
  return useMemo(() => {
    const summaries: ExchangeSummary[] = [];
    for (const id of selection) {
      const s = trafficStore.get(id);
      if (s) summaries.push(s);
    }
    summaries.sort((a, b) => a.id - b.id);
    return { ids: summaries.map((s) => s.id), summaries };
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [selection, tick]);
}

/** Several exchanges selected (DESIGN.md §6.5): stats + bulk actions + Compare for two. */
export function MultiSelectPane() {
  const { ids, summaries } = useSelectedSummaries();
  const stats = useMemo(() => computeMultiStats(summaries, 5), [summaries]);
  const [compareOpen, setCompareOpen] = useState(false);
  const allPinned = stats.count > 0 && stats.pinned === stats.count;
  const canCompare = ids.length === 2;
  const viewMode = useTrafficViewMode();
  const colors = new Set(summaries.map((s) => s.color));
  const highlight = colors.size === 1 ? summaries[0]?.color : 'mixed';

  // Keyboard shortcuts for the selection are registered once, by the traffic list.
  useCommands([
    { id: 'selection.har', title: `Export ${pluralize(ids.length, 'exchange')} as HAR`, group: 'Traffic', icon: FileDown, run: () => exportHar('selected', ids) },
    { id: 'selection.urls', title: 'Copy selected URLs', group: 'Traffic', icon: Link, run: () => void copyUrls(ids) },
    { id: 'selection.curl', title: 'Copy selected as cURL', group: 'Traffic', icon: Copy, shortcut: 'mod+shift+c', run: () => void copyAsSnippet(ids, 'curl') },
    { id: 'selection.pin', title: allPinned ? 'Unpin selected' : 'Pin selected', group: 'Traffic', icon: Pin, shortcut: 'mod+p', run: () => void setPinned(ids, !allPinned) },
    ...(canCompare ? [{ id: 'selection.compare', title: 'Compare the two selected exchanges', group: 'Traffic', icon: GitCompareArrows, run: () => setCompareOpen(true) }] : []),
    { id: 'selection.delete', title: `Delete ${pluralize(ids.length, 'exchange')}`, group: 'Traffic', icon: Trash2, shortcut: 'delete', run: () => deleteExchanges(ids) },
  ]);

  const maxHost = stats.topHosts[0]?.count ?? 1;
  const total = Math.max(1, stats.count);

  return (
    <div className={styles.root}>
      <div className={styles.inner}>
        <header className={styles.head}>
          <span className={styles.icon} aria-hidden>
            <Layers size={18} strokeWidth={1.75} />
          </span>
          <div className={styles.titles}>
            <h2 className={styles.title}>
              <span className="tnum">{formatNumber(stats.count)}</span> exchanges selected
            </h2>
            <p className={styles.subtitle}>
              {pluralize(stats.hostCount, 'host')}
              {stats.firstStart != null && stats.lastEnd != null && (
                <>
                  {' · '}
                  <span className="tnum">
                    {formatTime(stats.firstStart, false)} → {formatTime(stats.lastEnd, false)}
                  </span>
                </>
              )}
            </p>
          </div>
          <Tooltip content="Clear selection" shortcut="escape">
            <button type="button" className={styles.clear} onClick={() => trafficStore.clearSelection()} aria-label="Clear selection">
              <X size={15} strokeWidth={1.75} aria-hidden />
            </button>
          </Tooltip>
        </header>

        <div className={styles.actions} role="toolbar" aria-label="Bulk actions">
          {canCompare && (
            <Button size="sm" variant="primary" icon={GitCompareArrows} onClick={() => setCompareOpen(true)}>
              Compare
            </Button>
          )}
          {viewMode === 'list' && (
            <Button size="sm" icon={ChartGantt} onClick={() => setTrafficView('timeline')}>
              Timeline
            </Button>
          )}
          <Button size="sm" icon={FileDown} onClick={() => exportHar('selected', ids)}>
            Export HAR
          </Button>
          <Button size="sm" icon={Link} onClick={() => void copyUrls(ids)}>
            Copy URLs
          </Button>
          <Tooltip content="Copy as cURL" shortcut="mod+shift+c">
            <Button size="sm" icon={Copy} onClick={() => void copyAsSnippet(ids, 'curl')}>
              Copy as cURL
            </Button>
          </Tooltip>
          <Tooltip content={allPinned ? 'Unpin all' : 'Pin all'} shortcut="mod+p">
            <Button size="sm" icon={allPinned ? PinOff : Pin} onClick={() => void setPinned(ids, !allPinned)}>
              {allPinned ? 'Unpin' : 'Pin'}
            </Button>
          </Tooltip>
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <Button size="sm" icon={Highlighter}>
                Highlight
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              <DropdownMenuItems items={highlightItems(highlight, (c) => void setHighlight(ids, c))} />
            </DropdownMenuContent>
          </DropdownMenu>
          <Button size="sm" variant="subtle-danger" icon={Trash2} onClick={() => deleteExchanges(ids)}>
            Delete
          </Button>
        </div>

        <div className={styles.tiles}>
          <Tile label="Total size" value={formatBytes(stats.totalBytes)} hint={`↑ ${formatBytes(stats.requestBytes)}  ↓ ${formatBytes(stats.responseBytes)}`} />
          <Tile label="Time span" value={formatDuration(stats.spanMs)} hint={stats.count > 1 ? 'first start → last end' : undefined} />
          <Tile label="Avg duration" value={stats.avgDuration != null ? formatDuration(stats.avgDuration) : '—'} hint={stats.maxDuration != null ? `slowest ${formatDuration(stats.maxDuration)}` : 'none finished'} />
          <Tile label="Failed" value={formatNumber(stats.failed)} hint={stats.count ? formatPercent(stats.failed / total) : undefined} tone={stats.failed > 0 ? 'danger' : undefined} />
        </div>

        <section className={styles.section} aria-label="Status breakdown">
          <h3 className={styles.sectionTitle}>Status</h3>
          <div className={styles.stack} role="img" aria-label={stats.buckets.map((b) => `${BUCKET_LABELS[b.bucket]}: ${b.count}`).join(', ')}>
            {stats.buckets.map((b) => (
              <Tooltip key={b.bucket} content={`${BUCKET_LABELS[b.bucket]} · ${formatNumber(b.count)} (${formatPercent(b.count / total)})`}>
                <span
                  className={clsx(styles.segment, b.bucket === 'err' && styles.hatched)}
                  style={{ flexGrow: b.count, '--seg-color': BUCKET_COLOR[b.bucket] } as CSSProperties}
                />
              </Tooltip>
            ))}
          </div>
          <ul className={styles.legend}>
            {stats.buckets.map((b) => (
              <li key={b.bucket} className={styles.legendItem}>
                <span className={clsx(styles.swatch, b.bucket === 'err' && styles.hatched)} style={{ '--seg-color': BUCKET_COLOR[b.bucket] } as CSSProperties} aria-hidden />
                <span>{BUCKET_LABELS[b.bucket]}</span>
                <span className={clsx(styles.legendCount, 'tnum')}>{formatNumber(b.count)}</span>
              </li>
            ))}
          </ul>
        </section>

        <section className={styles.section} aria-label="Top hosts">
          <h3 className={styles.sectionTitle}>
            Top hosts
            {stats.hostCount > stats.topHosts.length && <span className={styles.sectionHint}>of {formatNumber(stats.hostCount)}</span>}
          </h3>
          <ul className={styles.hosts}>
            {stats.topHosts.map((h) => (
              <li key={h.host} className={styles.host}>
                <span className={clsx(styles.hostName, 'truncate')} title={h.host}>
                  {h.host}
                </span>
                <Tooltip content={`${formatNumber(h.count)} of ${formatNumber(stats.count)} selected · ${formatBytes(h.bytes)}`}>
                  <span className={styles.hostTrack}>
                    <span className={styles.hostBar} style={{ width: `${Math.max(2, (h.count / maxHost) * 100)}%` }} />
                  </span>
                </Tooltip>
                <span className={clsx(styles.hostCount, 'tnum')}>{formatNumber(h.count)}</span>
                <span className={clsx(styles.hostBytes, 'tnum')}>{formatBytes(h.bytes)}</span>
              </li>
            ))}
          </ul>
        </section>

        {canCompare && <ComparePreview summaries={summaries} onCompare={() => setCompareOpen(true)} />}

        <p className={styles.tip}>
          <Kbd combo="mod+a" size="sm" /> selects all visible · <Kbd combo="escape" size="sm" /> clears the selection
        </p>
      </div>

      {canCompare && (
        <Dialog open={compareOpen} onOpenChange={setCompareOpen} title={`Compare #${ids[0]} and #${ids[1]}`} description="Request line, headers, bodies, status and timing side by side." width={1180} flush focusContent className={styles.compareDialog}>
          <div className={styles.compareBody}>
            <ExchangeCompare a={ids[0] as number} b={ids[1] as number} />
          </div>
        </Dialog>
      )}
    </div>
  );
}

function Tile({ label, value, hint, tone }: { label: string; value: string; hint?: string; tone?: 'danger' }) {
  return (
    <div className={styles.tile}>
      <span className={styles.tileLabel}>{label}</span>
      <span className={clsx(styles.tileValue, 'tnum', tone === 'danger' && styles.danger)}>{value}</span>
      {hint && <span className={clsx(styles.tileHint, 'tnum')}>{hint}</span>}
    </div>
  );
}

function ComparePreview({ summaries, onCompare }: { summaries: ExchangeSummary[]; onCompare: () => void }) {
  const [a, b] = summaries;
  if (!a || !b) return null;
  return (
    <section className={styles.section} aria-label="Compare">
      <h3 className={styles.sectionTitle}>Compare</h3>
      <button type="button" className={styles.compareCard} onClick={onCompare}>
        {[a, b].map((s) => (
          <span key={s.id} className={styles.compareRow}>
            <span className={clsx(styles.compareId, 'tnum')}>#{s.id}</span>
            <MethodBadge method={s.method} fixedWidth />
            <span className={clsx(styles.compareUrl, 'truncate')}>
              {s.host}
              <span className={styles.comparePath}>{s.path}</span>
            </span>
            <span className={clsx(styles.compareMeta, 'tnum')}>{s.status ?? '—'}</span>
            <span className={clsx(styles.compareMeta, 'tnum')}>{formatDuration(s.duration)}</span>
          </span>
        ))}
        <span className={styles.compareCta}>
          <GitCompareArrows size={13} strokeWidth={2} aria-hidden />
          Open side-by-side diff
        </span>
      </button>
    </section>
  );
}
