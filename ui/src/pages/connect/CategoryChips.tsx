import clsx from 'clsx';
import { useRef, type KeyboardEvent } from 'react';
import { CATEGORY_FILTERS, type CategoryFilter } from './catalog';
import styles from './CategoryChips.module.css';

export interface CategoryChipsProps {
  value: CategoryFilter;
  onValueChange: (value: CategoryFilter) => void;
  counts: Record<CategoryFilter, number>;
  /** Match counts for the current search (shown instead of totals while searching). */
  matchCounts?: Record<CategoryFilter, number> | null;
}

/**
 * Category filter chips (radio group with roving focus: ←/→ move, Home/End jump).
 * Categories with no interceptors are hidden.
 */
export function CategoryChips({ value, onValueChange, counts, matchCounts }: CategoryChipsProps) {
  const ref = useRef<HTMLDivElement | null>(null);
  const items = CATEGORY_FILTERS.filter((c) => c.id === 'all' || (counts[c.id] ?? 0) > 0);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const keys = ['ArrowLeft', 'ArrowRight', 'Home', 'End'];
    if (!keys.includes(e.key)) return;
    e.preventDefault();
    const i = items.findIndex((c) => c.id === value);
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? items.length - 1 : (i + (e.key === 'ArrowRight' ? 1 : -1) + items.length) % items.length;
    const target = items[next];
    if (!target) return;
    onValueChange(target.id);
    ref.current?.querySelector<HTMLButtonElement>(`[data-value="${target.id}"]`)?.focus();
  };

  return (
    <div ref={ref} className={styles.chips} role="radiogroup" aria-label="Category" onKeyDown={onKeyDown}>
      {items.map((c) => {
        const selected = c.id === value;
        const count = matchCounts ? (matchCounts[c.id] ?? 0) : (counts[c.id] ?? 0);
        return (
          <button
            key={c.id}
            type="button"
            role="radio"
            aria-checked={selected}
            tabIndex={selected ? 0 : -1}
            data-value={c.id}
            className={clsx(styles.chip, selected && styles.selected, matchCounts && count === 0 && styles.empty)}
            onClick={() => onValueChange(c.id)}
          >
            <span>{c.label}</span>
            <span className={clsx(styles.count, 'tnum')}>{count}</span>
          </button>
        );
      })}
    </div>
  );
}
