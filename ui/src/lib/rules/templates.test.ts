import { describe, expect, test } from 'bun:test';
import type { ActionType, MatcherType } from '../../api/types';
import { ACTIONS, actionInfo, bodyFromFile, bpsToKbps, contentTypeForFileName, defaultAction, detectPreset, kbpsToBps, languageForContentType, presetValues, THROTTLE_PRESETS } from './actions';
import { summarize } from './describe';
import { defaultMatcher, MATCHERS, matcherInfo, needsBody, parsePorts } from './matchers';
import { BLANK_TEMPLATE, DEFAULT_TEMPLATE, templateById, TEMPLATES } from './templates';
import { validateRule } from './validate';

const SPEC_TEMPLATES = [
  'mock',
  'map-remote',
  'map-local',
  'rewrite-request',
  'rewrite-response',
  'breakpoint',
  'delay',
  'throttle',
  'block-close',
  'block-reset',
  'block-timeout',
  'highlight',
  'script',
  'tls-profile',
  'ws-echo',
  'ws-reject',
  'ws-breakpoint',
];

describe('templates', () => {
  test('cover every template listed in DESIGN.md §7', () => {
    expect(TEMPLATES.map((t): string => t.id).sort()).toEqual([...SPEC_TEMPLATES].sort());
    expect(templateById(DEFAULT_TEMPLATE)?.id).toBe('mock');
    expect(templateById('blank')).toBe(BLANK_TEMPLATE);
    expect(templateById('nope')).toBeUndefined();
  });

  for (const t of [...TEMPLATES, BLANK_TEMPLATE]) {
    test(`${t.id} builds a valid rule`, () => {
      const rule = t.build();
      expect(rule.type).toBe('rule');
      expect(rule.enabled).toBe(true);
      expect(rule.id).toMatch(/^[0-9a-f-]{36}$/);
      expect(Array.isArray(rule.matchers)).toBe(true);
      expect(validateRule(rule)).toEqual([]);
      // Round-trips through JSON unchanged (pure data, no undefined/functions).
      expect(JSON.parse(JSON.stringify(rule))).toEqual(rule);
      expect(summarize(rule).length).toBeGreaterThan(5);
      expect(t.label.length).toBeGreaterThan(0);
      expect(t.description.length).toBeGreaterThan(10);
    });
  }

  test('fresh ids every build', () => {
    const a = TEMPLATES[0]!.build();
    const b = TEMPLATES[0]!.build();
    expect(a.id).not.toBe(b.id);
    expect(a.matchers).not.toBe(b.matchers);
  });

  test('action types match the template intent', () => {
    const type = (id: string) => templateById(id)!.build().action.type;
    expect(type('mock')).toBe('mock');
    expect(type('map-remote')).toBe('forward');
    expect(type('map-local')).toBe('map-local');
    expect(type('rewrite-request')).toBe('transform');
    expect(type('rewrite-response')).toBe('transform');
    expect(type('block-close')).toBe('close');
    expect(type('block-reset')).toBe('reset');
    expect(type('block-timeout')).toBe('timeout');
    expect(type('ws-echo')).toBe('ws-echo');
    expect(type('ws-reject')).toBe('ws-reject');
    expect(type('ws-breakpoint')).toBe('ws-breakpoint');
    expect(type('script')).toBe('script');
    expect(type('tls-profile')).toBe('tls-profile');
  });
});

describe('actions catalogue', () => {
  const ALL: ActionType[] = ['passthrough', 'forward', 'mock', 'map-local', 'close', 'reset', 'timeout', 'ws-echo', 'ws-reject', 'transform', 'breakpoint', 'delay', 'throttle', 'highlight', 'script', 'ws-breakpoint', 'tls-profile'];

  test('every action type has an entry and a valid default', () => {
    expect(ACTIONS.map((a) => a.type).sort()).toEqual([...ALL].sort());
    for (const type of ALL) {
      const a = defaultAction(type);
      expect(a.type).toBe(type);
      const issues = validateRule({ id: 'x', enabled: true, matchers: [], action: a }).filter((i) => i.path !== 'action.path');
      expect(issues).toEqual([]);
    }
    expect(actionInfo('mock').terminal).toBe(true);
    expect(actionInfo('delay').terminal).toBe(false);
  });

  test('switching keeps compatible settings', () => {
    const tr = { setHeaders: [['a', '1']] as [string, string][] };
    const fromTransform = defaultAction('passthrough', { type: 'transform', request: tr });
    expect(fromTransform).toEqual({ type: 'passthrough', request: tr });
    const reject = defaultAction('ws-reject', { type: 'mock', status: 418, headers: [], body: { type: 'text', text: 'x' } });
    expect(reject).toEqual({ type: 'ws-reject', status: 418, body: { type: 'text', text: 'x' } });
    const reject2 = defaultAction('ws-reject', { type: 'mock', status: 200, headers: [], body: { type: 'empty' } });
    expect(reject2.type === 'ws-reject' && reject2.status).toBe(403);
  });

  test('throttle presets round-trip', () => {
    for (const p of THROTTLE_PRESETS) expect(detectPreset(presetValues(p.id))).toBe(p.id);
    expect(detectPreset({ downloadBps: 1, uploadBps: 2, latencyMs: 3 })).toBe('custom');
    expect(kbpsToBps(1000)).toBe(125_000);
    expect(bpsToKbps(125_000)).toBe(1000);
  });

  test('content type → editor language', () => {
    expect(languageForContentType('application/json; charset=utf-8')).toBe('json');
    expect(languageForContentType('application/problem+json')).toBe('json');
    expect(languageForContentType('text/html')).toBe('html');
    expect(languageForContentType(undefined)).toBe('text');
  });
});

describe('matcher catalogue', () => {
  const ALL: MatcherType[] = ['method', 'url', 'host', 'port', 'path', 'protocol', 'query', 'header', 'cookie', 'body', 'json-body', 'form', 'graphql-operation', 'source', 'http-version', 'not', 'any', 'all'];

  test('every matcher type is offered with a default', () => {
    expect(MATCHERS.map((m) => m.type).sort()).toEqual([...ALL].sort());
    for (const type of ALL) {
      expect(defaultMatcher(type).type).toBe(type);
      expect(matcherInfo(type).label.length).toBeGreaterThan(0);
    }
  });

  test('needsBody mirrors Matcher::needs_body', () => {
    expect(needsBody(defaultMatcher('json-body'))).toBe(true);
    expect(needsBody({ type: 'not', matcher: { type: 'form', fields: [] } })).toBe(true);
    expect(needsBody({ type: 'any', matchers: [defaultMatcher('host')] })).toBe(false);
  });

  test('parsePorts', () => {
    expect(parsePorts('80, 443 8080')).toEqual([80, 443, 8080]);
    expect(parsePorts('80,80')).toEqual([80]);
    expect(parsePorts('')).toEqual([]);
    expect(parsePorts('0')).toBeNull();
    expect(parsePorts('70000')).toBeNull();
    expect(parsePorts('8o')).toBeNull();
  });
});

describe('loading a body from a file', () => {
  const enc = (t: string) => new TextEncoder().encode(t);

  test('content type by extension', () => {
    expect(contentTypeForFileName('users.JSON')).toBe('application/json');
    expect(contentTypeForFileName('index.html')).toBe('text/html; charset=utf-8');
    expect(contentTypeForFileName('logo.png')).toBe('image/png');
    expect(contentTypeForFileName('Makefile')).toBeUndefined();
  });

  test('text files stay text, with a language', () => {
    const r = bodyFromFile('users.json', enc('{\n  "a": 1\n}'));
    expect(r.body).toEqual({ type: 'text', text: '{\n  "a": 1\n}' });
    expect(r.contentType).toBe('application/json');
    expect(r.language).toBe('json');
  });

  test('binary files become base64', () => {
    const png = new Uint8Array([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);
    const r = bodyFromFile('logo.png', png);
    expect(r.body).toEqual({ type: 'base64', data: 'iVBORw0KGgo=' });
    expect(r.contentType).toBe('image/png');
    // Invalid UTF-8 without a known extension is binary too.
    expect(bodyFromFile('blob', new Uint8Array([0xff, 0xfe, 0x00])).body.type).toBe('base64');
  });
});
