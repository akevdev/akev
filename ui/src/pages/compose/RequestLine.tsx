import clsx from 'clsx';
import { Braces, Repeat, Send } from 'lucide-react';
import { memo, useMemo, type Ref } from 'react';
import { Button, IconButton, Popover, PopoverContent, PopoverTrigger, Spinner, Tooltip } from '../../components/ui';
import { MethodPicker } from './MethodPicker';
import type { RequestDraft, RequestOptions } from './model';
import { OptionsPopover } from './OptionsPopover';
import { UrlInput } from './UrlInput';
import { isDynamicVar, referencedVars, type VarMap } from './variables';
import styles from './RequestLine.module.css';

/** Every text of a draft that may contain variables. */
export function draftTexts(d: RequestDraft): string[] {
  const out = [d.url];
  for (const h of d.headers) if (h.enabled) out.push(h.name, h.value);
  if (d.auth.type === 'basic') out.push(d.auth.username, d.auth.password);
  if (d.auth.type === 'bearer') out.push(d.auth.token);
  const b = d.body;
  if (b.mode === 'json') out.push(b.json);
  if (b.mode === 'text') out.push(b.text);
  if (b.mode === 'form') for (const r of b.form) if (r.enabled) out.push(r.name, r.value);
  if (b.mode === 'multipart') for (const r of b.multipart) if (r.enabled) out.push(r.name, r.kind === 'text' ? r.value : '');
  return out;
}

export interface RequestLineProps {
  draft: RequestDraft;
  vars: VarMap;
  envName: string | null;
  sending: boolean;
  urlInvalid?: boolean;
  onMethodChange: (method: string) => void;
  onUrlChange: (url: string) => void;
  onOptionsChange: (options: RequestOptions) => void;
  onSend: () => void;
  /** "Repeat…": send the request many times. */
  onRepeat?: () => void;
  onCancel: () => void;
  /** Pasted/dropped text into the URL bar; return true when it was a command that was imported. */
  onImportText: (text: string) => boolean;
  onManageEnvironments: () => void;
  urlRef?: Ref<HTMLInputElement>;
}

/** Method · URL · variables · Send/Cancel · options. */
export const RequestLine = memo(function RequestLine(props: RequestLineProps) {
  const { draft, vars, envName, sending, urlInvalid } = props;
  const texts = useMemo(() => draftTexts(draft), [draft]);
  const referenced = useMemo(() => {
    const names: string[] = [];
    for (const t of texts) for (const n of referencedVars(t)) if (!names.includes(n)) names.push(n);
    return names;
  }, [texts]);
  const unresolved = referenced.filter((n) => !isDynamicVar(n) && !vars.has(n));

  return (
    <div className={styles.line}>
      <div className={clsx(styles.bar, urlInvalid && styles.barInvalid)}>
        <MethodPicker value={draft.method} onChange={props.onMethodChange} />
        <span className={styles.barDivider} aria-hidden />
        <UrlInput
          value={draft.url}
          onChange={props.onUrlChange}
          vars={vars}
          inputRef={props.urlRef}
          invalid={urlInvalid}
          onSubmit={props.onSend}
          onPasteText={props.onImportText}
          onBulkInsert={props.onImportText}
          placeholder="Enter a URL or paste a cURL command"
          aria-label="Request URL"
        />
        {referenced.length > 0 && (
          <Popover>
            <Tooltip content={unresolved.length ? `${unresolved.length} undefined variable${unresolved.length === 1 ? '' : 's'}` : `${referenced.length} variable${referenced.length === 1 ? '' : 's'}${envName ? ` from ${envName}` : ''}`}>
              <PopoverTrigger asChild>
                <button type="button" className={clsx(styles.varsChip, unresolved.length > 0 && styles.varsChipWarn)} aria-label="Variables used by this request">
                  <Braces size={13} strokeWidth={2} aria-hidden />
                  <span className="tnum">{unresolved.length || referenced.length}</span>
                </button>
              </PopoverTrigger>
            </Tooltip>
            <PopoverContent align="end" width={320} className={styles.varsPanel}>
              <div className={styles.varsHead}>
                <span>Variables</span>
                <span className={styles.varsEnv}>{envName ?? 'Globals only'}</span>
              </div>
              <ul className={styles.varsList}>
                {referenced.map((name) => {
                  const dyn = isDynamicVar(name);
                  const value = vars.get(name);
                  return (
                    <li key={name} className={styles.varsRow}>
                      <span className={clsx(styles.varsName, dyn ? styles.varsDyn : value === undefined ? styles.varsMissing : undefined)}>{name}</span>
                      <span className={clsx(styles.varsValue, value === undefined && !dyn && styles.varsUndefined)}>{dyn ? 'generated when sent' : value === undefined ? 'not defined' : value || '(empty)'}</span>
                    </li>
                  );
                })}
              </ul>
              <div className={styles.varsFoot}>
                {unresolved.length > 0 && <span className={styles.varsNote}>Undefined variables are sent literally.</span>}
                <Button size="sm" variant="secondary" onClick={props.onManageEnvironments}>
                  Manage environments…
                </Button>
              </div>
            </PopoverContent>
          </Popover>
        )}
      </div>
      {sending ? (
        <Tooltip content="Cancel request" shortcut="escape">
          <Button size="lg" variant="secondary" className={styles.sendButton} onClick={props.onCancel} aria-label="Cancel request">
            <span className={styles.cancelInner}>
              <Spinner size={14} tone="accent" />
              <span className={styles.sendLabel}>Cancel</span>
            </span>
          </Button>
        </Tooltip>
      ) : (
        <Tooltip content="Send request" shortcut="mod+enter">
          <Button size="lg" variant="primary" className={styles.sendButton} onClick={props.onSend} aria-label="Send request">
            <span className={styles.cancelInner}>
              <Send size={16} strokeWidth={1.75} aria-hidden />
              <span className={styles.sendLabel}>Send</span>
            </span>
          </Button>
        </Tooltip>
      )}
      {props.onRepeat && <IconButton size="lg" icon={Repeat} label="Repeat… (send many times)" disabled={sending} onClick={props.onRepeat} />}
      <OptionsPopover value={draft.options} onChange={props.onOptionsChange} />
    </div>
  );
});
