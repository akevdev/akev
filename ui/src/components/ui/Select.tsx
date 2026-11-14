import * as RadixSelect from '@radix-ui/react-select';
import clsx from 'clsx';
import { Check, ChevronDown, ChevronUp, type LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import type { ControlSize } from './Button';
import inputStyles from './Input.module.css';
import styles from './Select.module.css';

export interface SelectOption<T extends string = string> {
  /** Must be non-empty (Radix reserves `""`). */
  value: T;
  label: ReactNode;
  /** Text used for type-ahead when `label` isn't a string. */
  textValue?: string;
  icon?: LucideIcon;
  /** Custom leading node (e.g. a coloured dot); takes precedence over `icon`. */
  leading?: ReactNode;
  description?: string;
  disabled?: boolean;
}

export interface SelectGroup<T extends string = string> {
  label: string;
  options: SelectOption<T>[];
}

export interface SelectProps<T extends string = string> {
  value: T | undefined;
  onValueChange: (value: T) => void;
  options: readonly (SelectOption<T> | SelectGroup<T>)[];
  placeholder?: string;
  size?: ControlSize;
  disabled?: boolean;
  invalid?: boolean;
  className?: string;
  /** Required when there is no visible label. */
  'aria-label'?: string;
  id?: string;
  /** Custom rendering of the selected value in the trigger. */
  renderValue?: (option: SelectOption<T> | undefined) => ReactNode;
  /** Monospace trigger text. */
  mono?: boolean;
  /** Minimal trigger without border/background (toolbars). */
  variant?: 'default' | 'ghost';
}

function isGroup<T extends string>(o: SelectOption<T> | SelectGroup<T>): o is SelectGroup<T> {
  return 'options' in o;
}

/** Accessible select (Radix) styled like an input. */
export function Select<T extends string = string>({
  value,
  onValueChange,
  options,
  placeholder = 'Select…',
  size = 'md',
  disabled,
  invalid,
  className,
  id,
  renderValue,
  mono,
  variant = 'default',
  ...aria
}: SelectProps<T>) {
  const flat = options.flatMap((o) => (isGroup(o) ? o.options : [o]));
  const selected = flat.find((o) => o.value === value);

  const renderItem = (o: SelectOption<T>) => {
    const Icon = o.icon;
    return (
      <RadixSelect.Item key={o.value} value={o.value} disabled={o.disabled} textValue={o.textValue ?? (typeof o.label === 'string' ? o.label : o.value)} className={styles.item}>
        {o.leading ?? (Icon && <Icon size={14} strokeWidth={1.75} className={styles.itemIcon} aria-hidden />)}
        <span className={styles.itemText}>
          <RadixSelect.ItemText>{o.label}</RadixSelect.ItemText>
          {o.description && <span className={styles.itemDesc}>{o.description}</span>}
        </span>
        <RadixSelect.ItemIndicator className={styles.indicator}>
          <Check size={14} strokeWidth={2} aria-hidden />
        </RadixSelect.ItemIndicator>
      </RadixSelect.Item>
    );
  };

  const TriggerIcon = selected?.icon;
  return (
    <RadixSelect.Root value={value} onValueChange={(v) => onValueChange(v as T)} disabled={disabled}>
      <RadixSelect.Trigger
        id={id}
        aria-label={aria['aria-label']}
        className={clsx(inputStyles.wrap, inputStyles[size], styles.trigger, variant === 'ghost' && styles.ghost, mono && styles.mono, className)}
        data-invalid={invalid || undefined}
        data-disabled={disabled || undefined}
      >
        <span className={styles.value}>
          {renderValue ? (
            renderValue(selected)
          ) : selected ? (
            <>
              {selected.leading ?? (TriggerIcon && <TriggerIcon size={14} strokeWidth={1.75} className={styles.itemIcon} aria-hidden />)}
              <span className={styles.valueText}>{selected.label}</span>
            </>
          ) : (
            <span className={styles.placeholder}>{placeholder}</span>
          )}
        </span>
        <RadixSelect.Icon className={styles.chevron}>
          <ChevronDown size={14} strokeWidth={1.75} aria-hidden />
        </RadixSelect.Icon>
      </RadixSelect.Trigger>
      <RadixSelect.Portal>
        <RadixSelect.Content className={styles.content} position="popper" sideOffset={4} collisionPadding={8}>
          <RadixSelect.ScrollUpButton className={styles.scrollButton}>
            <ChevronUp size={14} aria-hidden />
          </RadixSelect.ScrollUpButton>
          <RadixSelect.Viewport className={styles.viewport}>
            {options.map((o, i) =>
              isGroup(o) ? (
                <RadixSelect.Group key={`g-${o.label}`}>
                  {i > 0 && <RadixSelect.Separator className={styles.separator} />}
                  <RadixSelect.Label className={styles.groupLabel}>{o.label}</RadixSelect.Label>
                  {o.options.map(renderItem)}
                </RadixSelect.Group>
              ) : (
                renderItem(o)
              ),
            )}
          </RadixSelect.Viewport>
          <RadixSelect.ScrollDownButton className={styles.scrollButton}>
            <ChevronDown size={14} aria-hidden />
          </RadixSelect.ScrollDownButton>
        </RadixSelect.Content>
      </RadixSelect.Portal>
    </RadixSelect.Root>
  );
}
