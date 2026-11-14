/**
 * Pure text operations behind the filter bar: applying autocomplete suggestions (UTF-8 byte
 * ranges), appending clauses from context menus ("Filter ▸ Same host"), quoting values.
 */

import type { Suggestion } from '../../../api/types';
import { byteOffsetToIndex } from '../../../lib/encoding';
import { hasTopLevelOr, tokenizeFilter } from './filterTokens';

export interface TextEdit {
  text: string;
  /** Caret position (UTF-16 index) after the edit. */
  caret: number;
}

/**
 * Apply a suggestion: replace `[replaceStart, replaceEnd)` (UTF-8 byte offsets) with
 * `insert`. A space is appended after completed values/flags so the user can keep typing
 * the next clause; field/operator completions (ending in `:`/`=`…) leave the caret right
 * after them so value suggestions follow.
 */
export function applySuggestion(text: string, s: Pick<Suggestion, 'insert' | 'replaceStart' | 'replaceEnd' | 'kind'>): TextEdit {
  const start = byteOffsetToIndex(text, s.replaceStart);
  const end = Math.max(start, byteOffsetToIndex(text, s.replaceEnd));
  const before = text.slice(0, start);
  const after = text.slice(end);
  let insert = s.insert;
  // Completed values get a separating space (unless one already follows).
  if (!suggestionContinues(insert, s.kind) && !/^\s/.test(after)) insert += ' ';
  return { text: before + insert + after, caret: before.length + insert.length };
}

/** Whether a completed suggestion should immediately re-open the dropdown (field → values). */
export function suggestionContinues(insert: string, kind: Suggestion['kind']): boolean {
  return /[:=<>~^$*.,(]$/.test(insert) || kind === 'field' || kind === 'operator';
}

/** Whitespace and syntax characters; `*` because a bare `*` is a wildcard (`header.accept:*`). */
const NEEDS_QUOTES = /[\s"'(),|&*]/;
/** Leading characters that would read as negation, a regex, or part of the operator (`host==x`). */
const BAD_START = /^[-/!=~<>^$:]/;

/** Quote a filter value when it contains whitespace or syntax characters, so it matches literally. */
export function quoteFilterValue(value: string): string {
  if (value === '') return '""';
  if (!NEEDS_QUOTES.test(value) && !BAD_START.test(value)) return value;
  return `"${value.replace(/\\/g, '\\\\').replace(/"/g, '\\"')}"`;
}

/** `field<op>value`, quoting the value when needed (`host=api.example.com`). */
export function clause(field: string, op: string, value: string, negate = false): string {
  return `${negate ? '-' : ''}${field}${op}${quoteFilterValue(value)}`;
}

/**
 * Append an AND clause to the user's filter text. Existing text with a top-level OR is
 * parenthesised so the new clause applies to all of it. Clauses already present (as a
 * whole token sequence) are not duplicated.
 */
export function appendClause(text: string, newClause: string): string {
  const current = text.trim();
  if (!current) return newClause;
  // Whitespace-separated terms (quotes respected by the tokenizer) for duplicate detection.
  const terms: string[] = [];
  let acc = '';
  for (const t of tokenizeFilter(current)) {
    if (t.kind === 'space') {
      if (acc) terms.push(acc);
      acc = '';
    } else acc += t.text;
  }
  if (acc) terms.push(acc);
  const hasOr = hasTopLevelOr(current);
  if (!hasOr && terms.includes(newClause.trim())) return current;
  const base = hasOr ? `(${current})` : current;
  return `${base} ${newClause}`;
}

/** Path without the query string (`/a/b?x=1` → `/a/b`). */
export function pathOnly(pathAndQuery: string): string {
  const q = pathAndQuery.indexOf('?');
  return q < 0 ? pathAndQuery : pathAndQuery.slice(0, q);
}

/**
 * Step through the recent-filter history with ↑/↓. `index` is -1 when not browsing.
 * Returns the new index and the text to show (`null` = restore the draft).
 */
export function historyStep(recent: readonly string[], index: number, direction: 1 | -1): { index: number; text: string | null } {
  if (recent.length === 0) return { index: -1, text: null };
  const next = Math.max(-1, Math.min(recent.length - 1, index + direction));
  return { index: next, text: next < 0 ? null : (recent[next] as string) };
}
