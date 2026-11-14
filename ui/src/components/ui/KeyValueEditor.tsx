// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus; focus goes to the inputs in them
import clsx from 'clsx';
import { FileText, Rows3, X } from 'lucide-react';
import { useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent } from 'react';
import type { Headers } from '../../api/types';
import { Checkbox } from './Checkbox';
import { Combobox, type ComboboxOption } from './Combobox';
import { IconButton } from './IconButton';
import { TextArea } from './TextArea';
import styles from './KeyValueEditor.module.css';

/** One editable row. `id` is a stable client-side key. */
export interface KeyValueRow {
  id: string;
  enabled: boolean;
  name: string;
  value: string;
}

let rowSeq = 0;
/** Create a row with a fresh id. */
export function makeRow(name = '', value = '', enabled = true): KeyValueRow {
  rowSeq += 1;
  return { id: `kv${rowSeq}`, enabled, name, value };
}

/** Headers (wire order) → rows. */
export function headersToRows(headers: Headers): KeyValueRow[] {
  return headers.map(([n, v]) => makeRow(n, v));
}

/** Enabled, non-empty rows → headers (wire order preserved). */
export function rowsToHeaders(rows: readonly KeyValueRow[]): Headers {
  return rows.filter((r) => r.enabled && r.name.trim() !== '').map((r) => [r.name.trim(), r.value] as [string, string]);
}

/**
 * Parse bulk text (`Name: value` per line; `# ` or `// ` prefix = disabled) into rows.
 * `separator` is `:` for headers and `=` for query/form params.
 */
export function parseBulk(text: string, separator: ':' | '=' = ':'): KeyValueRow[] {
  const rows: KeyValueRow[] = [];
  for (const rawLine of text.split(/\r?\n/)) {
    let line = rawLine.trim();
    if (!line) continue;
    let enabled = true;
    const disabled = /^(#|\/\/)\s?/.exec(line);
    if (disabled) {
      enabled = false;
      line = line.slice(disabled[0].length);
    }
    const i = line.indexOf(separator);
    if (i < 0) rows.push(makeRow(line, '', enabled));
    else rows.push(makeRow(line.slice(0, i).trim(), line.slice(i + 1).trim(), enabled));
  }
  return rows;
}

/** Rows → bulk text (inverse of {@link parseBulk}). */
export function formatBulk(rows: readonly KeyValueRow[], separator: ':' | '=' = ':'): string {
  return rows
    .filter((r) => r.name || r.value)
    .map((r) => `${r.enabled ? '' : '# '}${r.name}${separator === ':' ? ': ' : '='}${r.value}`)
    .join('\n');
}

export interface KeyValueEditorProps {
  rows: readonly KeyValueRow[];
  onChange: (rows: KeyValueRow[]) => void;
  namePlaceholder?: string;
  valuePlaceholder?: string;
  /** Autocomplete for names (e.g. `KNOWN_HEADER_NAMES`). */
  nameSuggestions?: readonly string[];
  /** Show enable checkboxes (default true). */
  allowDisable?: boolean;
  readOnly?: boolean;
  /** Bulk-edit separator: `:` (headers, default) or `=` (params). */
  separator?: ':' | '=';
  /** Show the bulk-edit toggle (default true). */
  allowBulk?: boolean;
  className?: string;
  'aria-label'?: string;
}

/**
 * Editable name/value table: enable checkbox, name (with suggestions), value (mono), delete.
 * Always keeps one trailing empty row; pasting `Name: value` lines splits them into rows;
 * a bulk-edit text mode is one click away.
 */
export function KeyValueEditor({
  rows,
  onChange,
  namePlaceholder = 'Name',
  valuePlaceholder = 'Value',
  nameSuggestions,
  allowDisable = true,
  readOnly,
  separator = ':',
  allowBulk = true,
  className,
  ...aria
}: KeyValueEditorProps) {
  const [bulk, setBulk] = useState<string | null>(null);
  const tableRef = useRef<HTMLDivElement | null>(null);
  // The always-present empty row. When typed into it becomes a real row *keeping its id*
  // (so the focused input survives the re-render) and a fresh placeholder replaces it.
  const [trailing, setTrailing] = useState<KeyValueRow>(() => makeRow());
  // Render the real rows plus one placeholder row that becomes real when typed into.
  const display = readOnly ? rows : [...rows, trailing];
  const suggestions = useMemo<ComboboxOption[] | undefined>(() => nameSuggestions?.map((n) => ({ value: n })), [nameSuggestions]);

  const update = (id: string, patch: Partial<KeyValueRow>) => {
    if (id === trailing.id) {
      onChange([...rows, { ...trailing, ...patch }]);
      setTrailing(makeRow());
      return;
    }
    onChange(rows.map((r) => (r.id === id ? { ...r, ...patch } : r)));
  };

  const remove = (id: string) => onChange(rows.filter((r) => r.id !== id));

  const onPaste = (e: ClipboardEvent<HTMLInputElement>, id: string) => {
    const text = e.clipboardData.getData('text');
    if (!text.includes('\n') && !(text.includes(separator) && id === trailing.id && text.indexOf(separator) > 0)) return;
    const parsed = parseBulk(text, separator);
    if (parsed.length === 0) return;
    e.preventDefault();
    const index = rows.findIndex((r) => r.id === id);
    const target = index >= 0 ? rows[index] : undefined;
    if (target && !target.name && !target.value) {
      onChange([...rows.slice(0, index), ...parsed, ...rows.slice(index + 1)]);
    } else if (index >= 0) {
      onChange([...rows.slice(0, index + 1), ...parsed, ...rows.slice(index + 1)]);
    } else {
      onChange([...rows, ...parsed]);
    }
  };

  const focusCell = (rowIndex: number, col: 'name' | 'value') => {
    requestAnimationFrame(() => {
      tableRef.current?.querySelector<HTMLInputElement>(`[data-row="${rowIndex}"][data-col="${col}"]`)?.focus();
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>, rowIndex: number, col: 'name' | 'value', row: KeyValueRow) => {
    if (e.key === 'Enter' && !e.defaultPrevented) {
      e.preventDefault();
      focusCell(rowIndex + 1, 'name');
    } else if (e.key === 'Backspace' && col === 'name' && row.name === '' && row.value === '' && row.id !== trailing.id) {
      e.preventDefault();
      remove(row.id);
      focusCell(Math.max(0, rowIndex - 1), 'value');
    }
  };

  if (bulk !== null) {
    return (
      <div className={clsx(styles.root, className)}>
        <div className={styles.bulkBar}>
          <span className={styles.bulkHint}>
            One per line as <code>name{separator === ':' ? ': ' : '='}value</code>. Prefix with <code>#</code> to disable.
          </span>
          <IconButton
            label="Table view"
            icon={Rows3}
            size="sm"
            onClick={() => {
              onChange(parseBulk(bulk, separator));
              setBulk(null);
            }}
          />
        </div>
        <TextArea mono autoResize minRows={4} maxRows={20} value={bulk} onChange={(e) => setBulk(e.target.value)} aria-label={`${aria['aria-label'] ?? 'Rows'} (bulk edit)`} />
      </div>
    );
  }

  return (
    <div className={clsx(styles.root, className)}>
      <div ref={tableRef} className={clsx(styles.table, !allowDisable && styles.noToggle, readOnly && styles.readOnly)} role="table" aria-label={aria['aria-label']}>
        {display.map((row, i) => {
          const isPlaceholder = row.id === trailing.id;
          return (
            <div key={row.id} role="row" className={clsx(styles.row, !row.enabled && styles.disabled, isPlaceholder && styles.placeholderRow)}>
              {allowDisable && (
                <span role="cell" className={styles.toggle}>
                  {!isPlaceholder && <Checkbox checked={row.enabled} onCheckedChange={(c) => update(row.id, { enabled: c })} disabled={readOnly} aria-label={`Enable ${row.name || 'row'}`} />}
                </span>
              )}
              <span role="cell" className={styles.cell}>
                {suggestions && !readOnly ? (
                  <Combobox
                    size="sm"
                    className={styles.inputWrap}
                    inputClassName={styles.input}
                    value={row.name}
                    onValueChange={(v) => update(row.id, { name: v })}
                    options={suggestions}
                    openOnFocus={false}
                    placeholder={namePlaceholder}
                    data-row={i}
                    data-col="name"
                    onPaste={(e) => onPaste(e, row.id)}
                    onKeyDown={(e) => onKeyDown(e, i, 'name', row)}
                    aria-label={namePlaceholder}
                  />
                ) : (
                  <input
                    className={clsx(styles.bare, styles.nameInput)}
                    value={row.name}
                    readOnly={readOnly}
                    placeholder={namePlaceholder}
                    spellCheck={false}
                    data-row={i}
                    data-col="name"
                    onChange={(e) => update(row.id, { name: e.target.value })}
                    onPaste={(e) => onPaste(e, row.id)}
                    onKeyDown={(e) => onKeyDown(e, i, 'name', row)}
                    aria-label={namePlaceholder}
                  />
                )}
              </span>
              <span role="cell" className={styles.cell}>
                <input
                  className={clsx(styles.bare, styles.valueInput)}
                  value={row.value}
                  readOnly={readOnly}
                  placeholder={valuePlaceholder}
                  spellCheck={false}
                  data-row={i}
                  data-col="value"
                  onChange={(e) => update(row.id, { value: e.target.value })}
                  onPaste={(e) => onPaste(e, row.id)}
                  onKeyDown={(e) => onKeyDown(e, i, 'value', row)}
                  aria-label={valuePlaceholder}
                />
              </span>
              {!readOnly && (
                <span role="cell" className={styles.actions}>
                  {!isPlaceholder && (
                    <button type="button" className={styles.remove} aria-label={`Remove ${row.name || 'row'}`} onClick={() => remove(row.id)} tabIndex={-1}>
                      <X size={13} strokeWidth={2} aria-hidden />
                    </button>
                  )}
                </span>
              )}
            </div>
          );
        })}
      </div>
      {allowBulk && !readOnly && (
        <div className={styles.footer}>
          <button type="button" className={styles.bulkToggle} onClick={() => setBulk(formatBulk(rows, separator))}>
            <FileText size={12} strokeWidth={1.75} aria-hidden />
            Bulk edit
          </button>
        </div>
      )}
    </div>
  );
}
