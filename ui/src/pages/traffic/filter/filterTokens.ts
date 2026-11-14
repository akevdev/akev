/**
 * Lexical tokens of the filter language for syntax highlighting in the filter bar
 * (DESIGN.md §6.2). Mirrors the akev-filter grammar closely enough to colour what the user
 * types; validation itself is the daemon's job (`filter-result.error`).
 *
 * Tokens cover the input completely and in order (whitespace included), so the highlighted
 * mirror renders exactly the same characters as the native `<input>` above it.
 */

export type FilterTokenKind =
  | 'space'
  /** `(` `)` */
  | 'paren'
  /** `OR` `AND` `NOT` `|` `||` `&&` */
  | 'keyword'
  /** `-` / `!` prefix */
  | 'negation'
  /** Field name before an operator (`status`, `header.x-trace`, `is`). */
  | 'field'
  /** `:` `=` `!=` `^=` `$=` `*=` `~` `>` `>=` `<` `<=` */
  | 'operator'
  /** Value after an operator. */
  | 'value'
  /** Quoted value or quoted free text. */
  | 'string'
  /** `/regex/flags` value. */
  | 'regex'
  /** `,` between alternated values. */
  | 'punct'
  /** Bare free text (matches the URL). */
  | 'text';

export interface FilterToken {
  kind: FilterTokenKind;
  /** UTF-16 index range into the input. */
  start: number;
  end: number;
  text: string;
}

const OP_RE = /^([a-zA-Z][\w-]*(?:\.[\w-]+)?)(!=|\^=|\$=|\*=|>=|<=|:|=|~|>|<)/;
const WS = /\s/;

function isSpace(c: string | undefined): boolean {
  return c !== undefined && WS.test(c);
}

/** Split a raw value (after the operator) into value / string / regex / punct tokens. */
function valueTokens(raw: string, offset: number, out: FilterToken[]): void {
  if (!raw) return;
  const first = raw[0];
  if (first === '"' || first === "'") {
    out.push({ kind: 'string', start: offset, end: offset + raw.length, text: raw });
    return;
  }
  if (first === '/' && /^\/.*\/[a-z]*$/s.test(raw) && raw.length > 1) {
    out.push({ kind: 'regex', start: offset, end: offset + raw.length, text: raw });
    return;
  }
  let i = 0;
  while (i < raw.length) {
    if (raw[i] === ',') {
      out.push({ kind: 'punct', start: offset + i, end: offset + i + 1, text: ',' });
      i++;
      continue;
    }
    const j = raw.indexOf(',', i);
    const end = j < 0 ? raw.length : j;
    out.push({ kind: 'value', start: offset + i, end: offset + end, text: raw.slice(i, end) });
    i = end;
  }
}

/** Tokenize a filter query for highlighting. Never throws. */
export function tokenizeFilter(q: string): FilterToken[] {
  const out: FilterToken[] = [];
  let i = 0;
  const push = (kind: FilterTokenKind, start: number, end: number) => out.push({ kind, start, end, text: q.slice(start, end) });
  while (i < q.length) {
    const c = q[i] as string;
    if (isSpace(c)) {
      const start = i;
      while (i < q.length && isSpace(q[i])) i++;
      push('space', start, i);
      continue;
    }
    if (c === '(' || c === ')') {
      push('paren', i, i + 1);
      i++;
      continue;
    }
    if ((c === '-' || c === '!') && i + 1 < q.length && !isSpace(q[i + 1]) && q[i + 1] !== '=') {
      push('negation', i, i + 1);
      i++;
      continue;
    }
    if (c === '|' || (c === '&' && q[i + 1] === '&')) {
      const len = q[i + 1] === c ? 2 : 1;
      push('keyword', i, i + len);
      i += len;
      continue;
    }
    // A term: read until whitespace or a paren, respecting quotes.
    const start = i;
    let quote: string | null = null;
    while (i < q.length) {
      const ch = q[i] as string;
      if (quote) {
        if (ch === '\\') i++;
        else if (ch === quote) quote = null;
      } else if (ch === '"' || ch === "'") quote = ch;
      else if (isSpace(ch) || ch === '(' || ch === ')') break;
      i++;
    }
    i = Math.min(i, q.length);
    const text = q.slice(start, i);
    const upper = text.toUpperCase();
    if (upper === 'OR' || upper === 'AND' || upper === 'NOT') {
      push('keyword', start, i);
      continue;
    }
    const m = OP_RE.exec(text);
    if (m) {
      const field = m[1] as string;
      const op = m[2] as string;
      push('field', start, start + field.length);
      push('operator', start + field.length, start + field.length + op.length);
      valueTokens(text.slice(field.length + op.length), start + field.length + op.length, out);
      continue;
    }
    push(text.startsWith('"') || text.startsWith("'") ? 'string' : 'text', start, i);
  }
  return out;
}

/**
 * Whether the query has a top-level `OR` (so appending an AND clause needs parentheses
 * around the existing text to keep its meaning).
 */
export function hasTopLevelOr(q: string): boolean {
  let depth = 0;
  for (const t of tokenizeFilter(q)) {
    if (t.kind === 'paren') depth += t.text === '(' ? 1 : -1;
    else if (depth === 0 && t.kind === 'keyword' && (t.text === '|' || t.text === '||' || t.text.toUpperCase() === 'OR')) return true;
  }
  return false;
}
