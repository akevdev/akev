import clsx from 'clsx';
import { CircleCheck, CircleSlash, CircleX, FlaskConical, Info, Play, TriangleAlert } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { ExchangeId, RuleId, RulesTestResult, RuleSet } from '../../../api/types';
import { Button, MethodBadge, Select, Spinner } from '../../../components/ui';
import { describeAction, ruleLabel } from '../../../lib/rules/describe';
import { findItem, replaceRule, walkRules, type RuleNode } from '../../../lib/rules/tree';
import { shallowEqual, trafficStore, useSingleSelectedId, useTraffic } from '../../../state/traffic';
import styles from './Editor.module.css';

interface Outcome {
  full: RulesTestResult;
  isolated: RulesTestResult;
  exchangeId: ExchangeId;
}

export interface TestPanelProps {
  rules: RuleSet;
  rule: RuleNode;
  /** Changing this runs the test (e.g. "Test against selected exchange" from a menu). */
  runNonce: number;
  onJumpToRule: (id: RuleId) => void;
}

/**
 * Dry-runs the rule set (including unsaved edits) against a captured exchange via
 * `POST /api/rules/test`, twice: in context (what would really happen) and in isolation
 * (do this rule's conditions match at all?), so the panel can explain *why*.
 */
export function TestPanel({ rules, rule, runNonce, onJumpToRule }: TestPanelProps) {
  const selected = useSingleSelectedId();
  const recent = useTraffic((s) => s.visible.slice(-60).reverse(), shallowEqual);
  const [exchangeId, setExchangeId] = useState<ExchangeId | null>(selected ?? recent[0] ?? null);
  const [state, setState] = useState<{ status: 'idle' } | { status: 'loading' } | { status: 'error'; message: string } | { status: 'done'; outcome: Outcome }>({ status: 'idle' });
  const seq = useRef(0);

  useEffect(() => {
    if (selected != null) setExchangeId(selected);
  }, [selected]);

  const options = useMemo(() => {
    const ids = exchangeId != null && !recent.includes(exchangeId) ? [exchangeId, ...recent] : recent;
    return ids.map((id) => {
      const s = trafficStore.get(id);
      const label = s ? `${s.method} ${s.host}${s.path.length > 60 ? `${s.path.slice(0, 60)}…` : s.path}` : `#${id}`;
      return {
        value: String(id),
        label: s ? (
          <span className={styles.exOption}>
            <MethodBadge method={s.method} />
            <span className="truncate">
              {s.host}
              <span className={styles.exPath}>{s.path}</span>
            </span>
          </span>
        ) : (
          `#${id}`
        ),
        textValue: label,
      };
    });
  }, [recent, exchangeId]);

  const run = async () => {
    if (exchangeId == null) return;
    const my = ++seq.current;
    setState({ status: 'loading' });
    const withDraft = replaceRule(rules, rule);
    const isolatedSet: RuleSet = { version: rules.version, items: [{ ...rule, enabled: true, maxMatches: undefined }] };
    try {
      const [full, isolated] = await Promise.all([api.rules.test({ exchangeId, rules: withDraft }), api.rules.test({ exchangeId, rules: isolatedSet })]);
      if (my === seq.current) setState({ status: 'done', outcome: { full, isolated, exchangeId } });
    } catch (e) {
      if (my === seq.current) setState({ status: 'error', message: errorMessage(e) });
    }
  };

  const lastNonce = useRef(runNonce);
  useEffect(() => {
    if (runNonce !== lastNonce.current) {
      lastNonce.current = runNonce;
      void run();
    }
    // biome-ignore lint/correctness/useExhaustiveDependencies: runs only when the listed values change
  }, [runNonce]);

  // Re-test stale results lazily: clear them when the rule changes.
  const ruleKey = JSON.stringify(rule);
  const lastKey = useRef(ruleKey);
  useEffect(() => {
    if (lastKey.current !== ruleKey) {
      lastKey.current = ruleKey;
      setState((s) => (s.status === 'done' ? { status: 'idle' } : s));
    }
  }, [ruleKey]);

  const summary = exchangeId != null ? trafficStore.get(exchangeId) : undefined;

  return (
    <div className={styles.testPanel}>
      <div className={styles.testBar}>
        {options.length > 0 ? (
          <Select
            value={exchangeId != null ? String(exchangeId) : undefined}
            onValueChange={(v) => {
              setExchangeId(Number(v));
              setState({ status: 'idle' });
            }}
            options={options}
            placeholder="Pick a captured request"
            className={styles.testSelect}
            aria-label="Captured request to test against"
          />
        ) : (
          <span className={styles.testEmpty}>No captured traffic yet — send a request through Akev to test against it.</span>
        )}
        <Button icon={state.status === 'loading' ? undefined : Play} loading={state.status === 'loading'} onClick={() => void run()} disabled={exchangeId == null}>
          Run test
        </Button>
      </div>
      {state.status === 'idle' && options.length > 0 && (
        <p className={styles.testHint}>
          <FlaskConical size={13} aria-hidden /> Dry run — nothing is sent, counters don't change. Unsaved edits are included.
          {selected == null && ' Tip: select an exchange in Traffic to test it here.'}
        </p>
      )}
      {state.status === 'error' && (
        <div className={clsx(styles.testResult, styles.testBad)} role="alert">
          <CircleX size={16} aria-hidden />
          <div>
            <div className={styles.testTitle}>The test couldn't run</div>
            <div className={styles.testDetail}>{state.message}</div>
          </div>
        </div>
      )}
      {state.status === 'loading' && (
        <div className={styles.testResult}>
          <Spinner size={14} />
          <span className={styles.testDetail}>Evaluating…</span>
        </div>
      )}
      {state.status === 'done' && <TestOutcome outcome={state.outcome} rules={replaceRule(rules, rule)} rule={rule} methodPath={summary ? `${summary.method} ${summary.path}` : ''} onJumpToRule={onJumpToRule} />}
    </div>
  );
}

function TestOutcome({ outcome, rules, rule, onJumpToRule }: { outcome: Outcome; rules: RuleSet; rule: RuleNode; methodPath: string; onJumpToRule: (id: RuleId) => void }) {
  const { full, isolated } = outcome;
  const conditionsMatch = isolated.matched.includes(rule.id);
  const applied = full.matched.includes(rule.id);
  const handledBy = full.terminalRule;
  const walked = walkRules(rules);
  const effective = walked.find((w) => w.rule.id === rule.id)?.effectiveEnabled ?? rule.enabled;
  const label = (id: RuleId) => {
    const item = findItem(rules, id);
    return item?.type === 'rule' ? ruleLabel(item) : id;
  };
  const order = (id: RuleId) => (walked.find((w) => w.rule.id === id)?.order ?? 0) + 1;

  let tone: 'good' | 'warn' | 'bad';
  let icon;
  let title: string;
  let detail: string;
  if (applied && handledBy === rule.id) {
    tone = 'good';
    icon = <CircleCheck size={16} aria-hidden />;
    title = 'This rule handles the request';
    detail = `Result: ${describeAction(full.action)}`;
  } else if (applied) {
    tone = 'good';
    icon = <CircleCheck size={16} aria-hidden />;
    title = 'Matches — this rule modifies the request';
    detail = handledBy ? `Then “${label(handledBy)}” (#${order(handledBy)}) responds: ${describeAction(full.action)}` : 'No respond rule matches afterwards, so it is passed through to the server.';
  } else if (conditionsMatch && !effective) {
    tone = 'warn';
    icon = <CircleSlash size={16} aria-hidden />;
    title = 'Conditions match, but the rule is disabled';
    detail = rule.enabled ? 'Its group is disabled — enable the group to apply it.' : 'Enable the rule to apply it.';
  } else if (conditionsMatch && handledBy) {
    tone = 'warn';
    icon = <TriangleAlert size={16} aria-hidden />;
    title = 'Conditions match, but an earlier rule responds first';
    detail = `“${label(handledBy)}” (#${order(handledBy)}) handles it. Drag this rule above it to take priority.`;
  } else if (conditionsMatch) {
    tone = 'warn';
    icon = <TriangleAlert size={16} aria-hidden />;
    title = "Conditions match, but the rule wasn't applied";
    detail = 'It may be invalid or have reached its match limit.';
  } else {
    tone = 'bad';
    icon = <CircleX size={16} aria-hidden />;
    title = "Doesn't match this request";
    detail = 'Check the conditions above — every one of them must match.';
  }

  // Matching scripts run during the dry run; empty console output is left out of the result.
  const scriptRan = full.scriptOutcome != null || full.error != null;
  const logs = scriptRan ? (full.scriptLogs ?? []) : null;
  const scriptError = full.error ?? null;
  const isScript = rule.action.type === 'script';

  return (
    <div className={clsx(styles.testResult, tone === 'good' && styles.testGood, tone === 'warn' && styles.testWarn, tone === 'bad' && styles.testBad)} role="status">
      {icon}
      <div className={styles.testBody}>
        <div className={styles.testTitle}>{title}</div>
        <div className={styles.testDetail}>{detail}</div>
        {full.matched.length > 0 && (
          <ol className={styles.testChain} aria-label="Matched rules in order">
            {full.matched.map((id) => (
              <li key={id}>
                <button type="button" className={clsx(styles.testChainItem, id === rule.id && styles.testChainSelf, id === handledBy && styles.testChainTerminal)} onClick={() => onJumpToRule(id)}>
                  <span className={clsx(styles.testOrder, 'tnum')}>#{order(id)}</span>
                  <span className="truncate">{label(id)}</span>
                  {id === handledBy && <span className={styles.testTag}>responds</span>}
                </button>
              </li>
            ))}
          </ol>
        )}
        {isScript && applied && (logs || scriptError) ? (
          <div className={styles.testConsole}>
            <div className={styles.testConsoleHead}>Console</div>
            {scriptError && <pre className={clsx(styles.testConsoleLine, styles.testConsoleError)}>{scriptError}</pre>}
            {logs && logs.length === 0 && !scriptError && <div className={styles.testConsoleEmpty}>No console output</div>}
            {logs?.map((line, i) => (
              <pre key={i} className={clsx(styles.testConsoleLine, /^\[(error|warn)\]/.test(line) && styles.testConsoleError)}>
                {line}
              </pre>
            ))}
          </div>
        ) : isScript && applied ? (
          <p className={styles.testNote}>
            <Info size={12} aria-hidden /> Dry runs check matching and order only — the script itself runs on live traffic, where its <code>console.log</code> output is captured. Syntax is checked as you type.
          </p>
        ) : null}
      </div>
    </div>
  );
}
