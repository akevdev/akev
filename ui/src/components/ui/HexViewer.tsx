import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { memo, useCallback, useEffect, useRef, useState, type KeyboardEvent, type MouseEvent } from 'react';
import { byteHex, bytesToBase64, bytesToHex } from '../../lib/encoding';
import { formatBytes, formatNumber } from '../../lib/format';
import { copyText } from '../../lib/platform';
import { Button } from './Button';
import { toast } from './toast';
import styles from './HexViewer.module.css';

export interface HexViewerProps {
  data: Uint8Array;
  /** Fill the parent height (default) or size to `maxHeight`. */
  fill?: boolean;
  maxHeight?: number;
  /** Bytes per row (default 16). */
  bytesPerRow?: 8 | 16 | 32;
  /** Hide the copy toolbar. */
  toolbar?: boolean;
  className?: string;
  'aria-label'?: string;
}

interface Range {
  start: number;
  end: number;
}

const ROW_H = 20;

function ascii(b: number): string {
  return b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '·';
}

interface RowProps {
  row: number;
  data: Uint8Array;
  perRow: number;
  /** Selection clipped to this row (absolute offsets) or null. */
  selStart: number;
  selEnd: number;
  offsetDigits: number;
}

const HexRow = memo(function HexRow({ row, data, perRow, selStart, selEnd, offsetDigits }: RowProps) {
  const base = row * perRow;
  const end = Math.min(data.length, base + perRow);
  const hex = [];
  const chars = [];
  for (let i = base; i < base + perRow; i++) {
    if (i < end) {
      const b = data[i] ?? 0;
      const selected = i >= selStart && i <= selEnd;
      const cls = clsx(styles.byte, b === 0 ? styles.zero : styles[`n${b >> 4}`], selected && styles.selected, (i - base) % 8 === 7 && i - base !== perRow - 1 && styles.gap);
      hex.push(
        <span key={i} data-offset={i} className={cls}>
          {byteHex(b)}
        </span>,
      );
      chars.push(
        <span key={i} data-offset={i} className={clsx(styles.char, b >= 0x20 && b < 0x7f ? styles.printable : styles.np, selected && styles.selected)}>
          {ascii(b)}
        </span>,
      );
    } else {
      hex.push(<span key={i} className={clsx(styles.byte, styles.empty, (i - base) % 8 === 7 && styles.gap)} />);
      chars.push(<span key={i} className={styles.char} />);
    }
  }
  return (
    <div className={styles.row}>
      <span className={styles.offset}>{base.toString(16).padStart(offsetDigits, '0')}</span>
      <span className={styles.hex}>{hex}</span>
      <span className={styles.ascii}>{chars}</span>
    </div>
  );
});

/**
 * Virtualised hex dump: offset gutter, per-byte hue-coloured hex pairs, ASCII column.
 * Drag to select (synced across both columns), Shift-click to extend, ⌘/Ctrl+C copies hex.
 */
export function HexViewer({ data, fill = true, maxHeight = 480, bytesPerRow = 16, toolbar = true, className, ...aria }: HexViewerProps) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [selection, setSelection] = useState<Range | null>(null);
  const dragAnchor = useRef<number | null>(null);
  const rows = Math.ceil(data.length / bytesPerRow);
  const offsetDigits = Math.max(8, data.length.toString(16).length);

  useEffect(() => setSelection(null), [data]);

  const virtualizer = useVirtualizer({
    count: rows,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ROW_H,
    overscan: 12,
  });

  const offsetFromEvent = (e: MouseEvent): number | null => {
    const el = (e.target as HTMLElement).closest<HTMLElement>('[data-offset]');
    if (!el) return null;
    const n = Number(el.dataset.offset);
    return Number.isFinite(n) ? n : null;
  };

  const onMouseDown = (e: MouseEvent) => {
    if (e.button !== 0) return;
    const off = offsetFromEvent(e);
    if (off == null) return;
    e.preventDefault();
    scrollRef.current?.focus();
    if (e.shiftKey && selection) {
      const anchor = dragAnchor.current ?? selection.start;
      setSelection({ start: Math.min(anchor, off), end: Math.max(anchor, off) });
      return;
    }
    dragAnchor.current = off;
    setSelection({ start: off, end: off });
    const onMove = (ev: globalThis.MouseEvent) => {
      const target = document.elementFromPoint(ev.clientX, ev.clientY)?.closest<HTMLElement>('[data-offset]');
      if (!target || dragAnchor.current == null) return;
      const n = Number(target.dataset.offset);
      if (!Number.isFinite(n)) return;
      const a = dragAnchor.current;
      setSelection({ start: Math.min(a, n), end: Math.max(a, n) });
    };
    const onUp = () => {
      window.removeEventListener('mousemove', onMove);
      window.removeEventListener('mouseup', onUp);
    };
    window.addEventListener('mousemove', onMove);
    window.addEventListener('mouseup', onUp);
  };

  const selectedBytes = useCallback(() => (selection ? data.subarray(selection.start, selection.end + 1) : data), [data, selection]);

  const copy = async (kind: 'hex' | 'base64') => {
    const bytes = selectedBytes();
    const ok = await copyText(kind === 'hex' ? bytesToHex(bytes, ' ') : bytesToBase64(bytes));
    if (ok) toast.success(`Copied ${formatBytes(bytes.length)} as ${kind === 'hex' ? 'hex' : 'base64'}`);
    else toast.error('Copy failed');
  };

  const onKeyDown = (e: KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'c') {
      e.preventDefault();
      void copy('hex');
    } else if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'a') {
      e.preventDefault();
      if (data.length) setSelection({ start: 0, end: data.length - 1 });
    } else if (e.key === 'Escape' && selection) {
      e.stopPropagation();
      setSelection(null);
    }
  };

  const items = virtualizer.getVirtualItems();
  const height = fill ? undefined : Math.min(maxHeight, rows * ROW_H + 16);

  return (
    <div className={clsx(styles.root, fill && styles.fill, className)}>
      {toolbar && (
        <div className={styles.toolbar}>
          <span className={clsx(styles.info, 'tnum')}>
            {selection
              ? `${formatNumber(selection.end - selection.start + 1)} bytes selected · 0x${selection.start.toString(16)}–0x${selection.end.toString(16)}`
              : `${formatNumber(data.length)} bytes`}
          </span>
          <Button size="sm" variant="ghost" onClick={() => void copy('hex')} disabled={data.length === 0}>
            Copy hex
          </Button>
          <Button size="sm" variant="ghost" onClick={() => void copy('base64')} disabled={data.length === 0}>
            Copy base64
          </Button>
        </div>
      )}
      <div
        ref={scrollRef}
        className={styles.scroll}
        style={height ? { height, flex: 'none' } : undefined}
        tabIndex={0}
        role="grid"
        aria-label={aria['aria-label'] ?? 'Hex view'}
        aria-rowcount={rows}
        onMouseDown={onMouseDown}
        onKeyDown={onKeyDown}
      >
        {data.length === 0 ? (
          <div className={styles.noBytes}>No bytes</div>
        ) : (
          <div className={styles.sizer} style={{ height: virtualizer.getTotalSize() }}>
            <div className={styles.window} style={{ transform: `translateY(${items[0]?.start ?? 0}px)` }}>
              {items.map((item) => {
                const base = item.index * bytesPerRow;
                const last = base + bytesPerRow - 1;
                const overlaps = selection && selection.start <= last && selection.end >= base;
                return (
                  <HexRow
                    key={item.key}
                    row={item.index}
                    data={data}
                    perRow={bytesPerRow}
                    selStart={overlaps ? Math.max(selection.start, base) : -1}
                    selEnd={overlaps ? Math.min(selection.end, last) : -2}
                    offsetDigits={offsetDigits}
                  />
                );
              })}
            </div>
          </div>
        )}
      </div>
    </div>
  );
}
