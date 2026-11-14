// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus (diff lines)
import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { ArrowRight, CheckCircle2, ChevronDown, ChevronUp, Columns2, FoldVertical, Rows2, Space, UnfoldVertical } from 'lucide-react';
import { memo, useCallback, useEffect, useMemo, useRef, useState, type CSSProperties } from 'react';
import { useShortcut } from '../../app/shortcuts';
import { changeStarts, collapseUnchanged, type DiffLine, type FoldItem, type SplitCell, type SplitRow, type TextDiffResult } from '../../lib/diff';
import { formatNumber } from '../../lib/format';
import { EmptyState, IconButton, SegmentedControl, Spinner } from '../ui';
import { linePieces, type LineToken } from './diff/pieces';
import { useHighlight, useTextDiff } from './diff/useTextDiff';
import styles from './TextDiff.module.css';

export interface TextDiffProps {
  left: string;
  right: string;
  leftTitle?: string;
  rightTitle?: string;
  /** CodeMirror language name for highlighting. */
  language?: string;
  /** Side-by-side (default) or unified. Initial mode unless `onModeChange` makes it controlled. */
  mode?: 'split' | 'unified';
  className?: string;
  /** Optional: makes `mode` controlled (the toolbar toggle calls this). */
  onModeChange?: (mode: 'split' | 'unified') => void;
  /** Optional: show the toolbar (layout, stats, change navigation, whitespace, expand). Default true. */
  toolbar?: boolean;
  /** Optional: fill the parent's height (default true); `false` sizes to content up to `maxHeight`. */
  fill?: boolean;
  /** Optional: max height (px) when `fill` is false. Default 520. */
  maxHeight?: number;
  /** Optional: unchanged lines kept around each change (default 3). */
  context?: number;
  /** Optional: initial "ignore whitespace" state. */
  ignoreWhitespace?: boolean;
  /** Optional: a result computed by the caller (e.g. ExchangeCompare) to avoid diffing twice. */
  result?: TextDiffResult | null;
  /** Optional: text when both sides are empty. */
  emptyText?: string;
}

type Mode = 'split' | 'unified';
type Item = FoldItem<SplitRow> | FoldItem<DiffLine>;

const ROW_H = 20;
const FOLD_H = 28;
const EMPTY_SET: ReadonlySet<string> = new Set();

const isRowChanged = (r: SplitRow) => r.left?.kind !== 'equal' || r.right?.kind !== 'equal';
const isLineChanged = (l: DiffLine) => l.kind !== 'equal';

// =====================================================================================
// Rows
// =====================================================================================

function Code({ text, tokens, segments }: { text: string; tokens: readonly LineToken[] | null | undefined; segments: SplitCell['segments'] }) {
  const pieces = useMemo(() => linePieces(text, tokens, segments), [text, tokens, segments]);
  if (text === '') return <span className={styles.code}>{'​'}</span>;
  return (
    <span className={styles.code}>
      {pieces.map((p, i) => (
        <span key={i} className={clsx(p.cls && styles[`syn_${p.cls}`], p.changed && styles.chg)}>
          {p.text}
        </span>
      ))}
    </span>
  );
}

const SIGN = { del: '−', add: '+', equal: '' } as const;
const SIGN_LABEL = { del: 'removed', add: 'added', equal: undefined } as const;

const SplitHalf = memo(function SplitHalf({ cell, tokens }: { cell: SplitCell | null; tokens: LineToken[][] | null }) {
  if (!cell) return <div className={clsx(styles.half, styles.filler)} role="cell" aria-hidden />;
  return (
    <div className={clsx(styles.half, styles[cell.kind])} role="cell">
      <span className={clsx(styles.no, 'tnum')}>{cell.no}</span>
      <span className={styles.sign} role="img" aria-label={SIGN_LABEL[cell.kind]}>
        {SIGN[cell.kind]}
      </span>
      <Code text={cell.text} tokens={tokens?.[cell.no - 1]} segments={cell.segments} />
    </div>
  );
});

const SplitRowView = memo(function SplitRowView({ row, tokensA, tokensB }: { row: SplitRow; tokensA: LineToken[][] | null; tokensB: LineToken[][] | null }) {
  return (
    <div className={styles.splitRow} role="row">
      <SplitHalf cell={row.left} tokens={tokensA} />
      <SplitHalf cell={row.right} tokens={tokensB} />
    </div>
  );
});

const UnifiedRowView = memo(function UnifiedRowView({ line, tokensA, tokensB }: { line: DiffLine; tokensA: LineToken[][] | null; tokensB: LineToken[][] | null }) {
  const tokens = line.kind === 'del' ? tokensA?.[(line.aNo ?? 0) - 1] : tokensB?.[(line.bNo ?? 0) - 1];
  return (
    <div className={clsx(styles.unifiedRow, styles[line.kind])} role="row">
      <span className={clsx(styles.no, 'tnum')} role="cell">
        {line.aNo ?? ''}
      </span>
      <span className={clsx(styles.no, 'tnum')} role="cell">
        {line.bNo ?? ''}
      </span>
      <span className={styles.sign} role="img" aria-label={SIGN_LABEL[line.kind]}>
        {SIGN[line.kind]}
      </span>
      <Code text={line.text} tokens={tokens} segments={line.segments} />
    </div>
  );
});

function FoldRow({ count, onExpand }: { count: number; onExpand: () => void }) {
  return (
    <button type="button" className={styles.fold} onClick={onExpand}>
      <UnfoldVertical size={13} strokeWidth={2} aria-hidden />
      Show {formatNumber(count)} unchanged line{count === 1 ? '' : 's'}
    </button>
  );
}

/** Highest line number on either side (the last row holding a cell on each side). */
function maxLineNo(result: TextDiffResult | null): number {
  if (!result) return 0;
  let a = 0;
  let b = 0;
  for (let i = result.rows.length - 1; i >= 0 && (!a || !b); i--) {
    const r = result.rows[i] as SplitRow;
    if (!a && r.left) a = r.left.no;
    if (!b && r.right) b = r.right.no;
  }
  return Math.max(a, b);
}

function findLastIndex<T>(xs: readonly T[], pred: (x: T) => boolean): number {
  for (let i = xs.length - 1; i >= 0; i--) if (pred(xs[i] as T)) return i;
  return -1;
}

// =====================================================================================
// TextDiff
// =====================================================================================

/**
 * Line diff viewer (DESIGN.md §6.5): split (side-by-side in one scroller, so both sides always
 * scroll together) or unified, intraline word highlights, syntax colouring, collapsed unchanged
 * regions ("Show N unchanged lines"), change navigation (Alt+↑/↓ while focused), ignore
 * whitespace, virtualised rows and off-main-thread diffing for large inputs.
 */
export function TextDiff(props: TextDiffProps) {
  const { left, right, leftTitle, rightTitle, language, className, onModeChange, toolbar = true, fill = true, maxHeight = 520, context = 3, emptyText, result: given } = props;
  const [innerMode, setInnerMode] = useState<Mode>(props.mode ?? 'split');
  useEffect(() => {
    if (props.mode) setInnerMode(props.mode);
  }, [props.mode]);
  const mode: Mode = onModeChange ? (props.mode ?? 'split') : innerMode;
  const setMode = (m: Mode) => (onModeChange ? onModeChange(m) : setInnerMode(m));

  const [ignoreWs, setIgnoreWs] = useState(props.ignoreWhitespace ?? false);
  const computed = useTextDiff(left, right, ignoreWs, given === undefined);
  const result = given !== undefined ? given : computed.result;
  const pending = given === undefined && computed.pending;

  const highlight = !!language && result != null;
  const tokensA = useHighlight(left, language, highlight);
  const tokensB = useHighlight(right, language, highlight);

  // Opened folds belong to one result; a new result starts collapsed again.
  const [expanded, setExpanded] = useState<{ key: TextDiffResult | null; ids: ReadonlySet<string> }>({ key: null, ids: EMPTY_SET });
  const [allExpanded, setAllExpanded] = useState(false);
  const expandedIds = expanded.key === result ? expanded.ids : EMPTY_SET;

  const items: Item[] = useMemo(() => {
    if (!result) return [];
    if (allExpanded) {
      return mode === 'split' ? result.rows.map((row, index) => ({ type: 'row' as const, index, row })) : result.lines.map((row, index) => ({ type: 'row' as const, index, row }));
    }
    const opts = { context, expanded: expandedIds };
    return mode === 'split' ? collapseUnchanged(result.rows, isRowChanged, opts) : collapseUnchanged(result.lines, isLineChanged, opts);
  }, [result, mode, context, expandedIds, allExpanded]);

  const starts = useMemo(
    () => (mode === 'split' ? changeStarts(items as FoldItem<SplitRow>[], isRowChanged) : changeStarts(items as FoldItem<DiffLine>[], isLineChanged)),
    [items, mode],
  );
  const hasFolds = useMemo(() => items.some((i) => i.type === 'fold'), [items]);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const virtualizer = useVirtualizer({
    count: items.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (items[i]?.type === 'fold' ? FOLD_H : ROW_H),
    overscan: 24,
    getItemKey: (i) => {
      const it = items[i];
      return it ? (it.type === 'fold' ? `f${it.id}` : `r${it.index}`) : i;
    },
  });

  // ---------------------------------------------------------------- change navigation
  const [current, setCurrent] = useState(-1);
  useEffect(() => setCurrent(-1), [result, mode]);
  const goTo = useCallback(
    (dir: 1 | -1) => {
      if (!starts.length) return;
      const top = scrollRef.current?.scrollTop ?? 0;
      const firstVisible = virtualizer.getVirtualItems().find((v) => v.end > top + 1)?.index ?? 0;
      let next: number;
      // Continue from the last jump unless the user has scrolled away from it since.
      const lastStart = current >= 0 ? starts[current] : undefined;
      const lastOffset = lastStart != null ? (virtualizer.getOffsetForIndex(Math.max(0, lastStart - 1), 'start')?.[0] ?? -1) : -1;
      if (lastStart != null && Math.abs(lastOffset - top) < ROW_H * 2) next = current + dir;
      else if (current < 0 && dir === 1) {
        // First jump: the first change at or below the top of the viewport.
        next = starts.findIndex((s) => s >= firstVisible);
        if (next < 0) next = 0;
      } else {
        next = dir === 1 ? starts.findIndex((s) => s > firstVisible + 1) : findLastIndex(starts, (s) => s < firstVisible + 1);
        if (next < 0) next = dir === 1 ? 0 : starts.length - 1;
      }
      next = (next + starts.length) % starts.length;
      setCurrent(next);
      // One line of context above the change.
      virtualizer.scrollToIndex(Math.max(0, (starts[next] as number) - 1), { align: 'start' });
    },
    [starts, current, virtualizer],
  );

  const [active, setActive] = useState(false);
  useShortcut('alt+arrowdown', () => goTo(1), { enabled: active && starts.length > 0, allowInOverlays: true, description: 'Next change', group: 'Diff' });
  useShortcut('alt+arrowup', () => goTo(-1), { enabled: active && starts.length > 0, allowInOverlays: true, description: 'Previous change', group: 'Diff' });

  const expandFold = (id: string) => setExpanded({ key: result, ids: new Set([...expandedIds, id]) });

  // ---------------------------------------------------------------- toolbar & titles
  const bothEmpty = left === '' && right === '';
  const stats = result?.stats;
  // Gutter sized to the longest line number (5-digit files don't overflow).
  const digits = useMemo(() => String(Math.max(9, maxLineNo(result))).length, [result]);

  const toolbarNode = toolbar ? (
    <div className={styles.toolbar} role="toolbar" aria-label="Diff options">
      <SegmentedControl
        size="sm"
        aria-label="Diff layout"
        value={mode}
        onValueChange={setMode}
        items={[
          { value: 'split', label: 'Split', icon: Columns2 },
          { value: 'unified', label: 'Unified', icon: Rows2 },
        ]}
        className={styles.modes}
      />
      {stats && !result?.identical && (
        <span className={clsx(styles.stats, 'tnum')}>
          {stats.added > 0 && <span className={styles.statAdd}>+{formatNumber(stats.added)}</span>}
          {stats.removed > 0 && <span className={styles.statDel}>−{formatNumber(stats.removed)}</span>}
          <span className={styles.statBlocks}>
            · {formatNumber(stats.blocks)} change{stats.blocks === 1 ? '' : 's'}
          </span>
        </span>
      )}
      {result?.identical && !bothEmpty && (
        <span className={styles.identical}>
          <CheckCircle2 size={13} strokeWidth={2} aria-hidden />
          Identical{ignoreWs ? ' ignoring whitespace' : ''}
        </span>
      )}
      {pending && (
        <span className={styles.pending}>
          <Spinner size={12} tone="muted" /> Comparing…
        </span>
      )}
      <span className={styles.spacer} />
      {starts.length > 0 && (
        <span className={clsx(styles.counter, 'tnum')} aria-live="polite">
          {current >= 0 ? `${current + 1} of ${starts.length}` : `${starts.length} change${starts.length === 1 ? '' : 's'}`}
        </span>
      )}
      <IconButton label="Previous change" icon={ChevronUp} size="sm" shortcut="alt+arrowup" disabled={!starts.length} onClick={() => goTo(-1)} />
      <IconButton label="Next change" icon={ChevronDown} size="sm" shortcut="alt+arrowdown" disabled={!starts.length} onClick={() => goTo(1)} />
      <span className={styles.sep} aria-hidden />
      <IconButton
        label={ignoreWs ? 'Compare whitespace' : 'Ignore whitespace'}
        icon={Space}
        size="sm"
        active={ignoreWs}
        disabled={given !== undefined}
        onClick={() => setIgnoreWs(!ignoreWs)}
      />
      <IconButton
        label={allExpanded ? 'Collapse unchanged lines' : 'Expand unchanged lines'}
        icon={allExpanded ? FoldVertical : UnfoldVertical}
        size="sm"
        active={allExpanded}
        disabled={!allExpanded && !hasFolds}
        onClick={() => setAllExpanded(!allExpanded)}
      />
    </div>
  ) : null;

  const titles =
    leftTitle || rightTitle ? (
      mode === 'split' ? (
        <div className={styles.titles}>
          <div className={styles.title}>
            <span className={clsx(styles.badge, styles.badgeDel)}>A</span>
            <span className={clsx(styles.titleText, 'truncate')} title={leftTitle}>
              {leftTitle}
            </span>
          </div>
          <div className={styles.title}>
            <span className={clsx(styles.badge, styles.badgeAdd)}>B</span>
            <span className={clsx(styles.titleText, 'truncate')} title={rightTitle}>
              {rightTitle}
            </span>
          </div>
        </div>
      ) : (
        <div className={clsx(styles.titles, styles.titlesUnified)}>
          <div className={styles.title}>
            <span className={clsx(styles.badge, styles.badgeDel)}>A</span>
            <span className={clsx(styles.titleText, 'truncate')} title={leftTitle}>
              {leftTitle}
            </span>
          </div>
          <ArrowRight size={12} strokeWidth={2} className={styles.arrow} aria-hidden />
          <div className={styles.title}>
            <span className={clsx(styles.badge, styles.badgeAdd)}>B</span>
            <span className={clsx(styles.titleText, 'truncate')} title={rightTitle}>
              {rightTitle}
            </span>
          </div>
        </div>
      )
    ) : null;

  // ---------------------------------------------------------------- body
  let body;
  if (bothEmpty) {
    body = (
      <div className={styles.state}>
        <EmptyState size="sm" title={emptyText ?? 'Both sides are empty'} />
      </div>
    );
  } else if (!result) {
    body = (
      <div className={clsx(styles.state, styles.comparing)}>
        <Spinner size={16} tone="muted" /> Comparing {formatNumber(left.length + right.length)} characters…
      </div>
    );
  } else {
    const virtualItems = virtualizer.getVirtualItems();
    const total = virtualizer.getTotalSize();
    body = (
      <div
        ref={scrollRef}
        className={clsx(styles.scroller, mode === 'split' ? styles.split : styles.unified, pending && styles.stale)}
        style={fill ? undefined : { height: Math.min(total + 1, maxHeight) }}
        // biome-ignore lint/a11y/noNoninteractiveTabindex: scrollable region: focusable so it can be scrolled with the keyboard
        tabIndex={0}
        role="table"
        aria-label={`Differences between ${leftTitle ?? 'A'} and ${rightTitle ?? 'B'}`}
        aria-rowcount={items.length}
      >
        <div className={styles.inner} style={{ height: total }}>
          {virtualItems.map((v) => {
            const it = items[v.index];
            if (!it) return null;
            return (
              <div key={v.key} data-index={v.index} ref={virtualizer.measureElement} className={styles.vrow} style={{ transform: `translateY(${v.start}px)` }}>
                {it.type === 'fold' ? (
                  <FoldRow count={it.count} onExpand={() => expandFold(it.id)} />
                ) : mode === 'split' ? (
                  <SplitRowView row={it.row as SplitRow} tokensA={tokensA} tokensB={tokensB} />
                ) : (
                  <UnifiedRowView line={it.row as DiffLine} tokensA={tokensA} tokensB={tokensB} />
                )}
              </div>
            );
          })}
        </div>
      </div>
    );
  }

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: tracks focus within the viewer to scope its shortcuts; not a control
    <div
      className={clsx(styles.root, fill && styles.fill, className)}
      onFocus={() => setActive(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setActive(false);
      }}
      data-mode={mode}
      style={{ '--diff-digits': digits } as CSSProperties}
    >
      {toolbarNode}
      {titles}
      {body}
    </div>
  );
}
