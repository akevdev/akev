import { describe, expect, test } from 'bun:test';
import type { Action, Matcher, Rule } from '../../api/types';
import {
  base64Error,
  forwardTargetError,
  headerNameError,
  issueAt,
  issuesAt,
  jsonPatchError,
  parseJson,
  regexError,
  scriptError,
  statusError,
  validateRule,
} from './validate';

const rule = (matchers: Matcher[], action: Action, extra: Partial<Rule> = {}): Rule => ({ id: 'r', enabled: true, matchers, action, ...extra });
const paths = (r: Rule) => validateRule(r).map((i) => i.path);

describe('field validators', () => {
  test('regex (Rust syntax, best effort)', () => {
    expect(regexError('^/api/v\\d+/users$')).toBeNull();
    expect(regexError('(?i)^abc')).toBeNull();
    expect(regexError('(?P<id>\\d+)')).toBeNull();
    expect(regexError('\\Afoo\\z')).toBeNull();
    expect(regexError('[[:alpha:]]+')).toBeNull();
    expect(regexError('\\p{L}+')).toBeNull();
    expect(regexError('a(b')).toMatch(/^Invalid regex/);
    expect(regexError('[')).toMatch(/^Invalid regex/);
    expect(regexError('foo(?=bar)')).toMatch(/Look-around/);
    expect(regexError('(?<!x)y')).toMatch(/Look-around/);
    expect(regexError('(a)\\1')).toMatch(/Back-references/);
  });

  test('header names & status', () => {
    expect(headerNameError('x-api-key')).toBeNull();
    expect(headerNameError('bad header')).toMatch(/valid header name/);
    expect(headerNameError('')).toMatch(/empty/);
    expect(statusError(200)).toBeNull();
    expect(statusError(999)).toBeNull();
    expect(statusError(99)).toMatch(/100 and 999/);
    expect(statusError(1000)).toMatch(/100 and 999/);
    expect(statusError(20.5)).toMatch(/100 and 999/);
    expect(statusError(null)).toMatch(/status/);
  });

  test('forward target (mirrors parse_forward_target)', () => {
    expect(forwardTargetError('http://localhost:3000')).toBeNull();
    expect(forwardTargetError('https://staging.example.com/api?x=1')).toBeNull();
    expect(forwardTargetError('localhost:3000/base')).toBeNull();
    expect(forwardTargetError('[::1]:8080')).toBeNull();
    expect(forwardTargetError('wss://echo.example.com')).toBeNull();
    expect(forwardTargetError('')).toMatch(/Enter/);
    expect(forwardTargetError('ftp://x')).toMatch(/Unsupported scheme/);
    expect(forwardTargetError('http://')).toMatch(/host/);
    expect(forwardTargetError('http://a b')).toMatch(/invalid characters/);
    expect(forwardTargetError('host:99999')).toMatch(/port/);
    expect(forwardTargetError('host:')).toMatch(/port/);
  });

  test('json patch (mirrors validate_patch)', () => {
    expect(jsonPatchError([{ op: 'add', path: '/a', value: 1 }])).toBeNull();
    expect(jsonPatchError([{ op: 'remove', path: '' }])).toBeNull();
    expect(jsonPatchError([{ op: 'move', from: '/a', path: '/b' }])).toBeNull();
    expect(jsonPatchError([{ op: 'nope', path: '/a' }])).toMatch(/unknown op/);
    expect(jsonPatchError([{ op: 'add', path: 'a', value: 1 }])).toMatch(/start with/);
    expect(jsonPatchError([{ op: 'add', path: '/a' }])).toMatch(/no "value"/);
    expect(jsonPatchError([{ op: 'copy', path: '/a' }])).toMatch(/no "from"/);
    expect(jsonPatchError([{ op: 'move', from: '/a', path: '/a/b' }])).toMatch(/into itself/);
    expect(jsonPatchError({})).toMatch(/array/);
    expect(jsonPatchError([1])).toMatch(/not an object/);
  });

  test('json text', () => {
    expect(parseJson('{"a":1}')).toEqual({ ok: true, value: { a: 1 } });
    const bad = parseJson('{\n  "a": 1,\n}');
    expect(bad.ok).toBe(false);
    expect(!bad.ok && bad.error).toMatch(/Invalid JSON/);
    expect(parseJson('  ').ok).toBe(false);
  });

  test('base64 & script', () => {
    expect(base64Error('aGVsbG8=')).toBeNull();
    expect(base64Error('aGVs bG8=')).toBeNull();
    expect(base64Error('***')).toMatch(/base64/);
    expect(scriptError('function onRequest(req) { return req; }')).toBeNull();
    expect(scriptError('const x = 1;')).toMatch(/onRequest/);
    expect(scriptError('function onRequest(req) { return req; ')).toMatch(/Syntax error/);
  });
});

describe('validateRule', () => {
  test('valid rule has no issues', () => {
    expect(
      validateRule(
        rule(
          [
            { type: 'method', methods: ['GET'] },
            { type: 'host', pattern: { op: 'glob', value: '*.example.com', caseSensitive: false } },
          ],
          { type: 'mock', status: 200, headers: [['content-type', 'application/json']], body: { type: 'json', value: {} } },
        ),
      ),
    ).toEqual([]);
  });

  test('matcher issues carry paths', () => {
    expect(
      paths(
        rule(
          [
            { type: 'method', methods: [] },
            { type: 'path', pattern: { op: 'regex', value: '(', caseSensitive: true } },
            { type: 'header', headers: [{ key: '' }, { key: 'bad name' }] },
            { type: 'host', pattern: { op: 'equals', value: '', caseSensitive: false } },
            { type: 'any', matchers: [] },
            { type: 'not', matcher: { type: 'port', ports: [] } },
          ],
          { type: 'close' },
        ),
      ),
    ).toEqual(['matchers.0.methods', 'matchers.1.pattern.value', 'matchers.2.headers.0.key', 'matchers.2.headers.1.key', 'matchers.3.pattern.value', 'matchers.4.matchers', 'matchers.5.matcher.ports']);
  });

  test('exists / key-only patterns need no value', () => {
    expect(paths(rule([{ type: 'header', headers: [{ key: 'x-a', value: { op: 'exists', value: '', caseSensitive: false, negate: true } }] }], { type: 'close' }))).toEqual([]);
    expect(paths(rule([{ type: 'query', params: [{ key: 'q', value: { op: 'equals', value: '', caseSensitive: false } }] }], { type: 'close' }))).toEqual([]);
  });

  test('action issues', () => {
    expect(paths(rule([], { type: 'mock', status: 42, headers: [['bad name', 'x']], body: { type: 'base64', data: '***' } }))).toEqual(['action.status', 'action.headers.0', 'action.body.data']);
    expect(paths(rule([], { type: 'forward', target: 'ftp://x', preserveHost: false }))).toEqual(['action.target']);
    expect(paths(rule([], { type: 'map-local', path: ' ', fallbackToUpstream: false, headers: [] }))).toEqual(['action.path']);
    expect(paths(rule([], { type: 'highlight', color: '' }))).toEqual(['action.color']);
    expect(paths(rule([], { type: 'breakpoint', request: false, response: false }))).toEqual(['action.request']);
    expect(paths(rule([], { type: 'delay', ms: -1, jitterMs: 0 }))).toEqual(['action.ms']);
    expect(paths(rule([], { type: 'script', source: 'nothing' }))).toEqual(['action.source']);
    expect(
      paths(
        rule([], {
          type: 'transform',
          request: { method: '', urlReplace: [{ find: '(', replace: '', regex: true, caseSensitive: false }], setHeaders: [['ok', 'a\nb']] },
          response: { status: 5, body: { type: 'json-patch', operations: [{ op: 'x' }] } },
        }),
      ),
    ).toEqual(['action.request.method', 'action.request.urlReplace.0.find', 'action.request.setHeaders.0', 'action.response.status', 'action.response.body.operations']);
  });

  test('match limit', () => {
    expect(paths(rule([], { type: 'close' }, { maxMatches: 0 }))).toEqual(['maxMatches']);
    expect(paths(rule([], { type: 'close' }, { maxMatches: 3 }))).toEqual([]);
  });

  test('issue lookup helpers', () => {
    const issues = validateRule(rule([{ type: 'any', matchers: [{ type: 'host', pattern: { op: 'equals', value: '', caseSensitive: false } }] }], { type: 'close' }));
    expect(issueAt(issues, 'matchers.0')).toBeUndefined();
    expect(issueAt(issues, 'matchers.0', true)).toBe('Enter a value');
    expect(issuesAt(issues, 'matchers.0.matchers.0').length).toBe(1);
  });
});

describe('status while typing', () => {
  test('empty / out-of-range statuses are reported, not clamped', () => {
    const base = { id: 'r', enabled: true, matchers: [] };
    const issues = (status: number) => validateRule({ ...base, action: { type: 'mock', status, headers: [], body: { type: 'empty' } } });
    expect(issues(Number.NaN)[0]).toMatchObject({ path: 'action.status', message: 'Enter a status code' });
    expect(issues(42)[0]).toMatchObject({ path: 'action.status', message: 'Status must be between 100 and 999' });
    expect(issues(1000)[0]?.path).toBe('action.status');
    expect(issues(299)).toEqual([]);
  });
});

describe('ws-breakpoint', () => {
  test('the optional payload pattern is checked', () => {
    expect(paths(rule([], { type: 'ws-breakpoint', direction: 'both' }))).toEqual([]);
    expect(paths(rule([], { type: 'ws-breakpoint', direction: 'sent', pattern: { op: 'contains', value: 'x', caseSensitive: false } }))).toEqual([]);
    expect(paths(rule([], { type: 'ws-breakpoint', direction: 'sent', pattern: { op: 'regex', value: 'a(b', caseSensitive: true } }))).toEqual(['action.pattern.value']);
    expect(paths(rule([], { type: 'ws-breakpoint', direction: 'sent', pattern: { op: 'contains', value: '', caseSensitive: false } }))).toEqual(['action.pattern.value']);
  });
});

describe('tls-profile', () => {
  test('the profile must be a known one', () => {
    for (const profile of ['native', 'chrome', 'firefox', 'safari', 'edge', 'mirror-client'] as const) {
      expect(paths(rule([], { type: 'tls-profile', profile }))).toEqual([]);
    }
    const bogus = { type: 'tls-profile', profile: 'netscape' } as unknown as Action;
    expect(paths(rule([], bogus))).toEqual(['action.profile']);
  });
});
