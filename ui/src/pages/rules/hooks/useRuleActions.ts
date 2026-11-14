import { useCallback, type RefObject } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import { toast, undoToast } from '../../../components/ui';
import { pluralize } from '../../../lib/format';
import type { HistoryEntry } from '../../../lib/rules/history';
import { BLANK_TEMPLATE, templateById, TEMPLATES, type TemplateId } from '../../../lib/rules/templates';
import {
  countRules,
  duplicateItem,
  findItem,
  insertAt,
  insertItem,
  moveItem,
  newId,
  removeItem,
  renameGroup,
  replaceRule,
  setAllEnabled,
  setItemEnabled,
  type DropPosition,
  type FlatRow,
  type GroupNode,
  type RuleNode,
} from '../../../lib/rules/tree';
import type { RuleEditorHandle } from '../editor/RuleEditor';
import type { RuleTreeHandle } from '../list/RuleTree';
import type { RulesModel } from '../useRulesModel';
import type { CollapsedGroups } from './useCollapsedGroups';
import type { RuleNavigation } from './useRuleNavigation';
import { shortLabel } from '../itemLabels';

/** Keyboard focus is in the rule list (so selection changes should move focus along). */
function focusInTree(): boolean {
  return (document.activeElement as HTMLElement | null)?.closest('[role="tree"]') != null;
}

interface Options {
  model: RulesModel;
  nav: RuleNavigation;
  groups: CollapsedGroups;
  rows: readonly FlatRow[];
  selectedId: RuleId | null;
  searching: boolean;
  clearSearch(): void;
  onTemplateUsed(id: TemplateId): void;
  /** A rule is gone: forget its stashed draft. */
  onRemoved(id: RuleId): void;
  setRenamingId(id: RuleId | null): void;
  editorRef: RefObject<RuleEditorHandle | null>;
  treeRef: RefObject<RuleTreeHandle | null>;
}

export type Mutate = (label: string, fn: (cur: RuleSet) => RuleSet | null, opts?: { focus?: RuleId | null; key?: string }) => HistoryEntry<RuleSet> | null;

/** Every change the Rules page makes to the rule set, each one an undoable history step. */
export function useRuleActions({ model, nav, groups, rows, selectedId, searching, clearSearch, onTemplateUsed, onRemoved, setRenamingId, editorRef, treeRef }: Options) {
  const { commit, getRules } = model;
  const { select, reveal, scrollTo, announce } = nav;
  const { expand, expandTo } = groups;

  const mutate = useCallback<Mutate>(
    (label, fn, opts = {}) => {
      editorRef.current?.flush();
      const cur = getRules();
      const next = fn(cur);
      if (!next || next === cur) return null;
      return commit(next, { label, origin: 'other', ...opts });
    },
    [commit, getRules, editorRef],
  );

  const createFromTemplate = useCallback(
    (id: TemplateId, opts: { groupId?: RuleId } = {}) => {
      const tpl = templateById(id) ?? TEMPLATES[0]!;
      const rule = tpl.build();
      const cur = getRules();
      const sel = selectedId ? findItem(cur, selectedId) : undefined;
      const groupId = opts.groupId ?? (sel?.type === 'group' ? sel.id : undefined);
      const pos: DropPosition = groupId ? { type: 'inside', targetId: groupId, at: 'start' } : cur.items[0] ? { type: 'before', targetId: cur.items[0].id } : { type: 'root-end' };
      const entry = mutate(`New ${tpl.label.toLowerCase()} rule`, (rs) => insertItem(rs, rule, pos), { focus: rule.id });
      if (!entry) return;
      if (tpl.id !== BLANK_TEMPLATE.id) onTemplateUsed(tpl.id);
      if (searching) clearSearch();
      if (groupId) expandTo(rule.id, entry.after);
      select(rule.id);
      scrollTo(rule.id, 'editor');
      announce(`Created ${tpl.label} rule`);
    },
    [announce, clearSearch, expandTo, getRules, mutate, onTemplateUsed, scrollTo, searching, select, selectedId],
  );

  const createGroup = useCallback(() => {
    const group: GroupNode = { type: 'group', id: newId(), title: 'New group', enabled: true, collapsed: false, items: [] };
    const entry = mutate('New group', (rs) => insertItem(rs, group, rs.items[0] ? { type: 'before', targetId: rs.items[0].id } : { type: 'root-end' }), { focus: group.id });
    if (!entry) return;
    if (searching) clearSearch();
    select(group.id);
    setRenamingId(group.id);
    scrollTo(group.id, 'none');
  }, [clearSearch, mutate, scrollTo, searching, select, setRenamingId]);

  const toggle = useCallback(
    (id: RuleId, enabled: boolean) => {
      const item = findItem(getRules(), id);
      mutate(`${enabled ? 'Enable' : 'Disable'} ${item?.type === 'group' ? 'group' : 'rule'}`, (rs) => setItemEnabled(rs, id, enabled), { focus: id });
    },
    [getRules, mutate],
  );

  const neighbourAfterRemoval = useCallback(
    (id: RuleId): RuleId | null => {
      const i = rows.findIndex((r) => r.id === id);
      if (i < 0) return null;
      const depth = rows[i]!.depth;
      for (let j = i + 1; j < rows.length; j++) if (rows[j]!.depth <= depth) return rows[j]!.id;
      for (let j = i - 1; j >= 0; j--) if (rows[j]!.id !== id) return rows[j]!.id;
      return null;
    },
    [rows],
  );

  const deleteItem = useCallback(
    (id: RuleId) => {
      const cur = getRules();
      const item = findItem(cur, id);
      if (!item) return;
      const removed = removeItem(cur, id);
      if (!removed) return;
      const next = neighbourAfterRemoval(id);
      const kind = item.type === 'group' ? 'group' : 'rule';
      const entry = mutate(`Delete ${kind}`, () => removed.set, { focus: id });
      if (!entry) return;
      onRemoved(id);
      if (selectedId === id || (item.type === 'group' && selectedId && !findItem(removed.set, selectedId))) select(next);
      if (next) scrollTo(next, 'row');
      const count = item.type === 'group' ? countRules(item.items) : 0;
      undoToast(item.type === 'group' ? `Deleted group “${shortLabel(item)}”${count ? ` and ${pluralize(count, 'rule')}` : ''}` : `Deleted “${shortLabel(item)}”`, () => {
        if (!model.undoEntry(entry.id)) {
          const rs = getRules();
          const parentOk = removed.parentId === null || findItem(rs, removed.parentId)?.type === 'group';
          mutate(`Restore ${kind}`, (r) => insertAt(r, removed.item, parentOk ? removed.parentId : null, removed.index), { focus: id });
        }
        reveal(id);
      });
    },
    [getRules, model, mutate, neighbourAfterRemoval, onRemoved, reveal, scrollTo, select, selectedId],
  );

  const duplicate = useCallback(
    (id: RuleId) => {
      let copyId: RuleId | null = null;
      const entry = mutate('Duplicate', (rs) => {
        const d = duplicateItem(rs, id);
        copyId = d?.id ?? null;
        return d?.set ?? null;
      });
      if (entry && copyId) {
        select(copyId);
        scrollTo(copyId, 'row');
        announce('Duplicated');
      }
    },
    [announce, mutate, scrollTo, select],
  );

  const move = useCallback(
    (id: RuleId, pos: DropPosition, how: 'drag' | 'keyboard') => {
      const item = findItem(getRules(), id);
      const entry = mutate(`Move ${item?.type === 'group' ? 'group' : 'rule'}`, (rs) => moveItem(rs, id, pos), { focus: id, key: how === 'keyboard' ? `move:${id}` : undefined });
      if (!entry) return;
      if (pos.type === 'inside' && groups.collapsed.has(pos.targetId)) expand(pos.targetId);
      else expandTo(id, entry.after);
      select(id);
      if (how === 'drag') scrollTo(id, 'row');
    },
    [expand, expandTo, getRules, groups.collapsed, mutate, scrollTo, select],
  );

  const rename = useCallback(
    (id: RuleId, title: string) => {
      setRenamingId(null);
      mutate('Rename group', (rs) => renameGroup(rs, id, title), { focus: id });
      treeRef.current?.focusRow(id);
    },
    [mutate, setRenamingId, treeRef],
  );

  const moveToNewGroup = useCallback(
    (id: RuleId) => {
      const group: GroupNode = { type: 'group', id: newId(), title: 'New group', enabled: true, collapsed: false, items: [] };
      const entry = mutate('Move to new group', (rs) => {
        const withGroup = insertItem(rs, group, { type: 'before', targetId: id });
        return withGroup ? moveItem(withGroup, id, { type: 'inside', targetId: group.id, at: 'end' }) : null;
      });
      if (entry) {
        select(group.id);
        setRenamingId(group.id);
        scrollTo(group.id, 'none');
      }
    },
    [mutate, scrollTo, select, setRenamingId],
  );

  const setAll = useCallback(
    (enabled: boolean) => {
      const entry = mutate(enabled ? 'Enable all rules' : 'Disable all rules', (rs) => setAllEnabled(rs, enabled));
      if (entry) undoToast(enabled ? 'Enabled every rule' : 'Disabled every rule', () => model.undoEntry(entry.id));
      else toast(enabled ? 'Every rule is already enabled' : 'Every rule is already disabled');
    },
    [model, mutate],
  );

  const undo = useCallback(() => {
    editorRef.current?.flush();
    const entry = model.undo();
    if (!entry) {
      announce('Nothing to undo');
      return;
    }
    toast(`Undid: ${entry.label}`, { duration: 1800, id: 'rules-undo' });
    if (entry.focus && findItem(entry.before, entry.focus)) reveal(entry.focus, focusInTree() ? 'row' : 'none');
    else if (selectedId && !findItem(entry.before, selectedId)) select(null);
  }, [announce, editorRef, model, reveal, select, selectedId]);

  const redo = useCallback(() => {
    editorRef.current?.flush();
    const entry = model.redo();
    if (!entry) {
      announce('Nothing to redo');
      return;
    }
    toast(`Redid: ${entry.label}`, { duration: 1800, id: 'rules-undo' });
    if (entry.focus && findItem(entry.after, entry.focus)) reveal(entry.focus, focusInTree() ? 'row' : 'none');
    else if (selectedId && !findItem(entry.after, selectedId)) select(null);
  }, [announce, editorRef, model, reveal, select, selectedId]);

  /** The open editor's change (coalesced per rule into one undo step). */
  const commitEdit = useCallback(
    (rule: RuleNode) => {
      const cur = getRules();
      if (!findItem(cur, rule.id)) return;
      commit(replaceRule(cur, rule), { label: 'Edit rule', key: `edit:${rule.id}`, focus: rule.id, origin: 'editor' });
    },
    [commit, getRules],
  );

  return { mutate, createFromTemplate, createGroup, toggle, deleteItem, duplicate, move, rename, moveToNewGroup, setAll, undo, redo, commitEdit };
}

export type RuleActions = ReturnType<typeof useRuleActions>;
