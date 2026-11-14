import clsx from 'clsx';
import { ArrowDown, ArrowUp, ChevronDown } from 'lucide-react';
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentProps, type ReactNode } from 'react';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItems,
  DropdownMenuTrigger,
  TabList,
  Tabs,
  TabsContent,
  type MenuItems,
  type TabItem,
} from '../../../../components/ui';
import { formatCompactNumber } from '../../../../lib/format';
import { fitTabs, sameIndices } from '../lib/tabsFit';
import styles from './SectionTabs.module.css';

export interface SectionTabsProps<T extends string> {
  /** `Request` / `Response` (rendered as a small caps label). */
  label: string;
  items: readonly TabItem<T>[];
  value: T;
  onValueChange: (value: T) => void;
  /** Hide the label (the context already says what this is, e.g. inside a dialog). */
  hideLabel?: boolean;
  /** Content after the label (e.g. an "Editing" pill). */
  badge?: ReactNode;
  /** Right-aligned controls in the tab strip. */
  trailing?: ReactNode;
  /** A bar shown with the tab strip (breakpoint method/URL row, status line). */
  toolbar?: ReactNode;
  /** Where `toolbar` goes: under the tabs (default, keeps side-by-side tab strips aligned) or above them. */
  toolbarPlacement?: 'below' | 'above';
  children: ReactNode;
  className?: string;
  /** Extra class for the head row (label + tabs). */
  headClassName?: string;
}

/** Tab strip metrics (Tabs.module.css, size `sm`): list padding `0 4px`, 2px gaps. */
const STRIP_PADDING = 8;
const STRIP_GAP = 2;
/** Fallback width of the "More" button before it has been measured. */
const MORE_FALLBACK = 64;

/**
 * A Request/Response section: label + underline tabs + panels (DESIGN.md §6.4). Tabs that don't
 * fit the section's width move into a "More ▾" menu (the selected tab always stays visible), so
 * narrow side-by-side sections never hide tabs behind a scroll.
 */
export function SectionTabs<T extends string>({ label, hideLabel, items, value, onValueChange, badge, trailing, toolbar, toolbarPlacement = 'below', children, className, headClassName }: SectionTabsProps<T>) {
  const { areaRef, moreRef, trailingRef, shown, hidden } = useTabOverflow(items, value);

  const moreItems: MenuItems = hidden.map((item) => ({
    key: item.value,
    label: item.label,
    icon: item.icon,
    hint: item.count != null ? formatCompactNumber(item.count) : typeof item.badge === 'string' ? item.badge : undefined,
    disabled: item.disabled,
    onSelect: () => onValueChange(item.value),
  }));

  return (
    <Tabs value={value} onValueChange={onValueChange} className={clsx(styles.section, className)}>
      {toolbarPlacement === 'above' && toolbar}
      <div className={clsx(styles.head, headClassName)}>
        <span className={clsx(styles.label, hideLabel && 'sr-only')} title={label}>
          <span className={styles.labelText}>{label}</span>
          <span className={styles.labelIcon} role="img" aria-label={label}>
            {label === 'Request' ? <ArrowUp size={13} strokeWidth={2} aria-hidden /> : <ArrowDown size={13} strokeWidth={2} aria-hidden />}
          </span>
        </span>
        {badge}
        <div ref={areaRef} className={styles.area}>
          <TabList items={shown} size="sm" className={styles.tabs} aria-label={`${label} views`} />
          {hidden.length > 0 && (
            <DropdownMenu modal={false}>
              <DropdownMenuTrigger asChild>
                <button ref={moreRef} type="button" className={styles.more} aria-label={`More ${label.toLowerCase()} views: ${hidden.map((h) => h.value).join(', ')}`}>
                  More
                  <ChevronDown size={12} strokeWidth={2} aria-hidden />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end">
                <DropdownMenuItems items={moreItems} />
              </DropdownMenuContent>
            </DropdownMenu>
          )}
          {trailing && (
            <div ref={trailingRef} className={styles.trailing}>
              {trailing}
            </div>
          )}
        </div>
      </div>
      {toolbarPlacement === 'below' && toolbar}
      {children}
    </Tabs>
  );
}

/**
 * Measures the rendered tab triggers (widths are cached per tab, so hidden tabs keep their last
 * measured width) and picks the subset that fits the strip. Runs in a layout effect, so the first
 * paint already shows the fitted strip.
 */
function useTabOverflow<T extends string>(items: readonly TabItem<T>[], value: T) {
  const areaRef = useRef<HTMLDivElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const trailingRef = useRef<HTMLDivElement | null>(null);
  const widths = useRef(new Map<string, number>());
  const moreWidth = useRef(MORE_FALLBACK);
  const [indices, setIndices] = useState<number[] | null>(null);

  // Indices can be stale for one render when the item list changes: drop out-of-range ones.
  const valid = indices && indices.every((i) => i < items.length) ? indices : null;
  const shown = useMemo(() => (valid ? valid.map((i) => items[i] as TabItem<T>) : items), [valid, items]);
  const hidden = useMemo(() => {
    if (!valid) return [];
    const set = new Set(valid);
    return items.filter((_, i) => !set.has(i));
  }, [valid, items]);

  // Latest inputs for the ResizeObserver callback.
  const latest = useRef({ items, value, shown });
  latest.current = { items, value, shown };

  const update = useCallback(() => {
    const area = areaRef.current;
    if (!area) return;
    const { items: all, value: active, shown: rendered } = latest.current;
    // Triggers render in the order of `shown`.
    area.querySelectorAll<HTMLElement>('[role="tab"]').forEach((el, i) => {
      const item = rendered[i];
      if (item) widths.current.set(item.value, el.getBoundingClientRect().width);
    });
    if (moreRef.current) moreWidth.current = moreRef.current.getBoundingClientRect().width + 4;
    const available = area.clientWidth - (trailingRef.current?.getBoundingClientRect().width ?? 0);
    if (available <= 0) return;
    const ws = all.map((it) => widths.current.get(it.value) ?? 0);
    const next = fitTabs(
      ws,
      available,
      all.findIndex((it) => it.value === active),
      { gap: STRIP_GAP, padding: STRIP_PADDING, more: moreWidth.current },
    );
    const nextOrNull = next.length === all.length ? null : next;
    setIndices((prev) => (sameIndices(prev, nextOrNull) ? prev : nextOrNull));
  }, []);

  // Re-fit when what the tabs display changes (labels, counts, badges), the selection moves or
  // a tab gets hidden/shown — not on every render of a live-updating parent.
  const signature = items.map((it) => `${it.value}\u0001${typeof it.label === 'string' ? it.label : ''}\u0001${it.count ?? ''}\u0001${typeof it.badge === 'string' ? it.badge : it.badge != null ? '•' : ''}`).join('\u0002');
  const shownKey = valid ? valid.join(',') : 'all';
  useLayoutEffect(update, [update, signature, value, shownKey]);

  useEffect(() => {
    const area = areaRef.current;
    if (!area) return;
    const ro = new ResizeObserver(() => update());
    ro.observe(area);
    // Web fonts finishing loading change every trigger's width.
    const list = area.querySelector('[role="tablist"]');
    if (list) ro.observe(list);
    return () => ro.disconnect();
  }, [update]);

  return { areaRef, moreRef, trailingRef, shown, hidden };
}

/** A panel that fills the section; `scroll` adds a scroll container, `pad` (default) pads it. */
export function SectionPanel({ scroll, pad = true, className, children, ...rest }: ComponentProps<typeof TabsContent> & { scroll?: boolean; pad?: boolean }) {
  return (
    <TabsContent className={clsx(styles.panel, className)} {...rest}>
      {scroll ? <div className={clsx(styles.scroll, pad && styles.pad)}>{children}</div> : children}
    </TabsContent>
  );
}
