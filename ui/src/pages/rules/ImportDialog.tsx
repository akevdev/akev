import clsx from 'clsx';
import { Folder, FolderPlus, Replace } from 'lucide-react';
import { useEffect, useMemo, useState } from 'react';
import type { RuleSet } from '../../api/types';
import { Button, Dialog, Kbd, Switch } from '../../components/ui';
import { actionInfo } from '../../lib/rules/actions';
import { summarizeTokens } from '../../lib/rules/describe';
import { pluralize } from '../../lib/format';
import { countGroups, flatten, walkRules } from '../../lib/rules/tree';
import { iconFor } from './icons';
import { onRadioGroupKeyDown } from './radioKeys';
import { Tokens } from './Tokens';
import styles from './RulesPage.module.css';

export type ImportMode = 'group' | 'replace';

export interface PendingImport {
  fileName: string;
  rules: RuleSet;
  /** Detected source format (for the description). */
  format: 'akev' | 'htk';
}

export function ImportDialog({ pending, existingCount, onCancel, onImport }: { pending: PendingImport | null; existingCount: number; onCancel: () => void; onImport: (mode: ImportMode, enabled: boolean) => void }) {
  const [mode, setMode] = useState<ImportMode>('group');
  const [enabled, setEnabled] = useState(true);
  useEffect(() => {
    if (pending) {
      setMode(existingCount > 0 ? 'group' : 'replace');
      setEnabled(true);
    }
  }, [pending, existingCount]);
  const walked = useMemo(() => (pending ? walkRules(pending.rules) : []), [pending]);
  const groups = pending ? countGroups(pending.rules.items) : 0;
  const rows = useMemo(() => (pending ? flatten(pending.rules, new Set()) : []), [pending]);
  const preview = rows.slice(0, 7);
  const hiddenRules = walked.length - preview.filter((r) => r.kind === 'rule').length;
  const empty = walked.length === 0;

  return (
    <Dialog
      open={pending !== null}
      onOpenChange={(o) => !o && onCancel()}
      title="Import rules"
      description={pending ? `${pending.fileName} · ${pending.format === 'htk' ? 'HTTP Toolkit rules, converted' : 'Akev rules'}` : undefined}
      width={560}
      footer={
        <>
          <Button variant="ghost" onClick={onCancel}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => onImport(mode, enabled)} disabled={empty} kbd="mod+enter">
            {mode === 'group' ? `Add ${walked.length} ${walked.length === 1 ? 'rule' : 'rules'}` : `Replace with ${walked.length} ${walked.length === 1 ? 'rule' : 'rules'}`}
          </Button>
        </>
      }
    >
      {/* biome-ignore lint/a11y/noStaticElementInteractions: scopes keyboard shortcuts for the controls inside it; not a control */}
      <div
        className={styles.importBody}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && !empty) {
            e.preventDefault();
            onImport(mode, enabled);
          }
        }}
      >
        <div className={styles.importStats}>
          <span className="tnum">
            <strong>{walked.length}</strong> {walked.length === 1 ? 'rule' : 'rules'}
          </span>
          {groups > 0 && (
            <span className="tnum">
              <strong>{groups}</strong> {groups === 1 ? 'group' : 'groups'}
            </span>
          )}
          <span className="tnum">
            <strong>{walked.filter((w) => w.effectiveEnabled).length}</strong> enabled
          </span>
        </div>
        {empty ? (
          <p className={styles.importEmpty}>This file doesn't contain any rules.</p>
        ) : (
          <ul className={styles.importPreview} aria-label="Rules in the file">
            {preview.map((row) => {
              const off = !row.item.enabled || !row.parentEnabled;
              if (row.item.type === 'group') {
                return (
                  <li key={row.id} className={clsx(styles.importGroup, off && styles.importDisabled)} style={{ paddingLeft: 6 + row.depth * 16 }}>
                    <span className={styles.importIcon} data-category="group" aria-hidden>
                      <Folder size={12} strokeWidth={1.9} />
                    </span>
                    <span className="truncate">{row.item.title || 'Untitled group'}</span>
                    <span className={clsx(styles.importCount, 'tnum')}>{row.ruleCount}</span>
                    {off && <span className={styles.importOff}>off</span>}
                  </li>
                );
              }
              const rule = row.item;
              const info = actionInfo(rule.action.type);
              const Icon = iconFor(info.icon);
              return (
                <li key={row.id} className={clsx(off && styles.importDisabled)} style={{ paddingLeft: 6 + row.depth * 16 }}>
                  <span className={styles.importIcon} data-category={info.category} aria-hidden>
                    <Icon size={12} strokeWidth={1.9} />
                  </span>
                  {rule.title ? <span className={clsx(styles.importTitle, 'truncate')}>{rule.title}</span> : <Tokens tokens={summarizeTokens(rule)} className="truncate" />}
                  {off && <span className={styles.importOff}>off</span>}
                </li>
              );
            })}
            {hiddenRules > 0 && <li className={styles.importMore}>and {pluralize(hiddenRules, 'more rule')}…</li>}
          </ul>
        )}
        <div className={styles.importModes} role="radiogroup" aria-label="How to import" onKeyDown={onRadioGroupKeyDown}>
          <button type="button" role="radio" aria-checked={mode === 'group'} tabIndex={mode === 'group' ? 0 : -1} className={clsx(styles.importMode, mode === 'group' && styles.importModeOn)} onClick={() => setMode('group')}>
            <FolderPlus size={16} strokeWidth={1.8} aria-hidden />
            <span>
              <span className={styles.importModeTitle}>Add as a new group</span>
              <span className={styles.importModeDesc}>Keeps your {existingCount} current {existingCount === 1 ? 'rule' : 'rules'}; the imported ones go into a group at the top.</span>
            </span>
          </button>
          <button type="button" role="radio" aria-checked={mode === 'replace'} tabIndex={mode === 'replace' ? 0 : -1} className={clsx(styles.importMode, mode === 'replace' && styles.importModeOn)} onClick={() => setMode('replace')}>
            <Replace size={16} strokeWidth={1.8} aria-hidden />
            <span>
              <span className={styles.importModeTitle}>Replace all rules</span>
              <span className={styles.importModeDesc}>
                {existingCount > 0 ? (
                  <>
                    Removes your {existingCount} current {existingCount === 1 ? 'rule' : 'rules'} — undo with <Kbd combo="mod+z" size="sm" />
                  </>
                ) : (
                  'You have no rules yet, so nothing is lost.'
                )}
              </span>
            </span>
          </button>
        </div>
        <Switch checked={enabled} onCheckedChange={setEnabled} label="Turn the imported rules on now" description={enabled ? 'They apply to traffic immediately' : 'They are added disabled so you can review them first'} />
      </div>
    </Dialog>
  );
}
