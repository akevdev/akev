// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { AlertTriangle, ArrowLeftRight, CheckCircle2, ChevronRight, Columns2, FileX2, FoldVertical, Rows2, Space, UnfoldVertical } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { api, ApiError, errorMessage } from '../../api/client';
import type { BodyView, ExchangeDetail, ExchangeId, Side } from '../../api/types';
import { useCommands } from '../../app/commands';
import { useShortcut } from '../../app/shortcuts';
import type { TextDiffResult } from '../../lib/diff';
import { formatBytes, formatDuration, formatNumber, formatTime } from '../../lib/format';
import { useExchangeSummary } from '../../state/traffic';
import { usePagePref } from '../../state/ui';
import { Button, EmptyState, IconButton, MethodBadge, SegmentedControl, Skeleton, Spinner, StatusBadge, Tooltip } from '../ui';
import { fetchBodyView } from './body/data';
import { useTextDiff } from './diff/useTextDiff';
import { bodyRefreshKey } from './model/body';
import { bodyText, COMPARE_SECTION_LABELS, compareTiming, formatDelta, headersText, requestLineText, statusLineText, type CompareSectionId, type TimingRow } from './model/compare';
import { TextDiff } from './TextDiff';
import { smoothScroll } from '../../lib/motion';
import styles from './ExchangeCompare.module.css';

export interface ExchangeCompareProps {
  /** The two exchanges to compare (request line, headers, bodies, status, timing). */
  a: ExchangeId;
  b: ExchangeId;
  className?: string;
}

type Mode = 'split' | 'unified';
const MODE_PREF = 'exchange.compare.mode';

// =====================================================================================
// Data
// =====================================================================================

interface Loaded {
  detail: ExchangeDetail;
  requestBody: BodyView | null;
  responseBody: BodyView | null;
  /** A body couldn't be fetched (compared as empty). */
  bodyErrors: Side[];
}

type LoadState = { status: 'loading' } | { status: 'error'; message: string; notFound: boolean } | { status: 'ready'; data: Loaded };

function hasBody(detail: ExchangeDetail, side: Side): boolean {
  const meta = side === 'request' ? detail.requestBody : detail.responseBody;
  return meta.capturedSize > 0 || meta.wireSize > 0;
}

async function loadExchange(id: ExchangeId): Promise<Loaded> {
  const detail = await api.exchanges.get(id);
  const bodyErrors: Side[] = [];
  const get = async (side: Side) => {
    if (!hasBody(detail, side)) return null;
    try {
      // Keyed by the body's state, so a view cached mid-flight isn't reused once it finished.
      return await fetchBodyView(id, side, false, bodyRefreshKey(detail.summary, side));
    } catch {
      bodyErrors.push(side);
      return null;
    }
  };
  const [requestBody, responseBody] = await Promise.all([get('request'), get('response')]);
  return { detail, requestBody, responseBody, bodyErrors };
}

/** Load one exchange (detail + both bodies); reloads when its state changes (e.g. completes). */
function useExchange(id: ExchangeId): LoadState & { retry: () => void } {
  const summary = useExchangeSummary(id);
  const version = `${summary?.state ?? ''}:${summary?.status ?? ''}:${summary?.responseSize ?? ''}`;
  const [state, setState] = useState<{ id: ExchangeId; value: LoadState }>({ id, value: { status: 'loading' } });
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let cancelled = false;
    setState((s) => (s.id === id && s.value.status === 'ready' ? s : { id, value: { status: 'loading' } }));
    loadExchange(id).then(
      (data) => !cancelled && setState({ id, value: { status: 'ready', data } }),
      (e: unknown) => !cancelled && setState({ id, value: { status: 'error', message: errorMessage(e), notFound: e instanceof ApiError && e.status === 404 } }),
    );
    return () => {
      cancelled = true;
    };
  }, [id, version, nonce]);
  const retry = useCallback(() => setNonce((n) => n + 1), []);
  return { ...(state.id === id ? state.value : { status: 'loading' }), retry };
}

// =====================================================================================
// Sections
// =====================================================================================

interface TextSection {
  id: Exclude<CompareSectionId, 'timing'>;
  left: string;
  right: string;
  language?: string;
  note?: string;
  emptyText: string;
  maxHeight: number;
}

function buildSections(a: Loaded, b: Loaded): TextSection[] {
  const ra = bodyText(a.requestBody);
  const rb = bodyText(b.requestBody);
  const sa = bodyText(a.responseBody);
  const sb = bodyText(b.responseBody);
  const binaryNote = (x: { binary: boolean }, y: { binary: boolean }) => (x.binary || y.binary ? 'Binary bodies are compared by type and size only.' : undefined);
  const lang = (x: { language: string; binary: boolean }, y: { language: string; binary: boolean }) => (!x.binary && !y.binary && x.language === y.language ? x.language : undefined);
  const failed = (side: Side) => (a.bodyErrors.includes(side) || b.bodyErrors.includes(side) ? "One of the bodies couldn't be loaded and is compared as empty." : undefined);
  return [
    { id: 'request-line', left: requestLineText(a.detail), right: requestLineText(b.detail), language: 'http', emptyText: 'No request line', maxHeight: 360 },
    { id: 'status', left: statusLineText(a.detail), right: statusLineText(b.detail), emptyText: 'No response', maxHeight: 200 },
    { id: 'request-headers', left: headersText(a.detail.request.headers), right: headersText(b.detail.request.headers), language: 'http', emptyText: 'No request headers on either side', maxHeight: 560 },
    { id: 'response-headers', left: headersText(a.detail.response?.headers), right: headersText(b.detail.response?.headers), language: 'http', emptyText: 'No response headers on either side', maxHeight: 560 },
    { id: 'request-body', left: ra.text, right: rb.text, language: lang(ra, rb), note: failed('request') ?? binaryNote(ra, rb), emptyText: 'No request body on either side', maxHeight: 640 },
    { id: 'response-body', left: sa.text, right: sb.text, language: lang(sa, sb), note: failed('response') ?? binaryNote(sa, sb), emptyText: 'No response body on either side', maxHeight: 640 },
  ];
}

function useSectionDiffs(sections: readonly TextSection[] | null, ignoreWs: boolean): ({ result: TextDiffResult | null; pending: boolean } | null)[] {
  // A fixed number of hook calls: one per text section.
  const get = (i: number) => sections?.[i];
  const d0 = useTextDiff(get(0)?.left ?? '', get(0)?.right ?? '', ignoreWs, !!sections);
  const d1 = useTextDiff(get(1)?.left ?? '', get(1)?.right ?? '', ignoreWs, !!sections);
  const d2 = useTextDiff(get(2)?.left ?? '', get(2)?.right ?? '', ignoreWs, !!sections);
  const d3 = useTextDiff(get(3)?.left ?? '', get(3)?.right ?? '', ignoreWs, !!sections);
  const d4 = useTextDiff(get(4)?.left ?? '', get(4)?.right ?? '', ignoreWs, !!sections);
  const d5 = useTextDiff(get(5)?.left ?? '', get(5)?.right ?? '', ignoreWs, !!sections);
  return sections ? [d0, d1, d2, d3, d4, d5] : [];
}

function timingDiffers(rows: readonly TimingRow[]): boolean {
  return rows.some((r) => (r.a == null) !== (r.b == null) || (r.delta != null && Math.abs(r.delta) >= 0.5));
}

// =====================================================================================
// Pieces
// =====================================================================================

function ExchangeCard({ label, state, onRetry }: { label: 'A' | 'B'; state: LoadState; onRetry: () => void }) {
  if (state.status === 'loading') {
    return (
      <div className={styles.card} aria-busy>
        <span className={clsx(styles.badge, label === 'A' ? styles.badgeA : styles.badgeB)}>{label}</span>
        <div className={styles.cardMain}>
          <Skeleton height={12} width="70%" />
          <Skeleton height={10} width="40%" />
        </div>
      </div>
    );
  }
  if (state.status === 'error') {
    return (
      <div className={clsx(styles.card, styles.cardError)}>
        <span className={clsx(styles.badge, label === 'A' ? styles.badgeA : styles.badgeB)}>{label}</span>
        <div className={styles.cardMain}>
          <span className={styles.cardErrorTitle}>
            <AlertTriangle size={13} strokeWidth={2} aria-hidden />
            {state.notFound ? 'This exchange no longer exists' : "Couldn't load this exchange"}
          </span>
          <span className={styles.cardMeta}>{state.notFound ? 'It was deleted or evicted from memory.' : state.message}</span>
        </div>
        {!state.notFound && (
          <Button size="sm" variant="ghost" onClick={onRetry}>
            Retry
          </Button>
        )}
      </div>
    );
  }
  const { detail } = state.data;
  const { summary, request, response } = detail;
  const url = request.url.replace(/^[a-z]+:\/\//i, '');
  return (
    <div className={styles.card}>
      <span className={clsx(styles.badge, label === 'A' ? styles.badgeA : styles.badgeB)}>{label}</span>
      <div className={styles.cardMain}>
        <div className={styles.cardLine}>
          <MethodBadge method={request.method} size="sm" />
          <StatusBadge summary={summary} statusMessage={response?.statusMessage} />
          <span className={clsx(styles.cardUrl, 'mono')} title={request.url}>
            {url}
          </span>
        </div>
        <div className={clsx(styles.cardMeta, 'tnum')}>
          <span>#{summary.id}</span>
          <span>{formatTime(summary.startedAt)}</span>
          {summary.duration != null && <span>{formatDuration(summary.duration)}</span>}
          <span>{formatBytes(summary.responseSize)}</span>
          {summary.source && <span>{summary.source}</span>}
        </div>
      </div>
    </div>
  );
}

function TimingCompare({ rows }: { rows: readonly TimingRow[] }) {
  if (!rows.length) return <div className={styles.sectionEmpty}>No timing information on either side</div>;
  const max = Math.max(1, ...rows.flatMap((r) => [r.a ?? 0, r.b ?? 0]));
  return (
    <div className={styles.timing} role="table" aria-label="Timing comparison">
      <div className={clsx(styles.timingRow, styles.timingHead)} role="row">
        <span role="columnheader">Phase</span>
        <span role="columnheader" className={styles.num}>
          A
        </span>
        <span role="columnheader" className={styles.num}>
          B
        </span>
        <span role="columnheader" aria-hidden />
        <span role="columnheader" className={styles.num}>
          Difference
        </span>
      </div>
      {rows.map((r) => {
        const slower = r.delta != null && r.delta >= 0.5;
        const faster = r.delta != null && r.delta <= -0.5;
        return (
          <div key={r.id} className={clsx(styles.timingRow, r.id === 'total' && styles.timingTotal)} role="row">
            <span role="cell" className={styles.phase}>
              {r.label}
            </span>
            <span role="cell" className={clsx(styles.num, 'tnum')}>
              {r.a != null ? formatDuration(r.a) : '—'}
            </span>
            <span role="cell" className={clsx(styles.num, 'tnum')}>
              {r.b != null ? formatDuration(r.b) : '—'}
            </span>
            <span role="cell" className={styles.bars} aria-hidden>
              <span className={styles.barTrack}>
                <span className={clsx(styles.bar, styles.barA)} style={{ width: `${((r.a ?? 0) / max) * 100}%` }} />
              </span>
              <span className={styles.barTrack}>
                <span className={clsx(styles.bar, styles.barB)} style={{ width: `${((r.b ?? 0) / max) * 100}%` }} />
              </span>
            </span>
            <span role="cell" className={clsx(styles.num, styles.delta, 'tnum', slower && styles.slower, faster && styles.faster)}>
              {formatDelta(r.a, r.b)}
            </span>
          </div>
        );
      })}
    </div>
  );
}

function SectionHeader({ id, open, onToggle, children }: { id: CompareSectionId; open: boolean; onToggle: () => void; children: ReactNode }) {
  return (
    <button type="button" className={styles.sectionHeader} aria-expanded={open} aria-controls={`cmp-${id}`} onClick={onToggle}>
      <ChevronRight size={14} strokeWidth={2} className={clsx(styles.chevron, open && styles.chevronOpen)} aria-hidden />
      <span className={styles.sectionTitle}>{COMPARE_SECTION_LABELS[id]}</span>
      {children}
    </button>
  );
}

/** `+3 −1`, omitting zero parts. */
function Stats({ added, removed }: { added: number; removed: number }) {
  return (
    <>
      {added > 0 && <span className={styles.add}>+{formatNumber(added)}</span>}
      {removed > 0 && <span className={styles.del}>−{formatNumber(removed)}</span>}
    </>
  );
}

function DiffStat({ result, pending }: { result: TextDiffResult | null; pending: boolean }) {
  if (pending || !result) return <Spinner size={12} tone="muted" />;
  if (result.identical) {
    return (
      <span className={styles.same}>
        <CheckCircle2 size={12} strokeWidth={2} aria-hidden />
        Identical
      </span>
    );
  }
  return (
    <span className={clsx(styles.stat, 'tnum')}>
      <Stats added={result.stats.added} removed={result.stats.removed} />
    </span>
  );
}

// =====================================================================================
// ExchangeCompare
// =====================================================================================

/**
 * Side-by-side comparison of two exchanges (DESIGN.md §6.5): a summary of what differs, then
 * collapsible sections — request line, status, request/response headers (normalised
 * `name: value` lines), request/response bodies (pretty-printed) and a timing table — each
 * diffed line by line. Identical sections start collapsed. A/B can be swapped.
 */
export function ExchangeCompare({ a, b, className }: ExchangeCompareProps) {
  const [swapped, setSwapped] = useState(false);
  // Load by identity so swapping is instant (no refetch).
  const ea = useExchange(a);
  const eb = useExchange(b);
  const left = swapped ? eb : ea;
  const right = swapped ? ea : eb;
  const [mode, setMode] = usePagePref<Mode>(MODE_PREF, 'split');
  const [ignoreWs, setIgnoreWs] = useState(false);

  const ready = left.status === 'ready' && right.status === 'ready';
  const ld = left.status === 'ready' ? left.data : null;
  const rd = right.status === 'ready' ? right.data : null;
  const sections = useMemo(() => (ld && rd ? buildSections(ld, rd) : null), [ld, rd]);
  const diffs = useSectionDiffs(sections, ignoreWs);
  const timing = useMemo(() => (ld && rd ? compareTiming(ld.detail, rd.detail) : []), [ld, rd]);

  const differing = new Set<CompareSectionId>();
  sections?.forEach((s, i) => {
    const r = diffs[i]?.result;
    if (r && !r.identical) differing.add(s.id);
  });
  if (ready && timingDiffers(timing)) differing.add('timing');
  const allIds: CompareSectionId[] = sections ? [...sections.map((s) => s.id), 'timing'] : [];
  const diffsPending = diffs.some((d) => d?.pending || (d && !d.result));

  // Open state: differing sections open by default; user toggles override.
  const [toggled, setToggled] = useState<Partial<Record<CompareSectionId, boolean>>>({});
  const isOpen = (id: CompareSectionId) => toggled[id] ?? differing.has(id);
  const toggle = (id: CompareSectionId) => setToggled((t) => ({ ...t, [id]: !isOpen(id) }));
  const allOpen = allIds.length > 0 && allIds.every(isOpen);
  const setAll = (open: boolean) => setToggled(Object.fromEntries(allIds.map((id) => [id, open])));
  useEffect(() => setToggled({}), [a, b]);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const reveal = (id: CompareSectionId) => {
    setToggled((t) => ({ ...t, [id]: true }));
    requestAnimationFrame(() => {
      const el = scrollRef.current?.querySelector<HTMLElement>(`[data-section="${id}"]`);
      el?.scrollIntoView({ block: 'start', behavior: smoothScroll() });
      el?.querySelector<HTMLElement>('button')?.focus({ preventScroll: true });
    });
  };

  const swap = () => setSwapped((s) => !s);
  const toggleMode = () => setMode(mode === 'split' ? 'unified' : 'split');
  useShortcut('alt+s', swap, { allowInOverlays: true, description: 'Swap A and B', group: 'Compare' });
  useShortcut('alt+l', toggleMode, { allowInOverlays: true, description: 'Toggle split / unified diff', group: 'Compare' });
  useShortcut('alt+w', () => setIgnoreWs((v) => !v), { allowInOverlays: true, description: 'Ignore whitespace', group: 'Compare' });
  useCommands(
    () => [
      { id: 'compare.swap', title: 'Swap compared exchanges', group: 'Traffic', icon: ArrowLeftRight, shortcut: 'alt+s', run: swap },
      { id: 'compare.layout', title: mode === 'split' ? 'Show unified diff' : 'Show side-by-side diff', group: 'Traffic', icon: mode === 'split' ? Rows2 : Columns2, shortcut: 'alt+l', run: toggleMode },
      { id: 'compare.whitespace', title: ignoreWs ? 'Compare whitespace' : 'Ignore whitespace in diffs', group: 'Traffic', icon: Space, shortcut: 'alt+w', run: () => setIgnoreWs((v) => !v) },
    ],
    [mode, ignoreWs],
  );

  // ---------------------------------------------------------------- summary
  const leftId = swapped ? b : a;
  const rightId = swapped ? a : b;
  const sameExchange = a === b;
  const failed = left.status === 'error' || right.status === 'error';
  let summary: ReactNode;
  if (failed) summary = null;
  else if (!ready) summary = <Skeleton height={12} width={260} />;
  else if (diffsPending) {
    summary = (
      <span className={styles.summaryText}>
        <Spinner size={12} tone="muted" /> Comparing…
      </span>
    );
  } else if (differing.size === 0) {
    summary = (
      <span className={clsx(styles.summaryText, styles.summarySame)}>
        <CheckCircle2 size={14} strokeWidth={2} aria-hidden />
        {sameExchange ? 'Comparing an exchange with itself — everything is identical.' : 'These exchanges are identical in every compared section.'}
      </span>
    );
  } else {
    summary = (
      <>
        <span className={styles.summaryText}>
          <strong className="tnum">
            {differing.size} of {allIds.length}
          </strong>{' '}
          sections differ
        </span>
        <span className={styles.chips}>
          {allIds
            .filter((id) => differing.has(id))
            .map((id) => {
              const i = sections?.findIndex((s) => s.id === id) ?? -1;
              const r = i >= 0 ? diffs[i]?.result : null;
              const total = id === 'timing' ? timing.find((t) => t.id === 'total') : null;
              return (
                <button key={id} type="button" className={styles.chip} onClick={() => reveal(id)}>
                  {COMPARE_SECTION_LABELS[id]}
                  {r && (
                    <span className={clsx(styles.chipStats, 'tnum')}>
                      <Stats added={r.stats.added} removed={r.stats.removed} />
                    </span>
                  )}
                  {total && total.delta != null && <span className={clsx('tnum', total.delta > 0 ? styles.slowerText : styles.fasterText)}>{formatDelta(total.a, total.b)}</span>}
                </button>
              );
            })}
        </span>
      </>
    );
  }

  // ---------------------------------------------------------------- body
  let content: ReactNode;
  if (left.status === 'error' && right.status === 'error') {
    content = (
      <div className={styles.state}>
        <EmptyState
          icon={left.notFound && right.notFound ? FileX2 : AlertTriangle}
          tone={left.notFound && right.notFound ? 'neutral' : 'danger'}
          size="sm"
          title="Couldn't load the exchanges"
          description={left.notFound && right.notFound ? 'Both exchanges were deleted or evicted from memory.' : left.message}
          actions={
            <Button
              size="sm"
              onClick={() => {
                left.retry();
                right.retry();
              }}
            >
              Retry
            </Button>
          }
        />
      </div>
    );
  } else if (left.status === 'error' || right.status === 'error') {
    content = (
      <div className={styles.state}>
        <EmptyState icon={FileX2} size="sm" title="Nothing to compare" description={`Exchange #${left.status === 'error' ? leftId : rightId} couldn't be loaded.`} />
      </div>
    );
  } else if (!sections) {
    content = (
      <div className={styles.loadingSections} role="status" aria-busy aria-label="Loading exchanges">
        {[0, 1, 2, 3].map((i) => (
          <div key={i} className={styles.loadingSection}>
            <Skeleton height={12} width={140} />
            <Skeleton height={10} width={`${70 - i * 10}%`} />
            <Skeleton height={10} width={`${55 + i * 8}%`} />
          </div>
        ))}
      </div>
    );
  } else {
    content = (
      <>
        {sections.map((s, i) => {
          const d = diffs[i];
          const open = isOpen(s.id);
          return (
            <section key={s.id} className={styles.section} data-section={s.id} aria-label={COMPARE_SECTION_LABELS[s.id]}>
              <SectionHeader id={s.id} open={open} onToggle={() => toggle(s.id)}>
                <DiffStat result={d?.result ?? null} pending={d?.pending ?? true} />
              </SectionHeader>
              {open && (
                <div id={`cmp-${s.id}`} className={styles.sectionBody}>
                  {s.note && <div className={styles.note}>{s.note}</div>}
                  <TextDiff
                    left={s.left}
                    right={s.right}
                    language={s.language}
                    mode={mode}
                    onModeChange={setMode}
                    toolbar={false}
                    fill={false}
                    maxHeight={s.maxHeight}
                    result={d?.result ?? null}
                    emptyText={s.emptyText}
                  />
                </div>
              )}
            </section>
          );
        })}
        <section className={styles.section} data-section="timing" aria-label="Timing">
          <SectionHeader id="timing" open={isOpen('timing')} onToggle={() => toggle('timing')}>
            {differing.has('timing') ? (
              (() => {
                const total = timing.find((t) => t.id === 'total');
                return total?.delta != null ? <span className={clsx(styles.stat, 'tnum', total.delta > 0 ? styles.slowerText : styles.fasterText)}>{formatDelta(total.a, total.b)}</span> : null;
              })()
            ) : (
              <span className={styles.same}>
                <CheckCircle2 size={12} strokeWidth={2} aria-hidden />
                Identical
              </span>
            )}
          </SectionHeader>
          {isOpen('timing') && (
            <div id="cmp-timing" className={styles.sectionBody}>
              <TimingCompare rows={timing} />
            </div>
          )}
        </section>
      </>
    );
  }

  return (
    <div className={clsx(styles.root, className)}>
      <header className={styles.header}>
        <div className={styles.cards}>
          <ExchangeCard label="A" state={left} onRetry={left.retry} />
          <Tooltip content="Swap A and B" shortcut="alt+s">
            <button type="button" className={styles.swap} onClick={swap} aria-label="Swap A and B">
              <ArrowLeftRight size={14} strokeWidth={2} aria-hidden />
            </button>
          </Tooltip>
          <ExchangeCard label="B" state={right} onRetry={right.retry} />
        </div>
        <div className={styles.summary}>
          <div className={styles.summaryMain} aria-live="polite">
            {summary}
          </div>
          <div className={styles.controls}>
            <SegmentedControl
              size="sm"
              aria-label="Diff layout"
              value={mode}
              onValueChange={setMode}
              items={[
                { value: 'split', label: 'Split', icon: Columns2 },
                { value: 'unified', label: 'Unified', icon: Rows2 },
              ]}
            />
            <IconButton label={ignoreWs ? 'Compare whitespace' : 'Ignore whitespace'} icon={Space} size="sm" shortcut="alt+w" active={ignoreWs} onClick={() => setIgnoreWs(!ignoreWs)} />
            <IconButton label={allOpen ? 'Collapse all sections' : 'Expand all sections'} icon={allOpen ? FoldVertical : UnfoldVertical} size="sm" disabled={!sections} onClick={() => setAll(!allOpen)} />
          </div>
        </div>
      </header>
      <div ref={scrollRef} className={styles.scroller}>
        {content}
      </div>
    </div>
  );
}
