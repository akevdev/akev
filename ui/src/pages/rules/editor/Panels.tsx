import { ArrowDownUp, FolderOpen, FolderMinus, MousePointerClick, Plus, SearchX, Trash2, Workflow } from 'lucide-react';
import clsx from 'clsx';
import { useEffect, useMemo, useState } from 'react';
import type { RuleId } from '../../../api/types';
import { Button, Kbd, Switch } from '../../../components/ui';
import { formatNumber } from '../../../lib/format';
import { actionInfo } from '../../../lib/rules/actions';
import { summarizeTokens } from '../../../lib/rules/describe';
import { formatHits } from '../../../lib/rules/stats';
import { countGroups, countRules, walkRules, type GroupNode, type RuleNode } from '../../../lib/rules/tree';
import { useAppStore } from '../../../state/app';
import { iconFor } from '../icons';
import { Tokens } from '../Tokens';
import styles from './Editor.module.css';

export function GroupPanel({
  group,
  onRename,
  onToggle,
  onAddRule,
  onUngroup,
  onDelete,
  onSelectRule,
}: {
  group: GroupNode;
  onRename: (title: string) => void;
  onToggle: (enabled: boolean) => void;
  onAddRule: () => void;
  onUngroup: () => void;
  onDelete: () => void;
  onSelectRule: (id: RuleId) => void;
}) {
  const [title, setTitle] = useState(group.title);
  useEffect(() => setTitle(group.title), [group.title]);
  const rules = countRules(group.items);
  const groups = countGroups(group.items);
  const walked = useMemo(() => walkRules({ version: 1, items: group.items }), [group.items]);
  const hits = useAppStore((s) => walked.reduce((n, w) => n + (s.ruleStats[w.rule.id]?.matches ?? 0), 0));
  const commit = () => {
    const t = title.trim();
    if (t && t !== group.title) onRename(t);
    else setTitle(group.title);
  };
  return (
    <div className={styles.editor}>
      <header className={styles.header}>
        <div className={styles.headerRow}>
          <span className={styles.headerIcon} data-category="group" aria-hidden>
            <FolderOpen size={16} strokeWidth={1.8} />
          </span>
          <input
            className={styles.titleInput}
            value={title}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={commit}
            onKeyDown={(e) => {
              if (e.key === 'Enter') commit();
              if (e.key === 'Escape') setTitle(group.title);
            }}
            aria-label="Group name"
            placeholder="Group name"
          />
          <Switch checked={group.enabled} onCheckedChange={onToggle} aria-label={group.enabled ? 'Disable group' : 'Enable group'} />
        </div>
        <div className={styles.metaLine}>
          <span className={styles.metaChip}>
            <span className="tnum">{rules}</span> {rules === 1 ? 'rule' : 'rules'}
            {groups > 0 && (
              <>
                {' '}
                · <span className="tnum">{groups}</span> {groups === 1 ? 'subgroup' : 'subgroups'}
              </>
            )}
          </span>
          <span className={styles.metaChip}>
            <span className="tnum">{formatNumber(hits)}</span> matches
          </span>
          {!group.enabled && <span className={styles.metaChip}>Disabled — none of its rules run</span>}
        </div>
      </header>
      <div className={styles.scroll}>
        <div className={styles.body}>
          <p className={styles.groupExplain}>
            Groups organise rules and switch them on or off together. Rules inside a group are evaluated in place, in the same top-to-bottom order as everything else.
          </p>
          {walked.length > 0 && (
            <ol className={styles.groupRules} aria-label={`Rules in ${group.title || 'this group'}`}>
              {walked.map((w) => (
                <GroupRuleItem key={w.rule.id} rule={w.rule} depth={w.depth} active={w.effectiveEnabled && group.enabled} onSelect={onSelectRule} />
              ))}
            </ol>
          )}
          <div className={styles.groupActions}>
            <Button icon={Plus} onClick={onAddRule}>
              New rule in group
            </Button>
            <Button variant="ghost" icon={FolderMinus} onClick={onUngroup}>
              Ungroup
            </Button>
            <Button variant="subtle-danger" icon={Trash2} onClick={onDelete}>
              Delete group
            </Button>
          </div>
        </div>
      </div>
    </div>
  );
}

function GroupRuleItem({ rule, depth, active, onSelect }: { rule: RuleNode; depth: number; active: boolean; onSelect: (id: RuleId) => void }) {
  const info = actionInfo(rule.action.type);
  const Icon = iconFor(info.icon);
  const hits = useAppStore((s) => s.ruleStats[rule.id]?.matches ?? 0);
  const tokens = useMemo(() => summarizeTokens(rule), [rule]);
  return (
    <li>
      <button type="button" className={clsx(styles.groupRule, !active && styles.groupRuleOff)} style={{ paddingLeft: 8 + depth * 16 }} onClick={() => onSelect(rule.id)}>
        <span className={styles.groupRuleIcon} data-category={info.category} aria-hidden>
          <Icon size={12} strokeWidth={1.9} />
        </span>
        {rule.title ? <span className={clsx(styles.groupRuleTitle, 'truncate')}>{rule.title}</span> : <Tokens tokens={tokens} className="truncate" />}
        {!rule.enabled && <span className={styles.groupRuleTag}>off</span>}
        <span className={clsx(styles.groupRuleHits, 'tnum')}>{hits > 0 ? formatHits(hits) : ''}</span>
      </button>
    </li>
  );
}

export function EditorPlaceholder({ ruleCount, onNew, missing }: { ruleCount: number; onNew: () => void; missing?: boolean }) {
  return (
    <div className={styles.placeholder}>
      {missing && (
        <div className={styles.placeholderMissing} role="status">
          <SearchX size={14} aria-hidden /> The rule in this link doesn't exist anymore — it may have been deleted.
        </div>
      )}
      <div className={styles.placeholderIcon} aria-hidden>
        <MousePointerClick size={22} strokeWidth={1.6} />
      </div>
      <h2 className={styles.placeholderTitle}>Select a rule to edit it</h2>
      <p className={styles.placeholderText}>
        {ruleCount} {ruleCount === 1 ? 'rule' : 'rules'} in this set. Edits apply live as you type — no save button.
      </p>
      <div className={styles.placeholderKeys}>
        <span>
          <Kbd combo="arrowup" size="sm" /> <Kbd combo="arrowdown" size="sm" /> select
        </span>
        <span>
          <Kbd combo="enter" size="sm" /> edit
        </span>
        <span>
          <Kbd combo="n" size="sm" /> new rule
        </span>
        <span>
          <Kbd combo="alt+arrowup" size="sm" /> move
        </span>
        <span>
          <Kbd combo="mod+z" size="sm" /> undo
        </span>
      </div>
      <Button variant="primary" icon={Plus} onClick={onNew} kbd="n">
        New rule
      </Button>
      <div className={styles.orderLegend}>
        <div className={styles.orderStep}>
          <ArrowDownUp size={14} aria-hidden />
          <span>
            <strong>Top to bottom.</strong> Drag rules to change priority.
          </span>
        </div>
        <div className={styles.orderStep}>
          <Workflow size={14} aria-hidden />
          <span>
            <strong>Modifiers stack</strong> (rewrite, delay, breakpoint, highlight, script) and evaluation continues.
          </span>
        </div>
        <div className={styles.orderStep}>
          <span className={styles.stopDot} aria-hidden />
          <span>
            <strong>The first respond rule wins</strong> (mock, map, close…). Nothing matched? The request passes through.
          </span>
        </div>
      </div>
    </div>
  );
}
