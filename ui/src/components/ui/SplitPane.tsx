import clsx from 'clsx';
import { useCallback, useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type PointerEvent, type ReactNode } from 'react';
import { useUiStore } from '../../state/ui';
import styles from './SplitPane.module.css';

export interface SplitPaneProps {
  /** Persistence key: the ratio is stored in UI prefs (`splitRatios[id]`). */
  id?: string;
  /** `horizontal` = panes side by side (default); `vertical` = stacked. */
  direction?: 'horizontal' | 'vertical';
  /** Initial/reset size of the first pane as a fraction (default 0.5). */
  defaultRatio?: number;
  /** Minimum first-pane size in px (default 200). */
  minFirst?: number;
  /** Minimum second-pane size in px (default 200). */
  minSecond?: number;
  /** Hide one pane (and the divider). */
  collapsed?: 'first' | 'second' | null;
  /** Exactly two children. */
  children: [ReactNode, ReactNode];
  className?: string;
  /** Called continuously while dragging. */
  onRatioChange?: (ratio: number) => void;
  /** Accessible label for the divider. */
  dividerLabel?: string;
  /**
   * Size the first pane in px (a sidebar or list) instead of by ratio. The caller owns and
   * persists the width: `onSizeChange` fires when a drag ends and on each resize key.
   */
  size?: number;
  onSizeChange?: (px: number) => void;
  /** Largest first pane in px (pixel mode). */
  maxFirst?: number;
  /** Width restored by double-click or Enter (pixel mode; defaults to `minFirst`). */
  defaultSize?: number;
}

/**
 * Resizable two-pane layout: 1px divider with a 7px hit area, pointer & keyboard resizing
 * (←/→ or ↑/↓ by 2% or 16 px, Shift for more, Home/End to the limits), double-click or Enter
 * to reset. The first pane is sized by a persisted ratio, or in px when `size` is given.
 */
export function SplitPane({
  id,
  direction = 'horizontal',
  defaultRatio = 0.5,
  minFirst = 200,
  minSecond = 200,
  collapsed = null,
  children,
  className,
  onRatioChange,
  dividerLabel = 'Resize panes',
  size: fixedSize,
  onSizeChange,
  maxFirst,
  defaultSize,
}: SplitPaneProps) {
  const stored = useUiStore((s) => (id ? s.splitRatios[id] : undefined));
  const setSplitRatio = useUiStore((s) => s.setSplitRatio);
  const [dragRatio, setDragRatio] = useState<number | null>(null);
  const [size, setSize] = useState(0);
  const containerRef = useRef<HTMLDivElement | null>(null);
  const frame = useRef<number | null>(null);
  const horizontal = direction === 'horizontal';

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver(([entry]) => {
      if (entry) setSize(horizontal ? entry.contentRect.width : entry.contentRect.height);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [horizontal]);

  const clamp = useCallback(
    (r: number) => {
      if (size <= 0) return Math.max(0.05, Math.min(0.95, r));
      const min = Math.min(minFirst / size, 0.5);
      const max = Math.max(1 - minSecond / size, 0.5);
      return Math.max(min, Math.min(max, r));
    },
    [size, minFirst, minSecond],
  );

  // Pixel mode: limits in px. While dragging, `dragRatio` holds the dragged width in px.
  const pixels = fixedSize !== undefined;
  const maxPx = Math.max(minFirst, Math.min(maxFirst ?? Number.POSITIVE_INFINITY, size > 0 ? size - minSecond - 1 : Number.POSITIVE_INFINITY));
  const clampPx = (px: number) => Math.round(Math.max(minFirst, Math.min(maxPx, px)));
  const limit = pixels ? clampPx : clamp;

  const ratio = pixels ? clampPx(dragRatio ?? fixedSize) : clamp(dragRatio ?? stored ?? defaultRatio);
  const reset = pixels ? (defaultSize ?? minFirst) : defaultRatio;

  const commit = (r: number) => {
    const c = limit(r);
    if (pixels) {
      onSizeChange?.(c);
      return;
    }
    if (id) setSplitRatio(id, c);
    onRatioChange?.(c);
  };

  const ratioFromPointer = (e: PointerEvent) => {
    const rect = containerRef.current?.getBoundingClientRect();
    if (!rect) return ratio;
    if (pixels) return horizontal ? e.clientX - rect.left : e.clientY - rect.top;
    return horizontal ? (e.clientX - rect.left) / rect.width : (e.clientY - rect.top) / rect.height;
  };

  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.currentTarget.setPointerCapture(e.pointerId);
    document.documentElement.dataset.resizing = horizontal ? 'col' : 'row';
    setDragRatio(ratio);
  };

  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (dragRatio === null) return;
    const r = limit(ratioFromPointer(e));
    if (frame.current != null) cancelAnimationFrame(frame.current);
    frame.current = requestAnimationFrame(() => {
      setDragRatio(r);
      if (!pixels) onRatioChange?.(r);
    });
  };

  const endDrag = (e: PointerEvent<HTMLDivElement>) => {
    if (dragRatio === null) return;
    if (frame.current != null) cancelAnimationFrame(frame.current);
    delete document.documentElement.dataset.resizing;
    const r = limit(ratioFromPointer(e));
    setDragRatio(null);
    commit(r);
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const dec = horizontal ? 'ArrowLeft' : 'ArrowUp';
    const inc = horizontal ? 'ArrowRight' : 'ArrowDown';
    const step = pixels ? (e.shiftKey ? 64 : 16) : e.shiftKey ? 0.1 : 0.02;
    let next: number | null = null;
    if (e.key === dec) next = ratio - step;
    else if (e.key === inc) next = ratio + step;
    else if (e.key === 'Home') next = pixels ? minFirst : 0;
    else if (e.key === 'End') next = pixels ? maxPx : 1;
    else if (e.key === 'Enter') next = reset;
    if (next !== null) {
      e.preventDefault();
      commit(next);
    }
  };

  const [first, second] = children;
  const firstStyle: CSSProperties =
    collapsed === 'second' ? { flex: '1 1 auto' } : pixels ? { flex: `0 0 ${ratio}px` } : { flex: `0 0 calc(${(ratio * 100).toFixed(3)}% - 0.5px)` };

  return (
    <div ref={containerRef} className={clsx(styles.root, horizontal ? styles.horizontal : styles.vertical, dragRatio !== null && styles.dragging, className)}>
      {collapsed !== 'first' && (
        <div className={styles.pane} style={firstStyle}>
          {first}
        </div>
      )}
      {!collapsed && (
        <div
          className={styles.divider}
          role="separator"
          aria-orientation={horizontal ? 'vertical' : 'horizontal'}
          aria-label={dividerLabel}
          aria-valuenow={pixels ? ratio : Math.round(ratio * 100)}
          aria-valuemin={pixels ? minFirst : 0}
          aria-valuemax={pixels ? (Number.isFinite(maxPx) ? maxPx : undefined) : 100}
          tabIndex={0}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          onDoubleClick={() => commit(reset)}
          onKeyDown={onKeyDown}
        />
      )}
      {collapsed !== 'second' && (
        <div className={styles.pane} style={{ flex: '1 1 0' }}>
          {second}
        </div>
      )}
    </div>
  );
}
