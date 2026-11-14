import * as ToggleGroup from '@radix-ui/react-toggle-group';
import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import { useLayoutEffect, useRef, useState, type ReactNode } from 'react';
import { formatCompactNumber } from '../../lib/format';
import { Tooltip } from './Tooltip';
import styles from './SegmentedControl.module.css';

export interface SegmentedItem<T extends string = string> {
  value: T;
  label?: ReactNode;
  icon?: LucideIcon;
  /** Small count after the label. */
  count?: number;
  tooltip?: string;
  disabled?: boolean;
  /** Accessible label for icon-only items. */
  'aria-label'?: string;
}

export interface SegmentedControlProps<T extends string = string> {
  value: T;
  onValueChange: (value: T) => void;
  items: readonly SegmentedItem<T>[];
  size?: 'sm' | 'md';
  className?: string;
  'aria-label'?: string;
  /** Stretch items to fill the width. */
  block?: boolean;
}

/**
 * Pill-style single-choice control (view modes, filters) with a sliding selection thumb.
 * Always has a value: clicking the selected item does nothing.
 */
export function SegmentedControl<T extends string = string>({ value, onValueChange, items, size = 'md', className, block, ...aria }: SegmentedControlProps<T>) {
  const rootRef = useRef<HTMLDivElement | null>(null);
  const [thumb, setThumb] = useState<{ left: number; width: number } | null>(null);
  const [animate, setAnimate] = useState(false);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const measure = () => {
      // `aria-checked`: a Tooltip around an item overrides its `data-state`.
      const el = root.querySelector<HTMLElement>('[aria-checked="true"], [data-state="on"]');
      if (!el) return setThumb(null);
      setThumb({ left: el.offsetLeft, width: el.offsetWidth });
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(root);
    return () => ro.disconnect();
  }, [value, items]);

  useLayoutEffect(() => {
    // Enable the slide transition only after the first measurement.
    if (thumb && !animate) {
      const id = requestAnimationFrame(() => setAnimate(true));
      return () => cancelAnimationFrame(id);
    }
    return undefined;
  }, [thumb, animate]);

  return (
    <ToggleGroup.Root
      ref={rootRef}
      type="single"
      value={value}
      onValueChange={(v) => v && onValueChange(v as T)}
      className={clsx(styles.root, styles[size], block && styles.block, className)}
      aria-label={aria['aria-label']}
    >
      {thumb && <span className={clsx(styles.thumb, animate && styles.animate)} style={{ transform: `translateX(${thumb.left}px)`, width: thumb.width }} aria-hidden />}
      {items.map((item) => {
        const Icon = item.icon;
        const node = (
          <ToggleGroup.Item
            key={item.value}
            value={item.value}
            disabled={item.disabled}
            aria-label={item['aria-label'] ?? (typeof item.label === 'string' ? item.label : item.tooltip)}
            className={clsx(styles.item, !item.label && styles.iconOnly)}
          >
            {Icon && <Icon size={size === 'sm' ? 13 : 14} strokeWidth={1.75} aria-hidden />}
            {item.label != null && <span className={styles.label}>{item.label}</span>}
            {item.count != null && <span className={clsx(styles.count, 'tnum')}>{formatCompactNumber(item.count)}</span>}
          </ToggleGroup.Item>
        );
        return item.tooltip ? (
          <Tooltip key={item.value} content={item.tooltip}>
            {node}
          </Tooltip>
        ) : (
          node
        );
      })}
    </ToggleGroup.Root>
  );
}
