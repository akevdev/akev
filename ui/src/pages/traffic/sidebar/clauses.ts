/** Sidebar clauses inside the user's filter text: find and remove whole terms. No React. */

/** Whether `clause` appears in `text` as a whole term. */
export function hasClause(text: string, c: string): boolean {
  return clauseIndex(text, c) >= 0;
}

function clauseIndex(text: string, c: string): number {
  let from = 0;
  for (;;) {
    const i = text.indexOf(c, from);
    if (i < 0) return -1;
    const before = i === 0 || /[\s(]/.test(text[i - 1] as string);
    const after = i + c.length === text.length || /[\s)]/.test(text[i + c.length] as string);
    if (before && after) return i;
    from = i + 1;
  }
}

/** Remove a whole-term clause, tidying the whitespace (and parentheses `appendClause` added). */
export function removeClause(text: string, c: string): string {
  const i = clauseIndex(text, c);
  if (i < 0) return text;
  let out = `${text.slice(0, i)} ${text.slice(i + c.length)}`.replace(/\s+/g, ' ').trim();
  // `(a | b) clause` was made by appendClause: unwrap when the group is all that's left.
  const m = /^\((.*)\)$/.exec(out);
  if (m && balanced(m[1] as string)) out = (m[1] as string).trim();
  return out;
}

function balanced(s: string): boolean {
  let depth = 0;
  for (const ch of s) {
    if (ch === '(') depth++;
    else if (ch === ')' && --depth < 0) return false;
  }
  return depth === 0;
}
