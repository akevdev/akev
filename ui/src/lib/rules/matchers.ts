/**
 * Matcher catalogue for the rule editor: labels, groups for the "+ Add condition" menu,
 * allowed operators per type and default instances. Pure data — the page maps `icon` keys
 * to lucide icons.
 */

import type { KeyValueMatch, Matcher, MatcherType, MatchOp, StringMatch } from '../../api/types';

export type MatcherGroup = 'Request line' | 'Headers & cookies' | 'Body' | 'Client' | 'Logic';

export const MATCHER_GROUPS: readonly MatcherGroup[] = ['Request line', 'Headers & cookies', 'Body', 'Client', 'Logic'];

export type MatcherValueKind = 'string' | 'methods' | 'ports' | 'protocols' | 'versions' | 'key-values' | 'json' | 'logic' | 'not';

export interface MatcherInfo {
  type: MatcherType;
  /** Fixed-width row label (`Method`, `Host`…). */
  label: string;
  /** One-line description for the add menu. */
  description: string;
  group: MatcherGroup;
  /** Icon key, mapped to a lucide icon by the page. */
  icon: string;
  value: MatcherValueKind;
  /** Operators for string-pattern matchers (or per key/value). */
  ops?: readonly MatchOp[];
  /** Input placeholder for the main value. */
  placeholder?: string;
  /** Key placeholder for key/value matchers. */
  keyPlaceholder?: string;
}

const TEXT_OPS: readonly MatchOp[] = ['equals', 'contains', 'starts-with', 'ends-with', 'glob', 'regex'];
const KV_OPS: readonly MatchOp[] = ['exists', 'equals', 'contains', 'starts-with', 'ends-with', 'glob', 'regex'];

export const MATCHERS: readonly MatcherInfo[] = [
  { type: 'method', label: 'Method', description: 'GET, POST, PUT… (any of)', group: 'Request line', icon: 'method', value: 'methods' },
  { type: 'host', label: 'Host', description: 'Hostname without port, e.g. *.example.com', group: 'Request line', icon: 'host', value: 'string', ops: TEXT_OPS, placeholder: 'api.example.com' },
  { type: 'path', label: 'Path', description: 'Path without the query, e.g. /api/**', group: 'Request line', icon: 'path', value: 'string', ops: TEXT_OPS, placeholder: '/api/users/*' },
  { type: 'url', label: 'URL', description: 'Full URL including the query string', group: 'Request line', icon: 'url', value: 'string', ops: TEXT_OPS, placeholder: 'https://api.example.com/v1/*' },
  { type: 'query', label: 'Query', description: 'Query parameters (all must match)', group: 'Request line', icon: 'query', value: 'key-values', ops: KV_OPS, keyPlaceholder: 'param' },
  { type: 'port', label: 'Port', description: 'Destination port (any of)', group: 'Request line', icon: 'port', value: 'ports' },
  { type: 'protocol', label: 'Protocol', description: 'http, https, ws or wss', group: 'Request line', icon: 'protocol', value: 'protocols' },
  { type: 'http-version', label: 'HTTP version', description: 'HTTP/1.1, HTTP/2, HTTP/3…', group: 'Request line', icon: 'version', value: 'versions' },
  { type: 'header', label: 'Header', description: 'Request headers (case-insensitive names)', group: 'Headers & cookies', icon: 'header', value: 'key-values', ops: KV_OPS, keyPlaceholder: 'header-name' },
  { type: 'cookie', label: 'Cookie', description: 'Cookies sent in the Cookie header', group: 'Headers & cookies', icon: 'cookie', value: 'key-values', ops: KV_OPS, keyPlaceholder: 'cookie' },
  { type: 'body', label: 'Body', description: 'Decoded request body as text', group: 'Body', icon: 'body', value: 'string', ops: TEXT_OPS, placeholder: 'text in the body' },
  { type: 'json-body', label: 'JSON body', description: 'Parsed JSON equals or includes a value', group: 'Body', icon: 'json', value: 'json' },
  { type: 'form', label: 'Form field', description: 'URL-encoded or multipart form fields', group: 'Body', icon: 'form', value: 'key-values', ops: KV_OPS, keyPlaceholder: 'field' },
  { type: 'graphql-operation', label: 'GraphQL op', description: 'GraphQL operation name', group: 'Body', icon: 'graphql', value: 'string', ops: TEXT_OPS, placeholder: 'GetUser' },
  { type: 'source', label: 'Source', description: 'Client app or interceptor (chrome, curl…)', group: 'Client', icon: 'source', value: 'string', ops: TEXT_OPS, placeholder: 'chrome' },
  { type: 'any', label: 'Any of', description: 'At least one nested condition (OR)', group: 'Logic', icon: 'any', value: 'logic' },
  { type: 'all', label: 'All of', description: 'Every nested condition (AND)', group: 'Logic', icon: 'all', value: 'logic' },
  { type: 'not', label: 'Not', description: 'Inverts a nested condition (NOT)', group: 'Logic', icon: 'not', value: 'not' },
];

const BY_TYPE = new Map<MatcherType, MatcherInfo>(MATCHERS.map((m) => [m.type, m]));

export function matcherInfo(type: MatcherType): MatcherInfo {
  return BY_TYPE.get(type) ?? { type, label: type, description: '', group: 'Logic', icon: 'unknown', value: 'string', ops: TEXT_OPS };
}

export const OP_LABELS: Readonly<Record<MatchOp, string>> = {
  equals: 'equals',
  contains: 'contains',
  'starts-with': 'starts with',
  'ends-with': 'ends with',
  glob: 'matches glob',
  regex: 'matches regex',
  exists: 'is present',
};

export const PROTOCOLS = ['http', 'https', 'ws', 'wss'] as const;
export const HTTP_VERSIONS = ['HTTP/1.0', 'HTTP/1.1', 'HTTP/2', 'HTTP/3'] as const;

/** A string pattern with defaults (`StringMatch::default()` is `equals ""`, case-insensitive). */
export function stringMatch(op: MatchOp = 'equals', value = '', caseSensitive = op === 'regex'): StringMatch {
  return { op, value, caseSensitive };
}

/** A new matcher of the given type with sensible defaults. */
export function defaultMatcher(type: MatcherType): Matcher {
  switch (type) {
    case 'method':
      return { type, methods: ['GET'] };
    case 'url':
      return { type, pattern: stringMatch('glob') };
    case 'host':
      return { type, pattern: stringMatch('equals') };
    case 'port':
      return { type, ports: [443] };
    case 'path':
      return { type, pattern: stringMatch('starts-with') };
    case 'protocol':
      return { type, protocols: ['https'] };
    case 'query':
      return { type, params: [{ key: '', value: stringMatch('equals') }] };
    case 'header':
      return { type, headers: [{ key: '', value: stringMatch('equals') }] };
    case 'cookie':
      return { type, cookies: [{ key: '', value: stringMatch('equals') }] };
    case 'body':
      return { type, pattern: stringMatch('contains') };
    case 'json-body':
      return { type, value: {}, partial: true };
    case 'form':
      return { type, fields: [{ key: '', value: stringMatch('equals') }] };
    case 'graphql-operation':
      return { type, pattern: stringMatch('equals') };
    case 'source':
      return { type, pattern: stringMatch('equals') };
    case 'http-version':
      return { type, versions: ['HTTP/2'] };
    case 'not':
      return { type, matcher: defaultMatcher('host') };
    case 'any':
      return { type, matchers: [defaultMatcher('host'), defaultMatcher('host')] };
    case 'all':
      return { type, matchers: [defaultMatcher('host'), defaultMatcher('path')] };
  }
}

/** The key/value list of a key/value matcher. */
export function kvList(m: Matcher): KeyValueMatch[] | null {
  switch (m.type) {
    case 'query':
      return m.params;
    case 'header':
      return m.headers;
    case 'cookie':
      return m.cookies;
    case 'form':
      return m.fields;
    default:
      return null;
  }
}

/** Replace the key/value list of a key/value matcher. */
export function withKvList(m: Matcher, list: KeyValueMatch[]): Matcher {
  switch (m.type) {
    case 'query':
      return { ...m, params: list };
    case 'header':
      return { ...m, headers: list };
    case 'cookie':
      return { ...m, cookies: list };
    case 'form':
      return { ...m, fields: list };
    default:
      return m;
  }
}

/** The string pattern of a single-pattern matcher. */
export function patternOf(m: Matcher): StringMatch | null {
  switch (m.type) {
    case 'url':
    case 'host':
    case 'path':
    case 'body':
    case 'graphql-operation':
    case 'source':
      return m.pattern;
    default:
      return null;
  }
}

/** Whether evaluating this matcher needs the request body (mirrors `Matcher::needs_body`). */
export function needsBody(m: Matcher): boolean {
  switch (m.type) {
    case 'body':
    case 'json-body':
    case 'form':
    case 'graphql-operation':
      return true;
    case 'not':
      return needsBody(m.matcher);
    case 'any':
    case 'all':
      return m.matchers.some(needsBody);
    default:
      return false;
  }
}

/** Parse a comma/space separated port list. Returns `null` on any invalid entry. */
export function parsePorts(text: string): number[] | null {
  const parts = text.split(/[\s,]+/).filter(Boolean);
  const out: number[] = [];
  for (const p of parts) {
    if (!/^\d{1,5}$/.test(p)) return null;
    const n = Number(p);
    if (n < 1 || n > 65535) return null;
    if (!out.includes(n)) out.push(n);
  }
  return out;
}
