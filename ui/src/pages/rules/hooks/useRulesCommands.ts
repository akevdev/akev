import { ChevronsDownUp, ChevronsUpDown, Download, FolderPlus, RotateCcw, ToggleLeft, ToggleRight, Upload } from 'lucide-react';
import type { RefObject } from 'react';
import type { RuleId } from '../../../api/types';
import { useCommands } from '../../../app/commands';
import { useShortcut, useShortcutDocs } from '../../../app/shortcuts';
import { TEMPLATES, type TemplateId } from '../../../lib/rules/templates';
import { iconFor } from '../icons';
import type { RuleActions } from './useRuleActions';

interface Options {
  actions: RuleActions;
  lastTemplate: TemplateId;
  totalRules: number;
  groupCount: number;
  selectedId: RuleId | null;
  searchRef: RefObject<HTMLInputElement | null>;
  openTemplateMenu(): void;
  resetStats(): void;
  startImport(): void;
  exportRules(): void;
  collapseAll(): void;
  expandAll(): void;
}

/** The Rules page's keyboard shortcuts and palette commands. */
export function useRulesCommands({ actions, lastTemplate, totalRules, groupCount, selectedId, searchRef, openTemplateMenu, resetStats, startImport, exportRules, collapseAll, expandAll }: Options): void {
  const { createFromTemplate, createGroup, setAll, undo, redo, duplicate } = actions;

  useShortcut('n', () => createFromTemplate(lastTemplate), { description: 'New rule (last template)', group: 'Rules' });
  useShortcut('alt+n', openTemplateMenu, { description: 'New rule from a template…', group: 'Rules' });
  useShortcut('shift+n', createGroup, { description: 'New group', group: 'Rules' });
  useShortcut('mod+f', () => searchRef.current?.focus(), { description: 'Search rules', group: 'Rules' });
  useShortcut('/', () => searchRef.current?.focus(), { group: 'Rules' });
  useShortcut('mod+z', undo, { description: 'Undo', group: 'Rules' });
  useShortcut('mod+shift+z', redo, { description: 'Redo', group: 'Rules' });
  useShortcut('mod+y', redo, { group: 'Rules' });
  useShortcut(
    'mod+d',
    () => {
      if (!selectedId) return false;
      duplicate(selectedId);
    },
    { description: 'Duplicate rule', group: 'Rules' },
  );
  useShortcutDocs([
    { combo: 'arrowup or arrowdown', description: 'Select previous / next rule', group: 'Rules' },
    { combo: 'enter', description: 'Edit the selected rule (rename a group)', group: 'Rules' },
    { combo: 'space', description: 'Enable / disable the selected rule', group: 'Rules' },
    { combo: 'alt+arrowup or alt+arrowdown', description: 'Move the selected rule up / down', group: 'Rules' },
    { combo: 'arrowleft or arrowright', description: 'Collapse / expand group', group: 'Rules' },
    { combo: 'delete', description: 'Delete (undoable)', group: 'Rules' },
    { combo: 'escape', description: 'Back from the editor to the list', group: 'Rules' },
  ]);

  useCommands(
    () => [
      ...TEMPLATES.map((t) => ({
        id: `rules.new.${t.id}`,
        title: `New rule: ${t.label}`,
        subtitle: t.description,
        group: 'Rules',
        icon: iconFor(t.icon),
        shortcut: t.id === lastTemplate ? 'n' : undefined,
        keywords: ['rule', 'template', t.section.toLowerCase()],
        run: () => createFromTemplate(t.id),
      })),
      { id: 'rules.new.blank', title: 'New blank rule', group: 'Rules', icon: iconFor('blank'), run: () => createFromTemplate('blank') },
      { id: 'rules.new-group', title: 'New rule group', group: 'Rules', icon: FolderPlus, shortcut: 'shift+n', run: createGroup },
      { id: 'rules.enable-all', title: 'Enable all rules', group: 'Rules', icon: ToggleRight, disabled: totalRules === 0, run: () => setAll(true) },
      { id: 'rules.disable-all', title: 'Disable all rules', group: 'Rules', icon: ToggleLeft, disabled: totalRules === 0, run: () => setAll(false) },
      { id: 'rules.reset-stats', title: 'Reset rule hit counters', group: 'Rules', icon: RotateCcw, run: resetStats },
      { id: 'rules.import', title: 'Import rules…', subtitle: 'Akev JSON or HTTP Toolkit .htkrules', group: 'Rules', icon: Upload, run: () => void startImport() },
      { id: 'rules.export', title: 'Export rules', group: 'Rules', icon: Download, run: () => void exportRules() },
      { id: 'rules.collapse-all', title: 'Collapse all rule groups', group: 'Rules', icon: ChevronsDownUp, disabled: groupCount === 0, run: collapseAll },
      { id: 'rules.expand-all', title: 'Expand all rule groups', group: 'Rules', icon: ChevronsUpDown, disabled: groupCount === 0, run: expandAll },
    ],
    [lastTemplate, totalRules, groupCount, createFromTemplate, createGroup, setAll, resetStats, startImport, exportRules, collapseAll, expandAll],
  );
}
