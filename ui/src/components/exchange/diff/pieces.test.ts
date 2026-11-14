import { describe, expect, test } from 'bun:test';
import { diffText, splitLines } from '../../../lib/diff';
import { groupTargets, pickTarget, requestKey, snippetFilename } from '../model/code';
import { lineRanges, linePieces, tokensByLine } from './pieces';
import type { TargetInfo } from '../../../api/types';

describe('lineRanges', () => {
  test('matches splitLines for every line-break style', () => {
    for (const text of ['', 'a', 'a\n', 'a\nb', 'a\r\nb\r\n', 'a\rb', '\n', '\n\n', 'a\n\nb', 'x\r\n\r\ny']) {
      const ranges = lineRanges(text);
      expect(ranges.map(([s, e]) => text.slice(s, e))).toEqual(splitLines(text));
    }
  });
});

describe('tokensByLine', () => {
  test('distributes ranges and splits multi-line tokens', () => {
    const text = 'ab "x\ny" cd\nz';
    // `"x\ny"` spans the first line break.
    const lines = tokensByLine(text, [
      [0, 2, 'k'],
      [3, 8, 's'],
      [9, 11, 'n'],
      [12, 13, 'b'],
    ]);
    expect(lines).toEqual([[[0, 2, 'k'], [3, 5, 's']], [[0, 2, 's'], [3, 5, 'n']], [[0, 1, 'b']]]);
  });

  test('empty lines and CRLF', () => {
    const text = 'a\r\n\r\nbb';
    expect(tokensByLine(text, [[0, 1, 'k'], [5, 7, 's']])).toEqual([[[0, 1, 'k']], [], [[0, 2, 's']]]);
    expect(tokensByLine('', [])).toEqual([]);
  });
});

describe('linePieces', () => {
  test('plain text without tokens or segments is one piece', () => {
    expect(linePieces('hello', null, null)).toEqual([{ text: 'hello', cls: null, changed: false }]);
    expect(linePieces('', null, null)).toEqual([]);
  });

  test('merges syntax tokens with change segments at every boundary', () => {
    // "name": "Ada"  — key token [0,6), punct [6,7), string [8,13); change covers "Ada" + quote.
    const text = '"name": "Ada"';
    const pieces = linePieces(
      text,
      [
        [0, 6, 'k'],
        [6, 7, 'p'],
        [8, 13, 's'],
      ],
      [
        { text: '"name": "', changed: false },
        { text: 'Ada', changed: true },
        { text: '"', changed: false },
      ],
    );
    expect(pieces.map((p) => p.text).join('')).toBe(text);
    expect(pieces).toEqual([
      { text: '"name"', cls: 'k', changed: false },
      { text: ':', cls: 'p', changed: false },
      { text: ' ', cls: null, changed: false },
      { text: '"', cls: 's', changed: false },
      { text: 'Ada', cls: 's', changed: true },
      { text: '"', cls: 's', changed: false },
    ]);
  });

  test('tolerates tokens past the end and overlapping tokens', () => {
    const pieces = linePieces('abc', [
      [0, 2, 'k'],
      [1, 9, 's'],
    ], null);
    expect(pieces).toEqual([
      { text: 'ab', cls: 'k', changed: false },
      { text: 'c', cls: 's', changed: false },
    ]);
  });

  test('round-trips real diff segments', () => {
    const r = diffText('const a = 1;\nconst b = 2;', 'const a = 1;\nconst b = 3;');
    const changed = r.lines.filter((l) => l.kind !== 'equal');
    for (const l of changed) {
      const pieces = linePieces(l.text, null, l.segments);
      expect(pieces.map((p) => p.text).join('')).toBe(l.text);
      expect(pieces.some((p) => p.changed)).toBe(true);
    }
  });
});

describe('code model', () => {
  const targets: TargetInfo[] = [
    { id: 'curl', name: 'cURL', language: 'shell', group: 'Shell' },
    { id: 'js-fetch', name: 'fetch', language: 'javascript', group: 'JavaScript' },
    { id: 'httpie', name: 'HTTPie', language: 'shell', group: 'Shell' },
    { id: 'raw-http', name: 'Raw HTTP', language: 'http', group: '' },
  ];

  test('groupTargets keeps first-appearance order', () => {
    expect(groupTargets(targets).map((g) => [g.label, g.targets.map((t) => t.id)])).toEqual([
      ['Shell', ['curl', 'httpie']],
      ['JavaScript', ['js-fetch']],
      ['Other', ['raw-http']],
    ]);
  });

  test('pickTarget falls back to cURL, then the first target', () => {
    expect(pickTarget(targets, 'httpie')?.id).toBe('httpie');
    expect(pickTarget(targets, 'nope')?.id).toBe('curl');
    expect(pickTarget(targets.slice(1), 'nope')?.id).toBe('js-fetch');
    expect(pickTarget([], 'curl')).toBeUndefined();
  });

  test('snippetFilename', () => {
    expect(snippetFilename('curl', 'shell')).toBe('request.sh');
    expect(snippetFilename('curl-windows', 'shell')).toBe('request.cmd');
    expect(snippetFilename('python-requests', 'python')).toBe('request.py');
    expect(snippetFilename('raw-http', 'http')).toBe('request.http');
    expect(snippetFilename('php-curl', 'weird')).toBe('request.txt');
  });

  test('requestKey changes with any part of the request', () => {
    const base = { method: 'GET', url: 'https://a.test/', headers: [['accept', '*/*']] as [string, string][], body: '' };
    const k = requestKey(base);
    expect(requestKey({ ...base })).toBe(k);
    expect(requestKey({ ...base, method: 'POST' })).not.toBe(k);
    expect(requestKey({ ...base, url: 'https://a.test/x' })).not.toBe(k);
    expect(requestKey({ ...base, headers: [['accept', 'text/html']] })).not.toBe(k);
    expect(requestKey({ ...base, body: 'e30=' })).not.toBe(k);
  });
});
