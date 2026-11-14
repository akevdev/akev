import * as RadixMenu from '@radix-ui/react-context-menu';
import clsx from 'clsx';
import { Check, ChevronRight, Circle, type LucideIcon } from 'lucide-react';
import { useState, type ComponentProps, type ReactElement, type ReactNode } from 'react';
import { Kbd } from './Kbd';
import type { MenuItems } from './menuTypes';
import styles from './Menu.module.css';

export const ContextMenuRoot = RadixMenu.Root;
export const ContextMenuTrigger = RadixMenu.Trigger;
export const ContextMenuSub = RadixMenu.Sub;
export const ContextMenuRadioGroup = RadixMenu.RadioGroup;

/** Styled, portalled menu surface. */
export function ContextMenuContent({ className, collisionPadding = 8, ...rest }: ComponentProps<typeof RadixMenu.Content>) {
  return (
    <RadixMenu.Portal>
      <RadixMenu.Content collisionPadding={collisionPadding} className={clsx(styles.content, className)} {...rest} />
    </RadixMenu.Portal>
  );
}

interface ItemExtras {
  icon?: LucideIcon;
  shortcut?: string;
  hint?: string;
  destructive?: boolean;
}

/** Menu item with optional icon, shortcut hint and destructive style. */
export function ContextMenuItem({ icon: Icon, shortcut, hint, destructive, className, children, ...rest }: ComponentProps<typeof RadixMenu.Item> & ItemExtras) {
  return (
    <RadixMenu.Item className={clsx(styles.item, destructive && styles.destructive, className)} {...rest}>
      {Icon ? <Icon size={14} strokeWidth={1.75} className={styles.icon} aria-hidden /> : null}
      <span className={styles.label}>{children}</span>
      {shortcut ? <Kbd combo={shortcut} size="sm" variant="subtle" className={styles.shortcut} /> : hint ? <span className={styles.hint}>{hint}</span> : null}
    </RadixMenu.Item>
  );
}

export function ContextMenuCheckboxItem({ className, children, shortcut, ...rest }: ComponentProps<typeof RadixMenu.CheckboxItem> & { shortcut?: string }) {
  return (
    <RadixMenu.CheckboxItem className={clsx(styles.item, className)} {...rest}>
      <span className={styles.check}>
        <RadixMenu.ItemIndicator>
          <Check size={13} strokeWidth={2.25} aria-hidden />
        </RadixMenu.ItemIndicator>
      </span>
      <span className={styles.label}>{children}</span>
      {shortcut && <Kbd combo={shortcut} size="sm" variant="subtle" className={styles.shortcut} />}
    </RadixMenu.CheckboxItem>
  );
}

export function ContextMenuRadioItem({ className, children, ...rest }: ComponentProps<typeof RadixMenu.RadioItem>) {
  return (
    <RadixMenu.RadioItem className={clsx(styles.item, className)} {...rest}>
      <span className={styles.check}>
        <RadixMenu.ItemIndicator>
          <Circle size={7} fill="currentColor" strokeWidth={0} aria-hidden />
        </RadixMenu.ItemIndicator>
      </span>
      <span className={styles.label}>{children}</span>
    </RadixMenu.RadioItem>
  );
}

export function ContextMenuSubTrigger({ icon: Icon, className, children, ...rest }: ComponentProps<typeof RadixMenu.SubTrigger> & { icon?: LucideIcon }) {
  return (
    <RadixMenu.SubTrigger className={clsx(styles.item, className)} {...rest}>
      {Icon && <Icon size={14} strokeWidth={1.75} className={styles.icon} aria-hidden />}
      <span className={styles.label}>{children}</span>
      <ChevronRight size={14} className={styles.chevron} aria-hidden />
    </RadixMenu.SubTrigger>
  );
}

export function ContextMenuSubContent({ className, ...rest }: ComponentProps<typeof RadixMenu.SubContent>) {
  return (
    <RadixMenu.Portal>
      <RadixMenu.SubContent sideOffset={6} alignOffset={-5} collisionPadding={8} className={clsx(styles.content, className)} {...rest} />
    </RadixMenu.Portal>
  );
}

export function ContextMenuSeparator({ className, ...rest }: ComponentProps<typeof RadixMenu.Separator>) {
  return <RadixMenu.Separator className={clsx(styles.separator, className)} {...rest} />;
}

export function ContextMenuLabel({ className, ...rest }: ComponentProps<typeof RadixMenu.Label>) {
  return <RadixMenu.Label className={clsx(styles.groupLabel, className)} {...rest} />;
}

/** Render data-driven items inside a `ContextMenuContent`. */
export function ContextMenuItems({ items }: { items: MenuItems }): ReactNode {
  return items.map((spec, i) => {
    if (!spec) return null;
    const key = spec.key ?? String(i);
    switch (spec.type) {
      case 'separator':
        return <ContextMenuSeparator key={key} />;
      case 'label':
        return <ContextMenuLabel key={key}>{spec.label}</ContextMenuLabel>;
      case 'checkbox':
        return (
          <ContextMenuCheckboxItem key={key} checked={spec.checked} onCheckedChange={spec.onCheckedChange} disabled={spec.disabled} shortcut={spec.shortcut} onSelect={(e) => e.preventDefault()}>
            {spec.label}
          </ContextMenuCheckboxItem>
        );
      case 'radio':
        return (
          <ContextMenuRadioGroup key={key} value={spec.value} onValueChange={spec.onValueChange}>
            {spec.options.map((o) => (
              <ContextMenuRadioItem key={o.value} value={o.value} disabled={o.disabled}>
                {o.label}
              </ContextMenuRadioItem>
            ))}
          </ContextMenuRadioGroup>
        );
      case 'submenu':
        return (
          <ContextMenuSub key={key}>
            <ContextMenuSubTrigger icon={spec.icon} disabled={spec.disabled}>
              {spec.label}
            </ContextMenuSubTrigger>
            <ContextMenuSubContent>
              <ContextMenuItems items={spec.items} />
            </ContextMenuSubContent>
          </ContextMenuSub>
        );
      default:
        return (
          <ContextMenuItem key={key} icon={spec.icon} shortcut={spec.shortcut} hint={spec.hint} destructive={spec.destructive} disabled={spec.disabled} onSelect={spec.onSelect}>
            {spec.label}
          </ContextMenuItem>
        );
    }
  });
}

export interface ContextMenuProps {
  /** The area that opens the menu on right-click / long-press / context-menu key. */
  children: ReactElement;
  /**
   * Items, or a function evaluated when the menu opens (so items reflect the state at
   * right-click time, e.g. the clicked row).
   */
  items: MenuItems | (() => MenuItems);
  onOpenChange?: (open: boolean) => void;
  disabled?: boolean;
  className?: string;
}

/**
 * Right-click menu for an area:
 * `<ContextMenu items={() => rowMenu(id)}><div>…</div></ContextMenu>`.
 */
export function ContextMenu({ children, items, onOpenChange, disabled, className }: ContextMenuProps) {
  const [resolved, setResolved] = useState<MenuItems>([]);
  return (
    <ContextMenuRoot
      onOpenChange={(open) => {
        if (open) setResolved(typeof items === 'function' ? items() : items);
        onOpenChange?.(open);
      }}
      modal={false}
    >
      <ContextMenuTrigger asChild disabled={disabled}>
        {children}
      </ContextMenuTrigger>
      <ContextMenuContent className={className}>
        <ContextMenuItems items={typeof items === 'function' ? resolved : items} />
      </ContextMenuContent>
    </ContextMenuRoot>
  );
}
