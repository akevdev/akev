/**
 * Command palette ranking. Deterministic and predictable (unlike pure fuzzy scoring, which
 * happily ranks "Launch Edge — Microsoft Edge is not installed" above "Theme: Dark" for the
 * query `theme`).
 *
 * Every whitespace-separated query token must match the title or the secondary text
 * (subtitle, group, keywords). Per token, best match wins:
 *
 * | match                                   | score |
 * |-----------------------------------------|-------|
 * | title starts with token                 | 1.00  |
 * | a title word starts with token          | 0.90  |
 * | title contains token                    | 0.75  |
 * | a secondary word starts with token      | 0.60  |
 * | secondary text contains token           | 0.45  |
 * | token is a subsequence of the title     | 0.15–0.35 (tighter = higher) |
 *
 * The command score is the mean of its token scores, plus a small bonus when the whole query
 * is a prefix of the title. 0 = hidden.
 */

const WORD_SPLIT = /[\s:/.,·—()[\]-]+/;

function words(text: string): string[] {
  return text.split(WORD_SPLIT).filter(Boolean);
}

/** Subsequence score in (0, 1], or 0 when `token` is not a subsequence of `text`. */
function subsequence(text: string, token: string): number {
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
  const span = last - first + 1;
  return token.length / span;
}

function tokenScore(token: string, title: string, titleWords: string[], secondary: string, secondaryWords: string[]): number {
  if (title.startsWith(token)) return 1;
  if (titleWords.some((w) => w.startsWith(token))) return 0.9;
  if (title.includes(token)) return 0.75;
  if (secondaryWords.some((w) => w.startsWith(token))) return 0.6;
  if (secondary.includes(token)) return 0.45;
  // Fuzzy only against the title, and only for tokens long enough to be meaningful.
  if (token.length >= 2) {
    const s = subsequence(title, token);
    if (s > 0) return 0.15 + 0.2 * s;
  }
  return 0;
}

/**
 * Score a command for a search query (0 = no match, higher = better, max ≈ 1.1).
 * `secondary` holds subtitle, group and keywords.
 */
export function scoreCommand(query: string, title: string, secondary: readonly string[] = []): number {
  const q = query.trim().toLowerCase();
  if (!q) return 1;
  const t = title.toLowerCase();
  const sec = secondary.join(' ').toLowerCase();
  const tWords = words(t);
  const sWords = words(sec);
  const tokens = q.split(/\s+/);
  let total = 0;
  for (const token of tokens) {
    const s = tokenScore(token, t, tWords, sec, sWords);
    if (s === 0) return 0;
    total += s;
  }
  const bonus = t.startsWith(q) ? 0.1 : 0;
  return total / tokens.length + bonus;
}
