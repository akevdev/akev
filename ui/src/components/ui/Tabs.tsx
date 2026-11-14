import * as RadixTabs from '@radix-ui/react-tabs';
import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import { useEffect, useRef, useState, type ComponentProps, type ReactNode } from 'react';
import { formatCompactNumber } from '../../lib/format';
import styles from './Tabs.module.css';

export interface TabItem<T extends string = string> {
  value: T;
  label: ReactNode;
  icon?: LucideIcon;
  /** Count badge (e.g. headers, messages). `0` renders dimmed. */
  count?: number;
  /** Small text badge after the label (e.g. a body size). */
  badge?: ReactNode;
  disabled?: boolean;
}

export interface TabsProps<T extends string = string> extends Omit<ComponentProps<typeof RadixTabs.Root>, 'value' | 'onValueChange' | 'defaultValue'> {
  value: T;
  onValueChange: (value: T) => void;
}

/**
 * Tabs root. Compose with {@link TabList} and {@link TabsContent}:
 * ```tsx
 * <Tabs value={tab} onValueChange={setTab}>
 *   <TabList items={[{ value: 'headers', label: 'Headers', count: 12 }, …]} />
 *   <TabsContent value="headers">…</TabsContent>
 * </Tabs>
 * ```
 */
export function Tabs<T extends string = string>({ value, onValueChange, className, ...rest }: TabsProps<T>) {
  return <RadixTabs.Root value={value} onValueChange={(v) => onValueChange(v as T)} className={clsx(styles.root, className)} {...rest} />;
}

export interface TabListProps<T extends string = string> {
  items: readonly TabItem<T>[];
  size?: 'sm' | 'md';
  /** Content pinned to the right of the tab strip (actions). */
  trailing?: ReactNode;
  className?: string;
  'aria-label'?: string;
}

/** Underline tab strip; overflows horizontally with fading edges. */
export function TabList<T extends string = string>({ items, size = 'md', trailing, className, ...aria }: TabListProps<T>) {
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const [fade, setFade] = useState<{ left: boolean; right: boolean }>({ left: false, right: false });

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const update = () => {
      const left = el.scrollLeft > 1;
      const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setFade((f) => (f.left === left && f.right === right ? f : { left, right }));
    };
    update();
    el.addEventListener('scroll', update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => {
      el.removeEventListener('scroll', update);
      ro.disconnect();
    };
  }, [items.length]);

  return (
    <div className={clsx(styles.bar, styles[size], className)}>
      <div ref={scrollRef} className={styles.scroller} data-fade-left={fade.left || undefined} data-fade-right={fade.right || undefined}>
        <RadixTabs.List className={styles.list} aria-label={aria['aria-label']}>
          {items.map((item) => {
            const Icon = item.icon;
            return (
              <RadixTabs.Trigger key={item.value} value={item.value} disabled={item.disabled} className={styles.trigger}>
                {Icon && <Icon size={size === 'sm' ? 13 : 14} strokeWidth={1.75} aria-hidden />}
                <span>{item.label}</span>
                {item.count != null && (
                  <span className={clsx(styles.count, item.count === 0 && styles.zero, 'tnum')}>{formatCompactNumber(item.count)}</span>
                )}
                {item.badge != null && <span className={clsx(styles.badge, 'tnum')}>{item.badge}</span>}
              </RadixTabs.Trigger>
            );
          })}
        </RadixTabs.List>
      </div>
      {trailing && <div className={styles.trailing}>{trailing}</div>}
    </div>
  );
}

/** Panel for one tab value. */
export function TabsContent({ className, ...rest }: ComponentProps<typeof RadixTabs.Content>) {
  return <RadixTabs.Content className={clsx(styles.content, className)} {...rest} />;
}
