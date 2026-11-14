// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { CircleAlert, Eye, EyeOff, Plus, X } from 'lucide-react';
import { useEffect, useState, type CSSProperties, type KeyboardEvent } from 'react';
import { IconButton, Input, Tooltip, undoToast } from '../../components/ui';
import styles from './PairTable.module.css';

export interface PairColumn<K extends string> {
  key: K;
  label: string;
  placeholder?: string;
  validate?: (value: string) => string | null;
  normalize?: (value: string) => string;
  mono?: boolean;
  secret?: boolean;
  /** Empty allowed (stored as absent). */
  optional?: boolean;
  /** CSS grid track (default `minmax(0, 1fr)`). */
  width?: string;
}

type Row<K extends string> = Partial<Record<K, string>>;

export interface PairTableProps<K extends string> {
  columns: readonly PairColumn<K>[];
  rows: readonly Row<K>[];
  onChange: (rows: Row<K>[]) => void;
  /** Noun for labels and messages ("override", "certificate"). */
  noun: string;
  emptyText?: string;
  'aria-label': string;
  /** Column whose value must be unique (e.g. the host). */
  uniqueKey?: K;
}

const trim = (s: string) => s.trim();

function cellError<K extends string>(col: PairColumn<K>, raw: string): string | null {
  const value = (col.normalize ?? trim)(raw);
  if (!value) return col.optional ? null : `${col.label} is required`;
  return col.validate?.(value) ?? null;
}

interface SecretInputProps {
  value: string;
  onChange: (v: string) => void;
  onBlur?: () => void;
  onKeyDown?: (e: KeyboardEvent<HTMLInputElement>) => void;
  placeholder?: string;
  invalid?: boolean;
  label: string;
}

function SecretInput({ value, onChange, onBlur, onKeyDown, placeholder, invalid, label }: SecretInputProps) {
  const [shown, setShown] = useState(false);
  return (
    <Input
      size="sm"
      type={shown ? 'text' : 'password'}
      value={value}
      placeholder={placeholder}
      invalid={invalid}
      aria-label={label}
      onChange={(e) => onChange(e.target.value)}
      onBlur={onBlur}
      onKeyDown={onKeyDown}
      autoComplete="new-password"
      trailing={
        <button type="button" className={styles.reveal} aria-label={shown ? 'Hide passphrase' : 'Show passphrase'} onMouseDown={(e) => e.preventDefault()} onClick={() => setShown((s) => !s)} tabIndex={-1}>
          {shown ? <EyeOff size={13} strokeWidth={1.75} aria-hidden /> : <Eye size={13} strokeWidth={1.75} aria-hidden />}
        </button>
      }
    />
  );
}

interface EditRowProps<K extends string> {
  columns: readonly PairColumn<K>[];
  row: Row<K>;
  index: number;
  onCommit: (index: number, row: Row<K>) => void;
  onRemove: (index: number) => void;
  noun: string;
  isDuplicate: (key: K, value: string, index: number) => boolean;
}

/** Validate + normalise a whole row. */
function checkRow<K extends string>(columns: readonly PairColumn<K>[], text: Row<K>, isDuplicate: (key: K, value: string) => boolean): { row: Row<K>; errors: Partial<Record<K, string>> } {
  const row: Row<K> = {};
  const errors: Partial<Record<K, string>> = {};
  for (const col of columns) {
    const raw = text[col.key] ?? '';
    const err = cellError(col, raw) ?? (isDuplicate(col.key, raw) ? 'Already in the list' : null);
    if (err) errors[col.key] = err;
    const value = (col.normalize ?? trim)(raw);
    if (value || !col.optional) row[col.key] = value;
  }
  return { row, errors };
}

/** One existing row: cells edit in place and commit on blur / Enter when valid. */
function EditRow<K extends string>({ columns, row, index, onCommit, onRemove, noun, isDuplicate }: EditRowProps<K>) {
  const [text, setText] = useState<Row<K>>(row);
  const [errors, setErrors] = useState<Partial<Record<K, string>>>({});
  useEffect(() => {
    setText(row);
    setErrors({});
  }, [row]);

  const commit = () => {
    const res = checkRow(columns, text, (k, v) => isDuplicate(k, v, index));
    setErrors(res.errors);
    if (Object.keys(res.errors).length > 0) return;
    if (columns.some((c) => (res.row[c.key] ?? '') !== (row[c.key] ?? ''))) onCommit(index, res.row);
  };

  return (
    <div className={styles.row} role="row">
      {columns.map((col) => {
        const err = errors[col.key];
        const set = (v: string) => setText((t) => ({ ...t, [col.key]: v }));
        const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
          if (e.key === 'Enter') commit();
          if (e.key === 'Escape') {
            setText(row);
            setErrors({});
          }
        };
        const label = `${col.label} of ${noun} ${index + 1}`;
        return (
          <div key={col.key} className={styles.cell} role="cell">
            <Tooltip content={err} side="bottom">
              <div className={styles.cellInner}>
                {col.secret ? (
                  <SecretInput value={text[col.key] ?? ''} onChange={set} onBlur={commit} onKeyDown={onKeyDown} placeholder={col.optional ? 'None' : col.placeholder} invalid={!!err} label={label} />
                ) : (
                  <Input size="sm" mono={col.mono} value={text[col.key] ?? ''} placeholder={col.placeholder} invalid={!!err} aria-label={label} onChange={(e) => set(e.target.value)} onBlur={commit} onKeyDown={onKeyDown} />
                )}
              </div>
            </Tooltip>
          </div>
        );
      })}
      <div className={styles.actionCell} role="cell">
        <IconButton label={`Remove ${noun}`} icon={X} size="sm" variant="danger" onClick={() => onRemove(index)} />
      </div>
    </div>
  );
}

/** Small editable table of string records (DNS overrides, client certificates). */
export function PairTable<K extends string>({ columns, rows, onChange, noun, emptyText, uniqueKey, ...aria }: PairTableProps<K>) {
  const [draft, setDraft] = useState<Row<K>>({});
  const [draftErrors, setDraftErrors] = useState<Partial<Record<K, string>>>({});
  const template = { '--pair-cols': `${columns.map((c) => c.width ?? 'minmax(0, 1fr)').join(' ')} 28px` } as CSSProperties;

  const isDuplicate = (key: K, value: string, index: number) => {
    if (key !== uniqueKey) return false;
    const v = value.trim().toLowerCase();
    return v !== '' && rows.some((r, i) => i !== index && (r[key] ?? '').toLowerCase() === v);
  };

  const add = () => {
    const res = checkRow(columns, draft, (k, v) => isDuplicate(k, v, -1));
    setDraftErrors(res.errors);
    if (Object.keys(res.errors).length > 0) return;
    onChange([...rows, res.row]);
    setDraft({});
  };

  const remove = (index: number) => {
    const removed = rows[index];
    if (!removed) return;
    const before = [...rows];
    onChange(rows.filter((_, i) => i !== index));
    const first = columns[0];
    undoToast(`Removed ${noun}`, () => onChange(before), { description: first ? removed[first.key] : undefined });
  };

  const firstDraftError = columns.map((c) => draftErrors[c.key]).find(Boolean);
  const draftEmpty = columns.every((c) => !(draft[c.key] ?? '').trim());

  return (
    <div className={styles.table} role="table" aria-label={aria['aria-label']} style={template}>
      <div className={clsx(styles.row, styles.head)} role="row">
        {columns.map((c) => (
          <div key={c.key} className={styles.th} role="columnheader">
            {c.label}
            {c.optional && <span className={styles.optional}> · optional</span>}
          </div>
        ))}
        <div role="columnheader" aria-label="Actions" />
      </div>
      {rows.length === 0 && (
        <div className={styles.empty} role="row">
          <span role="cell">{emptyText ?? `No ${noun}s yet.`}</span>
        </div>
      )}
      {rows.map((row, index) => (
        <EditRow key={index} columns={columns} row={row} index={index} onCommit={(i, r) => onChange(rows.map((x, j) => (j === i ? r : x)))} onRemove={remove} noun={noun} isDuplicate={isDuplicate} />
      ))}
      <div className={clsx(styles.row, styles.addRow)} role="row">
        {columns.map((col) => {
          const set = (v: string) => {
            setDraft((d) => ({ ...d, [col.key]: v }));
            setDraftErrors((e) => ({ ...e, [col.key]: undefined }));
          };
          const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
            if (e.key === 'Enter') add();
          };
          const label = `New ${noun}: ${col.label}`;
          return (
            <div key={col.key} className={styles.cell} role="cell">
              {col.secret ? (
                <SecretInput value={draft[col.key] ?? ''} onChange={set} onKeyDown={onKeyDown} placeholder={col.placeholder} invalid={!!draftErrors[col.key]} label={label} />
              ) : (
                <Input size="sm" mono={col.mono} value={draft[col.key] ?? ''} placeholder={col.placeholder} invalid={!!draftErrors[col.key]} aria-label={label} onChange={(e) => set(e.target.value)} onKeyDown={onKeyDown} />
              )}
            </div>
          );
        })}
        <div className={styles.actionCell} role="cell">
          <IconButton label={`Add ${noun}`} icon={Plus} size="sm" variant="subtle" disabled={draftEmpty} onClick={add} />
        </div>
      </div>
      {firstDraftError && (
        <p className={styles.error} role="alert">
          <CircleAlert size={12} strokeWidth={2} aria-hidden />
          {firstDraftError}
        </p>
      )}
    </div>
  );
}
