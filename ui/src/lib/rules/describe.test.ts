import { describe, expect, test } from 'bun:test';
import type { Action, KeyValueMatch, Matcher, MatchOp, Rule, StringMatch } from '../../api/types';
import {
  debugString,
  describeAction,
  describeMatcher,
  formatRate,
  jsonCompact,
  ruleLabel,
  summarize,
  summarizeTokens,
  tokensToText,
  truncate,
} from './describe';

// Helpers mirroring crates/akev-rules/src/tests (helpers::sm / neg / kv / rule / mock).
const sm = (op: MatchOp, value: string): StringMatch => ({ op, value, caseSensitive: op === 'regex' });
const neg = (m: StringMatch): StringMatch => ({ ...m, negate: true });
const kv = (key: string, value?: StringMatch): KeyValueMatch => (value ? { key, value } : { key });
const rule = (matchers: Matcher[], action: Action, extra: Partial<Rule> = {}): Rule => ({ id: 'r', enabled: true, matchers, action, ...extra });
const mock = (status: number): Action => ({ type: 'mock', status, headers: [], body: { type: 'empty' } });

describe('summaries (parity with describe_tests.rs::summaries)', () => {
  test('method + host + path', () => {
    const r = rule(
      [
        { type: 'method', methods: ['GET'] },
        { type: 'host', pattern: sm('glob', '*.example.com') },
        { type: 'path', pattern: sm('glob', '/api/*') },
      ],
      mock(200),
    );
    expect(summarize(r)).toBe('GET requests to *.example.com/api/* → mock 200');
  });

  test('json body + max matches', () => {
    const r = rule(
      [
        { type: 'method', methods: ['POST', 'PUT', 'PATCH'] },
        { type: 'path', pattern: sm('equals', '/login') },
        { type: 'json-body', value: { user: 'admin' }, partial: true },
      ],
      { type: 'delay', ms: 500, jitterMs: 0 },
      { maxMatches: 1 },
    );
    expect(summarize(r)).toBe('POST, PUT or PATCH requests to /login with JSON body including {"user":"admin"} → delay 500 ms (once)');
    expect(summarize({ ...r, maxMatches: 3 }).endsWith('(3 times)')).toBe(true);
  });

  test('all requests / websockets / header + ports', () => {
    expect(summarize(rule([], { type: 'passthrough' }))).toBe('All requests → pass through');
    expect(summarize(rule([{ type: 'protocol', protocols: ['ws', 'wss'] }], { type: 'ws-echo' }))).toBe('WebSockets → echo WebSocket messages');
    expect(
      summarize(
        rule(
          [
            { type: 'header', headers: [kv('authorization', neg(sm('exists', '')))] },
            { type: 'port', ports: [8080, 8443] },
          ],
          { type: 'breakpoint', request: true, response: false },
        ),
      ),
    ).toBe('Requests with header no authorization on ports 8080 or 8443 → pause request');
  });

  test('empty method list', () => {
    expect(summarize(rule([{ type: 'method', methods: [] }], mock(200)))).toBe('No requests → mock 200');
  });

  test('single ws protocol adds "over ws"', () => {
    expect(summarize(rule([{ type: 'protocol', protocols: ['WSS'] }], { type: 'ws-echo' }))).toBe('WebSockets over wss → echo WebSocket messages');
  });

  test('non-plain host/path patterns are described as phrases', () => {
    expect(summarize(rule([{ type: 'host', pattern: sm('ends-with', 'example.com') }], mock(404)))).toBe('Requests with host ending with example.com → mock 404');
    expect(summarize(rule([{ type: 'path', pattern: neg(sm('glob', '/a')) }], mock(404)))).toBe('Requests with path not matching /a → mock 404');
  });
});

describe('matcher phrases (parity with describe_tests.rs::matcher_phrases)', () => {
  test('key/value matchers', () => {
    expect(describeMatcher({ type: 'query', params: [kv('q', sm('equals', '1')), kv('flag')] })).toBe('with query q=1 and flag');
    expect(describeMatcher({ type: 'cookie', cookies: [kv('session', sm('regex', '^a'))] })).toBe('with cookie session matching /^a/');
    expect(describeMatcher({ type: 'header', headers: [kv('x-a', sm('equals', '1'))] })).toBe('with header x-a: 1');
    expect(describeMatcher({ type: 'form', fields: [kv('a', sm('starts-with', 'b'))] })).toBe('with form field a starting with b');
  });

  test('single-pattern matchers', () => {
    expect(describeMatcher({ type: 'body', pattern: sm('contains', 'x') })).toBe('with body containing "x"');
    expect(describeMatcher({ type: 'graphql-operation', pattern: sm('equals', 'GetUser') })).toBe('for GraphQL operation GetUser');
    expect(describeMatcher({ type: 'graphql-operation', pattern: sm('regex', '^Get') })).toBe('for GraphQL operation matching /^Get/');
    expect(describeMatcher({ type: 'source', pattern: sm('equals', 'chrome') })).toBe('from chrome');
    expect(describeMatcher({ type: 'source', pattern: sm('contains', 'py') })).toBe('from source containing "py"');
    expect(describeMatcher({ type: 'http-version', versions: ['HTTP/2'] })).toBe('over HTTP/2');
    expect(describeMatcher({ type: 'port', ports: [8080] })).toBe('on port 8080');
    expect(describeMatcher({ type: 'url', pattern: sm('starts-with', 'https://a') })).toBe('with URL starting with https://a');
    expect(describeMatcher({ type: 'json-body', value: [1, 2], partial: false })).toBe('with JSON body [1,2]');
  });

  test('logic', () => {
    expect(describeMatcher({ type: 'not', matcher: { type: 'path', pattern: sm('starts-with', '/admin') } })).toBe('not (with path starting with /admin)');
    expect(
      describeMatcher({
        type: 'any',
        matchers: [
          { type: 'url', pattern: sm('contains', 'a') },
          { type: 'host', pattern: neg(sm('equals', 'b')) },
        ],
      }),
    ).toBe('(with URL containing "a" or with host not = b)');
    expect(describeMatcher({ type: 'all', matchers: [{ type: 'method', methods: ['GET', 'HEAD'] }] })).toBe('(using GET or HEAD)');
  });
});

describe('action phrases (parity with describe_tests.rs::action_phrases)', () => {
  test('forward', () => {
    expect(describeAction({ type: 'forward', target: 'http://localhost:3000', stripPrefix: '/api', preserveHost: true })).toBe(
      'forward to http://localhost:3000 (stripping /api) keeping the Host header',
    );
  });

  test('transform', () => {
    expect(
      describeAction({
        type: 'transform',
        request: {
          method: 'POST',
          setHeaders: [
            ['X-A', '1'],
            ['x-a', '2'],
          ],
          body: { type: 'json-patch', operations: [{ op: 'remove', path: '/a' }] },
        },
        response: { status: 404 },
      }),
    ).toBe('transform (request: method → POST, set header X-A, patch JSON body (1 op); response: status → 404)');
    expect(describeAction({ type: 'transform' })).toBe('transform (nothing)');
    expect(describeAction({ type: 'transform', request: {} })).toBe('transform (nothing)');
    expect(describeAction({ type: 'passthrough', response: { removeHeaders: ['ETag'], statusMessage: 'Hi' } })).toBe(
      'pass through (response: status message → "Hi", remove header ETag)',
    );
  });

  test('throttle / delay / highlight / ws / script / map-local', () => {
    expect(describeAction({ type: 'throttle', downloadBps: 125_000, uploadBps: 32_000, latencyMs: 100 })).toBe('throttle to 1.0 Mbit/s down, 256 kbit/s up, +100 ms latency');
    expect(describeAction({ type: 'throttle', downloadBps: 0, uploadBps: 0, latencyMs: 0 })).toBe('throttle (unlimited)');
    expect(describeAction({ type: 'delay', ms: 10, jitterMs: 5 })).toBe('delay 10 ms (+0–5 ms)');
    expect(describeAction({ type: 'highlight', color: 'red', note: 'look' })).toBe('highlight red (look)');
    expect(describeAction({ type: 'highlight', color: 'red' })).toBe('highlight red');
    expect(describeAction({ type: 'ws-reject', status: 403, body: { type: 'empty' } })).toBe('reject the WebSocket (403)');
    expect(describeAction({ type: 'script', source: '' })).toBe('run script');
    expect(describeAction({ type: 'map-local', path: '/srv', fallbackToUpstream: false, headers: [] })).toBe('serve local files from /srv');
    expect(describeAction({ type: 'breakpoint', request: true, response: true })).toBe('pause request and response');
    expect(describeAction({ type: 'breakpoint', request: false, response: true })).toBe('pause response');
    expect(describeAction({ type: 'breakpoint', request: false, response: false })).toBe('breakpoint (disabled)');
    for (const a of [{ type: 'close' }, { type: 'reset' }, { type: 'timeout' }] as Action[]) expect(describeAction(a).length).toBeGreaterThan(0);
    expect(describeAction({ type: 'timeout' })).toBe('time out (never respond)');
  });

  test('body transforms', () => {
    expect(describeAction({ type: 'transform', response: { body: { type: 'replace', body: { type: 'text', text: 'x' } } } })).toBe(
      'transform (response: replace body with text "x")',
    );
    expect(describeAction({ type: 'transform', response: { body: { type: 'match-replace', replacements: [] } } })).toBe('transform (response: find/replace in body (0 rules))');
    expect(describeAction({ type: 'transform', request: { body: { type: 'json-merge', patch: {} } } })).toBe('transform (request: merge into JSON body)');
    expect(describeAction({ type: 'transform', request: { setQuery: [['a', '1'], ['b', null]], urlReplace: [{ find: 'a', replace: 'b', regex: false, caseSensitive: false }] } })).toBe(
      'transform (request: rewrite URL, set query a and b)',
    );
  });
});

describe('imported protocol matchers are implicit', () => {
  test('http+https protocol pair is dropped', () => {
    const r = rule(
      [
        { type: 'protocol', protocols: ['http', 'https'] },
        { type: 'method', methods: ['POST'] },
        { type: 'path', pattern: sm('equals', '/login') },
      ],
      mock(200),
    );
    expect(summarize(r)).toBe('POST requests to /login → mock 200');
    expect(summarize(rule([{ type: 'protocol', protocols: ['https'] }], mock(200)))).toBe('Requests over https → mock 200');
  });
});

describe('tokens', () => {
  test('plain text equals token concatenation minus extras', () => {
    const r = rule([{ type: 'method', methods: ['GET'] }, { type: 'host', pattern: sm('equals', 'api.example.com') }], {
      type: 'mock',
      status: 200,
      headers: [['Content-Type', 'application/json; charset=utf-8']],
      body: { type: 'empty' },
    });
    const tokens = summarizeTokens(r);
    expect(tokensToText(tokens)).toBe(summarize(r));
    expect(summarize(r)).toBe('GET requests to api.example.com → mock 200');
    expect(tokens.find((x) => x.kind === 'method')?.text).toBe('GET');
    expect(tokens.find((x) => x.kind === 'target')?.text).toBe('api.example.com');
    expect(tokens.find((x) => x.kind === 'status')?.text).toBe('200');
    // UI-only content type detail.
    const extra = tokens.find((x) => x.extra);
    expect(extra?.text.trim()).toBe('application/json');
  });

  test('ruleLabel prefers the title', () => {
    expect(ruleLabel({ title: '  Mock users ', matchers: [], action: mock(200) })).toBe('Mock users');
    expect(ruleLabel({ title: '  ', matchers: [], action: mock(200) })).toBe('All requests → mock 200');
  });
});

describe('formatting helpers', () => {
  test('truncate counts unicode scalars', () => {
    expect(truncate('abc', 3)).toBe('abc');
    expect(truncate('abcd', 3)).toBe('abc…');
    expect(truncate('😀😀😀😀', 2)).toBe('😀😀…');
  });
  test('debugString escapes like Rust {:?}', () => {
    expect(debugString('a"b\\c\nd\te')).toBe('"a\\"b\\\\c\\nd\\te"');
    expect(debugString('\u001b')).toBe('"\\u{1b}"');
    expect(debugString("it's")).toBe('"it\'s"');
  });
  test('jsonCompact', () => {
    expect(jsonCompact({ a: [1, 'x', null], b: true })).toBe('{"a":[1,"x",null],"b":true}');
    expect(jsonCompact(undefined)).toBe('null');
  });
  test('formatRate', () => {
    expect(formatRate(125_000)).toBe('1.0 Mbit/s');
    expect(formatRate(32_000)).toBe('256 kbit/s');
    expect(formatRate(100)).toBe('800 bit/s');
  });
  test('long values are truncated at 60 chars', () => {
    const long = 'x'.repeat(70);
    expect(describeMatcher({ type: 'url', pattern: sm('contains', long) })).toBe(`with URL containing "${'x'.repeat(60)}…"`);
  });
});

describe('editor robustness', () => {
  test('a half-typed (non-numeric) status renders as an ellipsis, not NaN', () => {
    expect(describeAction({ type: 'mock', status: Number.NaN, headers: [], body: { type: 'empty' } })).toBe('mock …');
    expect(describeAction({ type: 'ws-reject', status: Number.NaN, body: { type: 'empty' } })).toBe('reject the WebSocket (…)');
  });
});

describe('ws-breakpoint', () => {
  test('summaries match the daemon', () => {
    expect(describeAction({ type: 'ws-breakpoint', direction: 'both' })).toBe('pause WebSocket messages');
    expect(describeAction({ type: 'ws-breakpoint', direction: 'sent', pattern: sm('contains', 'auth') })).toBe('pause sent WebSocket messages containing "auth"');
    expect(describeAction({ type: 'ws-breakpoint', direction: 'received', pattern: sm('regex', '^4') })).toBe('pause received WebSocket messages matching /^4/');
    expect(summarize(rule([{ type: 'host', pattern: sm('equals', 'chat.example.com') }], { type: 'ws-breakpoint', direction: 'both' }))).toBe(
      'Requests to chat.example.com → pause WebSocket messages',
    );
  });
});

describe('tls-profile', () => {
  test('summaries match the daemon', () => {
    expect(describeAction({ type: 'tls-profile', profile: 'chrome' })).toBe('impersonate Chrome upstream (TLS + HTTP/2)');
    expect(describeAction({ type: 'tls-profile', profile: 'native' })).toBe("use Akev's native TLS fingerprint upstream");
    expect(describeAction({ type: 'tls-profile', profile: 'mirror-client' })).toBe("mirror the client's browser TLS fingerprint upstream");
    expect(summarize(rule([{ type: 'host', pattern: sm('equals', 'shop.example.com') }], { type: 'tls-profile', profile: 'firefox' }))).toBe(
      'Requests to shop.example.com → impersonate Firefox upstream (TLS + HTTP/2)',
    );
  });
});
