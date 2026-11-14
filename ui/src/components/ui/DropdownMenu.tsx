import * as RadixMenu from '@radix-ui/react-dropdown-menu';
import clsx from 'clsx';
import { Check, ChevronRight, Circle, type LucideIcon } from 'lucide-react';
import type { ComponentProps, ReactElement, ReactNode } from 'react';
import { Kbd } from './Kbd';
import type { MenuItems } from './menuTypes';
import styles from './Menu.module.css';

export const DropdownMenu = RadixMenu.Root;
export const DropdownMenuTrigger = RadixMenu.Trigger;
export const DropdownMenuGroup = RadixMenu.Group;
export const DropdownMenuSub = RadixMenu.Sub;
export const DropdownMenuRadioGroup = RadixMenu.RadioGroup;

/** Styled, portalled menu surface. */
export function DropdownMenuContent({ className, sideOffset = 4, collisionPadding = 8, ...rest }: ComponentProps<typeof RadixMenu.Content>) {
  return (
    <RadixMenu.Portal>
      <RadixMenu.Content sideOffset={sideOffset} collisionPadding={collisionPadding} className={clsx(styles.content, className)} {...rest} />
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
export function DropdownMenuItem({ icon: Icon, shortcut, hint, destructive, className, children, ...rest }: ComponentProps<typeof RadixMenu.Item> & ItemExtras) {
  return (
    <RadixMenu.Item className={clsx(styles.item, destructive && styles.destructive, className)} {...rest}>
      {Icon ? <Icon size={14} strokeWidth={1.75} className={styles.icon} aria-hidden /> : null}
      <span className={styles.label}>{children}</span>
      {shortcut ? <Kbd combo={shortcut} size="sm" variant="subtle" className={styles.shortcut} /> : hint ? <span className={styles.hint}>{hint}</span> : null}
    </RadixMenu.Item>
  );
}

export function DropdownMenuCheckboxItem({ className, children, shortcut, ...rest }: ComponentProps<typeof RadixMenu.CheckboxItem> & { shortcut?: string }) {
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

export function DropdownMenuRadioItem({ className, children, ...rest }: ComponentProps<typeof RadixMenu.RadioItem>) {
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

export function DropdownMenuSubTrigger({ icon: Icon, className, children, ...rest }: ComponentProps<typeof RadixMenu.SubTrigger> & { icon?: LucideIcon }) {
  return (
    <RadixMenu.SubTrigger className={clsx(styles.item, className)} {...rest}>
      {Icon && <Icon size={14} strokeWidth={1.75} className={styles.icon} aria-hidden />}
      <span className={styles.label}>{children}</span>
      <ChevronRight size={14} className={styles.chevron} aria-hidden />
    </RadixMenu.SubTrigger>
  );
}

export function DropdownMenuSubContent({ className, ...rest }: ComponentProps<typeof RadixMenu.SubContent>) {
  return (
    <RadixMenu.Portal>
      <RadixMenu.SubContent sideOffset={6} alignOffset={-5} collisionPadding={8} className={clsx(styles.content, className)} {...rest} />
    </RadixMenu.Portal>
  );
}

export function DropdownMenuSeparator({ className, ...rest }: ComponentProps<typeof RadixMenu.Separator>) {
  return <RadixMenu.Separator className={clsx(styles.separator, className)} {...rest} />;
}

export function DropdownMenuLabel({ className, ...rest }: ComponentProps<typeof RadixMenu.Label>) {
  return <RadixMenu.Label className={clsx(styles.groupLabel, className)} {...rest} />;
}

/** Render data-driven items inside a `DropdownMenuContent`. */
export function DropdownMenuItems({ items }: { items: MenuItems }): ReactNode {
  return items.map((spec, i) => {
    if (!spec) return null;
    const key = spec.key ?? String(i);
    switch (spec.type) {
      case 'separator':
        return <DropdownMenuSeparator key={key} />;
      case 'label':
        return <DropdownMenuLabel key={key}>{spec.label}</DropdownMenuLabel>;
      case 'checkbox':
        return (
          <DropdownMenuCheckboxItem key={key} checked={spec.checked} onCheckedChange={spec.onCheckedChange} disabled={spec.disabled} shortcut={spec.shortcut} onSelect={(e) => e.preventDefault()}>
            {spec.label}
          </DropdownMenuCheckboxItem>
        );
      case 'radio':
        return (
          <DropdownMenuRadioGroup key={key} value={spec.value} onValueChange={spec.onValueChange}>
            {spec.options.map((o) => (
              <DropdownMenuRadioItem key={o.value} value={o.value} disabled={o.disabled}>
                {o.label}
              </DropdownMenuRadioItem>
            ))}
          </DropdownMenuRadioGroup>
        );
      case 'submenu':
        return (
          <DropdownMenuSub key={key}>
            <DropdownMenuSubTrigger icon={spec.icon} disabled={spec.disabled}>
              {spec.label}
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent>
              <DropdownMenuItems items={spec.items} />
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        );
      default:
        return (
          <DropdownMenuItem key={key} icon={spec.icon} shortcut={spec.shortcut} hint={spec.hint} destructive={spec.destructive} disabled={spec.disabled} onSelect={spec.onSelect}>
            {spec.label}
          </DropdownMenuItem>
        );
    }
  });
}

export interface MenuProps {
  /** The trigger element (e.g. an IconButton with `tooltip={false}` or a Button). */
  trigger: ReactElement;
  items: MenuItems;
  align?: 'start' | 'center' | 'end';
  side?: 'top' | 'right' | 'bottom' | 'left';
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  className?: string;
}

/** Convenience dropdown: `<Menu trigger={<Button iconRight={ChevronDown}>Export</Button>} items={[…]} />`. */
export function Menu({ trigger, items, align = 'start', side = 'bottom', open, onOpenChange, className }: MenuProps) {
  return (
    <DropdownMenu open={open} onOpenChange={onOpenChange} modal={false}>
      <DropdownMenuTrigger asChild>{trigger}</DropdownMenuTrigger>
      <DropdownMenuContent align={align} side={side} className={className}>
        <DropdownMenuItems items={items} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
