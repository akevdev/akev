import { describe, expect, test } from 'bun:test';
import {
  changeStarts,
  collapseUnchanged,
  diffInline,
  diffSequences,
  diffText,
  formatDiffStats,
  splitLines,
  tokenize,
  type DiffChunk,
} from './diff';

/** Re-apply chunks to `a` and check we get `b` (diff correctness invariant). */
function apply<T>(a: readonly T[], b: readonly T[], chunks: DiffChunk[]): T[] {
  const out: T[] = [];
  let ai = 0;
  let bi = 0;
  for (const c of chunks) {
    expect(c.aStart).toBe(ai);
    expect(c.bStart).toBe(bi);
    if (c.op === 'equal') {
      expect(c.aEnd - c.aStart).toBe(c.bEnd - c.bStart);
      for (let k = c.aStart; k < c.aEnd; k++) {
        expect(a[k]).toEqual(b[c.bStart + (k - c.aStart)] as T);
        out.push(a[k] as T);
      }
    } else if (c.op === 'insert') {
      expect(c.aStart).toBe(c.aEnd);
      for (let k = c.bStart; k < c.bEnd; k++) out.push(b[k] as T);
    } else {
      expect(c.bStart).toBe(c.bEnd);
    }
    ai = c.aEnd;
    bi = c.bEnd;
  }
  expect(ai).toBe(a.length);
  expect(bi).toBe(b.length);
  return out;
}

function editCount(chunks: DiffChunk[]): number {
  return chunks.reduce((n, c) => n + (c.op === 'equal' ? 0 : c.op === 'delete' ? c.aEnd - c.aStart : c.bEnd - c.bStart), 0);
}

describe('splitLines', () => {
  test('handles every line ending and the trailing newline', () => {
    expect(splitLines('')).toEqual([]);
    expect(splitLines('a')).toEqual(['a']);
    expect(splitLines('a\n')).toEqual(['a']);
    expect(splitLines('a\r\nb\rc\nd')).toEqual(['a', 'b', 'c', 'd']);
    expect(splitLines('\n')).toEqual(['']);
    expect(splitLines('a\n\n')).toEqual(['a', '']);
  });
});

describe('diffSequences', () => {
  test('identical and empty inputs', () => {
    expect(diffSequences([], [])).toEqual([]);
    expect(diffSequences(['a', 'b'], ['a', 'b'])).toEqual([{ op: 'equal', aStart: 0, aEnd: 2, bStart: 0, bEnd: 2 }]);
    expect(diffSequences([], ['x'])).toEqual([{ op: 'insert', aStart: 0, aEnd: 0, bStart: 0, bEnd: 1 }]);
    expect(diffSequences(['x'], [])).toEqual([{ op: 'delete', aStart: 0, aEnd: 1, bStart: 0, bEnd: 0 }]);
  });

  test('insertion, deletion and replacement in the middle', () => {
    const a = ['a', 'b', 'c', 'd'];
    const b = ['a', 'x', 'c', 'd', 'e'];
    const chunks = diffSequences(a, b);
    expect(apply(a, b, chunks)).toEqual(b);
    expect(chunks.map((c) => c.op)).toEqual(['equal', 'delete', 'insert', 'equal', 'insert']);
    expect(editCount(chunks)).toBe(3);
  });

  test('replacement blocks are delete-then-insert', () => {
    const chunks = diffSequences(['1', 'old', '2'], ['1', 'new', '2']);
    expect(chunks.map((c) => c.op)).toEqual(['equal', 'delete', 'insert', 'equal']);
  });

  test('finds minimal edits on the classic Myers example', () => {
    const a = 'ABCABBA'.split('');
    const b = 'CBABAC'.split('');
    const chunks = diffSequences(a, b);
    expect(apply(a, b, chunks)).toEqual(b);
    expect(editCount(chunks)).toBe(5); // D = 5
  });

  test('patience anchors keep unique lines aligned (moved braces do not confuse it)', () => {
    const a = ['function a() {', '  return 1;', '}', '', 'function b() {', '  return 2;', '}'];
    const b = ['function b() {', '  return 2;', '}', '', 'function a() {', '  return 1;', '}'];
    const chunks = diffSequences(a, b);
    expect(apply(a, b, chunks)).toEqual(b);
  });

  test('random edits always reproduce the target', () => {
    let seed = 42;
    const rnd = () => {
      seed = (seed * 1103515245 + 12345) & 0x7fffffff;
      return seed / 0x7fffffff;
    };
    for (let round = 0; round < 60; round++) {
      const a = Array.from({ length: Math.floor(rnd() * 40) }, () => String.fromCharCode(97 + Math.floor(rnd() * 5)));
      const b = a.filter(() => rnd() > 0.2);
      for (let i = 0; i < 5; i++) b.splice(Math.floor(rnd() * (b.length + 1)), 0, String.fromCharCode(97 + Math.floor(rnd() * 6)));
      expect(apply(a, b, diffSequences(a, b))).toEqual(b);
    }
  });

  test('falls back to a replacement when the budget is exhausted', () => {
    const a = Array.from({ length: 300 }, (_, i) => `x${i % 7}`);
    const b = Array.from({ length: 300 }, (_, i) => `y${i % 7}`);
    const chunks = diffSequences(a, b, 10);
    expect(apply(a, b, chunks)).toEqual(b);
    expect(chunks.map((c) => c.op)).toEqual(['delete', 'insert']);
  });

  test('stays fast on large documents', () => {
    const a = Array.from({ length: 20_000 }, (_, i) => `line ${i} ${i % 13 === 0 ? '{' : '}'}`);
    const b = a.slice();
    for (let i = 100; i < b.length; i += 997) b[i] = `changed ${i}`;
    b.splice(5000, 0, 'inserted A', 'inserted B');
    b.splice(15000, 30);
    const t0 = performance.now();
    const chunks = diffSequences(a, b);
    const elapsed = performance.now() - t0;
    expect(apply(a, b, chunks)).toEqual(b);
    expect(elapsed).toBeLessThan(1500);
  });

  test('unrelated large documents finish quickly (budget fallback)', () => {
    const a = Array.from({ length: 10_000 }, (_, i) => `}${i % 3}`);
    const b = Array.from({ length: 10_000 }, (_, i) => `]${i % 3}`);
    const t0 = performance.now();
    const chunks = diffSequences(a, b);
    expect(performance.now() - t0).toBeLessThan(1500);
    expect(apply(a, b, chunks)).toEqual(b);
  });
});

describe('tokenize & diffInline', () => {
  test('tokenises words, whitespace and punctuation (unicode aware)', () => {
    expect(tokenize('foo-bar  baz')).toEqual(['foo', '-', 'bar', '  ', 'baz']);
    expect(tokenize('čšž: 12')).toEqual(['čšž', ':', ' ', '12']);
    expect(tokenize('')).toEqual([]);
  });

  test('highlights only the changed words', () => {
    const r = diffInline('"status": "active",', '"status": "disabled",');
    expect(r).not.toBeNull();
    expect(r?.left).toEqual([
      { text: '"status": "', changed: false },
      { text: 'active', changed: true },
      { text: '",', changed: false },
    ]);
    expect(r?.right.find((s) => s.changed)?.text).toBe('disabled');
    expect(r?.left.map((s) => s.text).join('')).toBe('"status": "active",');
    expect(r?.right.map((s) => s.text).join('')).toBe('"status": "disabled",');
  });

  test('returns null for unrelated lines', () => {
    expect(diffInline('completely different', 'xyz 123 !!!')).toBeNull();
  });
});

describe('diffText', () => {
  test('identical texts', () => {
    const r = diffText('a\nb\n', 'a\nb');
    expect(r.identical).toBe(true);
    expect(r.stats).toEqual({ added: 0, removed: 0, blocks: 0 });
    expect(r.lines.every((l) => l.kind === 'equal')).toBe(true);
  });

  test('unified lines carry line numbers on the right sides', () => {
    const r = diffText('a\nb\nc', 'a\nB\nc\nd');
    expect(r.stats).toEqual({ added: 2, removed: 1, blocks: 2 });
    expect(r.lines).toEqual([
      { kind: 'equal', aNo: 1, bNo: 1, text: 'a' },
      { kind: 'del', aNo: 2, text: 'b', segments: undefined },
      { kind: 'add', bNo: 2, text: 'B', segments: undefined },
      { kind: 'equal', aNo: 3, bNo: 3, text: 'c' },
      { kind: 'add', bNo: 4, text: 'd', segments: undefined },
    ]);
  });

  test('split rows align replacements and pad with fillers', () => {
    const r = diffText('x\none\ny', 'x\nuno\ndos\ny');
    expect(r.rows.length).toBe(4);
    expect(r.rows[1]?.left?.text).toBe('one');
    expect(r.rows[1]?.right?.text).toBe('uno');
    expect(r.rows[2]?.left).toBeNull();
    expect(r.rows[2]?.right?.text).toBe('dos');
    expect(r.rows[3]?.left?.no).toBe(3);
    expect(r.rows[3]?.right?.no).toBe(4);
  });

  test('intraline segments for paired changed lines', () => {
    const r = diffText('{\n  "id": 1,\n  "name": "Ada"\n}', '{\n  "id": 2,\n  "name": "Ada"\n}');
    const del = r.lines.find((l) => l.kind === 'del');
    expect(del?.segments?.filter((s) => s.changed).map((s) => s.text)).toEqual(['1']);
  });

  test('ignoreWhitespace and ignoreCase options', () => {
    expect(diffText('a  b\nC', ' a b \nC', { ignoreWhitespace: true }).identical).toBe(true);
    expect(diffText('Content-Type: x', 'content-type: x', { ignoreCase: true }).identical).toBe(true);
    expect(diffText('Content-Type: x', 'content-type: x').identical).toBe(false);
  });

  test('empty vs non-empty', () => {
    const r = diffText('', 'a\nb');
    expect(r.stats.added).toBe(2);
    expect(r.rows.every((row) => row.left === null)).toBe(true);
  });
});

describe('collapseUnchanged', () => {
  const rows = Array.from({ length: 30 }, (_, i) => ({ i, changed: i === 15 }));
  const isChanged = (r: { changed: boolean }) => r.changed;

  test('keeps context around changes and folds the rest', () => {
    const items = collapseUnchanged(rows, isChanged, { context: 3 });
    expect(items[0]).toEqual({ type: 'fold', id: '0', start: 0, count: 12 });
    const visible = items.filter((x) => x.type === 'row').map((x) => (x.type === 'row' ? x.index : -1));
    expect(visible).toEqual([12, 13, 14, 15, 16, 17, 18]);
    expect(items[items.length - 1]).toEqual({ type: 'fold', id: '19', start: 19, count: 11 });
  });

  test('expanded folds stay open and short runs are never folded', () => {
    const items = collapseUnchanged(rows, isChanged, { context: 3, expanded: new Set(['0']) });
    expect(items.filter((x) => x.type === 'fold')).toHaveLength(1);
    const short = Array.from({ length: 9 }, (_, i) => ({ i, changed: i === 4 }));
    expect(collapseUnchanged(short, isChanged).every((x) => x.type === 'row')).toBe(true);
  });

  test('identical documents are not collapsed', () => {
    const same = Array.from({ length: 50 }, (_, i) => ({ i, changed: false }));
    expect(collapseUnchanged(same, isChanged)).toHaveLength(50);
  });

  test('changeStarts finds the first row of each block', () => {
    const r = Array.from({ length: 12 }, (_, i) => ({ i, changed: i === 2 || i === 3 || i === 9 }));
    const items = collapseUnchanged(r, isChanged, { context: 1, minHidden: 2 });
    const starts = changeStarts(items, isChanged).map((i) => items[i]);
    expect(starts.map((x) => (x?.type === 'row' ? x.index : -1))).toEqual([2, 9]);
  });
});

test('formatDiffStats', () => {
  expect(formatDiffStats({ added: 12, removed: 3, blocks: 2 })).toBe('+12 −3');
});
