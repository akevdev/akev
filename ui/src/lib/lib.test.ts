import { describe, expect, test } from 'bun:test';
import {
  base64ToBytes,
  base64ToText,
  byteHex,
  byteOffsetToIndex,
  bytesToBase64,
  bytesToHex,
  indexToByteOffset,
  looksLikeText,
  textToBase64,
  utf8Encode,
  utf8Length,
} from './encoding';
import {
  daysUntil,
  durationSeverity,
  formatBytes,
  formatCompactNumber,
  formatDuration,
  formatNumber,
  formatOffset,
  formatPercent,
  formatRelativeTime,
  pluralize,
} from './format';
import {
  contentKindLabel,
  decodeJwt,
  describeError,
  formatAuthority,
  getHeader,
  getHeaderAll,
  isTextualKind,
  methodColorVar,
  methodHasBody,
  parseCookieHeader,
  parseSetCookie,
  renderRawMessage,
  sortHeaders,
  statusClass,
  statusText,
  summaryUrl,
  tagColorVar,
  typeChipClause,
  TYPE_CHIPS,
} from './http';
import { KNOWN_HEADER_NAMES, lookupHeaderDoc } from './headerDocs';
import { LruCache } from './lru';
import { debounce } from './timing';
import { CONTENT_KINDS, type Headers } from '../api/types';

describe('format', () => {
  test('formatBytes', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024)).toBe('1 KB');
    expect(formatBytes(12.4 * 1024)).toBe('12.4 KB');
    expect(formatBytes(148 * 1024)).toBe('148 KB');
    expect(formatBytes(3.1 * 1024 * 1024)).toBe('3.1 MB');
    // Never "1024 KB": rounding promotes to the next unit.
    expect(formatBytes(1024 * 1024 - 1)).toBe('1 MB');
    expect(formatBytes(-1)).toBe('—');
    expect(formatBytes(null)).toBe('—');
    expect(formatBytes(Number.NaN)).toBe('—');
  });

  test('formatDuration', () => {
    expect(formatDuration(0)).toBe('0 ms');
    expect(formatDuration(0.42)).toBe('0.4 ms');
    expect(formatDuration(143.2)).toBe('143 ms');
    expect(formatDuration(1200)).toBe('1.2 s');
    expect(formatDuration(1000)).toBe('1 s');
    expect(formatDuration(1050)).toBe('1.05 s');
    expect(formatDuration(12_500)).toBe('12.5 s');
    expect(formatDuration(65_000)).toBe('1m 05s');
    expect(formatDuration(3_720_000)).toBe('1h 02m');
    expect(formatDuration(undefined)).toBe('—');
    expect(formatDuration(-5)).toBe('0 ms');
  });

  test('formatDuration never shows a value that should have rolled over', () => {
    expect(formatDuration(999.6)).toBe('1 s');
    expect(formatDuration(9_999)).toBe('10 s');
    expect(formatDuration(59_970)).toBe('1m 00s');
  });

  test('durationSeverity thresholds (warning >1s, danger >5s)', () => {
    expect(durationSeverity(999)).toBe('normal');
    expect(durationSeverity(1001)).toBe('slow');
    expect(durationSeverity(5001)).toBe('very-slow');
    expect(durationSeverity(undefined)).toBe('normal');
  });

  test('relative times', () => {
    const now = 1_700_000_000_000;
    expect(formatRelativeTime(now - 1000, now)).toBe('just now');
    expect(formatRelativeTime(now - 12_000, now)).toBe('12s ago');
    expect(formatRelativeTime(now - 5 * 60_000, now)).toBe('5m ago');
    expect(formatRelativeTime(now - 3 * 3_600_000, now)).toBe('3h ago');
    expect(formatRelativeTime(now - 30 * 3_600_000, now)).toBe('yesterday');
    expect(formatRelativeTime(now - 4 * 86_400_000, now)).toBe('4d ago');
    expect(formatRelativeTime(now + 5 * 60_000, now)).toBe('in 5m');
    expect(formatRelativeTime(now - 90 * 86_400_000, now)).toMatch(/^\d{4}-\d{2}-\d{2}$/);
  });

  test('numbers', () => {
    expect(formatNumber(1234567)).toBe(new Intl.NumberFormat().format(1234567));
    expect(formatCompactNumber(1234)).toBe('1.2K');
    expect(formatPercent(0.425)).toBe('43%');
    expect(formatPercent(0.042)).toBe('4.2%');
    expect(formatOffset(1204)).toBe('+1.204s');
    expect(formatOffset(12)).toBe('+12ms');
    expect(pluralize(1, 'exchange')).toBe('1 exchange');
    expect(pluralize(3, 'exchange')).toBe('3 exchanges');
    expect(daysUntil('2026-01-11T00:00:00Z', Date.parse('2026-01-01T00:00:00Z'))).toBe(10);
    expect(Number.isNaN(daysUntil('garbage'))).toBe(true);
  });
});

describe('encoding', () => {
  const text = 'a€😀b'; // 1 + 3 + 4 + 1 bytes; 1 + 1 + 2 + 1 UTF-16 units

  test('utf8Length matches TextEncoder', () => {
    for (const s of ['', 'abc', text, 'Ünïcödé', '\ud800 lone surrogate']) {
      expect(utf8Length(s)).toBe(utf8Encode(s).length);
    }
  });

  test('byte offsets ↔ string indexes', () => {
    expect(byteOffsetToIndex(text, 0)).toBe(0);
    expect(byteOffsetToIndex(text, 1)).toBe(1);
    expect(byteOffsetToIndex(text, 4)).toBe(2);
    expect(byteOffsetToIndex(text, 8)).toBe(4);
    expect(byteOffsetToIndex(text, 9)).toBe(5);
    expect(byteOffsetToIndex(text, 99)).toBe(5);
    // Inside a multi-byte char → snaps to its start.
    expect(byteOffsetToIndex(text, 2)).toBe(1);
    expect(byteOffsetToIndex(text, 6)).toBe(2);
    for (let i = 0; i <= text.length; i++) {
      if (i === 3) continue; // middle of the surrogate pair
      expect(byteOffsetToIndex(text, indexToByteOffset(text, i))).toBe(i);
    }
  });

  test('base64 round trips', () => {
    const bytes = new Uint8Array([0, 1, 2, 250, 255]);
    expect(base64ToBytes(bytesToBase64(bytes))).toEqual(bytes);
    expect(base64ToText(textToBase64(text))).toBe(text);
    expect(base64ToText(bytesToBase64(new Uint8Array([0xff, 0xfe])))).toBeNull();
    expect(base64ToBytes('aGV5\nIQ==')).toEqual(utf8Encode('hey!'));
  });

  test('hex', () => {
    expect(bytesToHex(new Uint8Array([0xde, 0xad, 0xbe, 0xef]), ' ')).toBe('de ad be ef');
    expect(byteHex(0x0a)).toBe('0a');
    expect(looksLikeText(utf8Encode('{"hello": "world"}\n'))).toBe(true);
    expect(looksLikeText(new Uint8Array([0, 1, 2, 3, 0, 0, 0xff, 0x89]))).toBe(false);
  });
});

describe('http helpers', () => {
  test('status texts and classes', () => {
    expect(statusText(200)).toBe('OK');
    expect(statusText(418)).toBe("I'm a teapot");
    expect(statusText(799)).toBe('');
    expect(statusClass(101)).toBe('1xx');
    expect(statusClass(204)).toBe('2xx');
    expect(statusClass(302)).toBe('3xx');
    expect(statusClass(404)).toBe('4xx');
    expect(statusClass(503)).toBe('5xx');
    expect(statusClass(undefined)).toBe('err');
  });

  test('methods', () => {
    expect(methodColorVar('get')).toBe('var(--m-get)');
    expect(methodColorVar('PROPFIND')).toBe('var(--m-other)');
    expect(methodHasBody('POST')).toBe(true);
    expect(methodHasBody('GET')).toBe(false);
  });

  test('headers keep order, duplicates and casing', () => {
    const h: Headers = [
      ['X-B', '1'],
      [':authority', 'example.com'],
      ['set-cookie', 'a=1'],
      ['Set-Cookie', 'b=2'],
      ['accept', '*/*'],
    ];
    expect(getHeader(h, 'SET-COOKIE')).toBe('a=1');
    expect(getHeaderAll(h, 'set-cookie')).toEqual(['a=1', 'b=2']);
    expect(sortHeaders(h).map(([k]) => k)).toEqual([':authority', 'accept', 'set-cookie', 'Set-Cookie', 'X-B']);
    expect(h[0]?.[0]).toBe('X-B'); // not mutated
  });

  test('bundled header docs cover the common headers', () => {
    expect(KNOWN_HEADER_NAMES.length).toBeGreaterThanOrEqual(120);
    const doc = lookupHeaderDoc('  content-TYPE ');
    expect(doc?.name).toBe('Content-Type');
    expect(doc?.summary.length).toBeGreaterThan(10);
    expect(doc?.url).toMatch(/^https:\/\/developer\.mozilla\.org\//);
    expect(lookupHeaderDoc('x-definitely-not-a-header')).toBeUndefined();
    for (const name of ['Authorization', 'Cache-Control', 'Set-Cookie', 'Strict-Transport-Security', 'Access-Control-Allow-Origin', 'User-Agent']) {
      expect(lookupHeaderDoc(name)).toBeDefined();
    }
  });

  test('authority & URL', () => {
    expect(formatAuthority('https', 'example.com', 443)).toBe('example.com');
    expect(formatAuthority('http', 'example.com', 8080)).toBe('example.com:8080');
    expect(formatAuthority('https', '::1', 8443)).toBe('[::1]:8443');
    expect(summaryUrl({ scheme: 'wss', host: 'h', port: 443, path: '/ws?x=1' })).toBe('wss://h/ws?x=1');
  });

  test('cookies', () => {
    expect(parseCookieHeader('a=1; b=two=2;  ; flag')).toEqual([
      { name: 'a', value: '1' },
      { name: 'b', value: 'two=2' },
      { name: '', value: 'flag' },
    ]);
    const c = parseSetCookie('sid=abc; Path=/; Domain=.example.com; Max-Age=3600; Secure; HttpOnly; SameSite=Lax; Priority=High; Partitioned');
    expect(c).toMatchObject({ name: 'sid', value: 'abc', path: '/', domain: '.example.com', maxAge: 3600, secure: true, httpOnly: true, sameSite: 'Lax', partitioned: true });
    expect(c.other).toEqual([['Priority', 'High']]);
    expect(parseSetCookie('x=; Expires=Thu, 01 Jan 1970 00:00:00 GMT').expires).toBe('Thu, 01 Jan 1970 00:00:00 GMT');
  });

  test('JWT decoding (no verification)', () => {
    const b64url = (o: unknown) => textToBase64(JSON.stringify(o)).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '');
    const token = `${b64url({ alg: 'HS256', typ: 'JWT' })}.${b64url({ sub: 'ada', exp: 1000, iat: 900, name: 'Ädä' })}.sig`;
    const jwt = decodeJwt(`Bearer ${token}`, 2_000_000);
    expect(jwt?.header.alg).toBe('HS256');
    expect(jwt?.payload.name).toBe('Ädä');
    expect(jwt?.expiresAt).toBe(1_000_000);
    expect(jwt?.issuedAt).toBe(900_000);
    expect(jwt?.expired).toBe(true);
    expect(decodeJwt(token, 0)?.expired).toBe(false);
    expect(decodeJwt('not.a.jwt')).toBeNull();
    expect(decodeJwt('abc')).toBeNull();
  });

  test('content kinds all have labels', () => {
    for (const k of CONTENT_KINDS) expect(contentKindLabel(k).length).toBeGreaterThan(0);
    expect(isTextualKind('json')).toBe(true);
    expect(isTextualKind('image')).toBe(false);
  });

  test('type chips translate to filter clauses', () => {
    expect(typeChipClause('all')).toBe('');
    expect(typeChipClause('doc')).toBe('type:html');
    expect(typeChipClause('fetch')).toContain('json');
    expect(typeChipClause('llm')).toBe('is:llm');
    expect(new Set(TYPE_CHIPS.map((c) => c.id)).size).toBe(TYPE_CHIPS.length);
  });

  test('errors explain cause with host substitution', () => {
    const e = describeError('connection-refused', 'api.example.com');
    expect(e.title).toContain('api.example.com');
    expect(e.title).not.toContain('{host}');
    expect(describeError('dns-failed', '').title).not.toContain('{host}');
  });

  test('tag colours', () => {
    expect(tagColorVar('red')).toBe('var(--tag-red)');
    expect(tagColorVar('magenta')).toBe('var(--tag-grey)');
    expect(tagColorVar(null)).toBe('var(--tag-grey)');
  });

  test('raw message rendering', () => {
    expect(renderRawMessage('GET / HTTP/1.1', [['Host', 'a']], null, 12)).toBe('GET / HTTP/1.1\r\nHost: a\r\n\r\n<12 bytes binary>');
    expect(renderRawMessage('HTTP/1.1 204 No Content', [], null)).toBe('HTTP/1.1 204 No Content\r\n\r\n');
  });
});

describe('utilities', () => {
  test('LruCache evicts least recently used', () => {
    const lru = new LruCache<string, number>(2);
    lru.set('a', 1);
    lru.set('b', 2);
    expect(lru.get('a')).toBe(1); // a is now most recent
    lru.set('c', 3);
    expect(lru.get('b')).toBeUndefined();
    expect(lru.get('a')).toBe(1);
    expect(lru.get('c')).toBe(3);
  });

  test('debounce flush / cancel', async () => {
    const calls: number[] = [];
    const d = debounce((n: number) => calls.push(n), 10);
    d(1);
    d(2);
    expect(d.pending()).toBe(true);
    d.flush();
    expect(calls).toEqual([2]);
    d(3);
    d.cancel();
    await new Promise((r) => setTimeout(r, 20));
    expect(calls).toEqual([2]);
  });
});
