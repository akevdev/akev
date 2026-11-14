/**
 * Pure helpers that turn a diff line into renderable pieces: syntax-highlight tokens (per line)
 * merged with intraline change segments. No React, no CodeMirror.
 */

import type { Segment } from '../../../lib/diff';

/** A highlight token inside one line: `[from, to, className]` (offsets relative to the line). */
export type LineToken = readonly [from: number, to: number, cls: string];

/** A renderable run of a line. */
export interface Piece {
  text: string;
  /** Syntax class (highlighter output), if any. */
  cls: string | null;
  /** Part of an intraline change. */
  changed: boolean;
}

/**
 * Start offsets of every line, consistent with `splitLines` in lib/diff.ts: `\r\n`, `\r` and
 * `\n` all end a line, a single trailing newline doesn't start a new line, and `''` has none.
 * Returns `[start, end)` pairs (end excludes the line break).
 */
export function lineRanges(text: string): [number, number][] {
  const out: [number, number][] = [];
  if (text === '') return out;
  let start = 0;
  const n = text.length;
  for (let i = 0; i < n; i++) {
    const ch = text.charCodeAt(i);
    if (ch === 10 /* \n */ || ch === 13 /* \r */) {
      out.push([start, i]);
      if (ch === 13 && text.charCodeAt(i + 1) === 10) i++;
      start = i + 1;
    }
  }
  if (start < n || out.length === 0) out.push([start, n]);
  return out;
}

/**
 * Distribute document-wide highlight ranges (sorted by `from`, non-overlapping as produced by
 * `highlightTree`) onto lines. Ranges spanning line breaks are split.
 */
export function tokensByLine(text: string, ranges: readonly (readonly [number, number, string])[]): LineToken[][] {
  const lines = lineRanges(text);
  const out: LineToken[][] = lines.map(() => []);
  let li = 0;
  for (const [from, to, cls] of ranges) {
    if (to <= from) continue;
    // Lines ending at or before `from` can't contain any of this range (line breaks are
    // excluded from lines, and empty lines hold nothing).
    while (li < lines.length && (lines[li] as [number, number])[1] <= from) li++;
    for (let k = li; k < lines.length; k++) {
      const [ls, le] = lines[k] as [number, number];
      if (ls >= to) break;
      const a = Math.max(from, ls);
      const b = Math.min(to, le);
      if (b > a) (out[k] as LineToken[]).push([a - ls, b - ls, cls]);
    }
  }
  return out;
}

/**
 * Merge a line's highlight tokens with its intraline change segments into pieces, splitting at
 * every boundary of either. Text not covered by a token gets `cls: null`.
 */
export function linePieces(text: string, tokens: readonly LineToken[] | null | undefined, segments: readonly Segment[] | null | undefined): Piece[] {
  if (text === '') return [];
  // Change boundaries.
  const changes: [number, number, boolean][] = [];
  if (segments?.length) {
    let pos = 0;
    for (const s of segments) {
      changes.push([pos, pos + s.text.length, s.changed]);
      pos += s.text.length;
    }
    if (pos < text.length) changes.push([pos, text.length, false]);
  } else changes.push([0, text.length, false]);

  // Token runs covering the whole line (gaps → null class).
  const runs: [number, number, string | null][] = [];
  let at = 0;
  for (const [from, to, cls] of tokens ?? []) {
    const a = Math.max(from, at);
    const b = Math.min(to, text.length);
    if (b <= a) continue;
    if (a > at) runs.push([at, a, null]);
    runs.push([a, b, cls]);
    at = b;
  }
  if (at < text.length) runs.push([at, text.length, null]);

  const out: Piece[] = [];
  let ci = 0;
  let ri = 0;
  let pos = 0;
  while (pos < text.length && ci < changes.length && ri < runs.length) {
    const c = changes[ci] as [number, number, boolean];
    const r = runs[ri] as [number, number, string | null];
    const end = Math.min(c[1], r[1]);
    if (end > pos) {
      const last = out[out.length - 1];
      const piece = text.slice(pos, end);
      if (last && last.cls === r[2] && last.changed === c[2]) last.text += piece;
      else out.push({ text: piece, cls: r[2], changed: c[2] });
      pos = end;
    }
    if (c[1] <= pos) ci++;
    if (r[1] <= pos) ri++;
  }
  if (pos < text.length) out.push({ text: text.slice(pos), cls: null, changed: false });
  return out;
}
