/**
 * A tiny, XSS-safe Markdown parser producing a plain AST (rendered by React, which escapes all
 * text). Supports the subset people actually put in API docs, READMEs and error explanations:
 * ATX/Setext headings, paragraphs, emphasis, strong, strikethrough, inline code, fenced and
 * indented code blocks, block quotes, ordered/unordered/task lists, thematic breaks, GFM tables,
 * links and autolinks. Raw HTML is shown as text. Images are never loaded (privacy): they become
 * link placeholders. Only `http(s):`, `mailto:` and in-page `#anchors` are linkable.
 */

export type Inline =
  | { t: 'text'; v: string }
  | { t: 'code'; v: string }
  | { t: 'strong'; c: Inline[] }
  | { t: 'em'; c: Inline[] }
  | { t: 'del'; c: Inline[] }
  | { t: 'link'; href: string | null; c: Inline[] }
  | { t: 'image'; alt: string; src: string | null }
  | { t: 'br' };

export type Align = 'left' | 'center' | 'right' | null;

export type Block =
  | { t: 'heading'; level: 1 | 2 | 3 | 4 | 5 | 6; c: Inline[] }
  | { t: 'paragraph'; c: Inline[] }
  | { t: 'code'; lang: string; v: string }
  | { t: 'quote'; c: Block[] }
  | { t: 'list'; ordered: boolean; start: number; items: { task: boolean | null; c: Block[] }[] }
  | { t: 'hr' }
  | { t: 'table'; align: Align[]; head: Inline[][]; rows: Inline[][][] };

/** A link target if it is safe to open, else `null`. */
export function safeHref(raw: string): string | null {
  const href = raw.trim().replace(/^<|>$/g, '');
  if (/^#[\w\-.:]*$/.test(href)) return href;
  // Strip whitespace/control chars browsers ignore inside schemes (`java\tscript:`).
  const probe = href.replace(/[\u0000- \u007f]/g, '').toLowerCase();
  if (/^(https?:|mailto:)/.test(probe)) return href;
  return null;
}

// =====================================================================================
// Inline parsing
// =====================================================================================

const ESCAPABLE = /[\\`*_{}[\]()#+\-.!|~<>"']/;

/** Find the closing bracket for `[` at `start`, honouring nesting and code spans. */
function matchBracket(s: string, start: number): number {
  let depth = 0;
  for (let i = start; i < s.length; i++) {
    const ch = s[i];
    if (ch === '\\') {
      i++;
      continue;
    }
    if (ch === '`') {
      const end = s.indexOf('`', i + 1);
      if (end > 0) i = end;
      continue;
    }
    if (ch === '[') depth++;
    else if (ch === ']') {
      depth--;
      if (depth === 0) return i;
    }
  }
  return -1;
}

/** Parse `(url "title")` after a link label; returns [url, endIndex] or null. */
function linkTarget(s: string, start: number): [string, number] | null {
  if (s[start] !== '(') return null;
  let depth = 0;
  for (let i = start; i < s.length; i++) {
    const ch = s[i];
    if (ch === '\\') {
      i++;
      continue;
    }
    if (ch === '(') depth++;
    else if (ch === ')') {
      depth--;
      if (depth === 0) {
        const inner = s.slice(start + 1, i).trim();
        const url = inner.replace(/\s+("[^"]*"|'[^']*'|\([^)]*\))\s*$/, '');
        return [url, i + 1];
      }
    } else if (ch === '\n') return null;
  }
  return null;
}

function pushText(out: Inline[], v: string): void {
  if (!v) return;
  const last = out[out.length - 1];
  if (last && last.t === 'text') last.v += v;
  else out.push({ t: 'text', v });
}

/** Parse inline Markdown. `depth` bounds recursion on adversarial input. */
export function parseInline(s: string, depth = 0): Inline[] {
  const out: Inline[] = [];
  if (depth > 8) {
    pushText(out, s);
    return out;
  }
  let i = 0;
  let buf = '';
  const flush = () => {
    pushText(out, buf);
    buf = '';
  };
  while (i < s.length) {
    const ch = s[i] as string;
    // Escapes
    if (ch === '\\' && i + 1 < s.length) {
      const next = s[i + 1] as string;
      if (next === '\n') {
        flush();
        out.push({ t: 'br' });
        i += 2;
        continue;
      }
      if (ESCAPABLE.test(next)) {
        buf += next;
        i += 2;
        continue;
      }
    }
    // Hard line break: two+ spaces before newline
    if (ch === '\n') {
      if (/ {2,}$/.test(buf)) {
        buf = buf.replace(/ +$/, '');
        flush();
        out.push({ t: 'br' });
      } else buf += ' ';
      i++;
      continue;
    }
    // Code spans
    if (ch === '`') {
      let ticks = 1;
      while (s[i + ticks] === '`') ticks++;
      const fence = '`'.repeat(ticks);
      const end = s.indexOf(fence, i + ticks);
      if (end > 0) {
        flush();
        let v = s.slice(i + ticks, end).replace(/\n/g, ' ');
        if (v.length > 1 && v.startsWith(' ') && v.endsWith(' ') && v.trim()) v = v.slice(1, -1);
        out.push({ t: 'code', v });
        i = end + ticks;
        continue;
      }
      buf += fence;
      i += ticks;
      continue;
    }
    // Images & links
    if ((ch === '!' && s[i + 1] === '[') || ch === '[') {
      const open = ch === '!' ? i + 1 : i;
      const close = matchBracket(s, open);
      if (close > 0) {
        const target = linkTarget(s, close + 1);
        if (target) {
          flush();
          const label = s.slice(open + 1, close);
          if (ch === '!') out.push({ t: 'image', alt: label, src: safeHref(target[0]) });
          else out.push({ t: 'link', href: safeHref(target[0]), c: parseInline(label, depth + 1) });
          i = target[1];
          continue;
        }
      }
    }
    // Autolinks <https://…>
    if (ch === '<') {
      const m = /^<((?:https?:\/\/|mailto:)[^\s<>]+)>/i.exec(s.slice(i));
      if (m?.[1]) {
        flush();
        out.push({ t: 'link', href: safeHref(m[1]), c: [{ t: 'text', v: m[1] }] });
        i += m[0].length;
        continue;
      }
    }
    // Bare URLs
    if ((ch === 'h' || ch === 'H') && /^https?:\/\//i.test(s.slice(i, i + 8)) && (i === 0 || /[\s(]/.test(s[i - 1] as string))) {
      const m = /^https?:\/\/[^\s<>]+[^\s<>.,;:!?'")\]]/i.exec(s.slice(i));
      if (m) {
        flush();
        out.push({ t: 'link', href: safeHref(m[0]), c: [{ t: 'text', v: m[0] }] });
        i += m[0].length;
        continue;
      }
    }
    // Strong / emphasis / strikethrough
    if (ch === '*' || ch === '_' || ch === '~') {
      let run = 1;
      while (s[i + run] === ch) run++;
      const prev = i > 0 ? (s[i - 1] as string) : ' ';
      const next = s[i + run] ?? ' ';
      // `_` inside words (snake_case) is literal.
      const intraword = ch === '_' && /[\p{L}\p{N}]/u.test(prev);
      const canOpen = !/\s/.test(next) && !intraword;
      if (canOpen && (ch !== '~' || run === 2)) {
        const n = ch === '~' ? 2 : Math.min(run, 3);
        const marker = ch.repeat(n);
        const end = findCloser(s, i + n, marker, ch);
        if (end > 0) {
          flush();
          const inner = parseInline(s.slice(i + n, end), depth + 1);
          if (ch === '~') out.push({ t: 'del', c: inner });
          else if (n === 3) out.push({ t: 'strong', c: [{ t: 'em', c: inner }] });
          else if (n === 2) out.push({ t: 'strong', c: inner });
          else out.push({ t: 'em', c: inner });
          i = end + n;
          continue;
        }
      }
      buf += ch.repeat(run);
      i += run;
      continue;
    }
    buf += ch;
    i++;
  }
  flush();
  return out;
}

function findCloser(s: string, from: number, marker: string, ch: string): number {
  let i = from;
  while (i < s.length) {
    const c = s[i];
    if (c === '\\') {
      i += 2;
      continue;
    }
    if (c === '`') {
      const end = s.indexOf('`', i + 1);
      i = end > 0 ? end + 1 : i + 1;
      continue;
    }
    if (s.startsWith(marker, i) && !/\s/.test(s[i - 1] ?? ' ') && s[i + marker.length] !== ch) {
      const after = s[i + marker.length] ?? ' ';
      if (!(ch === '_' && /[\p{L}\p{N}]/u.test(after))) return i;
    }
    i++;
  }
  return -1;
}

// =====================================================================================
// Block parsing
// =====================================================================================

const FENCE = /^ {0,3}(`{3,}|~{3,})\s*([^`\s]*)[^`]*$/;
const HEADING = /^ {0,3}(#{1,6})(?:[ \t]+(.*?))?(?:[ \t]+#+)?[ \t]*$/;
const HR = /^ {0,3}((\*[ \t]*){3,}|(-[ \t]*){3,}|(_[ \t]*){3,})$/;
const BULLET = /^( {0,3})([-*+])([ \t]+|$)(.*)$/;
const ORDERED = /^( {0,3})(\d{1,9})([.)])([ \t]+|$)(.*)$/;
const QUOTE = /^ {0,3}> ?(.*)$/;
const TABLE_SEP = /^ {0,3}\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$/;

function splitRow(line: string): string[] {
  let l = line.trim();
  if (l.startsWith('|')) l = l.slice(1);
  if (l.endsWith('|') && !l.endsWith('\\|')) l = l.slice(0, -1);
  const cells: string[] = [];
  let cur = '';
  for (let i = 0; i < l.length; i++) {
    const ch = l[i];
    if (ch === '\\' && l[i + 1] === '|') {
      cur += '|';
      i++;
    } else if (ch === '|') {
      cells.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  cells.push(cur.trim());
  return cells;
}

function isBlockStart(line: string): boolean {
  return FENCE.test(line) || HEADING.test(line) || HR.test(line) || BULLET.test(line) || ORDERED.test(line) || QUOTE.test(line);
}

/** Parse Markdown into blocks. Input is capped by the caller (previews limit size). */
export function parseMarkdown(src: string, depth = 0): Block[] {
  const lines = src.replace(/\r\n?/g, '\n').replace(/\t/g, '    ').split('\n');
  const blocks: Block[] = [];
  let i = 0;
  while (i < lines.length) {
    const line = lines[i] as string;
    if (!line.trim()) {
      i++;
      continue;
    }
    // Fenced code
    const fence = FENCE.exec(line);
    if (fence?.[1]) {
      const marker = fence[1];
      const body: string[] = [];
      const close = new RegExp(`^ {0,3}${marker[0] === '`' ? '`' : '~'}{${marker.length},}\\s*$`);
      i++;
      while (i < lines.length && !close.test(lines[i] as string)) body.push(lines[i++] as string);
      i++;
      blocks.push({ t: 'code', lang: fence[2] ?? '', v: body.join('\n') });
      continue;
    }
    // Indented code
    if (/^ {4,}\S/.test(line)) {
      const body: string[] = [];
      while (i < lines.length && (/^ {4}/.test(lines[i] as string) || !(lines[i] as string).trim())) body.push((lines[i++] as string).slice(4));
      while (body.length && !body[body.length - 1]?.trim()) body.pop();
      blocks.push({ t: 'code', lang: '', v: body.join('\n') });
      continue;
    }
    const heading = HEADING.exec(line);
    if (heading?.[1]) {
      blocks.push({ t: 'heading', level: heading[1].length as 1 | 2 | 3 | 4 | 5 | 6, c: parseInline((heading[2] ?? '').trim(), 0) });
      i++;
      continue;
    }
    if (HR.test(line)) {
      blocks.push({ t: 'hr' });
      i++;
      continue;
    }
    // Block quote
    if (QUOTE.test(line)) {
      const body: string[] = [];
      while (i < lines.length && (lines[i] as string).trim()) {
        const m = QUOTE.exec(lines[i] as string);
        body.push(m ? (m[1] ?? '') : (lines[i] as string));
        i++;
      }
      blocks.push({ t: 'quote', c: depth < 6 ? parseMarkdown(body.join('\n'), depth + 1) : [{ t: 'paragraph', c: [{ t: 'text', v: body.join(' ') }] }] });
      continue;
    }
    // Lists
    const bullet = BULLET.exec(line);
    const ordered = ORDERED.exec(line);
    if (bullet || ordered) {
      const isOrdered = !bullet;
      const start = ordered ? Number(ordered[2]) : 1;
      const items: { task: boolean | null; c: Block[] }[] = [];
      while (i < lines.length) {
        const l = lines[i] as string;
        const m = isOrdered ? ORDERED.exec(l) : BULLET.exec(l);
        if (!m) break;
        const indent = (m[1]?.length ?? 0) + (isOrdered ? (m[2]?.length ?? 0) + 1 : 1) + Math.max(1, (isOrdered ? m[4] : m[3])?.length ?? 1);
        const first = (isOrdered ? m[5] : m[4]) ?? '';
        const body: string[] = [first];
        i++;
        while (i < lines.length) {
          const next = lines[i] as string;
          if (!next.trim()) {
            // Blank line: continue only if the following line is indented (loose list).
            const after = lines[i + 1];
            if (after != null && after.startsWith(' '.repeat(Math.min(indent, 4))) && after.trim()) {
              body.push('');
              i++;
              continue;
            }
            break;
          }
          if ((isOrdered ? ORDERED : BULLET).test(next) && !/^ {2,}/.test(next)) break;
          if (/^ {2,}/.test(next)) body.push(next.replace(new RegExp(`^ {1,${indent}}`), ''));
          else if (isBlockStart(next)) break;
          else body.push(next.trim()); // lazy continuation
          i++;
        }
        let task: boolean | null = null;
        const tm = /^\[([ xX])\]\s+/.exec(body[0] ?? '');
        if (tm) {
          task = tm[1] !== ' ';
          body[0] = (body[0] ?? '').slice(tm[0].length);
        }
        items.push({ task, c: depth < 6 ? parseMarkdown(body.join('\n'), depth + 1) : [{ t: 'paragraph', c: parseInline(body.join(' ')) }] });
        while (i < lines.length && !(lines[i] as string).trim() && (isOrdered ? ORDERED : BULLET).test(lines[i + 1] ?? '')) i++;
      }
      blocks.push({ t: 'list', ordered: isOrdered, start, items });
      continue;
    }
    // Tables
    const sep = lines[i + 1];
    if (line.includes('|') && sep != null && TABLE_SEP.test(sep) && sep.includes('-')) {
      const head = splitRow(line);
      const align: Align[] = splitRow(sep).map((c) => (c.startsWith(':') && c.endsWith(':') ? 'center' : c.endsWith(':') ? 'right' : c.startsWith(':') ? 'left' : null));
      i += 2;
      const rows: Inline[][][] = [];
      while (i < lines.length && (lines[i] as string).includes('|') && (lines[i] as string).trim()) {
        const cells = splitRow(lines[i] as string);
        rows.push(head.map((_, k) => parseInline(cells[k] ?? '')));
        i++;
      }
      blocks.push({ t: 'table', align: head.map((_, k) => align[k] ?? null), head: head.map((h) => parseInline(h)), rows });
      continue;
    }
    // Paragraph (with Setext heading detection)
    // Leading indentation only: trailing double spaces are a hard line break.
    const para: string[] = [line.trimStart()];
    i++;
    let setext: 1 | 2 | null = null;
    while (i < lines.length) {
      const next = lines[i] as string;
      if (!next.trim()) break;
      if (/^ {0,3}=+\s*$/.test(next)) {
        setext = 1;
        i++;
        break;
      }
      if (/^ {0,3}-+\s*$/.test(next) && para.length) {
        setext = 2;
        i++;
        break;
      }
      if (isBlockStart(next)) break;
      para.push(next.replace(/^ +/, ''));
      i++;
    }
    const content = parseInline(para.join('\n'));
    blocks.push(setext ? { t: 'heading', level: setext, c: content } : { t: 'paragraph', c: content });
  }
  return blocks;
}

/** Plain text of inline nodes (for heading anchors, alt text, tests). */
export function inlineText(nodes: readonly Inline[]): string {
  return nodes
    .map((n) => {
      switch (n.t) {
        case 'text':
        case 'code':
          return n.v;
        case 'br':
          return '\n';
        case 'image':
          return n.alt;
        default:
          return inlineText(n.c);
      }
    })
    .join('');
}
