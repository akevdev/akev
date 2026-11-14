import { ChevronsDownUp, ChevronsUpDown, Copy, FlaskConical, FolderInput, FolderMinus, FolderPlus, Pencil, Plus, ToggleLeft, ToggleRight, Trash2 } from 'lucide-react';
import type { CSSProperties } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import type { MenuItems } from '../../../components/ui';
import type { TemplateId } from '../../../lib/rules/templates';
import { findItem, listGroups, locate, ungroup } from '../../../lib/rules/tree';
import type { RuleActions } from '../hooks/useRuleActions';
import type { RevealFocus } from '../hooks/useRuleNavigation';

export interface ItemMenuContext {
  rules: RuleSet;
  collapsed: ReadonlySet<RuleId>;
  lastTemplate: TemplateId;
  actions: RuleActions;
  toggleCollapse(id: RuleId): void;
  setRenamingId(id: RuleId): void;
  reveal(id: RuleId, focus?: RevealFocus): boolean;
  testRule(id: RuleId): void;
}

/** The row / editor menu for a rule or group (`key: 'edit'` marks the rule's Edit item). */
export function itemMenu(id: RuleId, ctx: ItemMenuContext): MenuItems {
  const { rules: rs, actions } = ctx;
  const item = findItem(rs, id);
  if (!item) return [];
  if (item.type === 'group') {
    const isCollapsed = ctx.collapsed.has(id);
    return [
      { label: 'Rename', icon: Pencil, shortcut: 'f2', onSelect: () => ctx.setRenamingId(id) },
      { label: 'New rule in group', icon: Plus, onSelect: () => actions.createFromTemplate(ctx.lastTemplate, { groupId: id }) },
      { label: item.enabled ? 'Disable group' : 'Enable group', icon: item.enabled ? ToggleLeft : ToggleRight, shortcut: 'space', onSelect: () => actions.toggle(id, !item.enabled) },
      { label: isCollapsed ? 'Expand' : 'Collapse', icon: isCollapsed ? ChevronsUpDown : ChevronsDownUp, onSelect: () => ctx.toggleCollapse(id) },
      { type: 'separator' },
      { label: 'Duplicate group', icon: Copy, shortcut: 'mod+d', onSelect: () => actions.duplicate(id) },
      { label: 'Ungroup', icon: FolderMinus, onSelect: () => actions.mutate('Ungroup', (r) => ungroup(r, id)) },
      { type: 'separator' },
      { label: 'Delete group', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => actions.deleteItem(id) },
    ];
  }
  const parentId = locate(rs.items, id)?.parent?.id ?? null;
  const groups = listGroups(rs);
  const moveTargets: MenuItems = [
    { key: 'root', label: 'Top level', disabled: parentId === null, hint: parentId === null ? 'current' : undefined, onSelect: () => actions.move(id, { type: 'root-end' }, 'drag') },
    ...(groups.length ? [{ type: 'separator' as const, key: 'sep' }] : []),
    ...groups.map(({ group, depth }) => ({
      key: group.id,
      label: <span style={{ paddingLeft: depth * 12 } as CSSProperties}>{group.title || 'Untitled group'}</span>,
      icon: FolderInput,
      disabled: group.id === parentId,
      hint: group.id === parentId ? 'current' : undefined,
      onSelect: () => actions.move(id, { type: 'inside', targetId: group.id, at: 'end' }, 'drag'),
    })),
    { type: 'separator', key: 'sep2' },
    { key: 'new', label: 'New group…', icon: FolderPlus, onSelect: () => actions.moveToNewGroup(id) },
  ];
  return [
    { key: 'edit', label: 'Edit', icon: Pencil, shortcut: 'enter', onSelect: () => ctx.reveal(id, 'editor') },
    { label: 'Duplicate', icon: Copy, shortcut: 'mod+d', onSelect: () => actions.duplicate(id) },
    { type: 'submenu', label: 'Move to group', icon: FolderInput, items: moveTargets },
    { label: 'Test against selected exchange', icon: FlaskConical, onSelect: () => ctx.testRule(id) },
    { label: item.enabled ? 'Disable' : 'Enable', icon: item.enabled ? ToggleLeft : ToggleRight, shortcut: 'space', onSelect: () => actions.toggle(id, !item.enabled) },
    { type: 'separator' },
    { label: 'Delete', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => actions.deleteItem(id) },
  ];
}
