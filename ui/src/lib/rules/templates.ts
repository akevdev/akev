/**
 * Rule templates (DESIGN.md §7): the "New rule" menu, the empty-state gallery and palette
 * commands. Every template builds a complete, valid {@link RuleNode} with a fresh id.
 */

import type { Matcher, StringMatch } from '../../api/types';
import { defaultAction, SCRIPT_TEMPLATE } from './actions';
import { newId, type RuleNode } from './tree';

export type TemplateId =
  | 'mock'
  | 'map-remote'
  | 'map-local'
  | 'rewrite-request'
  | 'rewrite-response'
  | 'breakpoint'
  | 'delay'
  | 'throttle'
  | 'block-close'
  | 'block-reset'
  | 'block-timeout'
  | 'highlight'
  | 'script'
  | 'tls-profile'
  | 'ws-echo'
  | 'ws-reject'
  | 'ws-breakpoint'
  | 'blank';

export type TemplateSection = 'Respond' | 'Modify' | 'Block' | 'WebSocket';

export interface RuleTemplate {
  id: TemplateId;
  label: string;
  /** One-line description (menu, gallery, palette). */
  description: string;
  section: TemplateSection;
  /** Icon key mapped to a lucide icon by the page. */
  icon: string;
  build(): RuleNode;
}

const sm = (op: StringMatch['op'], value: string): StringMatch => ({ op, value, caseSensitive: op === 'regex' });

const host = (value: string, op: StringMatch['op'] = 'equals'): Matcher => ({ type: 'host', pattern: sm(op, value) });
const path = (value: string, op: StringMatch['op'] = 'glob'): Matcher => ({ type: 'path', pattern: sm(op, value) });

function rule(matchers: Matcher[], action: RuleNode['action'], extra: Partial<RuleNode> = {}): RuleNode {
  return { type: 'rule', id: newId(), enabled: true, matchers, action, ...extra };
}

export const TEMPLATES: readonly RuleTemplate[] = [
  {
    id: 'mock',
    label: 'Mock response',
    description: 'Answer matching requests with a canned status, headers and body',
    section: 'Respond',
    icon: 'mock',
    build: () =>
      rule([{ type: 'method', methods: ['GET'] }, host('api.example.com'), path('/api/users/*')], {
        type: 'mock',
        status: 200,
        headers: [['content-type', 'application/json']],
        body: { type: 'json', value: { users: [{ id: 1, name: 'Ada Lovelace' }] } },
      }),
  },
  {
    id: 'map-remote',
    label: 'Map Remote',
    description: 'Send requests to another host — e.g. staging or a local server',
    section: 'Respond',
    icon: 'forward',
    build: () => rule([host('api.example.com')], { type: 'forward', target: 'http://localhost:3000', preserveHost: false }),
  },
  {
    id: 'map-local',
    label: 'Map Local',
    description: 'Serve responses from files on disk, re-read on every request',
    section: 'Respond',
    icon: 'map-local',
    build: () => rule([host('cdn.example.com'), path('/assets/**')], { type: 'map-local', path: '~/project/dist', stripPrefix: '/assets', fallbackToUpstream: true, headers: [] }),
  },
  {
    id: 'rewrite-request',
    label: 'Rewrite request',
    description: 'Add or remove headers, change the URL, method or body before it is sent',
    section: 'Modify',
    icon: 'rewrite-request',
    build: () => rule([host('api.example.com')], { type: 'transform', request: { setHeaders: [['authorization', 'Bearer dev-token']] } }),
  },
  {
    id: 'rewrite-response',
    label: 'Rewrite response',
    description: 'Change the status, headers or body before the client sees it',
    section: 'Modify',
    icon: 'rewrite-response',
    build: () =>
      rule([host('api.example.com')], {
        type: 'transform',
        response: { setHeaders: [['access-control-allow-origin', '*']], body: { type: 'json-merge', patch: { debug: true } } },
      }),
  },
  {
    id: 'breakpoint',
    label: 'Breakpoint',
    description: 'Pause matching requests so you can inspect and edit them by hand',
    section: 'Modify',
    icon: 'breakpoint',
    build: () => rule([{ type: 'method', methods: ['POST'] }, host('api.example.com')], { type: 'breakpoint', request: true, response: false }),
  },
  {
    id: 'delay',
    label: 'Delay',
    description: 'Add latency (with optional jitter) to test loading states',
    section: 'Modify',
    icon: 'delay',
    build: () => rule([host('api.example.com')], { type: 'delay', ms: 1500, jitterMs: 500 }),
  },
  {
    id: 'throttle',
    label: 'Throttle',
    description: 'Simulate 3G, 4G, DSL or a custom slow network',
    section: 'Modify',
    icon: 'throttle',
    build: () => rule([], defaultAction('throttle')),
  },
  {
    id: 'highlight',
    label: 'Highlight',
    description: 'Colour matching exchanges in Traffic so they stand out',
    section: 'Modify',
    icon: 'highlight',
    build: () => rule([host('*.stripe.com', 'glob')], { type: 'highlight', color: 'purple', note: 'Payments' }),
  },
  {
    id: 'script',
    label: 'Script',
    description: 'Run JavaScript to rewrite or answer requests dynamically',
    section: 'Modify',
    icon: 'script',
    build: () => rule([host('api.example.com')], { type: 'script', source: SCRIPT_TEMPLATE }),
  },
  {
    id: 'tls-profile',
    label: 'Impersonate a browser',
    description: "Present Chrome's TLS and HTTP/2 fingerprint to a site that blocks proxies",
    section: 'Modify',
    icon: 'tls-profile',
    build: () => rule([host('shop.example.com')], { type: 'tls-profile', profile: 'chrome' }),
  },
  {
    id: 'block-close',
    label: 'Block — close connection',
    description: 'Close the connection without responding, like a dropped request',
    section: 'Block',
    icon: 'close',
    build: () => rule([host('ads.example.com')], { type: 'close' }),
  },
  {
    id: 'block-reset',
    label: 'Block — reset connection',
    description: 'Abort with a TCP reset to test network error handling',
    section: 'Block',
    icon: 'reset',
    build: () => rule([host('api.example.com'), path('/flaky/**')], { type: 'reset' }),
  },
  {
    id: 'block-timeout',
    label: 'Block — time out',
    description: 'Accept the request and never answer, to test timeouts',
    section: 'Block',
    icon: 'timeout',
    build: () => rule([host('api.example.com'), path('/slow/**')], { type: 'timeout' }),
  },
  {
    id: 'ws-echo',
    label: 'WebSocket echo',
    description: 'Accept WebSocket upgrades and echo every message back',
    section: 'WebSocket',
    icon: 'ws-echo',
    build: () => rule([{ type: 'protocol', protocols: ['ws', 'wss'] }, path('/socket')], { type: 'ws-echo' }),
  },
  {
    id: 'ws-reject',
    label: 'WebSocket reject',
    description: 'Refuse WebSocket upgrades with an HTTP status',
    section: 'WebSocket',
    icon: 'ws-reject',
    build: () => rule([{ type: 'protocol', protocols: ['ws', 'wss'] }], { type: 'ws-reject', status: 403, body: { type: 'text', text: 'WebSockets are disabled' } }),
  },
  {
    id: 'ws-breakpoint',
    label: 'Pause WebSocket messages',
    description: 'Hold matching messages so you can edit, drop or forward each one',
    section: 'WebSocket',
    icon: 'ws-breakpoint',
    build: () => rule([host('api.example.com')], { type: 'ws-breakpoint', direction: 'both' }),
  },
];

/** A blank rule (matches everything, passes through) — "start from scratch". */
export const BLANK_TEMPLATE: RuleTemplate = {
  id: 'blank',
  label: 'Blank rule',
  description: 'Start from scratch: matches every request and passes it through',
  section: 'Respond',
  icon: 'blank',
  build: () => rule([], { type: 'passthrough' }),
};

export const TEMPLATE_SECTIONS: readonly TemplateSection[] = ['Respond', 'Modify', 'Block', 'WebSocket'];

const BY_ID = new Map<TemplateId, RuleTemplate>([...TEMPLATES, BLANK_TEMPLATE].map((t) => [t.id, t]));

export function templateById(id: string | null | undefined): RuleTemplate | undefined {
  return id ? BY_ID.get(id as TemplateId) : undefined;
}

export const DEFAULT_TEMPLATE: TemplateId = 'mock';
