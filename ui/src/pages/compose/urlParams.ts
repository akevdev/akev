/**
 * Two-way sync between the URL's query string and the Params table.
 *
 * Fidelity rules:
 * - URL → rows never rewrites the URL: what the user typed stays byte-for-byte.
 * - Rows → URL keeps each untouched param's original encoding (`%20` vs `+`, `%2F` vs `/`),
 *   order and duplicates; only edited/new params are (minimally) encoded.
 * - `{{variables}}` are never encoded.
 * - Disabled rows live only in the table (they are not in the URL) and keep their position.
 */

import { makeRow } from '../../components/ui/KeyValueEditor';
import { decodeQueryComponent, parseQueryString, splitUrl } from '../../lib/query';
import type { ParamRow } from './model';

const VAR_SPLIT = /(\{\{[^{}]*\}\})/;

/**
 * Minimal encoding for a query component typed in the table: only characters that would change
 * the meaning of the query (or are invalid in URLs) are escaped; Unicode and `{{vars}}` stay
 * readable.
 */
export function encodeQueryComponent(s: string, isName = false): string {
  return s
    .split(VAR_SPLIT)
    .map((part, i) => {
      if (i % 2 === 1) return part; // a {{variable}}
      let out = '';
      for (const ch of part) {
        const code = ch.codePointAt(0) ?? 0;
        if (ch === '%' || ch === '&' || ch === '#' || ch === '+' || ch === '"' || ch === '<' || ch === '>' || ch === '`' || ch === '\\' || (isName && ch === '=')) {
          out += encodeURIComponent(ch);
        } else if (ch === ' ') {
          out += '%20';
        } else if (code < 0x20 || code === 0x7f) {
          out += encodeURIComponent(ch);
        } else {
          out += ch;
        }
      }
      return out;
    })
    .join('');
}

export interface ParsedParam {
  name: string;
  value: string;
  rawName: string;
  rawValue: string;
  noEquals: boolean;
}

/** Params currently in the URL, in order (duplicates kept, empty segments skipped). */
export function paramsFromUrl(url: string): ParsedParam[] {
  return parseQueryString(splitUrl(url).query).map((p) => ({ name: p.name, value: p.value, rawName: p.rawName, rawValue: p.rawValue ?? '', noEquals: p.rawValue == null }));
}

function encodedName(row: ParamRow): string {
  return row.rawName !== undefined && decodeQueryComponent(row.rawName) === row.name ? row.rawName : encodeQueryComponent(row.name, true);
}

function encodedValue(row: ParamRow): string {
  return row.rawValue !== undefined && decodeQueryComponent(row.rawValue) === row.value ? row.rawValue : encodeQueryComponent(row.value);
}

/** Enabled, non-empty rows → raw query string (no `?`). */
export function buildQueryFromRows(rows: readonly ParamRow[]): string {
  const segs: string[] = [];
  for (const r of rows) {
    if (!r.enabled || (r.name === '' && r.value === '')) continue;
    const name = encodedName(r);
    segs.push(r.noEquals && r.value === '' ? name : `${name}=${encodedValue(r)}`);
  }
  return segs.join('&');
}

/** Replace the URL's query string with the enabled rows (base and fragment untouched). */
export function urlWithParams(url: string, rows: readonly ParamRow[]): string {
  const { base, hash } = splitUrl(url);
  const q = buildQueryFromRows(rows);
  return `${base}${q ? `?${q}` : ''}${hash}`;
}

function sameParam(row: ParamRow, p: ParsedParam): boolean {
  return row.enabled && row.name === p.name && row.value === p.value && row.rawName === p.rawName && row.rawValue === p.rawValue && !!row.noEquals === p.noEquals;
}

/**
 * Reconcile table rows with a (new) URL: enabled rows take the URL's params in order (keeping
 * their ids so focus/keys stay stable), disabled rows keep their place, extra URL params are
 * appended and surplus enabled rows dropped. Returns the same array when nothing changed.
 */
export function mergeParamsFromUrl(rows: readonly ParamRow[], url: string): ParamRow[] {
  const parsed = paramsFromUrl(url);
  const out: ParamRow[] = [];
  let i = 0;
  let changed = false;
  for (const row of rows) {
    if (!row.enabled) {
      out.push(row);
      continue;
    }
    const p = parsed[i];
    if (!p) {
      changed = true; // surplus enabled row
      continue;
    }
    i++;
    if (sameParam(row, p)) out.push(row);
    else {
      changed = true;
      out.push({ ...row, enabled: true, name: p.name, value: p.value, rawName: p.rawName, rawValue: p.rawValue, noEquals: p.noEquals || undefined });
    }
  }
  for (; i < parsed.length; i++) {
    const p = parsed[i] as ParsedParam;
    changed = true;
    out.push({ ...makeRow(p.name, p.value), rawName: p.rawName, rawValue: p.rawValue, noEquals: p.noEquals || undefined });
  }
  return changed ? out : (rows as ParamRow[]);
}

/** Params rows for a URL (fresh ids). */
export function rowsFromUrl(url: string): ParamRow[] {
  return mergeParamsFromUrl([], url);
}
