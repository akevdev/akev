import { useCallback, useEffect, useMemo, useState } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import { allIds } from '../../../lib/rules/tree';
import type { StashedDraft } from '../editor/RuleEditor';

export interface DraftStashes {
  stashes: ReadonlyMap<RuleId, StashedDraft>;
  /** Rules with an invalid draft (shown flagged in the list). */
  invalidDrafts: ReadonlySet<RuleId>;
  onStash(id: RuleId, stash: StashedDraft | null): void;
  drop(id: RuleId): void;
}

/** Invalid editor drafts kept per rule while the user looks at other rules. */
export function useDraftStashes(rules: RuleSet): DraftStashes {
  const [stashes, setStashes] = useState<ReadonlyMap<RuleId, StashedDraft>>(new Map());
  const invalidDrafts = useMemo(() => new Set(stashes.keys()), [stashes]);

  // Drop stashed drafts of rules that no longer exist.
  useEffect(() => {
    if (stashes.size === 0) return;
    const ids = new Set(allIds(rules.items));
    if ([...stashes.keys()].some((id) => !ids.has(id))) setStashes((s) => new Map([...s].filter(([id]) => ids.has(id))));
  }, [rules, stashes]);

  const onStash = useCallback((id: RuleId, stash: StashedDraft | null) => {
    setStashes((s) => {
      if (!stash && !s.has(id)) return s;
      const n = new Map(s);
      if (stash) n.set(id, stash);
      else n.delete(id);
      return n;
    });
  }, []);

  const drop = useCallback((id: RuleId) => onStash(id, null), [onStash]);

  return { stashes, invalidDrafts, onStash, drop };
}
