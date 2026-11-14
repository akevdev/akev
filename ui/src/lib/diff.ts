/**
 * Line & intraline text diffing for the Compare dialog and TextDiff (DESIGN.md §6.5).
 *
 * Algorithm: lines are interned to integer ids, the common prefix/suffix is trimmed, then the
 * remaining range is split recursively on *patience anchors* (lines that occur exactly once on
 * both sides, kept in order via a longest increasing subsequence). Ranges without anchors fall
 * back to Myers' O(ND) greedy diff with a cost budget; when the budget is exhausted the range is
 * emitted as a plain replacement. This keeps output human-friendly (patience) while bounding the
 * worst case (two unrelated 50k-line documents) to linear-ish time.
 *
 * Pure functions only — no React, no DOM.
 */

// =====================================================================================
// Types
// =====================================================================================

export type DiffOp = 'equal' | 'insert' | 'delete';

/** A run of lines: `a[aStart, aEnd)` ↔ `b[bStart, bEnd)`. Equal runs have equal lengths. */
export interface DiffChunk {
  op: DiffOp;
  aStart: number;
  aEnd: number;
  bStart: number;
  bEnd: number;
}

/** A piece of a line for intraline highlighting. */
export interface Segment {
  text: string;
  /** Part of the change (highlighted more strongly). */
  changed: boolean;
}

export type LineKind = 'equal' | 'add' | 'del';

/** One rendered line of a diff (unified view). Line numbers are 1-based. */
export interface DiffLine {
  kind: LineKind;
  /** Line number on the left side (old), if the line exists there. */
  aNo?: number;
  /** Line number on the right side (new), if the line exists there. */
  bNo?: number;
  text: string;
  /** Intraline segments when this line is part of a changed pair. */
  segments?: Segment[];
}

/** One cell of the split view. `null` = filler (no line on this side). */
export interface SplitCell {
  no: number;
  text: string;
  kind: LineKind;
  segments?: Segment[];
}

export interface SplitRow {
  left: SplitCell | null;
  right: SplitCell | null;
}

export interface DiffStats {
  added: number;
  removed: number;
  /** Number of change blocks (consecutive non-equal runs). */
  blocks: number;
}

export interface DiffOptions {
  /** Compare lines ignoring leading/trailing whitespace and collapsing inner runs. */
  ignoreWhitespace?: boolean;
  /** Compare lines case-insensitively. */
  ignoreCase?: boolean;
  /** Myers cost budget per anchor-less range (default 2000 edit steps). */
  maxCost?: number;
  /** Skip intraline highlighting for lines longer than this (default 4000 chars). */
  maxIntralineLength?: number;
}

export interface TextDiffResult {
  identical: boolean;
  stats: DiffStats;
  /** Unified lines in display order. */
  lines: DiffLine[];
  /** Split rows in display order. */
  rows: SplitRow[];
  chunks: DiffChunk[];
}

// =====================================================================================
// Line splitting
// =====================================================================================

/**
 * Split text into lines on `\r\n`, `\n` or `\r`. A single trailing newline does not produce an
 * extra empty line; the empty string has no lines.
 */
export function splitLines(text: string): string[] {
  if (text === '') return [];
  const lines = text.split(/\r\n|\r|\n/);
  if (lines.length > 1 && lines[lines.length - 1] === '') lines.pop();
  return lines;
}

// =====================================================================================
// Core sequence diff (on integer ids)
// =====================================================================================

type Emit = (op: DiffOp, a: number, b: number, len: number) => void;

/** Longest increasing subsequence of `values` (indices into the input), O(n log n). */
function lis(values: readonly number[]): number[] {
  const n = values.length;
  if (n === 0) return [];
  const tails: number[] = []; // index into values of the smallest tail for each length
  const prev = new Int32Array(n).fill(-1);
  for (let i = 0; i < n; i++) {
    const v = values[i] as number;
    let lo = 0;
    let hi = tails.length;
    while (lo < hi) {
      const mid = (lo + hi) >> 1;
      if ((values[tails[mid] as number] as number) < v) lo = mid + 1;
      else hi = mid;
    }
    if (lo > 0) prev[i] = tails[lo - 1] as number;
    tails[lo] = i;
  }
  const out: number[] = new Array(tails.length);
  let k = tails[tails.length - 1] as number;
  for (let i = tails.length - 1; i >= 0; i--) {
    out[i] = k;
    k = prev[k] as number;
  }
  return out;
}

/** Unique-in-both anchors of a[aLo,aHi) / b[bLo,bHi), as ordered [ai, bi] pairs. */
function patienceAnchors(a: Int32Array, aLo: number, aHi: number, b: Int32Array, bLo: number, bHi: number): [number, number][] {
  // count: id -> [countA, countB, lastIndexA, lastIndexB]
  const counts = new Map<number, [number, number, number, number]>();
  for (let i = aLo; i < aHi; i++) {
    const id = a[i] as number;
    const c = counts.get(id);
    if (c) {
      c[0]++;
      c[2] = i;
    } else counts.set(id, [1, 0, i, -1]);
  }
  for (let j = bLo; j < bHi; j++) {
    const c = counts.get(b[j] as number);
    if (!c) continue;
    c[1]++;
    c[3] = j;
  }
  const pairs: [number, number][] = [];
  for (const c of counts.values()) if (c[0] === 1 && c[1] === 1) pairs.push([c[2], c[3]]);
  if (pairs.length === 0) return pairs;
  pairs.sort((x, y) => x[0] - y[0]);
  const keep = lis(pairs.map((p) => p[1]));
  return keep.map((i) => pairs[i] as [number, number]);
}

/**
 * Myers' greedy forward algorithm over a[aLo,aHi) / b[bLo,bHi) with a trace for backtracking.
 * Returns false (emitting nothing) if the edit distance exceeds `maxCost`.
 */
function myers(a: Int32Array, aLo: number, aHi: number, b: Int32Array, bLo: number, bHi: number, maxCost: number, emit: Emit): boolean {
  const n = aHi - aLo;
  const m = bHi - bLo;
  const max = Math.min(n + m, maxCost);
  const offset = max + 1;
  const v = new Int32Array(2 * max + 3);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= max; d++) {
    // Store only the relevant window [-d, d] (+1 margin) to keep memory O(D²) small.
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && (v[offset + k - 1] as number) < (v[offset + k + 1] as number))) x = v[offset + k + 1] as number;
      else x = (v[offset + k - 1] as number) + 1;
      let y = x - k;
      while (x < n && y < m && a[aLo + x] === b[bLo + y]) {
        x++;
        y++;
      }
      v[offset + k] = x;
      if (x >= n && y >= m) {
        found = d;
        break;
      }
    }
    trace.push(v.slice(offset - d - 1, offset + d + 2));
    if (found >= 0) break;
  }
  if (found < 0) return false;

  // Backtrack, collecting ops in reverse.
  const ops: [DiffOp, number, number][] = [];
  let x = n;
  let y = m;
  for (let d = found; d > 0; d--) {
    const vd = trace[d - 1] as Int32Array; // window for d-1: index = k + (d-1) + 1
    const get = (k: number) => vd[k + d] as number;
    const k = x - y;
    let prevK: number;
    if (k === -d || (k !== d && get(k - 1) < get(k + 1))) prevK = k + 1;
    else prevK = k - 1;
    const prevX = get(prevK);
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) {
      x--;
      y--;
      ops.push(['equal', x, y]);
    }
    if (x === prevX) {
      y--;
      ops.push(['insert', x, y]);
    } else {
      x--;
      ops.push(['delete', x, y]);
    }
  }
  while (x > 0 && y > 0) {
    x--;
    y--;
    ops.push(['equal', x, y]);
  }
  for (let i = ops.length - 1; i >= 0; i--) {
    const [op, ox, oy] = ops[i] as [DiffOp, number, number];
    emit(op, aLo + ox, bLo + oy, 1);
  }
  return true;
}

function diffRange(a: Int32Array, aLo: number, aHi: number, b: Int32Array, bLo: number, bHi: number, maxCost: number, emit: Emit, depth = 0): void {
  // Common prefix.
  let pre = 0;
  while (aLo + pre < aHi && bLo + pre < bHi && a[aLo + pre] === b[bLo + pre]) pre++;
  if (pre) emit('equal', aLo, bLo, pre);
  aLo += pre;
  bLo += pre;
  // Common suffix.
  let suf = 0;
  while (aHi - suf > aLo && bHi - suf > bLo && a[aHi - suf - 1] === b[bHi - suf - 1]) suf++;
  const aEnd = aHi - suf;
  const bEnd = bHi - suf;

  if (aLo === aEnd) {
    if (bLo < bEnd) emit('insert', aLo, bLo, bEnd - bLo);
  } else if (bLo === bEnd) {
    emit('delete', aLo, bLo, aEnd - aLo);
  } else {
    // Bound recursion depth for adversarial inputs; deep ranges go straight to Myers.
    const anchors = depth < 48 ? patienceAnchors(a, aLo, aEnd, b, bLo, bEnd) : [];
    if (anchors.length > 0) {
      let pa = aLo;
      let pb = bLo;
      for (const [ai, bi] of anchors) {
        diffRange(a, pa, ai, b, pb, bi, maxCost, emit, depth + 1);
        emit('equal', ai, bi, 1);
        pa = ai + 1;
        pb = bi + 1;
      }
      diffRange(a, pa, aEnd, b, pb, bEnd, maxCost, emit, depth + 1);
    } else if (!myers(a, aLo, aEnd, b, bLo, bEnd, maxCost, emit)) {
      emit('delete', aLo, bLo, aEnd - aLo);
      emit('insert', aEnd, bLo, bEnd - bLo);
    }
  }
  if (suf) emit('equal', aEnd, bEnd, suf);
}

/**
 * Diff two sequences of comparable keys into merged chunks (equal / delete / insert). Within a
 * replacement, deletes always precede inserts.
 */
export function diffSequences<T>(a: readonly T[], b: readonly T[], maxCost = 2000): DiffChunk[] {
  const ids = new Map<T, number>();
  const intern = (xs: readonly T[]) => {
    const out = new Int32Array(xs.length);
    for (let i = 0; i < xs.length; i++) {
      const x = xs[i] as T;
      let id = ids.get(x);
      if (id === undefined) {
        id = ids.size;
        ids.set(x, id);
      }
      out[i] = id;
    }
    return out;
  };
  const ia = intern(a);
  const ib = intern(b);
  const chunks: DiffChunk[] = [];
  const emit: Emit = (op, ai, bi, len) => {
    if (len <= 0) return;
    const last = chunks[chunks.length - 1];
    const aLen = op === 'insert' ? 0 : len;
    const bLen = op === 'delete' ? 0 : len;
    if (last && last.op === op && last.aEnd === ai && last.bEnd === bi) {
      last.aEnd += aLen;
      last.bEnd += bLen;
    } else {
      chunks.push({ op, aStart: ai, aEnd: ai + aLen, bStart: bi, bEnd: bi + bLen });
    }
  };
  diffRange(ia, 0, ia.length, ib, 0, ib.length, maxCost, emit);
  return normalizeChunks(chunks);
}

/** Merge adjacent same-op chunks and order each change block as delete-then-insert. */
function normalizeChunks(chunks: DiffChunk[]): DiffChunk[] {
  const out: DiffChunk[] = [];
  let i = 0;
  while (i < chunks.length) {
    const c = chunks[i] as DiffChunk;
    if (c.op === 'equal') {
      const last = out[out.length - 1];
      if (last && last.op === 'equal' && last.aEnd === c.aStart && last.bEnd === c.bStart) {
        last.aEnd = c.aEnd;
        last.bEnd = c.bEnd;
      } else out.push({ ...c });
      i++;
      continue;
    }
    // Collect a change block (any mix of deletes/inserts until the next equal).
    let aStart = c.aStart;
    let bStart = c.bStart;
    let aEnd = c.aStart;
    let bEnd = c.bStart;
    let first = true;
    while (i < chunks.length && (chunks[i] as DiffChunk).op !== 'equal') {
      const x = chunks[i] as DiffChunk;
      if (first) {
        aStart = x.aStart;
        bStart = x.bStart;
        aEnd = x.aStart;
        bEnd = x.bStart;
        first = false;
      }
      aStart = Math.min(aStart, x.aStart);
      bStart = Math.min(bStart, x.bStart);
      aEnd = Math.max(aEnd, x.aEnd);
      bEnd = Math.max(bEnd, x.bEnd);
      i++;
    }
    if (aEnd > aStart) out.push({ op: 'delete', aStart, aEnd, bStart, bEnd: bStart });
    if (bEnd > bStart) out.push({ op: 'insert', aStart: aEnd, aEnd, bStart, bEnd });
  }
  return out;
}

// =====================================================================================
// Intraline diff
// =====================================================================================

/** Tokenise a line into words, whitespace runs and single punctuation characters. */
export function tokenize(line: string): string[] {
  return line.match(/[\p{L}\p{N}_]+|\s+|[^\p{L}\p{N}_\s]/gu) ?? [];
}

/**
 * Word-level diff of two lines into segments for each side. Returns `null` when the lines are
 * too dissimilar for intraline highlighting to help (less than ~35% of characters shared).
 */
export function diffInline(a: string, b: string, maxCost = 400): { left: Segment[]; right: Segment[] } | null {
  const ta = tokenize(a);
  const tb = tokenize(b);
  const chunks = diffSequences(ta, tb, maxCost);
  const left: Segment[] = [];
  const right: Segment[] = [];
  let common = 0;
  const push = (arr: Segment[], text: string, changed: boolean) => {
    if (!text) return;
    const last = arr[arr.length - 1];
    if (last && last.changed === changed) last.text += text;
    else arr.push({ text, changed });
  };
  for (const c of chunks) {
    if (c.op === 'equal') {
      const text = ta.slice(c.aStart, c.aEnd).join('');
      common += text.length;
      push(left, text, false);
      push(right, text, false);
    } else if (c.op === 'delete') push(left, ta.slice(c.aStart, c.aEnd).join(''), true);
    else push(right, tb.slice(c.bStart, c.bEnd).join(''), true);
  }
  const longest = Math.max(a.length, b.length);
  if (longest > 0 && common / longest < 0.35) return null;
  // Whitespace-only unchanged islands between changes read as noise: absorb them.
  for (const segs of [left, right]) {
    for (let i = 1; i < segs.length - 1; i++) {
      const s = segs[i] as Segment;
      if (!s.changed && /^\s{1,2}$/.test(s.text) && (segs[i - 1] as Segment).changed && (segs[i + 1] as Segment).changed) s.changed = true;
    }
  }
  return { left: mergeSegments(left), right: mergeSegments(right) };
}

function mergeSegments(segs: Segment[]): Segment[] {
  const out: Segment[] = [];
  for (const s of segs) {
    const last = out[out.length - 1];
    if (last && last.changed === s.changed) last.text += s.text;
    else out.push({ ...s });
  }
  return out;
}

// =====================================================================================
// Text diff (lines + rows)
// =====================================================================================

function normalizer(opts: DiffOptions): (line: string) => string {
  const ws = opts.ignoreWhitespace;
  const ci = opts.ignoreCase;
  if (!ws && !ci) return (l) => l;
  return (l) => {
    let s = ws ? l.trim().replace(/\s+/g, ' ') : l;
    if (ci) s = s.toLowerCase();
    return s;
  };
}

/** Full line diff of two texts with unified lines, split rows, stats and intraline segments. */
export function diffText(left: string, right: string, opts: DiffOptions = {}): TextDiffResult {
  const a = splitLines(left);
  const b = splitLines(right);
  const norm = normalizer(opts);
  const chunks = diffSequences(a.map(norm), b.map(norm), opts.maxCost ?? 2000);
  const maxInline = opts.maxIntralineLength ?? 4000;

  const lines: DiffLine[] = [];
  const rows: SplitRow[] = [];
  const stats: DiffStats = { added: 0, removed: 0, blocks: 0 };

  for (let ci = 0; ci < chunks.length; ci++) {
    const c = chunks[ci] as DiffChunk;
    if (c.op === 'equal') {
      for (let k = 0; k < c.aEnd - c.aStart; k++) {
        const ai = c.aStart + k;
        const bi = c.bStart + k;
        const textA = a[ai] as string;
        const textB = b[bi] as string;
        lines.push({ kind: 'equal', aNo: ai + 1, bNo: bi + 1, text: textB });
        rows.push({ left: { no: ai + 1, text: textA, kind: 'equal' }, right: { no: bi + 1, text: textB, kind: 'equal' } });
      }
      continue;
    }
    // A change block: a delete chunk, optionally followed by an insert chunk.
    stats.blocks++;
    const del = c.op === 'delete' ? c : null;
    const nextChunk = chunks[ci + 1];
    const ins = c.op === 'insert' ? c : nextChunk && nextChunk.op === 'insert' ? nextChunk : null;
    if (del && ins) ci++;
    const delCount = del ? del.aEnd - del.aStart : 0;
    const insCount = ins ? ins.bEnd - ins.bStart : 0;
    stats.removed += delCount;
    stats.added += insCount;

    // Pair lines positionally for intraline highlighting.
    const pairs = Math.min(delCount, insCount);
    const inline: ({ left: Segment[]; right: Segment[] } | null)[] = [];
    for (let k = 0; k < pairs; k++) {
      const la = a[(del as DiffChunk).aStart + k] as string;
      const lb = b[(ins as DiffChunk).bStart + k] as string;
      inline.push(la.length <= maxInline && lb.length <= maxInline && la !== lb ? diffInline(la, lb) : null);
    }

    for (let k = 0; k < delCount; k++) {
      const ai = (del as DiffChunk).aStart + k;
      lines.push({ kind: 'del', aNo: ai + 1, text: a[ai] as string, segments: inline[k]?.left });
    }
    for (let k = 0; k < insCount; k++) {
      const bi = (ins as DiffChunk).bStart + k;
      lines.push({ kind: 'add', bNo: bi + 1, text: b[bi] as string, segments: inline[k]?.right });
    }
    for (let k = 0; k < Math.max(delCount, insCount); k++) {
      const leftCell: SplitCell | null =
        k < delCount ? { no: (del as DiffChunk).aStart + k + 1, text: a[(del as DiffChunk).aStart + k] as string, kind: 'del', segments: inline[k]?.left } : null;
      const rightCell: SplitCell | null =
        k < insCount ? { no: (ins as DiffChunk).bStart + k + 1, text: b[(ins as DiffChunk).bStart + k] as string, kind: 'add', segments: inline[k]?.right } : null;
      rows.push({ left: leftCell, right: rightCell });
    }
  }

  return { identical: stats.added === 0 && stats.removed === 0, stats, lines, rows, chunks };
}

// =====================================================================================
// Collapsing unchanged regions
// =====================================================================================

/** A display item: a real row/line, or a fold standing in for `count` hidden unchanged ones. */
export type FoldItem<T> = { type: 'row'; index: number; row: T } | { type: 'fold'; id: string; start: number; count: number };

/**
 * Collapse long runs of unchanged rows, keeping `context` rows around each change. Runs shorter
 * than `context * 2 + minHidden` stay expanded. `expanded` holds fold ids the user opened
 * (a fold id is `start` of the hidden run). Identical documents are not collapsed.
 */
export function collapseUnchanged<T>(
  rows: readonly T[],
  isChanged: (row: T) => boolean,
  options: { context?: number; minHidden?: number; expanded?: ReadonlySet<string> } = {},
): FoldItem<T>[] {
  const context = options.context ?? 3;
  const minHidden = options.minHidden ?? 4;
  const expanded = options.expanded ?? new Set<string>();
  const out: FoldItem<T>[] = [];
  const n = rows.length;
  const anyChange = rows.some(isChanged);
  if (!anyChange) return rows.map((row, index) => ({ type: 'row', index, row }));
  let i = 0;
  while (i < n) {
    if (isChanged(rows[i] as T)) {
      out.push({ type: 'row', index: i, row: rows[i] as T });
      i++;
      continue;
    }
    let j = i;
    while (j < n && !isChanged(rows[j] as T)) j++;
    // unchanged run [i, j)
    const atStart = i === 0;
    const atEnd = j === n;
    const keepBefore = atStart ? 0 : context;
    const keepAfter = atEnd ? 0 : context;
    const hidden = j - i - keepBefore - keepAfter;
    const id = String(i + keepBefore);
    if (hidden >= minHidden && !expanded.has(id)) {
      for (let k = i; k < i + keepBefore; k++) out.push({ type: 'row', index: k, row: rows[k] as T });
      out.push({ type: 'fold', id, start: i + keepBefore, count: hidden });
      for (let k = j - keepAfter; k < j; k++) out.push({ type: 'row', index: k, row: rows[k] as T });
    } else {
      for (let k = i; k < j; k++) out.push({ type: 'row', index: k, row: rows[k] as T });
    }
    i = j;
  }
  return out;
}

/** Indices (into `items`) where each change block starts — for next/previous change navigation. */
export function changeStarts<T>(items: readonly FoldItem<T>[], isChanged: (row: T) => boolean): number[] {
  const out: number[] = [];
  let prevChanged = false;
  items.forEach((item, i) => {
    const changed = item.type === 'row' && isChanged(item.row);
    if (changed && !prevChanged) out.push(i);
    prevChanged = changed;
  });
  return out;
}

/** `+12 −3` style summary. */
export function formatDiffStats(stats: DiffStats): string {
  return `+${stats.added} −${stats.removed}`;
}
