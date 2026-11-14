/**
 * Client-side fuzzy search used by the Connect catalogue.
 *
 * Deliberately predictable: every whitespace-separated query token must match at least one
 * field (AND), and per token the best (match quality × field weight) wins. Match quality,
 * best first:
 *
 * | match                                          | quality |
 * |------------------------------------------------|---------|
 * | field equals token                             | 1.00    |
 * | field starts with token                        | 0.95    |
 * | a word equals token                            | 0.92    |
 * | a word starts with token                       | 0.85    |
 * | token is the acronym of the words (`ft`)       | 0.70    |
 * | field contains token                           | 0.60    |
 * | one typo away from a word / word prefix (≥ 4)  | 0.45    |
 * | token is a tight subsequence of the field      | 0.10–0.40 |
 *
 * The document score is the mean token score plus a bonus when the primary field starts
 * with the whole query. Ties keep the input order (stable), so catalogue order is preserved.
 */

export interface SearchField {
  text: string;
  /** Relative importance (name 1, description ~0.5). */
  weight: number;
}

const WORD_SPLIT = /[\s\-_/.,·:()[\]+]+/;

/** Lowercase and strip diacritics (`Électron` → `electron`). */
export function normalizeText(text: string): string {
  return text.toLowerCase().normalize('NFKD').replace(/[̀-ͯ]/g, '');
}

function wordsOf(text: string): string[] {
  return text.split(WORD_SPLIT).filter(Boolean);
}

/**
 * True when `a` and `b` differ by at most one edit (insertion, deletion, substitution or a
 * swap of two adjacent characters).
 */
export function withinOneEdit(a: string, b: string): boolean {
  if (a === b) return true;
  const la = a.length;
  const lb = b.length;
  if (Math.abs(la - lb) > 1) return false;
  let i = 0;
  while (i < la && i < lb && a[i] === b[i]) i++;
  if (la === lb) {
    // substitution: rest equal after skipping one char each
    if (a.slice(i + 1) === b.slice(i + 1)) return true;
    // transposition of a[i], a[i+1]
    return a[i] === b[i + 1] && a[i + 1] === b[i] && a.slice(i + 2) === b.slice(i + 2);
  }
  // insertion / deletion
  return la > lb ? a.slice(i + 1) === b.slice(i) : a.slice(i) === b.slice(i + 1);
}

/** Subsequence tightness in (0, 1], or 0 when `token` is not a subsequence of `text`. */
export function subsequenceScore(text: string, token: string): number {
  let ti = 0;
  let first = -1;
  let last = -1;
  for (let i = 0; i < text.length && ti < token.length; i++) {
    if (text[i] === token[ti]) {
      if (first < 0) first = i;
      last = i;
      ti++;
    }
  }
  if (ti < token.length) return 0;
  return token.length / (last - first + 1);
}

/** Match quality (0..1) of one normalised token against one normalised text. */
export function tokenMatch(token: string, text: string): number {
  if (!token) return 1;
  if (!text) return 0;
  if (text === token) return 1;
  if (text.startsWith(token)) return 0.95;
  const words = wordsOf(text);
  if (words.includes(token)) return 0.92;
  if (words.some((w) => w.startsWith(token))) return 0.85;
  if (token.length >= 2 && words.length >= token.length && words.map((w) => w[0]).join('').startsWith(token)) return 0.7;
  if (text.includes(token)) return 0.6;
  if (token.length >= 4 && words.some((w) => withinOneEdit(token, w) || (w.length > token.length && withinOneEdit(token, w.slice(0, token.length))))) return 0.45;
  if (token.length >= 2) {
    const s = subsequenceScore(text, token);
    // Loose subsequences ("ce" in "description") are noise, not matches.
    if (s >= 0.5) return 0.1 + 0.3 * s;
  }
  return 0;
}

/**
 * Score a document (its fields, first = primary) for a query. 0 = no match. An empty query
 * matches everything with score 1.
 */
export function scoreFields(query: string, fields: readonly SearchField[]): number {
  const q = normalizeText(query.trim());
  if (!q) return 1;
  const normalized = fields.map((f) => ({ text: normalizeText(f.text), weight: f.weight }));
  const tokens = q.split(/\s+/).filter(Boolean);
  let total = 0;
  for (const token of tokens) {
    let best = 0;
    for (const f of normalized) {
      const m = tokenMatch(token, f.text);
      if (m > 0) best = Math.max(best, m * f.weight);
    }
    if (best === 0) return 0;
    total += best;
  }
  const primary = normalized[0]?.text ?? '';
  const bonus = primary.startsWith(q) ? 0.25 : 0;
  return total / tokens.length + bonus;
}

/**
 * Filter and rank items by a query. `tieBreak` adds a tiny per-item boost (e.g. available
 * before unavailable) that only reorders otherwise-equal matches. Stable.
 */
export function rankByQuery<T>(items: readonly T[], query: string, fieldsOf: (item: T) => readonly SearchField[], tieBreak?: (item: T) => number): T[] {
  if (!query.trim()) return [...items];
  const scored: { item: T; score: number; index: number }[] = [];
  items.forEach((item, index) => {
    const s = scoreFields(query, fieldsOf(item));
    if (s > 0) scored.push({ item, score: s + (tieBreak ? tieBreak(item) * 0.001 : 0), index });
  });
  scored.sort((a, b) => b.score - a.score || a.index - b.index);
  return scored.map((s) => s.item);
}
