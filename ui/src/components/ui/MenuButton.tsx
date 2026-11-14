import clsx from 'clsx';
import { ChevronDown, type LucideIcon } from 'lucide-react';
import { useState, type MouseEvent, type ReactElement, type ReactNode } from 'react';
import { DropdownMenu, DropdownMenuContent, DropdownMenuItems, DropdownMenuTrigger } from './DropdownMenu';
import { IconButton } from './IconButton';
import type { MenuItems } from './menuTypes';
import { Tooltip } from './Tooltip';
import styles from './MenuButton.module.css';

type ItemsProp = MenuItems | (() => MenuItems);

function resolve(items: ItemsProp): MenuItems {
  return typeof items === 'function' ? items() : items;
}

/**
 * Opening a dialog from a menu item: defer to the next task so the menu has fully closed (and
 * returned focus) before the dialog traps focus.
 */
export function afterMenuClose(fn: () => void): () => void {
  return () => {
    setTimeout(fn, 0);
  };
}

export interface MenuButtonProps {
  /** Tooltip and accessible name of the trigger. */
  label: string;
  icon?: LucideIcon;
  /** Items, or a factory evaluated each time the menu opens. */
  items: ItemsProp;
  shortcut?: string;
  size?: 'sm' | 'md';
  align?: 'start' | 'center' | 'end';
  /** Show a small chevron next to the icon (menus that read as "▾"). */
  chevron?: boolean;
  /** A custom trigger (e.g. a `Button` with a label) instead of the icon button. */
  trigger?: ReactElement;
  /** Extra content at the top of the menu (e.g. a description). */
  header?: ReactNode;
  disabled?: boolean;
  /** Keep clicks from reaching a clickable ancestor (list rows). */
  isolate?: boolean;
  className?: string;
  menuClassName?: string;
  tabIndex?: number;
  onOpenChange?: (open: boolean) => void;
}

/**
 * Icon button with a tooltip that opens a dropdown menu (the stock `Menu` has no tooltip).
 * Items given as a factory are built only while the menu is open.
 */
export function MenuButton({
  label,
  icon,
  items,
  shortcut,
  size = 'md',
  align = 'end',
  chevron,
  trigger,
  header,
  disabled,
  isolate,
  className,
  menuClassName,
  tabIndex,
  onOpenChange,
}: MenuButtonProps) {
  const [open, setOpen] = useState(false);
  const stop = isolate ? (e: MouseEvent) => e.stopPropagation() : undefined;
  const Icon = icon;
  return (
    <DropdownMenu
      open={open}
      onOpenChange={(o) => {
        setOpen(o);
        onOpenChange?.(o);
      }}
      modal={false}
    >
      <Tooltip content={open ? null : label} shortcut={shortcut}>
        <DropdownMenuTrigger asChild disabled={disabled}>
          {trigger ??
            (chevron && Icon ? (
              <button type="button" className={clsx(styles.chevronTrigger, className)} aria-label={label} disabled={disabled} tabIndex={tabIndex} onClick={stop}>
                <Icon size={16} strokeWidth={1.75} aria-hidden />
                <ChevronDown size={11} strokeWidth={2} aria-hidden className={styles.chevron} />
              </button>
            ) : (
              <IconButton label={label} icon={icon} size={size} tooltip={false} disabled={disabled} className={className} tabIndex={tabIndex} onClick={stop} onDoubleClick={stop} />
            ))}
        </DropdownMenuTrigger>
      </Tooltip>
      <DropdownMenuContent align={align} className={clsx(styles.menu, menuClassName)} onClick={stop}>
        {header}
        {open && <DropdownMenuItems items={resolve(items)} />}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export interface SplitMenuButtonProps {
  /** Primary action label (tooltip + aria-label). */
  label: string;
  icon: LucideIcon;
  shortcut?: string;
  onClick: () => void;
  /** Label for the chevron that opens the menu. */
  menuLabel: string;
  items: ItemsProp;
  disabled?: boolean;
  /** Menu header content (labels, hints). */
  header?: ReactNode;
}

/** Icon button + adjoining chevron menu (e.g. "Copy as cURL" ▾ other targets). */
export function SplitMenuButton({ label, icon: Icon, shortcut, onClick, menuLabel, items, disabled, header }: SplitMenuButtonProps) {
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.split} data-disabled={disabled || undefined}>
      <Tooltip content={label} shortcut={shortcut}>
        <button type="button" className={styles.splitMain} aria-label={label} onClick={onClick} disabled={disabled}>
          <Icon size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </Tooltip>
      <DropdownMenu open={open} onOpenChange={setOpen} modal={false}>
        <Tooltip content={open ? null : menuLabel}>
          <DropdownMenuTrigger asChild disabled={disabled}>
            <button type="button" className={styles.splitChevron} aria-label={menuLabel} disabled={disabled}>
              <ChevronDown size={12} strokeWidth={2} aria-hidden />
            </button>
          </DropdownMenuTrigger>
        </Tooltip>
        <DropdownMenuContent align="end" className={styles.menu}>
          {header}
          {open && <DropdownMenuItems items={resolve(items)} />}
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  );
}
