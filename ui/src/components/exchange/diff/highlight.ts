/**
 * Syntax highlighting for diff lines without an editor: parse the whole text with the
 * CodeMirror language's Lezer parser and emit per-line class tokens. Everything (Lezer
 * highlight, the grammar) is loaded lazily, so TextDiff costs nothing until it has text to
 * colour. Class names are short ids mapped to `--syn-*` tokens by TextDiff.module.css.
 */

import type { Parser } from '@lezer/common';
import { tokensByLine, type LineToken } from './pieces';
import { loadLanguage, normalizeLanguage, type CodeLanguage } from '../../ui/codemirror/languages';

/** Skip highlighting above this many characters per side (plain text stays fast). */
export const HIGHLIGHT_LIMIT = 400_000;

type Highlighter = import('@lezer/highlight').Highlighter;
type HighlightModule = typeof import('@lezer/highlight');

let highlighterPromise: Promise<{ mod: HighlightModule; highlighter: Highlighter }> | null = null;

function loadHighlighter() {
  highlighterPromise ??= import('@lezer/highlight').then((mod) => {
    const t = mod.tags;
    const highlighter = mod.tagHighlighter([
      { tag: [t.propertyName, t.definition(t.propertyName), t.attributeName], class: 'k' },
      { tag: [t.string, t.special(t.string), t.regexp, t.character, t.attributeValue], class: 's' },
      { tag: [t.number, t.integer, t.float], class: 'n' },
      { tag: [t.bool, t.null, t.atom, t.special(t.variableName)], class: 'b' },
      { tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword, t.modifier], class: 'kw' },
      { tag: [t.tagName, t.angleBracket], class: 'tg' },
      { tag: [t.function(t.variableName), t.function(t.propertyName)], class: 'fn' },
      { tag: [t.typeName, t.className, t.namespace], class: 'ty' },
      { tag: [t.meta, t.processingInstruction, t.documentMeta, t.annotation], class: 'm' },
      { tag: [t.comment, t.lineComment, t.blockComment, t.docComment], class: 'c' },
      { tag: [t.punctuation, t.separator, t.bracket, t.squareBracket, t.brace, t.paren, t.operator, t.derefOperator], class: 'p' },
      { tag: [t.heading, t.heading1, t.heading2, t.heading3], class: 'h' },
      { tag: [t.link, t.url], class: 'l' },
    ]);
    return { mod, highlighter };
  });
  return highlighterPromise;
}

/** Pull a Lezer parser out of whatever `loadLanguage` returned (LanguageSupport / Language). */
function parserOf(ext: unknown): Parser | null {
  const probe = (x: unknown): Parser | null => {
    if (!x || typeof x !== 'object') return null;
    const o = x as { language?: { parser?: Parser }; parser?: Parser };
    if (o.language?.parser && typeof o.language.parser.parse === 'function') return o.language.parser;
    if (o.parser && typeof o.parser.parse === 'function') return o.parser;
    return null;
  };
  if (Array.isArray(ext)) {
    for (const e of ext) {
      const p = probe(e);
      if (p) return p;
    }
    return null;
  }
  return probe(ext);
}

/**
 * Highlight `text` as `language`, returning tokens per line (aligned with `splitLines`), or
 * `null` when there's nothing to highlight (plain text, too large, unknown grammar, failure).
 */
export async function highlightLines(text: string, language: string | null | undefined): Promise<LineToken[][] | null> {
  const lang: CodeLanguage = normalizeLanguage(language);
  if (lang === 'text' || !text || text.length > HIGHLIGHT_LIMIT) return null;
  try {
    const [{ mod, highlighter }, ext] = await Promise.all([loadHighlighter(), loadLanguage(lang)]);
    const parser = parserOf(ext);
    if (!parser) return null;
    const tree = parser.parse(text);
    const ranges: [number, number, string][] = [];
    mod.highlightTree(tree, highlighter, (from, to, cls) => {
      ranges.push([from, to, cls.split(' ')[0] as string]);
    });
    ranges.sort((a, b) => a[0] - b[0]);
    return tokensByLine(text, ranges);
  } catch {
    return null;
  }
}
