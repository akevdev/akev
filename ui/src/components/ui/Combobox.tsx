import * as RadixPopover from '@radix-ui/react-popover';
import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import { useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import type { ControlSize } from './Button';
import { Input, type InputProps } from './Input';
import styles from './Combobox.module.css';

export interface ComboboxOption {
  /** Text inserted into the input when chosen. */
  value: string;
  /** Display label (defaults to `value`). */
  label?: ReactNode;
  /** Muted text on the right. */
  detail?: string;
  icon?: LucideIcon;
}

export interface ComboboxProps extends Omit<InputProps, 'value' | 'onChange' | 'onSelect' | 'size'> {
  value: string;
  onValueChange: (value: string) => void;
  /** Suggestions, or a function computing them from the current text. */
  options: readonly ComboboxOption[] | ((query: string) => readonly ComboboxOption[]);
  /** Called when a suggestion is accepted (after `onValueChange`). */
  onSelectOption?: (option: ComboboxOption) => void;
  /** Built-in case-insensitive filtering of a static list (default true). */
  filter?: boolean;
  size?: ControlSize;
  /** Text shown when nothing matches (omit to hide the popup instead). */
  emptyText?: string;
  maxResults?: number;
  /** Open the suggestions when the input gains focus (default true). */
  openOnFocus?: boolean;
}

/**
 * Text input with a suggestion list (ARIA combobox pattern): ↑/↓ to move, Enter/Tab to
 * accept, Escape to close. Free text is always allowed.
 */
export function Combobox({
  value,
  onValueChange,
  options,
  onSelectOption,
  filter = true,
  size = 'md',
  emptyText,
  maxResults = 50,
  openOnFocus = true,
  onKeyDown,
  onFocus,
  onBlur,
  ...inputProps
}: ComboboxProps) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const listId = useId();
  const inputRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);

  const items = useMemo(() => {
    const source = typeof options === 'function' ? options(value) : options;
    if (typeof options === 'function' || !filter) return source.slice(0, maxResults);
    const q = value.trim().toLowerCase();
    if (!q) return source.slice(0, maxResults);
    const starts: ComboboxOption[] = [];
    const contains: ComboboxOption[] = [];
    for (const o of source) {
      const v = o.value.toLowerCase();
      if (v === q) continue;
      if (v.startsWith(q)) starts.push(o);
      else if (v.includes(q) || (o.detail && o.detail.toLowerCase().includes(q))) contains.push(o);
      if (starts.length >= maxResults) break;
    }
    return [...starts, ...contains].slice(0, maxResults);
  }, [options, value, filter, maxResults]);

  const showList = open && (items.length > 0 || !!emptyText);
  const activeIndex = Math.min(active, Math.max(0, items.length - 1));

  const accept = (o: ComboboxOption) => {
    onValueChange(o.value);
    onSelectOption?.(o);
    setOpen(false);
  };

  const scrollIntoView = (i: number) => {
    const el = listRef.current?.querySelector<HTMLElement>(`[data-index="${i}"]`);
    el?.scrollIntoView({ block: 'nearest' });
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      if (!open) {
        setOpen(true);
        return;
      }
      if (items.length === 0) return;
      const next = (activeIndex + (e.key === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
      setActive(next);
      scrollIntoView(next);
    } else if ((e.key === 'Enter' || e.key === 'Tab') && showList && items[activeIndex]) {
      if (e.key === 'Enter') e.preventDefault();
      accept(items[activeIndex]);
    } else if (e.key === 'Escape' && open) {
      e.preventDefault();
      e.stopPropagation();
      setOpen(false);
    }
  };

  return (
    <RadixPopover.Root open={showList} onOpenChange={(o) => !o && setOpen(false)}>
      <RadixPopover.Anchor asChild>
        <div className={styles.anchor}>
          <Input
            {...inputProps}
            ref={inputRef}
            size={size}
            role="combobox"
            aria-expanded={showList}
            aria-controls={showList ? listId : undefined}
            aria-autocomplete="list"
            aria-activedescendant={showList && items[activeIndex] ? `${listId}-${activeIndex}` : undefined}
            value={value}
            onChange={(e) => {
              onValueChange(e.target.value);
              setActive(0);
              setOpen(true);
            }}
            onKeyDown={handleKeyDown}
            onFocus={(e) => {
              if (openOnFocus) setOpen(true);
              onFocus?.(e);
            }}
            onBlur={(e) => {
              setOpen(false);
              onBlur?.(e);
            }}
          />
        </div>
      </RadixPopover.Anchor>
      <RadixPopover.Portal>
        <RadixPopover.Content
          className={styles.content}
          align="start"
          sideOffset={4}
          collisionPadding={8}
          onOpenAutoFocus={(e) => e.preventDefault()}
          onCloseAutoFocus={(e) => e.preventDefault()}
          onInteractOutside={(e) => {
            if (inputRef.current && e.target instanceof Node && inputRef.current.contains(e.target)) e.preventDefault();
          }}
        >
          <div ref={listRef} id={listId} role="listbox" className={styles.list}>
            {items.length === 0 && emptyText && <div className={styles.empty}>{emptyText}</div>}
            {items.map((o, i) => {
              const Icon = o.icon;
              return (
                // biome-ignore lint/a11y/useFocusableInteractive lint/a11y/useKeyWithClickEvents: focus stays in the input, which drives the list (aria-activedescendant)
                <div
                  key={`${o.value}-${i}`}
                  id={`${listId}-${i}`}
                  data-index={i}
                  role="option"
                  aria-selected={i === activeIndex}
                  className={clsx(styles.option, i === activeIndex && styles.active)}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseMove={() => i !== activeIndex && setActive(i)}
                  onClick={() => accept(o)}
                >
                  {Icon && <Icon size={14} strokeWidth={1.75} className={styles.icon} aria-hidden />}
                  <span className={styles.label}>{o.label ?? o.value}</span>
                  {o.detail && <span className={styles.detail}>{o.detail}</span>}
                </div>
              );
            })}
          </div>
        </RadixPopover.Content>
      </RadixPopover.Portal>
    </RadixPopover.Root>
  );
}
