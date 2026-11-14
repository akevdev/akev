import { DndContext, DragOverlay, PointerSensor, useSensor, useSensors, type DragEndEvent, type DragStartEvent } from '@dnd-kit/core';
import { defaultRangeExtractor, useVirtualizer, type Range } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { Folder } from 'lucide-react';
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import { actionInfo } from '../../../lib/rules/actions';
import { ruleLabel, summarizeTokens } from '../../../lib/rules/describe';
import { canMove, currentSlotIndex, dropAt, moveItem, dropSlots, type DropIndicator, type DropPosition, type FlatRow } from '../../../lib/rules/tree';
import { iconFor } from '../icons';
import { Tokens } from '../Tokens';
import { GroupRow, RuleRow, type RowActions } from './Rows';
import styles from './RuleTree.module.css';

export const RULE_ROW_H = 40;
export const GROUP_ROW_H = 34;
const rowHeight = (r: FlatRow) => (r.kind === 'group' ? GROUP_ROW_H : RULE_ROW_H);

export interface RuleTreeHandle {
  scrollToId(id: RuleId): void;
  focusRow(id?: RuleId | null): void;
}

export interface RuleTreeProps {
  rules: RuleSet;
  rows: readonly FlatRow[];
  selectedId: RuleId | null;
  serverErrors: ReadonlyMap<RuleId, string>;
  invalidDrafts: ReadonlySet<RuleId>;
  renamingId: RuleId | null;
  /** Drag & drop disabled (searching). */
  dragDisabled: boolean;
  actions: RowActions;
  onMove(id: RuleId, position: DropPosition, how: 'drag' | 'keyboard'): void;
  onDelete(id: RuleId): void;
  onDuplicate(id: RuleId): void;
  onToggleSelected(): void;
  /** Screen-reader announcement. */
  announce(message: string): void;
  describeDrop(id: RuleId, position: DropPosition): string;
  'aria-label'?: string;
}

/**
 * The virtualised rule tree: fixed-height rows (rules 40px, groups 34px), pointer drag &
 * drop through dnd-kit with before/after/inside indicators computed from the pointer, and
 * full keyboard support (↑/↓/Home/End, ←/→ collapse, Space toggle, Enter edit, Delete,
 * Alt+↑/↓ move).
 */
export const RuleTree = forwardRef<RuleTreeHandle, RuleTreeProps>(function RuleTree(props, ref) {
  const { rules, rows, selectedId, serverErrors, invalidDrafts, renamingId, dragDisabled, actions, onMove, onDelete, onDuplicate, onToggleSelected, announce, describeDrop } = props;
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [activeId, setActiveId] = useState<RuleId | null>(null);
  const [indicator, setIndicator] = useState<DropIndicator | null>(null);
  const pointerY = useRef(0);

  const heights = useMemo(() => rows.map(rowHeight), [rows]);
  const offsets = useMemo(() => {
    const out: number[] = [];
    let top = 0;
    for (const h of heights) {
      out.push(top);
      top += h;
    }
    out.push(top);
    return out;
  }, [heights]);
  const indexById = useMemo(() => new Map(rows.map((r, i) => [r.id, i])), [rows]);
  const activeIndex = activeId ? (indexById.get(activeId) ?? -1) : -1;

  const rangeExtractor = useCallback(
    (range: Range) => {
      const base = defaultRangeExtractor(range);
      // Keep the dragged row mounted while the list scrolls under it.
      if (activeIndex >= 0 && !base.includes(activeIndex)) return [...base, activeIndex].sort((a, b) => a - b);
      return base;
    },
    [activeIndex],
  );

  const virtualizer = useVirtualizer({
    count: rows.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => heights[i] ?? RULE_ROW_H,
    getItemKey: (i) => rows[i]?.id ?? i,
    overscan: 12,
    rangeExtractor,
  });

  // Row heights depend on the row kind; re-measure when the rows change.
  useEffect(() => {
    virtualizer.measure();
  }, [heights, virtualizer]);

  const scrollToId = useCallback(
    (id: RuleId) => {
      const i = indexById.get(id);
      if (i == null) return;
      virtualizer.scrollToIndex(i, { align: 'auto' });
    },
    [indexById, virtualizer],
  );

  const focusRow = useCallback(
    (id?: RuleId | null) => {
      const target = id ?? selectedId ?? rows[0]?.id;
      if (!target) {
        scrollRef.current?.focus();
        return;
      }
      scrollToId(target);
      requestAnimationFrame(() => {
        const el = scrollRef.current?.querySelector<HTMLElement>(`[data-row-id="${CSS.escape(target)}"]`);
        el?.focus({ preventScroll: true });
      });
    },
    [rows, selectedId, scrollToId],
  );

  useImperativeHandle(ref, () => ({ scrollToId, focusRow }), [scrollToId, focusRow]);

  // ---------------------------------------------------------------- pointer drag & drop
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 4 } }));

  const computeIndicator = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const y = pointerY.current - rect.top + el.scrollTop - LIST_PAD_TOP;
    setIndicator((prev) => {
      const next = dropAt(rows, heights, y);
      if (prev && next && prev.rowIndex === next.rowIndex && prev.mode === next.mode && prev.depth === next.depth && samePos(prev.position, next.position)) return prev;
      if (next && activeId && !isUseful(rules, activeId, next)) return null;
      return next;
    });
  }, [rows, heights, activeId, rules]);

  useEffect(() => {
    if (!activeId) return;
    const onMovePointer = (e: PointerEvent) => {
      pointerY.current = e.clientY;
      computeIndicator();
    };
    const el = scrollRef.current;
    window.addEventListener('pointermove', onMovePointer);
    el?.addEventListener('scroll', computeIndicator, { passive: true });
    return () => {
      window.removeEventListener('pointermove', onMovePointer);
      el?.removeEventListener('scroll', computeIndicator);
    };
  }, [activeId, computeIndicator]);

  const onDragStart = (e: DragStartEvent) => {
    const id = String(e.active.id);
    setActiveId(id);
    const ev = e.activatorEvent as PointerEvent;
    pointerY.current = ev.clientY;
    actions.select(id);
  };

  const onDragEnd = (_e: DragEndEvent) => {
    const id = activeId;
    const ind = indicator;
    setActiveId(null);
    setIndicator(null);
    if (id && ind) onMove(id, ind.position, 'drag');
  };

  const onDragCancel = () => {
    setActiveId(null);
    setIndicator(null);
  };

  // ---------------------------------------------------------------- keyboard
  const moveSelection = (delta: number | 'home' | 'end') => {
    if (rows.length === 0) return;
    const cur = selectedId ? (indexById.get(selectedId) ?? -1) : -1;
    let next: number;
    if (delta === 'home') next = 0;
    else if (delta === 'end') next = rows.length - 1;
    else next = cur < 0 ? (delta > 0 ? 0 : rows.length - 1) : Math.max(0, Math.min(rows.length - 1, cur + delta));
    const id = rows[next]?.id;
    if (id) {
      actions.select(id);
      focusRow(id);
    }
  };

  const keyboardMove = (dir: -1 | 1) => {
    if (!selectedId || dragDisabled) return;
    const slots = dropSlots(rules, rows, selectedId);
    const cur = currentSlotIndex(rules, slots, selectedId);
    const target = slots[cur + dir];
    if (cur < 0 || !target) {
      announce(dir < 0 ? 'Already at the top' : 'Already at the bottom');
      return;
    }
    announce(`Moved ${describeDrop(selectedId, target.position)}`);
    onMove(selectedId, target.position, 'keyboard');
    focusRow(selectedId);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.defaultPrevented || renamingId) return;
    const target = e.target as HTMLElement;
    if (target.tagName === 'INPUT' || target.tagName === 'TEXTAREA') return;
    const row = selectedId ? rows[indexById.get(selectedId) ?? -1] : undefined;
    const mod = e.metaKey || e.ctrlKey;
    let handled = true;
    switch (e.key) {
      case 'ArrowDown':
      case 'j':
        if (e.altKey) keyboardMove(1);
        else if (!mod) moveSelection(1);
        else handled = false;
        break;
      case 'ArrowUp':
      case 'k':
        if (e.altKey) keyboardMove(-1);
        else if (!mod) moveSelection(-1);
        else handled = false;
        break;
      case 'Home':
        moveSelection('home');
        break;
      case 'End':
        moveSelection('end');
        break;
      case 'PageDown':
        moveSelection(10);
        break;
      case 'PageUp':
        moveSelection(-10);
        break;
      case 'ArrowRight':
        if (row?.kind === 'group' && !row.expanded) actions.toggleCollapse(row.id);
        else if (row?.kind === 'group') moveSelection(1);
        else handled = false;
        break;
      case 'ArrowLeft':
        if (row?.kind === 'group' && row.expanded) actions.toggleCollapse(row.id);
        else if (row?.parentId) {
          actions.select(row.parentId);
          focusRow(row.parentId);
        } else handled = false;
        break;
      case 'Enter':
        if (row) actions.activate(row.id);
        break;
      case 'F2':
        if (row?.kind === 'group') actions.setRenaming(row.id);
        else if (row) actions.activate(row.id);
        break;
      case ' ':
        onToggleSelected();
        break;
      case 'Delete':
      case 'Backspace':
        if (row) onDelete(row.id);
        break;
      case 'd':
      case 'D':
        if (mod && row) onDuplicate(row.id);
        else handled = false;
        break;
      default:
        handled = false;
    }
    if (handled) {
      e.preventDefault();
      e.stopPropagation();
    }
  };

  const items = virtualizer.getVirtualItems();
  const tabbableId = selectedId && indexById.has(selectedId) ? selectedId : (rows[0]?.id ?? null);
  const activeRow = activeIndex >= 0 ? rows[activeIndex] : undefined;
  const totalHeight = offsets[offsets.length - 1] ?? 0;

  return (
    <DndContext sensors={sensors} onDragStart={onDragStart} onDragEnd={onDragEnd} onDragCancel={onDragCancel} autoScroll={{ threshold: { x: 0, y: 0.12 }, acceleration: 12 }}>
      <div
        ref={scrollRef}
        className={clsx(styles.scroller, activeId && styles.dragging)}
        role="tree"
        aria-label={props['aria-label'] ?? 'Rules'}
        aria-multiselectable={false}
        onKeyDown={onKeyDown}
        tabIndex={rows.length === 0 ? 0 : -1}
      >
        <div className={styles.inner} style={{ height: totalHeight + LIST_PAD_TOP + LIST_PAD_BOTTOM }}>
          {items.map((vi) => {
            const row = rows[vi.index];
            if (!row) return null;
            const style: CSSProperties = { transform: `translateY(${(offsets[vi.index] ?? 0) + LIST_PAD_TOP}px)`, height: heights[vi.index] };
            return (
              <div key={row.id} className={styles.slot} style={style} role="none">
                {row.kind === 'rule' ? (
                  <RuleRow
                    rule={row.item as Extract<FlatRow['item'], { type: 'rule' }>}
                    depth={row.depth}
                    parentEnabled={row.parentEnabled}
                    selected={row.id === selectedId}
                    tabbable={row.id === tabbableId}
                    actions={actions}
                    dragDisabled={dragDisabled}
                    error={serverErrors.get(row.id)}
                    invalidDraft={invalidDrafts.has(row.id)}
                  />
                ) : (
                  <GroupRow
                    group={row.item as Extract<FlatRow['item'], { type: 'group' }>}
                    depth={row.depth}
                    parentEnabled={row.parentEnabled}
                    selected={row.id === selectedId}
                    tabbable={row.id === tabbableId}
                    actions={actions}
                    dragDisabled={dragDisabled}
                    expanded={row.expanded}
                    ruleCount={row.ruleCount}
                    renaming={row.id === renamingId}
                  />
                )}
              </div>
            );
          })}
          {indicator && activeId && (
            <div
              className={clsx(indicator.mode === 'group' ? styles.dropGroup : styles.dropLine)}
              style={
                indicator.mode === 'group'
                  ? { transform: `translateY(${(offsets[indicator.rowIndex] ?? 0) + LIST_PAD_TOP}px)`, height: heights[indicator.rowIndex], '--depth': indicator.depth } as CSSProperties
                  : { transform: `translateY(${(offsets[indicator.rowIndex] ?? totalHeight) + LIST_PAD_TOP - 1}px)`, '--depth': indicator.depth } as CSSProperties
              }
              aria-hidden
            />
          )}
        </div>
      </div>
      <DragOverlay dropAnimation={null}>{activeRow ? <DragPreview row={activeRow} /> : null}</DragOverlay>
    </DndContext>
  );
});

const LIST_PAD_TOP = 4;
const LIST_PAD_BOTTOM = 24;

function samePos(a: DropPosition, b: DropPosition): boolean {
  if (a.type !== b.type) return false;
  if (a.type === 'root-end' || b.type === 'root-end') return true;
  if (a.targetId !== b.targetId) return false;
  return a.type !== 'inside' || b.type !== 'inside' || a.at === b.at;
}

/** Hide indicators that would be a no-op or are invalid (into itself / its own children). */
function isUseful(rules: RuleSet, id: RuleId, ind: DropIndicator): boolean {
  const pos = ind.position;
  if (pos.type !== 'root-end' && pos.targetId === id) return false;
  if (!canMove(rules, id, pos)) return false;
  // A drop that leaves everything where it is isn't worth an indicator.
  return moveItem(rules, id, pos) !== rules;
}

function DragPreview({ row }: { row: FlatRow }) {
  if (row.item.type === 'group') {
    return (
      <div className={clsx(styles.preview, styles.previewGroup)}>
        <Folder size={14} strokeWidth={1.75} aria-hidden />
        <span className="truncate">{row.item.title}</span>
        <span className={clsx(styles.count, 'tnum')}>{row.ruleCount}</span>
      </div>
    );
  }
  const info = actionInfo(row.item.action.type);
  const Icon = iconFor(info.icon);
  return (
    <div className={styles.preview}>
      <span className={styles.iconTile} data-category={info.category} aria-hidden>
        <Icon size={13} strokeWidth={1.9} />
      </span>
      {row.item.title ? <span className="truncate">{ruleLabel(row.item)}</span> : <Tokens tokens={summarizeTokens(row.item)} className="truncate" />}
    </div>
  );
}
