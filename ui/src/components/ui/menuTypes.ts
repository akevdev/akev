import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';

/** Data-driven menu item (used by `Menu`, `DropdownMenuItems`, `ContextMenuArea`...). */
export type MenuItemSpec =
  | {
      type?: 'item';
      /** Stable key (defaults to the index). */
      key?: string;
      label: ReactNode;
      icon?: LucideIcon;
      /** Display-only shortcut (`mod+c`). */
      shortcut?: string;
      /** Muted text on the right (when there is no shortcut). */
      hint?: string;
      onSelect?: (event: Event) => void;
      disabled?: boolean;
      /** Red, for delete-like actions. */
      destructive?: boolean;
    }
  | {
      type: 'checkbox';
      key?: string;
      label: ReactNode;
      icon?: LucideIcon;
      checked: boolean;
      onCheckedChange: (checked: boolean) => void;
      shortcut?: string;
      disabled?: boolean;
    }
  | {
      type: 'radio';
      key?: string;
      value: string;
      onValueChange: (value: string) => void;
      options: readonly { value: string; label: ReactNode; icon?: LucideIcon; disabled?: boolean }[];
    }
  | { type: 'submenu'; key?: string; label: ReactNode; icon?: LucideIcon; items: MenuItems; disabled?: boolean }
  | { type: 'separator'; key?: string }
  | { type: 'label'; key?: string; label: ReactNode };

/** Falsy entries are skipped, so items can be conditional: `[canPin && {...}]`. */
export type MenuItems = readonly (MenuItemSpec | false | null | undefined)[];
