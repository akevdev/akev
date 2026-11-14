import { useCallback, useEffect, useMemo, useRef, useState, useSyncExternalStore } from 'react';
import { errorMessage } from '../../api/client';
import type { RuleId, RuleSet } from '../../api/types';
import { toast } from '../../components/ui';
import { pluralize } from '../../lib/format';
import { History, type HistoryEntry } from '../../lib/rules/history';
import { mergeRuleSets, rebaseChange } from '../../lib/rules/merge';
import { deepEqual, emptyRuleSet, reconcile } from '../../lib/rules/tree';
import { useAppStore } from '../../state/app';
import { useLicenseStore } from '../../state/license';

export type SaveState = 'idle' | 'saving' | 'error';

export interface CommitOptions {
  /** History label (`Delete rule`). */
  label: string;
  /** Coalescing key: consecutive commits with the same key merge into one undo step. */
  key?: string;
  /** Item to select when this step is undone/redone. */
  focus?: RuleId | null;
  /**
   * Who made the change. Changes not made by the open editor bump `revision` so the editor
   * re-reads the rule (e.g. the list's enable switch while the rule is open).
   */
  origin?: 'editor' | 'other';
}

export interface RulesModel {
  /** The rule set to display (local truth while saves are in flight). */
  rules: RuleSet;
  /** Rules have been received from the daemon. */
  loaded: boolean;
  saveState: SaveState;
  /** Per-rule errors from the daemon's last accepted save. */
  serverErrors: ReadonlyMap<RuleId, string>;
  /** Bumps whenever the rule set changes for a reason other than the open editor. */
  revision: number;
  commit(next: RuleSet, options: CommitOptions): HistoryEntry<RuleSet> | null;
  undo(): HistoryEntry<RuleSet> | null;
  redo(): HistoryEntry<RuleSet> | null;
  /** Undo a specific entry if it's still the latest; returns false otherwise. */
  undoEntry(id: number): boolean;
  history: History<RuleSet>;
  /** Re-send the current rules (after a failed save). */
  retry(): void;
  /** The latest rule set, including commits made earlier in the same event. */
  getRules(): RuleSet;
}

/**
 * The Rules page's data controller.
 *
 * Saves are serialised (one `PUT /api/rules` in flight; newer states coalesce into the next
 * one) and the page renders its own local copy while any save is pending, so the daemon's
 * `rules` broadcast echoing an *older* save can never make the UI flicker backwards. Once the
 * queue drains, the page follows the store again (and picks up changes made elsewhere, e.g.
 * via MCP or another window).
 *
 * `PUT /api/rules` replaces the whole set, so nothing here ever sends a stale snapshot: undo
 * and redo re-apply their step on top of the current rules (`rebaseChange`), and local edits
 * made while a save is in flight are rebased onto rule changes broadcast in the meantime.
 */
export function useRulesModel(): RulesModel {
  const storeRules = useAppStore((s) => s.rules);
  const saveRules = useAppStore((s) => s.saveRules);
  const [local, setLocal] = useState<RuleSet | null>(null);
  const [saveState, setSaveState] = useState<SaveState>('idle');
  const [serverErrors, setServerErrors] = useState<ReadonlyMap<RuleId, string>>(new Map());
  const [revision, setRevision] = useState(0);

  const historyRef = useRef<History<RuleSet> | null>(null);
  if (!historyRef.current) historyRef.current = new History<RuleSet>({ coalesceMs: 1500, limit: 200 });
  const history = historyRef.current;
  useSyncExternalStore(history.subscribe, history.getVersion, history.getVersion);

  // ---- view (structurally shared so memoised rows survive echoes)
  const prevView = useRef<RuleSet | null>(null);
  const view = useMemo(() => {
    const next = local ?? storeRules ?? emptyRuleSet();
    const out = prevView.current ? reconcile(prevView.current, next) : next;
    prevView.current = out;
    return out;
  }, [local, storeRules]);
  const viewRef = useRef(view);
  viewRef.current = view;

  // ---- local edits not yet confirmed by the daemon
  /** Mirrors `local` for callbacks/effects. */
  const localRef = useRef<RuleSet | null>(null);
  /** The daemon's rule set the local edits are based on. */
  const baseRef = useRef<RuleSet | null>(null);
  /** States sent with `PUT /api/rules` since local editing started (their echoes aren't remote). */
  const sentRef = useRef<RuleSet[]>([]);
  const setLocalState = useCallback((rs: RuleSet | null) => {
    localRef.current = rs;
    setLocal(rs);
  }, []);
  /** Called before a local change: remember what it is based on. */
  const beginLocal = useCallback(() => {
    if (localRef.current) return;
    baseRef.current = viewRef.current;
    sentRef.current = [];
  }, []);

  // ---- save queue
  const pending = useRef<RuleSet | null>(null);
  const seq = useRef(0);
  const busyRef = useRef(false);
  const scheduleRef = useRef<(rs: RuleSet) => void>(() => {});

  const pump = useCallback(async () => {
    if (busyRef.current) return;
    busyRef.current = true;
    setSaveState('saving');
    let failed = false;
    while (pending.current) {
      const rs = pending.current;
      pending.current = null;
      const mySeq = ++seq.current;
      sentRef.current.push(rs);
      if (sentRef.current.length > 8) sentRef.current.shift();
      try {
        const res = await saveRules(rs);
        if (mySeq === seq.current) setServerErrors(new Map(res.errors.map(([id, msg]) => [id, msg] as const)));
        failed = false;
      } catch (e) {
        failed = true;
        // Keep the local state (the user's work) and offer a retry instead of reverting.
        useAppStore.setState({ rules: viewRef.current });
        toast.error("Couldn't save rules", {
          id: 'rules-save-error',
          description: errorMessage(e),
          action: { label: 'Retry', onClick: () => scheduleRef.current(viewRef.current) },
        });
      }
    }
    busyRef.current = false;
    setSaveState(failed ? 'error' : 'idle');
    if (!failed) {
      setLocalState(null);
      baseRef.current = null;
      sentRef.current = [];
    }
  }, [saveRules, setLocalState]);

  const schedule = useCallback(
    (rs: RuleSet) => {
      pending.current = rs;
      setLocalState(rs);
      void pump();
    },
    [pump, setLocalState],
  );
  scheduleRef.current = schedule;

  // ---- rule changes from the daemon (MCP, another window, our own echoes)
  const lastStore = useRef(storeRules);
  useEffect(() => {
    const prev = lastStore.current;
    lastStore.current = storeRules;
    if (!storeRules || prev === storeRules) return;
    const mine = localRef.current;
    if (mine) {
      // Our optimistic writes, their echoes and failed-save rollbacks aren't remote changes.
      const own = (x: RuleSet) => deepEqual(x, mine) || sentRef.current.some((s) => s === x || deepEqual(s, x)) || (baseRef.current != null && deepEqual(baseRef.current, x));
      if (own(storeRules)) return;
      // Someone else changed the rules while ours are unsaved/in flight: rebase our edits onto
      // theirs, so the next PUT keeps both instead of overwriting their change.
      const { set } = mergeRuleSets(baseRef.current ?? prev ?? storeRules, mine, storeRules, 'ours');
      baseRef.current = storeRules;
      setRevision((r) => r + 1);
      if (!deepEqual(set, mine)) {
        viewRef.current = set;
        scheduleRef.current(set);
      }
      return;
    }
    if (busyRef.current) return;
    if (prev && !deepEqual(prev, storeRules)) setRevision((r) => r + 1);
  }, [storeRules]);

  const commit = useCallback(
    (next: RuleSet, options: CommitOptions): HistoryEntry<RuleSet> | null => {
      const before = viewRef.current;
      if (next === before || deepEqual(next, before)) return null;
      beginLocal();
      const entry = history.push({ label: options.label, before, after: next, key: options.key, focus: options.focus });
      viewRef.current = next;
      if (options.origin !== 'editor') setRevision((r) => r + 1);
      schedule(next);
      return entry;
    },
    [history, schedule, beginLocal],
  );

  /** Re-apply an undo/redo step (`from` → `to`) on top of the current rules. */
  const applyStep = useCallback(
    (from: RuleSet, to: RuleSet) => {
      const { set, conflicts } = rebaseChange(viewRef.current, from, to);
      beginLocal();
      viewRef.current = set;
      setRevision((r) => r + 1);
      schedule(set);
      if (conflicts.length) {
        toast.warning('Rules changed elsewhere', {
          id: 'rules-undo-conflict',
          description: `${pluralize(conflicts.length, 'item')} also changed since — kept those newer changes.`,
        });
      }
    },
    [schedule, beginLocal],
  );

  const undo = useCallback(() => {
    const entry = history.undo();
    if (entry) applyStep(entry.after, entry.before);
    return entry;
  }, [history, applyStep]);

  const redo = useCallback(() => {
    const entry = history.redo();
    if (entry) applyStep(entry.before, entry.after);
    return entry;
  }, [history, applyStep]);

  const undoEntry = useCallback(
    (id: number) => {
      if (!history.isLatest(id)) return false;
      undo();
      return true;
    },
    [history, undo],
  );

  // The daemon reports per-rule problems (incl. "inactive without Pro") only in reply to a
  // save: when the license changes, re-send the unchanged rules to refresh them.
  const licenseKey = useLicenseStore((s) => (s.status ? `${s.status.state}:${s.status.features.join(',')}` : ''));
  const lastLicenseKey = useRef(licenseKey);
  const serverErrorsRef = useRef(serverErrors);
  serverErrorsRef.current = serverErrors;
  useEffect(() => {
    if (licenseKey === lastLicenseKey.current) return;
    lastLicenseKey.current = licenseKey;
    if (serverErrorsRef.current.size > 0 && storeRules) scheduleRef.current(viewRef.current);
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [licenseKey]);

  const retry = useCallback(() => scheduleRef.current(viewRef.current), []);
  const getRules = useCallback(() => viewRef.current, []);

  return {
    rules: view,
    loaded: storeRules !== null,
    saveState,
    serverErrors,
    revision,
    commit,
    undo,
    redo,
    undoEntry,
    history,
    retry,
    getRules,
  };
}
