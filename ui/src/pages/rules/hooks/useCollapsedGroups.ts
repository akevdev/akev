import { useCallback, useMemo } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import { listGroups, locate } from '../../../lib/rules/tree';
import { usePagePref } from '../../../state/ui';

export interface CollapsedGroups {
  collapsed: ReadonlySet<RuleId>;
  toggleCollapse(id: RuleId): void;
  /** Expand one group (e.g. after dropping an item into it). */
  expand(id: RuleId): void;
  /** Expand every group around `id` so its row is visible. */
  expandTo(id: RuleId, rs?: RuleSet): void;
  allGroupIds: RuleId[];
  collapseAll(): void;
  expandAll(): void;
}

/** Which rule groups are collapsed in the list (persisted per page). */
export function useCollapsedGroups(rules: RuleSet, getRules: () => RuleSet): CollapsedGroups {
  const [collapsedPref, setCollapsedPref] = usePagePref<string[]>('rules.collapsed', []);
  const collapsed = useMemo(() => new Set(Array.isArray(collapsedPref) ? collapsedPref : []), [collapsedPref]);

  const toggleCollapse = useCallback(
    (id: RuleId) => {
      setCollapsedPref(collapsed.has(id) ? collapsedPref.filter((c) => c !== id) : [...collapsedPref, id]);
    },
    [collapsed, collapsedPref, setCollapsedPref],
  );

  const expand = useCallback(
    (id: RuleId) => {
      if (collapsed.has(id)) setCollapsedPref(collapsedPref.filter((c) => c !== id));
    },
    [collapsed, collapsedPref, setCollapsedPref],
  );

  const expandTo = useCallback(
    (id: RuleId, rs: RuleSet = getRules()) => {
      const loc = locate(rs.items, id);
      if (!loc) return;
      const ancestorIds = new Set(loc.ancestors.map((a) => a.id));
      if (collapsedPref.some((c) => ancestorIds.has(c))) setCollapsedPref(collapsedPref.filter((c) => !ancestorIds.has(c)));
    },
    [collapsedPref, setCollapsedPref, getRules],
  );

  const allGroupIds = useMemo(() => listGroups(rules).map((g) => g.group.id), [rules]);
  const collapseAll = useCallback(() => setCollapsedPref(allGroupIds), [allGroupIds, setCollapsedPref]);
  const expandAll = useCallback(() => setCollapsedPref([]), [setCollapsedPref]);

  return { collapsed, toggleCollapse, expand, expandTo, allGroupIds, collapseAll, expandAll };
}
