import clsx from 'clsx';
import { CaseSensitive, ChevronDown, Plus, Regex, WandSparkles, X } from 'lucide-react';
import { useEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import type { Headers, MatchReplace } from '../../../api/types';
import { Button, CodeEditor, headersToRows, IconButton, Input, KeyValueEditor, Menu, Tooltip, type KeyValueRow } from '../../../components/ui';
import { KNOWN_HEADER_NAMES } from '../../../lib/headerDocs';
import { statusColorVar, statusText } from '../../../lib/http';
import { deepEqual } from '../../../lib/rules/tree';
import { headerNameError, parseJson } from '../../../lib/rules/validate';
import { FieldError, useFieldError, useIssue, useTouch } from './context';
import styles from './Editor.module.css';

// =====================================================================================
// small toggles
// =====================================================================================

export interface ToggleChipProps {
  pressed: boolean;
  onPressedChange: (pressed: boolean) => void;
  label: string;
  children: ReactNode;
  tone?: 'default' | 'danger';
  disabled?: boolean;
}

/** Compact pressed-state toggle (Aa case sensitivity, ≠ negate, .* regex). */
export function ToggleChip({ pressed, onPressedChange, label, children, tone = 'default', disabled }: ToggleChipProps) {
  return (
    <Tooltip content={label}>
      <button
        type="button"
        aria-label={label}
        aria-pressed={pressed}
        disabled={disabled}
        className={clsx(styles.toggleChip, pressed && styles.toggleChipOn, tone === 'danger' && styles.toggleChipDanger)}
        onClick={() => onPressedChange(!pressed)}
      >
        {children}
      </button>
    </Tooltip>
  );
}

export function CaseToggle({ caseSensitive, onChange, disabled }: { caseSensitive: boolean; onChange: (v: boolean) => void; disabled?: boolean }) {
  return (
    <ToggleChip pressed={caseSensitive} onPressedChange={onChange} label={caseSensitive ? 'Case-sensitive (click for case-insensitive)' : 'Case-insensitive (click for case-sensitive)'} disabled={disabled}>
      <CaseSensitive size={15} strokeWidth={1.9} aria-hidden />
    </ToggleChip>
  );
}

export function NegateToggle({ negate, onChange }: { negate: boolean; onChange: (v: boolean) => void }) {
  return (
    <ToggleChip pressed={negate} onPressedChange={onChange} label={negate ? 'Negated: matches when this does NOT match' : 'Negate (match when this does not match)'} tone="danger">
      <span className={styles.neq} aria-hidden>
        ≠
      </span>
    </ToggleChip>
  );
}

// =====================================================================================
// headers
// =====================================================================================

/**
 * Headers editor (wire order, duplicates allowed) on top of `KeyValueEditor`. Keeps its own
 * row ids so typing never loses focus, and re-syncs only when the value changes from outside.
 */
export function HeadersField({ headers, onChange, path, label = 'Headers', namePlaceholder = 'header-name', valuePlaceholder = 'value' }: { headers: Headers; onChange: (h: Headers) => void; path: string; label?: string; namePlaceholder?: string; valuePlaceholder?: string }) {
  const [rows, setRows] = useState<KeyValueRow[]>(() => headersToRows(headers));
  const lastEmitted = useRef<Headers>(headers);
  useEffect(() => {
    if (!deepEqual(headers, lastEmitted.current)) {
      lastEmitted.current = headers;
      setRows(headersToRows(headers));
    }
  }, [headers]);
  const err = useIssue(path, true);
  const localErr = rows.map((r) => (r.name.trim() ? headerNameError(r.name.trim()) : r.value ? 'Header name is empty' : null)).find(Boolean) ?? null;
  // A value without a name can't be saved: block applying until it is fixed.
  useFieldError(localErr);
  return (
    <div className={styles.headersField}>
      <KeyValueEditor
        rows={rows}
        onChange={(next) => {
          setRows(next);
          const h: Headers = next.filter((r) => r.name.trim() !== '').map((r) => [r.name.trim(), r.value] as [string, string]);
          lastEmitted.current = h;
          onChange(h);
        }}
        allowDisable={false}
        nameSuggestions={KNOWN_HEADER_NAMES}
        namePlaceholder={namePlaceholder}
        valuePlaceholder={valuePlaceholder}
        aria-label={label}
      />
      <FieldError>{localErr ?? err}</FieldError>
    </div>
  );
}

// =====================================================================================
// JSON
// =====================================================================================

const pretty = (v: unknown) => JSON.stringify(v, null, 2) ?? '';

/**
 * JSON editor bound to a parsed value. Invalid text stays local (and blocks saving through
 * the editor's field-error registry) until it parses again.
 */
export function JsonField({
  value,
  onChange,
  validate,
  minHeight = 96,
  maxHeight = 320,
  label = 'JSON',
  contentId,
}: {
  value: unknown;
  onChange: (v: unknown) => void;
  validate?: (v: unknown) => string | null;
  minHeight?: number;
  maxHeight?: number;
  label?: string;
  contentId?: string;
}) {
  const [text, setText] = useState(() => pretty(value));
  const [error, setError] = useState<string | null>(null);
  const lastValue = useRef(value);
  useEffect(() => {
    if (!deepEqual(value, lastValue.current)) {
      lastValue.current = value;
      setText(pretty(value));
      setError(null);
    }
  }, [value]);
  useFieldError(error);
  const update = (t: string) => {
    setText(t);
    const parsed = parseJson(t);
    if (!parsed.ok) {
      setError(parsed.error);
      return;
    }
    const custom = validate?.(parsed.value) ?? null;
    setError(custom);
    if (custom) return;
    lastValue.current = parsed.value;
    onChange(parsed.value);
  };
  const format = () => {
    const parsed = parseJson(text);
    if (parsed.ok) setText(pretty(parsed.value));
  };
  return (
    <div className={styles.codeField}>
      <div className={clsx(styles.codeFrame, error && styles.codeFrameInvalid)}>
        <CodeEditor value={text} onChange={update} language="json" minHeight={minHeight} maxHeight={maxHeight} lineNumbers={false} folding={false} contentId={contentId} aria-label={label} transparent />
        <div className={styles.codeTools}>
          <Tooltip content="Format JSON">
            <button type="button" className={styles.codeTool} onClick={format} aria-label="Format JSON" disabled={!!error}>
              <WandSparkles size={13} strokeWidth={1.9} aria-hidden />
            </button>
          </Tooltip>
        </div>
      </div>
      <FieldError>{error}</FieldError>
    </div>
  );
}

// =====================================================================================
// find & replace
// =====================================================================================

export function MatchReplaceList({ items, onChange, path, findPlaceholder = 'find', replacePlaceholder = 'replace with', addLabel = 'Add replacement' }: { items: MatchReplace[]; onChange: (items: MatchReplace[]) => void; path: string; findPlaceholder?: string; replacePlaceholder?: string; addLabel?: string }) {
  const touch = useTouch();
  const update = (i: number, patch: Partial<MatchReplace>) => onChange(items.map((x, j) => (j === i ? { ...x, ...patch } : x)));
  const [focusLast, setFocusLast] = useState(false);
  return (
    <div className={styles.stack}>
      {items.map((mr, i) => (
        <MatchReplaceRow
          key={i}
          mr={mr}
          path={`${path}.${i}`}
          autoFocus={focusLast && i === items.length - 1}
          findPlaceholder={findPlaceholder}
          replacePlaceholder={replacePlaceholder}
          onChange={(p) => update(i, p)}
          onBlur={() => touch(`${path}.${i}.find`)}
          onRemove={() => onChange(items.filter((_, j) => j !== i))}
        />
      ))}
      <div>
        <Button
          size="sm"
          variant="ghost"
          icon={Plus}
          onClick={() => {
            setFocusLast(true);
            onChange([...items, { find: '', replace: '', regex: false, caseSensitive: false }]);
          }}
        >
          {addLabel}
        </Button>
      </div>
    </div>
  );
}

function MatchReplaceRow({ mr, path, autoFocus, findPlaceholder, replacePlaceholder, onChange, onBlur, onRemove }: { mr: MatchReplace; path: string; autoFocus: boolean; findPlaceholder: string; replacePlaceholder: string; onChange: (p: Partial<MatchReplace>) => void; onBlur: () => void; onRemove: () => void }) {
  const err = useIssue(`${path}.find`);
  return (
    <div>
      <div className={styles.replaceRow}>
        <Input size="sm" mono value={mr.find} invalid={!!err} placeholder={mr.regex ? `${findPlaceholder} (regex)` : findPlaceholder} onChange={(e) => onChange({ find: e.target.value })} onBlur={onBlur} autoFocus={autoFocus} aria-label="Find" />
        <span className={styles.replaceArrow} aria-hidden>
          →
        </span>
        <Input size="sm" mono value={mr.replace} placeholder={mr.regex ? `${replacePlaceholder} ($1, \${name})` : replacePlaceholder} onChange={(e) => onChange({ replace: e.target.value })} aria-label="Replace with" />
        <ToggleChip pressed={mr.regex} onPressedChange={(regex) => onChange({ regex })} label={mr.regex ? 'Regular expression (Rust syntax)' : 'Plain text (click for regex)'}>
          <Regex size={14} strokeWidth={1.9} aria-hidden />
        </ToggleChip>
        <CaseToggle caseSensitive={mr.caseSensitive} onChange={(caseSensitive) => onChange({ caseSensitive })} />
        <IconButton label="Remove replacement" icon={X} size="sm" onClick={onRemove} />
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}

// =====================================================================================
// string list (header names to remove…)
// =====================================================================================

export function StringChipsField({ values, onChange, placeholder, path, validate, mono = true }: { values: string[]; onChange: (v: string[]) => void; placeholder: string; path: string; validate?: (v: string) => string | null; mono?: boolean }) {
  const [text, setText] = useState('');
  const err = useIssue(path, true);
  const localErr = text.trim() && validate ? validate(text.trim()) : null;
  const add = () => {
    const parts = text
      .split(/[,\n]/)
      .map((s) => s.trim())
      .filter(Boolean);
    if (parts.length === 0) return;
    if (validate && parts.some((p) => validate(p))) return;
    onChange([...values, ...parts.filter((p) => !values.some((v) => v.toLowerCase() === p.toLowerCase()))]);
    setText('');
  };
  return (
    <div>
      <div className={styles.chipsInput}>
        {values.map((v, i) => (
          <span key={`${v}-${i}`} className={clsx(styles.chip, mono && 'mono')}>
            {v}
            <button type="button" className={styles.chipRemove} aria-label={`Remove ${v}`} onClick={() => onChange(values.filter((_, j) => j !== i))}>
              <X size={11} strokeWidth={2.25} aria-hidden />
            </button>
          </span>
        ))}
        <input
          className={clsx(styles.chipsText, mono && 'mono')}
          value={text}
          placeholder={values.length ? '' : placeholder}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
            if (e.key === 'Enter' || e.key === ',') {
              e.preventDefault();
              add();
            } else if (e.key === 'Backspace' && !text && values.length) {
              onChange(values.slice(0, -1));
            }
          }}
          onBlur={add}
          aria-label={placeholder}
        />
      </div>
      <FieldError>{localErr ?? err}</FieldError>
    </div>
  );
}

// =====================================================================================
// status
// =====================================================================================

const COMMON_STATUSES = [200, 201, 204, 301, 302, 304, 400, 401, 403, 404, 409, 422, 429, 500, 502, 503, 504];

/**
 * Status code field: a leading colour dot, free typing (out-of-range codes show an inline
 * error and are not applied — never silently clamped), ↑/↓ stepping and a menu of common
 * codes; plus the reason phrase, whose placeholder previews the canonical one.
 */
export function StatusField({ status, statusMessage, onChange, path, allowMessage = true }: { status: number; statusMessage: string | undefined; onChange: (status: number, statusMessage: string | undefined) => void; path: string; allowMessage?: boolean }) {
  const statusErr = useIssue(`${path}status`);
  const messageErr = useIssue(`${path}statusMessage`);
  const touch = useTouch();
  const err = statusErr ?? messageErr;
  const reason = Number.isFinite(status) ? statusText(status) : '';
  const [text, setText] = useState(() => (Number.isFinite(status) ? String(status) : ''));
  const last = useRef(status);
  useEffect(() => {
    if (Object.is(status, last.current)) return;
    last.current = status;
    setText(Number.isFinite(status) ? String(status) : '');
  }, [status]);
  const emit = (t: string, message: string | undefined = statusMessage) => {
    setText(t);
    const n = t === '' ? Number.NaN : Number(t);
    last.current = n;
    onChange(n, message);
  };
  const step = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key !== 'ArrowUp' && e.key !== 'ArrowDown') return;
    e.preventDefault();
    const base = Number.isFinite(status) ? status : 200;
    const next = Math.max(100, Math.min(999, base + (e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 100 : 1)));
    emit(String(next));
  };
  return (
    <div>
      <div className={styles.statusRow}>
        <Input
          mono
          value={text}
          inputMode="numeric"
          maxLength={3}
          invalid={!!statusErr}
          onChange={(e) => emit(e.target.value.replace(/\D/g, '').slice(0, 3))}
          onKeyDown={step}
          onBlur={() => touch(`${path}status`)}
          className={styles.statusNumber}
          inputClassName="tnum"
          leading={<span className={styles.statusDot} style={{ '--dot': statusColorVar(Number.isFinite(status) ? status : null) } as CSSProperties} aria-hidden />}
          trailing={
            <Menu
              align="start"
              trigger={
                <button type="button" className={styles.statusPick} aria-label="Common status codes">
                  <ChevronDown size={13} strokeWidth={2} aria-hidden />
                </button>
              }
              items={COMMON_STATUSES.map((s) => ({
                key: String(s),
                label: (
                  <span className={styles.statusOption}>
                    <span className={styles.statusDot} style={{ '--dot': statusColorVar(s) } as CSSProperties} aria-hidden />
                    <span className="mono tnum">{s}</span>
                    <span className={styles.statusOptionText}>{statusText(s)}</span>
                  </span>
                ),
                hint: s === status ? '✓' : undefined,
                onSelect: () => emit(String(s), undefined),
              }))}
            />
          }
          aria-label="Status code"
        />
        {allowMessage ? (
          <Input
            value={statusMessage ?? ''}
            placeholder={reason || 'Reason phrase'}
            invalid={!!messageErr}
            onChange={(e) => onChange(status, e.target.value === '' ? undefined : e.target.value)}
            className={styles.reasonInput}
            aria-label="Reason phrase"
          />
        ) : (
          <span className={styles.reasonPreview}>{reason}</span>
        )}
      </div>
      <FieldError>{err}</FieldError>
    </div>
  );
}
