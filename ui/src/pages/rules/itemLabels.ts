import type { RuleId, RuleSet } from '../../api/types';
import { ruleLabel } from '../../lib/rules/describe';
import { locate, type GroupNode, type RuleNode } from '../../lib/rules/tree';

/** A rule's title or summary, or a group's title (for toasts and announcements). */
export function itemLabel(item: RuleNode | GroupNode | undefined): string {
  if (!item) return 'item';
  return item.type === 'group' ? item.title || 'Untitled group' : ruleLabel(item);
}

/** {@link itemLabel}, cut to 48 characters. */
export function shortLabel(item: RuleNode | GroupNode | undefined): string {
  const l = itemLabel(item);
  return l.length > 48 ? `${l.slice(0, 47)}…` : l;
}

/** Every group around the item is enabled. */
export function isParentChainEnabled(rules: RuleSet, id: RuleId): boolean {
  const loc = locate(rules.items, id);
  return !loc || loc.ancestors.every((a) => a.enabled);
}
