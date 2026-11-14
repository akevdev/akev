import clsx from 'clsx';
import { ArrowDownAZ, ArrowDownWideNarrow, ChevronRight, Filter, Globe, PanelLeftClose, Pin, PinOff, X } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { AppIcon, IconButton, SearchInput, SegmentedControl } from '../../../components/ui';
import { formatNumber } from '../../../lib/format';
import { trafficStore, useTraffic } from '../../../state/traffic';
import { usePagePref } from '../../../state/ui';
import { buildRows, hasClause, HostIndex, removeClause, type SidebarRow, type SidebarSort } from './hostIndex';
import { clearScope, toggleScope, togglePinnedHost, toggleSidebar, usePinnedHosts } from './sidebarState';
import styles from './TrafficSidebar.module.css';

const ROW_H = 28;
const OVERSCAN = 12;
/** Count updates are coalesced (live traffic would otherwise re-render every frame). */
const REFRESH_MS = 250;

/** Counts for the whole capture, kept in sync incrementally (see `hostIndex.ts`). */
const index = new HostIndex();
trafficStore.onLifecycle(() => index.invalidate());

function syncIndex(): boolean {
  const s = trafficStore.getSnapshot();
  return index.sync(s.order, s.summaries);
}

/** The index version, refreshed at most every {@link REFRESH_MS} while traffic flows. */
function useHostIndexVersion(): number {
  const [version, setVersion] = useState(() => {
    syncIndex();
    return index.version;
  });
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const tick = () => {
      timer = null;
      if (syncIndex()) setVersion(index.version);
    };
    const unsubscribe = trafficStore.subscribe(() => {
      if (timer == null) timer = setTimeout(tick, REFRESH_MS);
    });
    tick();
    return () => {
      unsubscribe();
      if (timer != null) clearTimeout(timer);
    };
  }, []);
  return version;
}

function rowLabel(r: SidebarRow): string {
  switch (r.kind) {
    case 'domain':
      return r.domain;
    case 'host':
      return r.host;
    case 'source':
      return r.source || 'Unknown app';
    default:
      return r.label;
  }
}

function Row({ row, active, expandedToggle }: { row: SidebarRow; active: boolean; expandedToggle: (domain: string) => void }) {
  if (row.kind === 'heading') {
    return (
      <div className={styles.heading} role="presentation">
        <span>{row.label}</span>
        {row.count != null && <span className="tnum">{formatNumber(row.count)}</span>}
      </div>
    );
  }
  if (row.kind === 'empty') return <div className={styles.emptyRow}>{row.label}</div>;
  const count = row.count;
  const label = rowLabel(row);
  return (
    <div className={clsx(styles.row, active && styles.rowActive, row.kind === 'host' && row.depth === 1 && styles.rowNested)}>
      {row.kind === 'domain' ? (
        <button type="button" className={styles.chevron} aria-label={row.expanded ? `Collapse ${row.domain}` : `Expand ${row.domain}`} aria-expanded={row.expanded} onClick={() => expandedToggle(row.domain)}>
          <ChevronRight size={12} strokeWidth={2.25} className={clsx(row.expanded && styles.chevronOpen)} aria-hidden />
        </button>
      ) : row.kind === 'host' && row.depth === 0 ? (
        <span className={styles.chevronSpacer} aria-hidden />
      ) : null}
      <button
        type="button"
        className={styles.pick}
        aria-pressed={active}
        title={active ? `Stop narrowing to ${label}` : row.kind === 'domain' ? `Show ${row.domain} and its ${row.hosts} hosts` : `Show only ${label}`}
        onClick={() => toggleScope(row.clause)}
      >
        {row.kind === 'source' ? <AppIcon name={row.source || undefined} size={13} /> : <Globe size={12} strokeWidth={2} className={styles.globe} aria-hidden />}
        <span className={clsx(styles.label, row.kind === 'source' && !row.source && styles.unknown)}>{label}</span>
        {row.kind === 'domain' && <span className={styles.hostCount}>{row.hosts} hosts</span>}
        <span className={clsx(styles.count, 'tnum')}>{formatNumber(count)}</span>
      </button>
      {row.kind === 'host' && (
        <IconButton
          size="sm"
          icon={row.pinned ? PinOff : Pin}
          label={row.pinned ? `Unpin ${row.host}` : `Pin ${row.host} to the top`}
          className={clsx(styles.pin, row.pinned && styles.pinOn)}
          onClick={() => togglePinnedHost(row.host)}
        />
      )}
    </div>
  );
}

/**
 * The Traffic page's left sidebar: hosts grouped by registrable domain (pinned hosts first) or
 * client apps, with request counts for the whole capture. Picking an item ANDs a clause onto the
 * filter (shown on top, removable) instead of replacing what the user typed.
 */
export function TrafficSidebar() {
  const version = useHostIndexVersion();
  const [view, setView] = usePagePref<'hosts' | 'apps'>('traffic.sidebarView', 'hosts');
  const [sort, setSort] = usePagePref<SidebarSort>('traffic.sidebarSort', 'name');
  const [expanded, setExpanded] = useState<ReadonlySet<string>>(() => new Set());
  const [query, setQuery] = useState('');
  const pinned = usePinnedHosts();
  const filterText = useTraffic((s) => s.filter.text);

  // biome-ignore lint/correctness/useExhaustiveDependencies: version marks new counts in the (mutable) index
  const rows = useMemo(() => buildRows(index, { view, pinned, expanded, sort, query }), [version, view, pinned, expanded, sort, query]);
  const active = useMemo(() => {
    const out = new Map<string, string>();
    for (const r of rows) if ('clause' in r && !out.has(r.clause) && hasClause(filterText, r.clause)) out.set(r.clause, rowLabel(r));
    return out;
  }, [rows, filterText]);
  const rest = useMemo(() => [...active.keys()].reduce((t, c) => removeClause(t, c), filterText).trim(), [active, filterText]);

  const toggleDomain = (domain: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(domain)) next.delete(domain);
      else next.add(domain);
      return next;
    });

  // ---- virtualised rows (fixed height)
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(600);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scrollTop + height) / ROW_H) + OVERSCAN);

  return (
    <aside className={styles.sidebar} aria-label="Hosts and apps">
      <div className={styles.head}>
        <SegmentedControl<'hosts' | 'apps'>
          size="sm"
          value={view}
          onValueChange={setView}
          items={[
            { value: 'hosts', label: 'Hosts' },
            { value: 'apps', label: 'Apps' },
          ]}
          aria-label="Group by"
          className={styles.views}
        />
        <IconButton
          size="sm"
          icon={sort === 'count' ? ArrowDownWideNarrow : ArrowDownAZ}
          label={sort === 'count' ? 'Sorted by request count (click: by name)' : 'Sorted by name (click: by request count)'}
          onClick={() => setSort(sort === 'count' ? 'name' : 'count')}
        />
        <IconButton size="sm" icon={PanelLeftClose} label="Hide sidebar" shortcut="mod+b" onClick={toggleSidebar} />
      </div>
      <div className={styles.search}>
        <SearchInput size="sm" value={query} onValueChange={setQuery} placeholder={view === 'apps' ? 'Find an app…' : 'Find a host…'} aria-label={view === 'apps' ? 'Find an app' : 'Find a host'} />
      </div>
      {active.size > 0 && (
        <section className={styles.scope} aria-label="Sidebar filter">
          <Filter size={12} strokeWidth={2.25} className={styles.scopeIcon} aria-hidden />
          <div className={styles.scopeBody}>
            {[...active].map(([c, label]) => (
              <span key={c} className={styles.scopeChip}>
                <span className={styles.scopeLabel}>Only {label}</span>
                <button type="button" className={styles.scopeClear} aria-label={`Stop narrowing to ${label}`} onClick={() => clearScope(c)}>
                  <X size={11} strokeWidth={2.5} aria-hidden />
                </button>
              </span>
            ))}
            <span className={styles.scopeRest}>{rest ? <>and your filter <code>{rest}</code></> : 'added to the filter bar'}</span>
          </div>
        </section>
      )}
      <div ref={scrollRef} className={styles.scroller} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}>
        <div className={styles.canvas} style={{ height: rows.length * ROW_H } as CSSProperties}>
          {rows.slice(first, last).map((r, i) => (
            <div key={r.key} className={styles.slot} style={{ transform: `translateY(${(first + i) * ROW_H}px)` }}>
              <Row row={r} active={'clause' in r && active.has(r.clause)} expandedToggle={toggleDomain} />
            </div>
          ))}
        </div>
      </div>
    </aside>
  );
}

export default TrafficSidebar;
