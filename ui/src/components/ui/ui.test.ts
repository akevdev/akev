import { describe, expect, test } from 'bun:test';
import { formatBulk, headersToRows, makeRow, parseBulk, rowsToHeaders } from './KeyValueEditor';
import { appIconFor, APP_ICON_NAMES } from './AppIcon';
import { normalizeLanguage } from './codemirror/languages';
import { httpParser } from './codemirror/http';
import { qrPath } from './QrCode';
import { StringStream } from '@codemirror/language';

describe('KeyValueEditor helpers', () => {
  test('parseBulk splits Name: value lines, keeps colons in values, # disables', () => {
    const rows = parseBulk('Content-Type: application/json\n\n# X-Debug: 1\nHost:example.com:8080\n// Old: header\nflag');
    expect(rows.map((r) => [r.name, r.value, r.enabled])).toEqual([
      ['Content-Type', 'application/json', true],
      ['X-Debug', '1', false],
      ['Host', 'example.com:8080', true],
      ['Old', 'header', false],
      ['flag', '', true],
    ]);
  });

  test('parseBulk with = separator (query/form params)', () => {
    expect(parseBulk('a=1\nb=x=y', '=').map((r) => [r.name, r.value])).toEqual([
      ['a', '1'],
      ['b', 'x=y'],
    ]);
  });

  test('formatBulk is the inverse of parseBulk', () => {
    const rows = [makeRow('A', '1'), makeRow('B', '2', false), makeRow('', '')];
    const text = formatBulk(rows);
    expect(text).toBe('A: 1\n# B: 2');
    expect(parseBulk(text).map((r) => [r.name, r.value, r.enabled])).toEqual([
      ['A', '1', true],
      ['B', '2', false],
    ]);
    expect(formatBulk(rows, '=')).toBe('A=1\n# B=2');
  });

  test('rows ↔ headers keep wire order, drop disabled and empty names', () => {
    const rows = headersToRows([
      ['X-B', '1'],
      ['x-a', '2'],
    ]);
    rows.push(makeRow('  ', 'ignored'), makeRow('Off', 'x', false), makeRow(' Trim ', ' v '));
    expect(rowsToHeaders(rows)).toEqual([
      ['X-B', '1'],
      ['x-a', '2'],
      ['Trim', ' v '],
    ]);
    expect(new Set(rows.map((r) => r.id)).size).toBe(rows.length);
  });
});

describe('AppIcon', () => {
  test('maps process names and interceptor keys to icons', () => {
    expect(appIconFor('chrome')).toBe('chrome');
    expect(appIconFor('Google Chrome Helper')).toBe('chrome');
    expect(appIconFor('chromium-browser')).toBe('chromium');
    expect(appIconFor('python3')).toBe('python');
    expect(appIconFor('Go-http-client')).toBe('go');
    expect(appIconFor('okhttp')).toBe('android');
    expect(appIconFor('fresh-terminal')).toBe('terminal');
    expect(appIconFor('system-proxy')).toBe('globe');
    expect(appIconFor('Akev Compose')).toBe('akev');
    expect(appIconFor('something-unknown')).toBe('app');
    expect(appIconFor(undefined)).toBe('app');
  });

  test('covers every icon required by DESIGN.md §5', () => {
    for (const n of ['chrome', 'firefox', 'edge', 'brave', 'opera', 'vivaldi', 'safari', 'terminal', 'node', 'python', 'curl', 'android', 'apple', 'docker', 'java', 'go', 'rust', 'electron', 'app']) {
      expect(APP_ICON_NAMES).toContain(n as never);
    }
  });
});

describe('CodeEditor languages', () => {
  test('normalizeLanguage maps codegen ids, kinds and extensions', () => {
    expect(normalizeLanguage('json-lines')).toBe('json');
    expect(normalizeLanguage('graph-ql')).toBe('json');
    expect(normalizeLanguage('curl')).toBe('shell');
    expect(normalizeLanguage('raw-http')).toBe('http');
    expect(normalizeLanguage('py')).toBe('python');
    expect(normalizeLanguage('svg')).toBe('xml');
    expect(normalizeLanguage('csharp')).toBe('csharp');
    expect(normalizeLanguage('brainfuck')).toBe('text');
    expect(normalizeLanguage(undefined)).toBe('text');
  });

  /** Tokenise a document with the HTTP stream parser → [text, style][] per line. */
  function tokens(doc: string): [string, string | null][][] {
    const state = httpParser.startState?.(2) as Parameters<typeof httpParser.token>[1];
    return doc.split('\n').map((line) => {
      if (line === '') {
        httpParser.blankLine?.(state, 2);
        return [];
      }
      const stream = new StringStream(line, 2, 2);
      const out: [string, string | null][] = [];
      while (!stream.eol()) {
        const style = httpParser.token(stream, state);
        out.push([stream.current(), style]);
        stream.start = stream.pos;
      }
      return out.filter(([text]) => text.trim() !== '');
    });
  }

  test('http language highlights request line, headers and leaves the body plain', () => {
    const t = tokens('POST /api?x=1 HTTP/1.1\nContent-Type: application/json\n:authority: example.com\n\n{"a": "b: c"}');
    expect(t[0]).toEqual([
      ['POST', 'keyword'],
      ['/api?x=1', 'string'],
      ['HTTP/1.1', 'atom'],
    ]);
    expect(t[1]).toEqual([
      ['Content-Type', 'propertyName'],
      [':', 'punctuation'],
      ['application/json', 'string'],
    ]);
    expect(t[2]?.[0]).toEqual([':authority', 'propertyName']);
    expect(t[4]).toEqual([['{"a": "b: c"}', null]]);
  });

  test('http language highlights status lines', () => {
    const t = tokens('HTTP/2 404 Not Found\nserver: akev');
    expect(t[0]).toEqual([
      ['HTTP/2', 'keyword'],
      ['404', 'number'],
      ['Not Found', 'comment'],
    ]);
    expect(t[1]?.[0]).toEqual(['server', 'propertyName']);
  });
});

describe('QrCode', () => {
  test('qrPath produces a square module grid with merged runs', () => {
    const { path, size } = qrPath('http://192.168.1.23:8000/');
    expect(size).toBeGreaterThanOrEqual(21);
    expect((size - 17) % 4).toBe(0); // QR versions are 21, 25, 29, …
    expect(path.startsWith('M')).toBe(true);
    // Finder pattern: the top-left 7 modules of row 0 are one dark run.
    expect(path).toContain('M0 0h7v1h-7z');
  });

  test('throws for data that cannot fit (component renders a fallback)', () => {
    expect(() => qrPath('x'.repeat(5000), 'H')).toThrow();
  });
});
