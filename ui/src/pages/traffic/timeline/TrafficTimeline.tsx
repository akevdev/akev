import clsx from 'clsx';
import { ChartGantt, List, Maximize2, ZoomIn, ZoomOut } from 'lucide-react';
import { useEffect, useLayoutEffect, useMemo, useRef, useState, useSyncExternalStore, type CSSProperties, type WheelEvent } from 'react';
import { api } from '../../../api/client';
import type { ExchangeId, ExchangeSummary } from '../../../api/types';
import { niceTicks, PHASE_COLORS, PHASE_INFO, tickLabel, type PhaseId } from '../../../components/exchange/model/timing';
import { Button, EmptyState, IconButton, MethodBadge, StatusBadge } from '../../../components/ui';
import { formatNumber, formatTime } from '../../../lib/format';
import { isMac } from '../../../lib/platform';
import { shallowEqual, trafficStore, useTraffic } from '../../../state/traffic';
import { useRowHeight } from '../../../state/ui';
import { isRunning, rowPlot, timelineIds, timeRange, TimingCache, type SegmentId } from './timelineModel';
import { setTrafficView } from './timelineState';
import styles from './TrafficTimeline.module.css';

/** Width of the sticky label column (px). */
const LABEL_W = 300;
const HEADER_H = 30;
const OVERSCAN = 15;
/** Most zoom (track width × the visible width). */
const MAX_ZOOM = 64;

const cache = new TimingCache((ids, signal) => api.exchanges.timings(ids, signal));
trafficStore.onLifecycle((e) => {
  if (e.type === 'removed') cache.forget(e.ids);
  else cache.reset();
});

const SEGMENT_COLORS: Readonly<Record<SegmentId, string>> = { ...PHASE_COLORS, total: 'var(--text-subtle)' };
const LEGEND: readonly PhaseId[] = ['dns', 'connect', 'tls', 'sent', 'wait', 'download'];

/** A clock that ticks once a second while something on screen is still running. */
function useNow(live: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!live) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [live]);
  return live ? now : Date.now();
}

function Bar({ s, t0, scale, now }: { s: ExchangeSummary; t0: number; scale: number; now: number }) {
  const running = isRunning(s);
  useSyncExternalStore(cache.subscribe, cache.getVersion, cache.getVersion);
  useEffect(() => cache.ensure(s.id, !running), [s.id, running]);
  const plot = rowPlot(s, cache.get(s.id), now);
  return (
    <div className={styles.bars} title={plot.title}>
      {plot.segments.map((seg, i) => (
        <span
          // biome-ignore lint/suspicious/noArrayIndexKey: segments are positional
          key={i}
          className={clsx(styles.segment, seg.id === 'total' && styles.segmentTotal, seg.id === 'pending' && styles.segmentPending)}
          style={{ left: (seg.start - t0) * scale, width: Math.max(2, seg.span * scale), background: SEGMENT_COLORS[seg.id] } as CSSProperties}
        />
      ))}
    </div>
  );
}

function Row({ id, top, t0, scale, now, selected }: { id: ExchangeId; top: number; t0: number; scale: number; now: number; selected: boolean }) {
  const s = useTraffic((snap) => snap.summaries.get(id));
  if (!s) return null;
  return (
    <div
      role="row"
      aria-selected={selected}
      className={clsx(styles.row, selected && styles.rowSelected)}
      style={{ transform: `translateY(${top}px)` }}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        if (isMac ? e.metaKey : e.ctrlKey) trafficStore.toggle(id);
        else trafficStore.select(id);
      }}
    >
      <div role="rowheader" className={styles.label}>
        <MethodBadge method={s.method} />
        <StatusBadge summary={s} tooltip={false} />
        <span className={styles.url}>
          <span className={styles.host}>{s.host}</span>
          <span className={styles.path}>{s.path.split('?')[0]}</span>
        </span>
      </div>
      <div role="gridcell" className={styles.track}>
        <Bar s={s} t0={t0} scale={scale} now={now} />
      </div>
    </div>
  );
}

/**
 * The traffic timeline (Traffic › Timeline): the visible exchanges — or the selection, when
 * several are selected — as a waterfall on one wall-clock axis, with the Timing tab's phases
 * (DNS, connect, TLS, sent, waiting, download) once loaded for the rows on screen. Virtualised;
 * plots at most the newest 5,000 and says so.
 */
export function TrafficTimeline() {
  const rowH = useRowHeight();
  const { visible, selected } = useTraffic((s) => ({ visible: s.visible, selected: s.selection.ids }), shallowEqual);
  const { ids, hidden, selection } = useMemo(() => timelineIds(visible, selected), [visible, selected]);
  const summaries = useTraffic((s) => s.summaries);
  const version = useTraffic((s) => s.version);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [size, setSize] = useState({ w: 800, h: 600 });
  const [scrollTop, setScrollTop] = useState(0);
  const [zoom, setZoom] = useState<number>(1);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const read = () => setSize({ w: el.clientWidth, h: el.clientHeight });
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Open at the newest exchanges, like the list's live tail.
  const opened = useRef(false);
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el || opened.current || ids.length === 0) return;
    opened.current = true;
    el.scrollTop = el.scrollHeight;
  }, [ids.length]);

  const first = Math.max(0, Math.floor(scrollTop / rowH) - OVERSCAN);
  const last = Math.min(ids.length, Math.ceil((scrollTop + size.h) / rowH) + OVERSCAN);
  // biome-ignore lint/correctness/useExhaustiveDependencies: version marks summary updates (the map is stable)
  const live = useMemo(() => ids.slice(first, last).some((id) => {
    const s = summaries.get(id);
    return s != null && isRunning(s);
  }), [ids, first, last, summaries, version]);
  const now = useNow(live);
  // biome-ignore lint/correctness/useExhaustiveDependencies: version marks summary updates (the map is stable)
  const { t0, t1 } = useMemo(() => timeRange(ids, summaries, now), [ids, summaries, now, version]);

  const trackW = Math.max(200, (size.w - LABEL_W - 16) * zoom);
  const scale = trackW / (t1 - t0);
  const ticks = useMemo(() => niceTicks(t1 - t0, Math.max(3, Math.round(trackW / 140))), [t1, t0, trackW]);

  /** Zoom keeping the time under the pointer in place, or (buttons) the rows on screen in view. */
  const zoomTo = (next: number, pointerX?: number) => {
    const el = scrollRef.current;
    const z = Math.min(MAX_ZOOM, Math.max(1, next));
    if (!el || z === zoom) return;
    const middle = (size.w - LABEL_W) / 2;
    let anchorX = pointerX ?? middle;
    let at = (el.scrollLeft + anchorX) / (trackW || 1);
    if (pointerX == null) {
      const shown = timeRange(ids.slice(first, last), summaries, now);
      at = ((shown.t0 + shown.t1) / 2 - t0) / (t1 - t0);
      anchorX = middle;
    }
    setZoom(z);
    requestAnimationFrame(() => {
      const w = Math.max(200, (size.w - LABEL_W - 16) * z);
      el.scrollLeft = Math.max(0, at * w - anchorX);
    });
  };
  const onWheel = (e: WheelEvent<HTMLDivElement>) => {
    if (!(e.ctrlKey || e.metaKey)) return;
    e.preventDefault();
    const rect = e.currentTarget.getBoundingClientRect();
    zoomTo(e.deltaY < 0 ? zoom * 2 : zoom / 2, e.clientX - rect.left - LABEL_W);
  };

  const rows = [];
  for (let i = first; i < last; i++) {
    const id = ids[i] as ExchangeId;
    rows.push(<Row key={i % (last - first + 1)} id={id} top={HEADER_H + i * rowH} t0={t0} scale={scale} now={now} selected={selected.has(id)} />);
  }

  return (
    <div className={styles.root}>
      <div className={styles.bar}>
        <span className={styles.summary}>
          <ChartGantt size={14} aria-hidden />
          {selection ? `${formatNumber(ids.length)} selected exchanges` : `${formatNumber(ids.length)} exchanges`}
          {ids.length > 0 && <span className={styles.span}>· {tickLabel(t1 - t0)} from {formatTime(t0)}</span>}
        </span>
        <span className={styles.legend} aria-label="Phases">
          {LEGEND.map((p) => (
            <span key={p} className={styles.legendItem}>
              <span className={styles.swatch} style={{ background: PHASE_COLORS[p] }} aria-hidden />
              {PHASE_INFO[p].label}
            </span>
          ))}
        </span>
        <span className={styles.tools}>
          <IconButton size="sm" icon={ZoomOut} label="Zoom out" disabled={zoom <= 1} onClick={() => zoomTo(zoom / 2)} />
          <span className={clsx(styles.zoom, 'tnum')}>{zoom}×</span>
          <IconButton size="sm" icon={ZoomIn} label={`Zoom in (${isMac ? '⌘' : 'Ctrl'} + scroll)`} disabled={zoom >= MAX_ZOOM} onClick={() => zoomTo(zoom * 2)} />
          <IconButton size="sm" icon={Maximize2} label="Fit everything" disabled={zoom === 1} onClick={() => zoomTo(1)} />
          <IconButton size="sm" icon={List} label="Back to the list" shortcut="t" onClick={() => setTrafficView('list')} />
        </span>
      </div>
      {hidden > 0 && (
        <div className={styles.cap} role="status">
          Showing the newest {formatNumber(ids.length)} of {formatNumber(ids.length + hidden)} {selection ? 'selected exchanges' : 'exchanges'}. Narrow the filter{selection ? '' : ' or select the exchanges to compare'} to see the rest.
        </div>
      )}
      {ids.length === 0 ? (
        <div className={styles.empty}>
          <EmptyState icon={ChartGantt} title="Nothing to plot" description="Exchanges matching the filter appear here on one time axis." actions={<Button onClick={() => setTrafficView('list')}>Back to the list</Button>} />
        </div>
      ) : (
        <div ref={scrollRef} className={styles.scroller} onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)} onWheel={onWheel} role="grid" aria-label="Exchanges on a timeline" aria-rowcount={ids.length}>
          <div className={styles.canvas} style={{ height: HEADER_H + ids.length * rowH, width: LABEL_W + trackW + 16, ['--row-h' as string]: `${rowH}px`, ['--label-w' as string]: `${LABEL_W}px` }}>
            <div className={styles.header} role="row">
              <div className={styles.headerLabel} role="columnheader">
                Exchange
              </div>
              <div className={styles.ticks} role="columnheader" style={{ width: trackW }}>
                {ticks.map((t) => (
                  <span key={t} className={styles.tick} style={{ left: t * scale }}>
                    {tickLabel(t)}
                  </span>
                ))}
              </div>
            </div>
            <div className={styles.grid} style={{ left: LABEL_W, width: trackW, backgroundSize: `${(ticks[1] ?? 1) * scale}px 100%` }} aria-hidden />
            {rows}
          </div>
        </div>
      )}
    </div>
  );
}

export default TrafficTimeline;
