import { ArrowLeftRight, Plug, Send, Workflow, type LucideIcon } from 'lucide-react';
import type { PageId } from './router';

export interface NavItem {
  page: PageId;
  label: string;
  icon: LucideIcon;
  path: string;
  shortcut: string;
  description: string;
}

/** Primary navigation (DESIGN.md §4). Order defines Mod+1…4. */
export const NAV_ITEMS: readonly NavItem[] = [
  { page: 'traffic', label: 'Traffic', icon: ArrowLeftRight, path: '/traffic', shortcut: 'mod+1', description: 'Inspect captured traffic' },
  { page: 'rules', label: 'Rules', icon: Workflow, path: '/rules', shortcut: 'mod+2', description: 'Mock, rewrite and breakpoint traffic' },
  { page: 'compose', label: 'Compose', icon: Send, path: '/compose', shortcut: 'mod+3', description: 'Send requests' },
  { page: 'connect', label: 'Connect', icon: Plug, path: '/connect', shortcut: 'mod+4', description: 'Connect browsers, apps and devices' },
];
