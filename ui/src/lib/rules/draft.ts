/**
 * Draft bookkeeping for the rule editor. The editor edits a local copy (the *draft*) of the
 * selected rule; valid drafts are applied, invalid ones stay local. These helpers decide how a
 * draft reacts when the rule changes underneath it (undo/redo, the list's enable switch, other
 * windows) and when the user comes back to a rule they left with errors.
 */

import type { Rule } from '../../api/types';
import { deepEqual } from './tree';

/** An invalid draft kept while the user looks at other rules. */
export interface Stash<R extends Rule = Rule> {
  /** The user's unsaved (invalid) edits. */
  draft: R;
  /** The rule as it was when the draft was stashed. */
  base: R;
}

/**
 * Starting draft when (re)opening `rule`: the stashed draft if the rule hasn't changed since
 * (an `enabled` flip elsewhere is carried over), otherwise the rule itself.
 */
export function rebaseDraft<R extends Rule>(stash: Stash<R> | undefined, rule: R): R {
  if (!stash) return rule;
  if (deepEqual(stash.base, rule)) return stash.draft;
  if (deepEqual({ ...stash.base, enabled: rule.enabled }, rule)) return { ...stash.draft, enabled: rule.enabled };
  return rule;
}

/**
 * The draft after the rule changed from outside the editor (`base` → `rule`):
 * * no local edits → follow the rule;
 * * only `enabled` changed elsewhere → keep the edits, take the new `enabled`;
 * * anything else (undo/redo, another window) → the outside change wins.
 */
export function mergeExternalChange<R extends Rule>(draft: R, base: R, rule: R): R {
  if (deepEqual(draft, base)) return rule;
  if (deepEqual({ ...base, enabled: rule.enabled }, rule)) return deepEqual(draft, { ...draft, enabled: rule.enabled }) ? draft : { ...draft, enabled: rule.enabled };
  return rule;
}

/** Drop empty optional fields so the saved JSON stays minimal (`maxMatches`, `note`, `title`). */
export function normalizeRule<R extends Rule>(r: R): R {
  const out = { ...r };
  if (out.maxMatches == null) delete out.maxMatches;
  if (!out.note) delete out.note;
  if (out.title != null && out.title.trim() === '') delete out.title;
  return out;
}
