import type { RefObject } from 'react';
import type { RuleId, RuleSet } from '../../api/types';
import type { MenuItems } from '../../components/ui';
import type { TemplateId } from '../../lib/rules/templates';
import { locate, renameGroup, ungroup, type GroupNode, type RuleNode } from '../../lib/rules/tree';
import { EditorPlaceholder, GroupPanel } from './editor/Panels';
import { RuleEditor, type RuleEditorHandle, type StashedDraft } from './editor/RuleEditor';
import type { RuleActions } from './hooks/useRuleActions';
import type { RevealFocus } from './hooks/useRuleNavigation';
import { isParentChainEnabled } from './itemLabels';
import type { RulesModel } from './useRulesModel';
import styles from './RulesPage.module.css';

export interface RuleEditorPaneProps {
  model: RulesModel;
  actions: RuleActions;
  rules: RuleSet;
  selectedRule: RuleNode | undefined;
  selectedGroup: GroupNode | undefined;
  /** Evaluation order of the selected rule (0-based), when it is a rule. */
  selectedOrder: number | undefined;
  totalRules: number;
  stashes: ReadonlyMap<RuleId, StashedDraft>;
  onStash(id: RuleId, stash: StashedDraft | null): void;
  testNonce(id: RuleId): number;
  lastTemplate: TemplateId;
  menuFor(id: RuleId): MenuItems;
  reveal(id: RuleId, focus?: RevealFocus): boolean;
  focusRow(id: RuleId): void;
  /** The URL names an item that doesn't exist. */
  missing: boolean;
  editorRef: RefObject<RuleEditorHandle | null>;
}

/** Right side of the Rules page: the selected rule's editor, the group panel, or a placeholder. */
export function RuleEditorPane({ model, actions, rules, selectedRule, selectedGroup, selectedOrder, totalRules, stashes, onStash, testNonce, lastTemplate, menuFor, reveal, focusRow, missing, editorRef }: RuleEditorPaneProps) {
  const parentGroup = selectedRule ? (locate(rules.items, selectedRule.id)?.parent ?? null) : null;
  return (
    <section className={styles.editorPane} aria-label="Rule editor">
      {selectedRule && selectedOrder != null ? (
        <RuleEditor
          key={selectedRule.id}
          ref={editorRef}
          rule={selectedRule}
          rules={rules}
          revision={model.revision}
          stashed={stashes.get(selectedRule.id)}
          order={selectedOrder + 1}
          total={totalRules}
          groupTitle={parentGroup ? parentGroup.title || 'Untitled group' : null}
          parentEnabled={isParentChainEnabled(rules, selectedRule.id)}
          serverError={model.serverErrors.get(selectedRule.id)}
          saveState={model.saveState}
          testNonce={testNonce(selectedRule.id)}
          onCommit={actions.commitEdit}
          onStash={onStash}
          onRetrySave={model.retry}
          onToggle={(enabled) => actions.toggle(selectedRule.id, enabled)}
          menu={() => menuFor(selectedRule.id).filter((m) => !(m && 'key' in m && m.key === 'edit'))}
          onDuplicate={() => actions.duplicate(selectedRule.id)}
          onDelete={() => actions.deleteItem(selectedRule.id)}
          onJumpToRule={(id) => reveal(id)}
          onEscape={() => focusRow(selectedRule.id)}
        />
      ) : selectedGroup ? (
        <GroupPanel
          key={selectedGroup.id}
          group={selectedGroup}
          onRename={(title) => actions.mutate('Rename group', (rs) => renameGroup(rs, selectedGroup.id, title), { focus: selectedGroup.id })}
          onToggle={(enabled) => actions.toggle(selectedGroup.id, enabled)}
          onAddRule={() => actions.createFromTemplate(lastTemplate, { groupId: selectedGroup.id })}
          onUngroup={() => actions.mutate('Ungroup', (rs) => ungroup(rs, selectedGroup.id))}
          onDelete={() => actions.deleteItem(selectedGroup.id)}
          onSelectRule={(id) => reveal(id, 'none')}
        />
      ) : (
        <EditorPlaceholder ruleCount={totalRules} onNew={() => actions.createFromTemplate(lastTemplate)} missing={missing} />
      )}
    </section>
  );
}
