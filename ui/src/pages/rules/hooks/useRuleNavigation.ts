import { useCallback, useEffect, useRef, useState, type RefObject } from 'react';
import type { RuleId, RuleSet } from '../../../api/types';
import { navigate } from '../../../app/router';
import { ruleMatchesQuery } from '../../../lib/rules/stats';
import { findItem, type FlatRow } from '../../../lib/rules/tree';
import { takeRevealRule, useIntents } from '../../../state/intents';
import type { RuleEditorHandle } from '../editor/RuleEditor';
import type { RuleTreeHandle } from '../list/RuleTree';

export type RevealFocus = 'row' | 'editor' | 'none';

interface Options {
  loaded: boolean;
  rules: RuleSet;
  rows: readonly FlatRow[];
  selectedId: RuleId | null;
  getRules(): RuleSet;
  query: string;
  setQuery(query: string): void;
  expandTo(id: RuleId, rs?: RuleSet): void;
  treeRef: RefObject<RuleTreeHandle | null>;
  editorRef: RefObject<RuleEditorHandle | null>;
}

export interface RuleNavigation {
  /** Select an item (the URL is the selection: `#/rules/<id>`). */
  select(id: RuleId | null): void;
  /** Select an item, expand and scroll to it (clearing a search that hides it); false if it doesn't exist. */
  reveal(id: RuleId, focus?: RevealFocus): boolean;
  /** Scroll to an item after the next render, optionally focusing its row or the editor. */
  scrollTo(id: RuleId, focus: RevealFocus): void;
  /** Screen-reader announcement (polite live region). */
  announcement: string;
  announce(message: string): void;
  /** A reveal intent is waiting for its rule to arrive. */
  revealPending(): boolean;
}

/** Selection, scrolling, reveal intents from other pages and deep links for the Rules page. */
export function useRuleNavigation({ loaded, rules, rows, selectedId, getRules, query, setQuery, expandTo, treeRef, editorRef }: Options): RuleNavigation {
  const searching = query.trim().length > 0;

  const select = useCallback((id: RuleId | null) => {
    navigate(id ? `/rules/${encodeURIComponent(id)}` : '/rules', { replace: true });
  }, []);

  const [announcement, setAnnouncement] = useState('');
  const announce = useCallback((msg: string) => {
    setAnnouncement('');
    requestAnimationFrame(() => setAnnouncement(msg));
  }, []);

  const [scrollTarget, setScrollTarget] = useState<{ id: RuleId; focus: RevealFocus } | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: re-runs when the rows change so the target row exists
  useEffect(() => {
    if (!scrollTarget) return;
    const raf = requestAnimationFrame(() => {
      treeRef.current?.scrollToId(scrollTarget.id);
      if (scrollTarget.focus === 'row') treeRef.current?.focusRow(scrollTarget.id);
      if (scrollTarget.focus === 'editor') requestAnimationFrame(() => editorRef.current?.focusFirstField());
      setScrollTarget(null);
    });
    return () => cancelAnimationFrame(raf);
  }, [scrollTarget, rows, treeRef, editorRef]);

  const scrollTo = useCallback((id: RuleId, focus: RevealFocus) => setScrollTarget({ id, focus }), []);

  const reveal = useCallback(
    (id: RuleId, focus: RevealFocus = 'none') => {
      const rs = getRules();
      const item = findItem(rs, id);
      if (!item) return false;
      expandTo(id, rs);
      if (searching && item.type === 'rule' && !ruleMatchesQuery(item, query)) setQuery('');
      select(id);
      setScrollTarget({ id, focus });
      return true;
    },
    [expandTo, getRules, query, searching, select, setQuery],
  );

  // Intents: "reveal this rule" from other pages (Traffic → create rule, rule chips…).
  const revealId = useIntents((s) => s.revealRuleId);
  const pendingReveal = useRef<RuleId | null>(null);
  // biome-ignore lint/correctness/useExhaustiveDependencies: retried when the intent changes or new rules arrive
  useEffect(() => {
    if (!loaded) return;
    const id = takeRevealRule() ?? pendingReveal.current;
    if (!id) return;
    pendingReveal.current = reveal(id) ? null : id;
  }, [revealId, loaded, rules, reveal]);

  // Deep link (#/rules/<id>) on first load: make sure the row is visible.
  const deepLinked = useRef(false);
  useEffect(() => {
    if (deepLinked.current || !loaded || !selectedId) return;
    deepLinked.current = true;
    expandTo(selectedId);
    setScrollTarget({ id: selectedId, focus: 'none' });
  }, [loaded, selectedId, expandTo]);

  const revealPending = useCallback(() => pendingReveal.current != null, []);

  return { select, reveal, scrollTo, announcement, announce, revealPending };
}
