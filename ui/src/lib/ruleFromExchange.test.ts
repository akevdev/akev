import { describe, expect, test } from 'bun:test';
import type { Headers, Rule } from '../api/types';
import { textToBase64 } from './encoding';
import {
  blockHostRule,
  bodySourceFromBase64,
  breakpointRule,
  extensionFor,
  mapLocalRule,
  mockableHeaders,
  mockRule,
  pathWithoutQuery,
  prependRule,
  requestMatchers,
  rewriteRule,
  ruleChipLabel,
  suggestLocalPath,
  type RuleSource,
} from './ruleFromExchange';

const src: RuleSource = { id: 42, method: 'post', scheme: 'https', host: 'api.example.com', port: 443, path: '/v1/users?page=2', contentKind: 'json', contentType: 'application/json' };

describe('requestMatchers', () => {
  test('method + host + path without query', () => {
    expect(requestMatchers(src)).toEqual([
      { type: 'method', methods: ['POST'] },
      { type: 'host', pattern: { op: 'equals', value: 'api.example.com', caseSensitive: false } },
      { type: 'path', pattern: { op: 'equals', value: '/v1/users', caseSensitive: true } },
    ]);
  });

  test('non-default ports add a port matcher', () => {
    const m = requestMatchers({ ...src, scheme: 'http', host: 'localhost', port: 3000, path: '/' });
    expect(m).toContainEqual({ type: 'port', ports: [3000] });
    expect(m.find((x) => x.type === 'path')).toEqual({ type: 'path', pattern: { op: 'equals', value: '/', caseSensitive: true } });
  });

  test('default ports are omitted (http 80, wss 443)', () => {
    expect(requestMatchers({ ...src, scheme: 'http', port: 80 }).some((m) => m.type === 'port')).toBe(false);
    expect(requestMatchers({ ...src, scheme: 'wss', port: 443 }).some((m) => m.type === 'port')).toBe(false);
  });

  test('empty path becomes /', () => {
    expect(requestMatchers({ ...src, path: '?a=1' }).at(-1)).toEqual({ type: 'path', pattern: { op: 'equals', value: '/', caseSensitive: true } });
  });

  test('the query and fragment are not part of the path', () => {
    expect(pathWithoutQuery('/a?b#c')).toBe('/a');
    expect(pathWithoutQuery('/a#c')).toBe('/a');
    expect(pathWithoutQuery('?x')).toBe('/');
  });
});

describe('mockableHeaders', () => {
  test('drops hop-by-hop, length, encoding and pseudo headers; keeps order and duplicates', () => {
    const h: Headers = [
      [':status', '200'],
      ['Content-Type', 'application/json'],
      ['Content-Length', '12'],
      ['Content-Encoding', 'gzip'],
      ['Transfer-Encoding', 'chunked'],
      ['Connection', 'keep-alive'],
      ['Keep-Alive', 'timeout=5'],
      ['Set-Cookie', 'a=1'],
      ['set-cookie', 'b=2'],
      ['Upgrade', 'h2c'],
      ['TE', 'trailers'],
      ['Trailer', 'x'],
      ['Proxy-Connection', 'keep-alive'],
      ['Cache-Control', 'no-store'],
      ['Date', 'Mon, 09 Nov 2026 07:10:29 GMT'],
      ['age', '12'],
    ];
    expect(mockableHeaders(h)).toEqual([
      ['Content-Type', 'application/json'],
      ['Set-Cookie', 'a=1'],
      ['set-cookie', 'b=2'],
      ['Cache-Control', 'no-store'],
    ]);
  });
});

describe('bodySourceFromBase64', () => {
  test('UTF-8 text stays text', () => {
    expect(bodySourceFromBase64(textToBase64('{"a":1}'))).toEqual({ type: 'text', text: '{"a":1}' });
  });
  test('binary bodies stay base64', () => {
    const b64 = btoa(String.fromCharCode(0xff, 0x00));
    expect(bodySourceFromBase64(b64)).toEqual({ type: 'base64', data: b64 });
  });
  test('empty otherwise', () => {
    expect(bodySourceFromBase64('')).toEqual({ type: 'empty' });
  });
});

describe('rule builders', () => {
  test('mock rule carries status, cleaned headers and body', () => {
    const rule = mockRule(src, { status: 201, statusMessage: 'Created', headers: [['Content-Type', 'application/json'], ['Content-Length', '7']] }, { type: 'text', text: '{"a":1}' }, 'r1');
    expect(rule).toEqual({
      id: 'r1',
      enabled: true,
      note: 'Created from exchange #42',
      matchers: requestMatchers(src),
      action: { type: 'mock', status: 201, statusMessage: 'Created', headers: [['Content-Type', 'application/json']], body: { type: 'text', text: '{"a":1}' } },
    });
  });

  test('mock rule omits an empty status message', () => {
    const rule = mockRule(src, { status: 204, headers: [] }, { type: 'empty' }, 'r1');
    expect(rule.action).toEqual({ type: 'mock', status: 204, headers: [], body: { type: 'empty' } });
  });

  test('breakpoint pauses the chosen stage', () => {
    expect(breakpointRule(src, { request: true, response: false }, 'b').action).toEqual({ type: 'breakpoint', request: true, response: false });
    expect(breakpointRule(src, { request: false, response: true }, 'b').action).toEqual({ type: 'breakpoint', request: false, response: true });
    expect(breakpointRule(src, { request: true, response: false }, 'b').matchers).toEqual(requestMatchers(src));
  });

  test('block host matches only the host and closes', () => {
    const r = blockHostRule(src, 'x');
    expect(r.matchers).toEqual([{ type: 'host', pattern: { op: 'equals', value: 'api.example.com', caseSensitive: false } }]);
    expect(r.action).toEqual({ type: 'close' });
  });

  test('map-local prefills a path and falls back upstream', () => {
    const r = mapLocalRule(src, { baseDir: '/home/me/.akev' }, 'm');
    expect(r.action).toEqual({ type: 'map-local', path: '/home/me/.akev/mocks/api.example.com/v1/users.json', fallbackToUpstream: true, headers: [] });
  });

  test('rewrite is an empty transform', () => {
    expect(rewriteRule(src, 'w').action).toEqual({ type: 'transform', request: {}, response: {} });
  });

  test('rules without an exchange id have no note', () => {
    const { id: _id, ...noId } = src;
    expect((breakpointRule(noId as RuleSource, { request: true, response: false }, 'b') as Rule).note).toBeUndefined();
  });
});

describe('suggestLocalPath', () => {
  test('directory paths become index files', () => {
    expect(suggestLocalPath({ ...src, path: '/' , contentKind: 'html', contentType: 'text/html' })).toBe('mocks/api.example.com/index.html');
    expect(suggestLocalPath({ ...src, path: '/docs/', contentKind: 'html', contentType: 'text/html' })).toBe('mocks/api.example.com/docs/index.html');
  });
  test('keeps existing extensions and strips traversal', () => {
    expect(suggestLocalPath({ ...src, path: '/static/../app.js?v=3', contentKind: 'javascript', contentType: 'text/javascript' })).toBe('mocks/api.example.com/static/app.js');
  });
  test('decodes percent-encoding and sanitises ports in the host', () => {
    expect(suggestLocalPath({ ...src, host: '[::1]:8080', path: '/a%20b', contentKind: 'unknown', contentType: undefined })).toBe('mocks/[__1]_8080/a b');
  });
  test('windows base directories use backslashes', () => {
    expect(suggestLocalPath({ ...src, path: '/x' }, 'C:\\Users\\me\\Akev\\')).toBe('C:\\Users\\me\\Akev\\mocks\\api.example.com\\x.json');
  });
  test('extensionFor prefers the MIME type', () => {
    expect(extensionFor({ contentKind: 'image', contentType: 'image/svg+xml; charset=utf-8' })).toBe('svg');
    expect(extensionFor({ contentKind: 'image', contentType: undefined })).toBe('');
    expect(extensionFor({ contentKind: 'css' })).toBe('css');
  });
});

describe('prependRule / ruleChipLabel', () => {
  test('prepends at the top and keeps the version', () => {
    const r = breakpointRule(src, { request: true, response: false }, 'new');
    const set = prependRule({ version: 1, items: [{ type: 'group', id: 'g', title: 'G', enabled: true, collapsed: false, items: [] }] }, r);
    expect(set.items[0]).toEqual({ type: 'rule', ...r });
    expect(set.items).toHaveLength(2);
    expect(prependRule(null, r)).toEqual({ version: 1, items: [{ type: 'rule', ...r }] });
  });
  test('new rules get unique UUIDs', () => {
    const a = blockHostRule(src).id;
    expect(a).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/);
    expect(blockHostRule(src).id).not.toBe(a);
  });
  test('chip labels use the title, else the action name', () => {
    const r = blockHostRule(src, 'x');
    expect(ruleChipLabel({ type: 'rule', ...r, title: 'Block ads' })).toBe('Block ads');
    expect(ruleChipLabel({ type: 'rule', ...r })).toBe('Close rule');
    expect(ruleChipLabel({ type: 'rule', ...breakpointRule(src, { request: true, response: false }) })).toBe('Breakpoint rule');
    expect(ruleChipLabel(undefined)).toBe('Rule');
  });
});
