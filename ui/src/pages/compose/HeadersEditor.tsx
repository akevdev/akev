// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { ChevronDown, Lock, Plus } from 'lucide-react';
import { memo, useMemo } from 'react';
import { Button, Checkbox, KeyValueEditor, Menu, Tooltip, makeRow, type KeyValueRow } from '../../components/ui';
import { KNOWN_HEADER_NAMES } from '../../lib/headerDocs';
import type { RequestDraft } from './model';
import { autoHeaders, serializeBody } from './request';
import { substitute, type VarMap } from './variables';
import styles from './RequestEditor.module.css';

const PRESETS: readonly [string, string][] = [
  ['Accept', 'application/json'],
  ['Accept', '*/*'],
  ['Accept-Encoding', 'gzip, deflate, br'],
  ['Accept-Language', 'en-US,en;q=0.9'],
  ['Cache-Control', 'no-cache'],
  ['User-Agent', 'Akev'],
  ['X-Requested-With', 'XMLHttpRequest'],
  ['If-None-Match', '""'],
];

export interface HeadersEditorProps {
  draft: RequestDraft;
  vars: VarMap;
  onHeaders: (rows: KeyValueRow[]) => void;
  onToggleAuto: (key: string, enabled: boolean) => void;
  onOpenAuth: () => void;
  onOpenBody: () => void;
}

/** Auto headers (dimmed, read-only, some switchable) above the user's header table. */
export const HeadersEditor = memo(function HeadersEditor({ draft, vars, onHeaders, onToggleAuto, onOpenAuth, onOpenBody }: HeadersEditorProps) {
  const bodySize = useMemo(() => (draft.body.mode === 'none' ? null : serializeBody(draft.body, (s) => substitute(s, vars).text).size), [draft.body, vars]);
  const auto = useMemo(() => autoHeaders(draft, { bodySize, resolvedUrl: substitute(draft.url, vars).text, subst: (s) => substitute(s, vars).text }), [draft, bodySize, vars]);

  return (
    <div className={styles.stack}>
      {auto.length > 0 && (
        <div className={styles.autoBlock}>
          <div className={styles.blockHead}>
            <span className={styles.blockTitle}>Auto-generated</span>
            <span className={styles.blockHint}>From the URL, body and auth — add a header with the same name to override.</span>
          </div>
          <div className={styles.autoTable} role="table" aria-label="Auto-generated headers">
            {auto.map((a) => {
              const off = a.disabled || a.overridden;
              const tip = a.overridden ? `Replaced by your ${a.name} header` : a.disabled ? 'Not sent' : a.sent ? a.note : `${a.note} · set by Akev when sending`;
              const jump = a.key === 'authorization' ? onOpenAuth : a.key === 'content-type' || a.key === 'content-length' ? onOpenBody : undefined;
              return (
                <div key={a.key} role="row" className={clsx(styles.autoRow, off && styles.autoOff)}>
                  <span role="cell" className={styles.autoToggle}>
                    {a.toggleable ? (
                      <Checkbox checked={!a.disabled} onCheckedChange={(c) => onToggleAuto(a.key, c)} aria-label={`Send ${a.name}`} disabled={a.overridden} />
                    ) : (
                      <Tooltip content={a.sent ? 'Managed by the Auth tab' : 'Always set by Akev'}>
                        <span className={styles.lock} tabIndex={-1}>
                          <Lock size={11} strokeWidth={2} aria-hidden />
                        </span>
                      </Tooltip>
                    )}
                  </span>
                  <span role="cell" className={styles.autoName}>
                    {a.name}
                  </span>
                  <span role="cell" className={styles.autoValue}>
                    <span className={styles.autoValueText} title={a.value}>
                      {a.value}
                    </span>
                    <Tooltip content={tip}>
                      {jump ? (
                        <button type="button" className={clsx(styles.autoNote, styles.autoNoteLink)} onClick={jump}>
                          {a.overridden ? 'overridden' : a.disabled ? 'off' : a.note.toLowerCase()}
                        </button>
                      ) : (
                        // biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip
                        <span className={styles.autoNote} tabIndex={0}>
                          {a.note.toLowerCase()}
                        </span>
                      )}
                    </Tooltip>
                  </span>
                </div>
              );
            })}
          </div>
        </div>
      )}
      <div className={styles.block}>
        <div className={styles.blockHead}>
          <span className={styles.blockTitle}>Headers</span>
          <span className={styles.blockHint}>Sent in this order. Paste <code>Name: value</code> lines to add several.</span>
          <Menu
            align="end"
            trigger={
              <Button size="sm" variant="ghost" icon={Plus} iconRight={ChevronDown} className={styles.blockAction}>
                Common
              </Button>
            }
            items={PRESETS.map(([name, value]) => ({
              key: `${name}:${value}`,
              label: (
                <span className={styles.presetItem}>
                  <span>{name}</span>
                  <span className={styles.presetValue}>{value}</span>
                </span>
              ),
              onSelect: () => onHeaders([...draft.headers, makeRow(name, value)]),
            }))}
          />
        </div>
        <KeyValueEditor rows={draft.headers} onChange={onHeaders} nameSuggestions={KNOWN_HEADER_NAMES} namePlaceholder="Header" valuePlaceholder="Value" aria-label="Request headers" />
      </div>
    </div>
  );
});
