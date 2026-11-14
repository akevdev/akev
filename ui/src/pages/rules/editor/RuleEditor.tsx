import clsx from 'clsx';
import { Check, CircleAlert, CloudOff, Copy, EllipsisVertical, FlaskConical, FolderInput, Info, LoaderCircle, Sparkles, Trash2, TriangleAlert } from 'lucide-react';
import { forwardRef, useCallback, useEffect, useImperativeHandle, useMemo, useRef, useState, type ReactNode } from 'react';
import type { Action, ActionType, RuleId, RuleSet } from '../../../api/types';
import { Button, Kbd, MenuButton, Switch, Tooltip, type MenuItems } from '../../../components/ui';
import { formatNumber, formatRelativeTime, pluralize } from '../../../lib/format';
import { actionNeedsPro, isEntitled } from '../../../lib/license';
import { actionInfo, defaultAction } from '../../../lib/rules/actions';
import { summarize, summarizeTokens } from '../../../lib/rules/describe';
import { limitState } from '../../../lib/rules/stats';
import { mergeExternalChange, normalizeRule, rebaseDraft, type Stash } from '../../../lib/rules/draft';
import { deepEqual, type RuleNode } from '../../../lib/rules/tree';
import { validateRule } from '../../../lib/rules/validate';
import { useAppStore } from '../../../state/app';
import { useLicenseStore } from '../../../state/license';
import { iconFor } from '../icons';
import { Tokens } from '../Tokens';
import type { SaveState } from '../useRulesModel';
import { ActionForm } from './ActionForms';
import { ActionPicker } from './ActionPicker';
import { EditorValidationContext, type EditorValidation } from './context';
import { MatcherList } from './Matchers';
import { OptionsSection } from './OptionsSection';
import { TestPanel } from './TestPanel';
import { smoothScroll } from '../../../lib/motion';
import styles from './Editor.module.css';

export interface RuleEditorHandle {
  /** Commit pending valid edits now. */
  flush(): void;
  focusTitle(): void;
  focusFirstField(): void;
}

export type StashedDraft = Stash<RuleNode>;

export interface RuleEditorProps {
  rule: RuleNode;
  /** Full rule set (for the test panel and context line). */
  rules: RuleSet;
  revision: number;
  stashed: StashedDraft | undefined;
  /** Evaluation order (1-based) and total rule count. */
  order: number;
  total: number;
  groupTitle: string | null;
  parentEnabled: boolean;
  serverError: string | undefined;
  saveState: SaveState;
  testNonce: number;
  onCommit(rule: RuleNode): void;
  onStash(id: RuleId, stash: StashedDraft | null): void;
  onRetrySave(): void;
  onToggle(enabled: boolean): void;
  menu(): MenuItems;
  onDuplicate(): void;
  onDelete(): void;
  onJumpToRule(id: RuleId): void;
  onEscape(): void;
}

const DEBOUNCE_MS = 400;

/**
 * Rule editor: title, When (conditions), Then (action), Options and Test. Edits are kept in
 * a local draft; valid drafts are applied after 400 ms of quiet (and on blur/unmount),
 * invalid ones stay local with inline errors — and survive switching to another rule.
 */
export const RuleEditor = forwardRef<RuleEditorHandle, RuleEditorProps>(function RuleEditor(props, ref) {
  const { rule, rules, revision, stashed, order, total, groupTitle, parentEnabled, serverError, saveState, testNonce, onCommit, onStash, onRetrySave, onToggle, menu, onEscape } = props;

  const [draft, setDraft] = useState<RuleNode>(() => rebaseDraft(stashed, rule));
  const baseRef = useRef<RuleNode>(rule);
  const draftRef = useRef(draft);
  draftRef.current = draft;
  const [fieldErrors, setFieldErrors] = useState<ReadonlyMap<string, string>>(new Map());
  const [touched, setTouched] = useState<ReadonlySet<string>>(() => (stashed ? new Set(['*']) : new Set()));
  const [testOpen, setTestOpen] = useState(false);
  const [testRun, setTestRun] = useState(0);
  const titleRef = useRef<HTMLInputElement | null>(null);
  const bodyRef = useRef<HTMLDivElement | null>(null);

  // ---- external changes (undo/redo, list toggles, other windows)
  const lastRevision = useRef(revision);
  useEffect(() => {
    if (revision === lastRevision.current) return;
    lastRevision.current = revision;
    const base = baseRef.current;
    baseRef.current = rule;
    setDraft((d) => mergeExternalChange(d, base, rule));
  }, [revision, rule]);

  const issues = useMemo(() => validateRule(draft), [draft]);
  const valid = issues.length === 0 && fieldErrors.size === 0;
  const dirty = !deepEqual(draft, rule);
  const errorCount = issues.length + fieldErrors.size;

  const commitNow = useCallback(() => {
    const d = draftRef.current;
    baseRef.current = d;
    onCommit(d);
  }, [onCommit]);

  // ---- debounced apply
  useEffect(() => {
    if (!dirty || !valid) return;
    const t = setTimeout(commitNow, DEBOUNCE_MS);
    return () => clearTimeout(t);
  }, [draft, dirty, valid, commitNow]);

  // ---- flush / stash on unmount
  const stateRef = useRef({ dirty, valid });
  stateRef.current = { dirty, valid };
  useEffect(() => {
    const id = rule.id;
    return () => {
      const { dirty: d, valid: v } = stateRef.current;
      if (d && v) onCommit(draftRef.current);
      onStash(id, d && !v ? { draft: draftRef.current, base: baseRef.current } : null);
    };
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, []);

  useImperativeHandle(
    ref,
    () => ({
      flush() {
        if (stateRef.current.dirty && stateRef.current.valid) commitNow();
      },
      focusTitle() {
        titleRef.current?.focus();
        titleRef.current?.select();
      },
      focusFirstField() {
        const el = bodyRef.current?.querySelector<HTMLElement>('input:not([disabled]), [role="combobox"], .cm-content, button[aria-pressed]');
        (el ?? titleRef.current)?.focus();
      },
    }),
    [commitNow],
  );

  // ---- validation context
  const setFieldError = useCallback((key: string, message: string | null) => {
    setFieldErrors((prev) => {
      if ((prev.get(key) ?? null) === message) return prev;
      const next = new Map(prev);
      if (message) next.set(key, message);
      else next.delete(key);
      return next;
    });
  }, []);
  const touch = useCallback((path: string) => setTouched((s) => (s.has(path) ? s : new Set(s).add(path))), []);
  const validation = useMemo<EditorValidation>(() => ({ issues, setFieldError, touch, isTouched: (p) => touched.has('*') || touched.has(p) }), [issues, setFieldError, touch, touched]);

  const update = (patch: Partial<RuleNode>) => setDraft((d) => ({ ...d, ...patch }));
  const setAction = (action: Action) => update({ action });
  const changeActionType = (type: ActionType) => {
    if (type === draft.action.type) return;
    setAction(defaultAction(type, draft.action));
  };

  const showErrors = () => {
    setTouched(new Set(['*']));
    requestAnimationFrame(() => bodyRef.current?.querySelector<HTMLElement>('[role="alert"]')?.scrollIntoView({ block: 'center', behavior: smoothScroll() }));
  };

  const openTest = () => {
    setTestOpen(true);
    setTestRun((n) => n + 1);
    requestAnimationFrame(() => bodyRef.current?.querySelector('#rule-test')?.scrollIntoView({ block: 'nearest', behavior: smoothScroll() }));
  };

  // Menu "Test against selected exchange" from the list.
  const lastTestNonce = useRef(testNonce);
  useEffect(() => {
    if (testNonce !== lastTestNonce.current) {
      lastTestNonce.current = testNonce;
      openTest();
    }
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [testNonce]);

  const tokens = useMemo(() => summarizeTokens(draft), [draft]);
  const summary = useMemo(() => summarize(draft), [draft]);
  const info = actionInfo(draft.action.type);
  const Icon = iconFor(info.icon);
  const stats = useAppStore((s) => s.ruleStats[rule.id]);
  const hits = stats?.matches ?? 0;
  const limit = limitState(draft, stats);
  const proFeature = actionNeedsPro(draft.action);
  const proLocked = useLicenseStore((s) => proFeature != null && !isEntitled(s.status, proFeature));

  const status: { tone: 'ok' | 'busy' | 'bad' | 'warn'; icon: ReactNode; text: string; action?: () => void; title?: string } = !valid
    ? { tone: 'bad', icon: <CircleAlert size={13} aria-hidden />, text: `Fix ${pluralize(errorCount, 'error')}`, action: showErrors, title: "Invalid edits aren't applied — click to show them" }
    : dirty || saveState === 'saving'
      ? { tone: 'busy', icon: <LoaderCircle size={13} className={styles.spin} aria-hidden />, text: 'Saving…' }
      : saveState === 'error'
        ? { tone: 'bad', icon: <CloudOff size={13} aria-hidden />, text: 'Not saved · Retry', action: onRetrySave }
        : serverError
          ? { tone: 'warn', icon: <TriangleAlert size={13} aria-hidden />, text: proLocked ? 'Saved · inactive' : 'Rejected', title: serverError }
          : { tone: 'ok', icon: <Check size={13} aria-hidden />, text: 'Saved' };

  return (
    <EditorValidationContext.Provider value={validation}>
      {/* biome-ignore lint/a11y/noStaticElementInteractions: scopes keyboard shortcuts for the controls inside it; not a control */}
      <div
        className={styles.editor}
        onKeyDown={(e) => {
          if (e.key === 'Escape' && !e.defaultPrevented && (e.target as HTMLElement).closest('[role="dialog"], [role="menu"], [role="listbox"]') === null) {
            commitNow();
            onEscape();
          }
        }}
        onBlur={(e) => {
          // Leaving the editor applies valid edits immediately.
          if (!e.currentTarget.contains(e.relatedTarget as Node | null) && stateRef.current.dirty && stateRef.current.valid) commitNow();
        }}
      >
        <header className={styles.header}>
          <div className={styles.headerRow}>
            <span className={styles.headerIcon} data-category={info.category} aria-hidden>
              <Icon size={16} strokeWidth={1.8} />
            </span>
            <input
              ref={titleRef}
              className={styles.titleInput}
              value={draft.title ?? ''}
              placeholder={summary}
              onChange={(e) => update({ title: e.target.value === '' ? undefined : e.target.value })}
              onBlur={() => {
                if (draft.title != null && draft.title.trim() === '') update({ title: undefined });
              }}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault();
                  commitNow();
                  bodyRef.current?.querySelector<HTMLElement>('input, button')?.focus();
                }
              }}
              aria-label="Rule title (optional — defaults to the summary)"
              spellCheck={false}
            />
            <Tooltip content={status.title}>
              <button type="button" className={clsx(styles.saveStatus, styles[`save_${status.tone}`])} onClick={status.action} disabled={!status.action} aria-live="polite">
                {status.icon}
                <span>{status.text}</span>
              </button>
            </Tooltip>
            <Tooltip content={draft.enabled ? 'Disable rule' : 'Enable rule'}>
              <span>
                <Switch
                  checked={draft.enabled}
                  onCheckedChange={onToggle}
                  aria-label={draft.enabled ? 'Disable rule' : 'Enable rule'}
                />
              </span>
            </Tooltip>
            <MenuButton label="Rule actions" icon={EllipsisVertical} items={menu} />
          </div>
          <div className={styles.contextLine}>
            {draft.title ? <Tokens tokens={tokens} className={clsx(styles.contextSummary, 'truncate')} /> : <span className={styles.contextAuto}>Named automatically from its conditions and action — type to give it a title</span>}
          </div>
          <div className={styles.metaLine}>
            {!draft.enabled && <span className={clsx(styles.metaChip, styles.metaOff)}>Disabled — not applied</span>}
            <span className={styles.metaChip}>
              Rule <span className="tnum">{order}</span> of <span className="tnum">{total}</span>
            </span>
            {groupTitle && (
              <span className={styles.metaChip}>
                <FolderInput size={12} aria-hidden /> {groupTitle}
                {!parentEnabled && <span className={styles.metaWarn}> · group disabled</span>}
              </span>
            )}
            <span className={clsx(styles.metaChip, info.terminal ? styles.metaStop : styles.metaContinue)}>{info.terminal ? 'Stops evaluation' : 'Stacks — evaluation continues'}</span>
            <span className={clsx(styles.metaChip, 'tnum')}>
              {formatNumber(hits)} {hits === 1 ? 'match' : 'matches'}
              {stats?.lastMatchedAt ? ` · last ${formatRelativeTime(stats.lastMatchedAt)}` : ''}
              {limit ? ` · ${limit.exhausted ? 'limit reached' : `${limit.remaining} of ${limit.limit} left`}` : ''}
            </span>
          </div>
        </header>

        <div className={styles.scroll} data-editor-scroll>
          <div ref={bodyRef} className={styles.body}>
            {serverError && (
              <div className={styles.banner} role="alert">
                <TriangleAlert size={15} aria-hidden />
                <div>
                  <div className={styles.bannerTitle}>Akev rejected this rule — it is skipped until fixed</div>
                  <div className={clsx(styles.bannerDetail, 'mono')}>{serverError}</div>
                </div>
              </div>
            )}

            <section className={styles.section} aria-labelledby="rule-when">
              <div className={styles.sectionHead}>
                <h3 id="rule-when" className={styles.sectionTitle}>
                  When
                </h3>
                <span className={styles.sectionHint}>{draft.matchers.length > 1 ? 'All conditions must match' : draft.matchers.length === 1 ? 'The request must match' : ''}</span>
              </div>
              {draft.matchers.length === 0 && (
                <div className={styles.everyRequest}>
                  <Sparkles size={14} aria-hidden />
                  <span>
                    <strong>Matches every request.</strong> Add a condition to narrow it down.
                  </span>
                </div>
              )}
              <MatcherList matchers={draft.matchers} onChange={(matchers) => update({ matchers })} path="matchers" joiner="and" />
            </section>

            <section className={styles.section} aria-labelledby="rule-then">
              <div className={styles.sectionHead}>
                <h3 id="rule-then" className={styles.sectionTitle}>
                  Then
                </h3>
              </div>
              <ActionPicker value={draft.action.type} onChange={changeActionType} />
              <div className={styles.actionForm} key={draft.action.type}>
                <ActionForm action={draft.action} onChange={setAction} ruleId={rule.id} onTest={openTest} />
              </div>
            </section>

            <OptionsSection
              maxMatches={draft.maxMatches}
              note={draft.note}
              used={stats?.matches ?? 0}
              onChange={(patch) => setDraft((d) => normalizeRule({ ...d, ...patch }))}
            />

            <section className={styles.section} aria-labelledby="rule-test" id="rule-test">
              <div className={styles.sectionHead}>
                <h3 id="rule-test-title" className={styles.sectionTitle}>
                  Test
                </h3>
                <span className={styles.sectionHint}>Dry-run against captured traffic</span>
                {!testOpen && (
                  <Button size="sm" variant="ghost" icon={FlaskConical} onClick={openTest} className={styles.sectionAction}>
                    Test against a request
                  </Button>
                )}
              </div>
              {testOpen && <TestPanel rules={rules} rule={draft} runNonce={testRun} onJumpToRule={props.onJumpToRule} />}
            </section>

            <div className={styles.footerActions}>
              <Button size="sm" variant="ghost" icon={Copy} onClick={props.onDuplicate}>
                Duplicate
              </Button>
              <Button size="sm" variant="subtle-danger" icon={Trash2} onClick={props.onDelete}>
                Delete rule
              </Button>
              <span className={styles.footerHint}>
                <Info size={12} aria-hidden /> Changes apply live and can be undone with <Kbd combo="mod+z" size="sm" />
              </span>
            </div>
          </div>
        </div>
      </div>
    </EditorValidationContext.Provider>
  );
});

