/**
 * Human-readable rule summaries — a faithful port of `crates/akev-rules/src/describe.rs`
 * (the CLI/MCP summaries), so the UI and the daemon describe rules identically:
 *
 * ```text
 * GET requests to *.example.com/api/* → mock 200
 * POST requests to /login with JSON body including {"user":"admin"} → delay 500 ms (once)
 * All requests → pass through
 * ```
 *
 * Everything is built from {@link Token}s so the rule list can colour methods, hosts, paths,
 * values and the action. `summarize()` (plain text) is exactly the concatenation of the
 * tokens of `summarizeTokens()` minus tokens flagged `extra` (UI-only details such as the
 * mock content type, which the Rust summary doesn't include).
 */

import type {
  Action,
  BodySource,
  BodyTransform,
  Headers,
  KeyValueMatch,
  Matcher,
  RequestTransform,
  ResponseTransform,
  Rule,
  StringMatch,
} from '../../api/types';
import { tlsProfileLabel } from '../tlsProfiles';

export type TokenKind =
  /** Plain connective text (`requests to`, `with header`). */
  | 'text'
  /** An HTTP method (`GET`). */
  | 'method'
  /** Host / path / URL patterns. */
  | 'target'
  /** A matched or configured value (header values, JSON, file paths…). */
  | 'value'
  /** The action verb (`mock`, `forward to`). */
  | 'action'
  /** A status code (`200`). */
  | 'status'
  /** The `→` between matchers and action. */
  | 'arrow'
  /** Secondary detail (`(once)`, `application/json`). */
  | 'muted'
  /** A highlight colour name. */
  | 'color';

export interface Token {
  kind: TokenKind;
  text: string;
  /** UI-only detail, excluded from the plain-text summary. */
  extra?: boolean;
}

type T = Token[];

const t = (text: string): Token => ({ kind: 'text', text });
const tok = (kind: TokenKind, text: string, extra?: boolean): Token => (extra ? { kind, text, extra } : { kind, text });

/** Concatenate tokens into plain text (skipping UI-only extras). */
export function tokensToText(tokens: readonly Token[]): string {
  let s = '';
  for (const x of tokens) if (!x.extra) s += x.text;
  return s;
}

// =====================================================================================
// string helpers mirroring Rust formatting
// =====================================================================================

/** Truncate to `max` Unicode scalar values, appending `…` (Rust `truncate`). */
export function truncate(s: string, max: number): string {
  const chars = Array.from(s);
  if (chars.length <= max) return s;
  return `${chars.slice(0, max).join('')}…`;
}

/** Rust `{:?}` formatting of a string: quoted with escapes. */
export function debugString(s: string): string {
  let out = '"';
  for (const ch of s) {
    const cp = ch.codePointAt(0) ?? 0;
    switch (ch) {
      case '"':
        out += '\\"';
        break;
      case '\\':
        out += '\\\\';
        break;
      case '\n':
        out += '\\n';
        break;
      case '\r':
        out += '\\r';
        break;
      case '\t':
        out += '\\t';
        break;
      case '\0':
        out += '\\0';
        break;
      default:
        out += cp < 0x20 || cp === 0x7f ? `\\u{${cp.toString(16)}}` : ch;
    }
  }
  return `${out}"`;
}

/** serde_json `Value::to_string()` (compact; key order preserved). */
export function jsonCompact(value: unknown): string {
  const s = JSON.stringify(value === undefined ? null : value);
  return s ?? 'null';
}

function capitalize(s: string): string {
  return s.length === 0 ? s : s[0]!.toUpperCase() + s.slice(1);
}

/** Join tokens lists with `, ` and a final ` or ` / ` and ` (Rust `join`). */
function joinTokens(items: readonly T[], word: string): T {
  if (items.length === 0) return [];
  if (items.length === 1) return [...(items[0] as T)];
  const out: T = [];
  items.forEach((item, i) => {
    if (i > 0) out.push(t(i === items.length - 1 ? ` ${word} ` : ', '));
    out.push(...item);
  });
  return out;
}

function joinStrings(items: readonly string[], word: string, kind: TokenKind = 'text'): T {
  return joinTokens(
    items.map((s) => [tok(kind, s)]),
    word,
  );
}

/** Plain-text join (used where the Rust code joins strings). */
export function joinText(items: readonly string[], word: string): string {
  if (items.length === 0) return '';
  if (items.length === 1) return items[0] as string;
  return `${items.slice(0, -1).join(', ')} ${word} ${items[items.length - 1]}`;
}

// =====================================================================================
// matchers
// =====================================================================================

function isPlain(p: StringMatch): boolean {
  return (p.op === 'equals' || p.op === 'glob') && !p.negate;
}

/** `starting with /api`, `matching /^a/`, `not = b`… */
function stringMatch(sm: StringMatch): T {
  const v = truncate(sm.value, 60);
  let base: T;
  switch (sm.op) {
    case 'equals':
      base = [t('= '), tok('value', v)];
      break;
    case 'contains':
      base = [t('containing '), tok('value', debugString(v))];
      break;
    case 'starts-with':
      base = [t('starting with '), tok('value', v)];
      break;
    case 'ends-with':
      base = [t('ending with '), tok('value', v)];
      break;
    case 'glob':
      base = [t('matching '), tok('value', v)];
      break;
    case 'regex':
      base = [t('matching '), tok('value', `/${v}/`)];
      break;
    case 'exists':
      base = [t('present')];
      break;
    default:
      base = [tok('value', v)];
  }
  return sm.negate ? [t('not '), ...base] : base;
}

function keyValues(kvs: readonly KeyValueMatch[], sep: string): T {
  const parts: T[] = kvs.map((kv) => {
    const sm = kv.value;
    if (!sm) return [tok('value', kv.key)];
    if (sm.op === 'equals' && !sm.negate) return [tok('value', `${kv.key}${sep}${truncate(sm.value, 40)}`)];
    if (sm.op === 'exists' && sm.negate) return [t('no '), tok('value', kv.key)];
    return [tok('value', kv.key), t(' '), ...stringMatch(sm)];
  });
  return joinTokens(parts, 'and');
}

/** Describe a single matcher as a phrase (`with header x-a: 1`, `on port 8080`…). */
export function matcherTokens(m: Matcher): T {
  switch (m.type) {
    case 'method':
      return [t('using '), ...joinStrings(m.methods, 'or', 'method')];
    case 'url':
      return [t('with URL '), ...stringMatch(m.pattern)];
    case 'host':
      return [t('with host '), ...stringMatch(m.pattern)];
    case 'port': {
      const list = m.ports.map(String);
      return m.ports.length === 1 ? [t('on port '), tok('value', list[0] as string)] : [t('on ports '), ...joinStrings(list, 'or', 'value')];
    }
    case 'path':
      return [t('with path '), ...stringMatch(m.pattern)];
    case 'protocol':
      return [t('over '), ...joinStrings(m.protocols, 'or', 'value')];
    case 'query':
      return [t('with query '), ...keyValues(m.params, '=')];
    case 'header':
      return [t('with header '), ...keyValues(m.headers, ': ')];
    case 'cookie':
      return [t('with cookie '), ...keyValues(m.cookies, '=')];
    case 'body':
      return [t('with body '), ...stringMatch(m.pattern)];
    case 'json-body': {
      const json = truncate(jsonCompact(m.value), 60);
      return m.partial ? [t('with JSON body including '), tok('value', json)] : [t('with JSON body '), tok('value', json)];
    }
    case 'form':
      return [t('with form field '), ...keyValues(m.fields, '=')];
    case 'graphql-operation':
      return m.pattern.op === 'equals' && !m.pattern.negate
        ? [t('for GraphQL operation '), tok('value', m.pattern.value)]
        : [t('for GraphQL operation '), ...stringMatch(m.pattern)];
    case 'source':
      return m.pattern.op === 'equals' && !m.pattern.negate
        ? [t('from '), tok('value', m.pattern.value)]
        : [t('from source '), ...stringMatch(m.pattern)];
    case 'http-version':
      return [t('over '), ...joinStrings(m.versions, 'or', 'value')];
    case 'not':
      return [t('not ('), ...matcherTokens(m.matcher), t(')')];
    case 'any':
      return [t('('), ...joinTokens(m.matchers.map(matcherTokens), 'or'), t(')')];
    case 'all':
      return [t('('), ...joinTokens(m.matchers.map(matcherTokens), 'and'), t(')')];
    default:
      return [t('(unknown condition)')];
  }
}

/** Describe a set of AND-ed matchers, e.g. `GET requests to example.com/api/*`. */
export function matchersTokens(ms: readonly Matcher[]): T {
  let methods: string[] | null = null;
  let ws = false;
  let host: StringMatch | null = null;
  let path: StringMatch | null = null;
  const rest: T[] = [];
  for (const m of ms) {
    if (m.type === 'method' && methods === null) {
      methods = m.methods;
      continue;
    }
    if (m.type === 'protocol' && m.protocols.length > 0 && m.protocols.every((p) => /^wss?$/i.test(p))) {
      ws = true;
      if (m.protocols.length === 1) rest.push([t('over '), tok('value', (m.protocols[0] as string).toLowerCase())]);
      continue;
    }
    // "HTTP, not WebSocket" (e.g. imported HTTP Toolkit rules) is implied by "requests".
    if (m.type === 'protocol' && m.protocols.length === 2 && ['http', 'https'].every((p) => m.protocols.some((q) => q.toLowerCase() === p))) continue;
    if (m.type === 'host' && host === null && isPlain(m.pattern)) {
      host = m.pattern;
      continue;
    }
    if (m.type === 'path' && path === null && isPlain(m.pattern)) {
      path = m.pattern;
      continue;
    }
    rest.push(matcherTokens(m));
  }
  const noun = ws ? 'WebSockets' : 'requests';
  const out: T = [];
  if (methods !== null && methods.length > 0) out.push(...joinStrings(methods, 'or', 'method'), t(` ${noun}`));
  else if (methods !== null) out.push(t(`No ${noun}`));
  else if (ms.length === 0) out.push(t(`All ${noun}`));
  else out.push(t(capitalize(noun)));
  if (host && path) out.push(t(' to '), tok('target', `${host.value}${path.value}`));
  else if (host) out.push(t(' to '), tok('target', host.value));
  else if (path) out.push(t(' to '), tok('target', path.value));
  for (const r of rest) out.push(t(' '), ...r);
  return out;
}

export function describeMatcher(m: Matcher): string {
  return tokensToText(matcherTokens(m));
}

// =====================================================================================
// actions
// =====================================================================================

function bodySource(b: BodySource): string {
  switch (b.type) {
    case 'empty':
      return 'an empty body';
    case 'text':
      return `text ${debugString(truncate(b.text, 40))}`;
    case 'base64':
      return 'binary data';
    case 'json':
      return `JSON ${truncate(jsonCompact(b.value), 40)}`;
    case 'file':
      return `file ${b.path}`;
    default:
      return 'a body';
  }
}

function bodyTransform(b: BodyTransform): string {
  switch (b.type) {
    case 'replace':
      return `replace body with ${bodySource(b.body)}`;
    case 'match-replace':
      return `find/replace in body (${b.replacements.length} rule${b.replacements.length === 1 ? '' : 's'})`;
    case 'json-merge':
      return 'merge into JSON body';
    case 'json-patch':
      return `patch JSON body (${b.operations.length} op${b.operations.length === 1 ? '' : 's'})`;
    default:
      return 'transform body';
  }
}

function headerNames(h: Headers): string {
  const names: string[] = [];
  for (const [k] of h) if (!names.some((n) => n.toLowerCase() === k.toLowerCase())) names.push(k);
  return joinText(names, 'and');
}

/** e.g. `set header X-Debug, replace body with text "x"`. */
export function describeRequestTransform(tr: RequestTransform): string {
  const parts: string[] = [];
  if (tr.method != null) parts.push(`method → ${tr.method}`);
  if (tr.urlReplace?.length) parts.push('rewrite URL');
  if (tr.setQuery?.length) parts.push(`set query ${joinText(tr.setQuery.map(([k]) => k), 'and')}`);
  if (tr.removeHeaders?.length) parts.push(`remove header ${joinText(tr.removeHeaders, 'and')}`);
  if (tr.setHeaders?.length) parts.push(`set header ${headerNames(tr.setHeaders)}`);
  if (tr.addHeaders?.length) parts.push(`add header ${headerNames(tr.addHeaders)}`);
  if (tr.body) parts.push(bodyTransform(tr.body));
  return parts.join(', ');
}

/** e.g. `status → 404, remove header ETag`. */
export function describeResponseTransform(tr: ResponseTransform): string {
  const parts: string[] = [];
  if (tr.status != null) parts.push(`status → ${tr.status}`);
  if (tr.statusMessage != null) parts.push(`status message → ${debugString(tr.statusMessage)}`);
  if (tr.removeHeaders?.length) parts.push(`remove header ${joinText(tr.removeHeaders, 'and')}`);
  if (tr.setHeaders?.length) parts.push(`set header ${headerNames(tr.setHeaders)}`);
  if (tr.addHeaders?.length) parts.push(`add header ${headerNames(tr.addHeaders)}`);
  if (tr.body) parts.push(bodyTransform(tr.body));
  return parts.join(', ');
}

/** Mirrors `RequestTransform::is_empty` (all fields default). */
export function isEmptyRequestTransform(tr: RequestTransform | undefined): boolean {
  if (!tr) return true;
  return tr.method == null && !tr.urlReplace?.length && !tr.setQuery?.length && !tr.setHeaders?.length && !tr.addHeaders?.length && !tr.removeHeaders?.length && tr.body == null;
}

export function isEmptyResponseTransform(tr: ResponseTransform | undefined): boolean {
  if (!tr) return true;
  return tr.status == null && tr.statusMessage == null && !tr.setHeaders?.length && !tr.addHeaders?.length && !tr.removeHeaders?.length && tr.body == null;
}

function transforms(req: RequestTransform | undefined, res: ResponseTransform | undefined): string | null {
  const parts: string[] = [];
  if (req && !isEmptyRequestTransform(req)) parts.push(`request: ${describeRequestTransform(req)}`);
  if (res && !isEmptyResponseTransform(res)) parts.push(`response: ${describeResponseTransform(res)}`);
  return parts.length ? parts.join('; ') : null;
}

/** Rust `rate()`: bytes/s → `1.0 Mbit/s`, `256 kbit/s`, `800 bit/s`. */
export function formatRate(bps: number): string {
  const bits = bps * 8;
  if (bits >= 1_000_000) return `${(bits / 1_000_000).toFixed(1)} Mbit/s`;
  if (bits >= 1_000) return `${(bits / 1_000).toFixed(0)} kbit/s`;
  return `${bits.toFixed(0)} bit/s`;
}

/** A status code; `…` while the editor holds a half-typed (non-numeric) value. */
function statusStr(status: number): string {
  return Number.isFinite(status) ? String(status) : '…';
}

function headerValue(h: Headers, name: string): string | undefined {
  const lower = name.toLowerCase();
  return h.find(([k]) => k.toLowerCase() === lower)?.[1];
}

/** Describe an action, e.g. `mock 200`, `forward to http://localhost:3000`. */
export function actionTokens(a: Action): T {
  switch (a.type) {
    case 'passthrough': {
      const tr = transforms(a.request, a.response);
      return tr ? [tok('action', 'pass through'), tok('muted', ` (${tr})`)] : [tok('action', 'pass through')];
    }
    case 'forward': {
      const out: T = [tok('action', 'forward to '), tok('target', a.target)];
      if (a.stripPrefix != null) out.push(tok('muted', ` (stripping ${a.stripPrefix})`));
      if (a.preserveHost) out.push(tok('muted', ' keeping the Host header'));
      const tr = transforms(a.request, a.response);
      if (tr) out.push(tok('muted', ` (${tr})`));
      return out;
    }
    case 'mock': {
      const out: T = [tok('action', 'mock '), tok('status', statusStr(a.status))];
      const ct = headerValue(a.headers ?? [], 'content-type')?.split(';')[0]?.trim();
      if (ct) out.push(tok('muted', ` ${ct}`, true));
      else if (a.body?.type === 'json') out.push(tok('muted', ' application/json', true));
      else if (a.body?.type === 'file') out.push(tok('muted', ` ${a.body.path}`, true));
      return out;
    }
    case 'map-local':
      return [tok('action', 'serve local files from '), tok('target', a.path)];
    case 'close':
      return [tok('action', 'close the connection')];
    case 'reset':
      return [tok('action', 'reset the connection')];
    case 'timeout':
      return [tok('action', 'time out'), tok('muted', ' (never respond)')];
    case 'ws-echo':
      return [tok('action', 'echo WebSocket messages')];
    case 'ws-reject':
      return [tok('action', 'reject the WebSocket '), t('('), tok('status', statusStr(a.status)), t(')')];
    case 'transform': {
      const tr = transforms(a.request, a.response);
      return [tok('action', 'transform'), tok('muted', tr ? ` (${tr})` : ' (nothing)')];
    }
    case 'breakpoint':
      if (a.request && a.response) return [tok('action', 'pause request and response')];
      if (a.request) return [tok('action', 'pause request')];
      if (a.response) return [tok('action', 'pause response')];
      return [tok('action', 'breakpoint'), tok('muted', ' (disabled)')];
    case 'delay':
      return a.jitterMs > 0 ? [tok('action', `delay ${a.ms} ms`), tok('muted', ` (+0–${a.jitterMs} ms)`)] : [tok('action', `delay ${a.ms} ms`)];
    case 'throttle': {
      const parts: string[] = [];
      if (a.downloadBps > 0) parts.push(`${formatRate(a.downloadBps)} down`);
      if (a.uploadBps > 0) parts.push(`${formatRate(a.uploadBps)} up`);
      if (a.latencyMs > 0) parts.push(`+${a.latencyMs} ms latency`);
      return parts.length ? [tok('action', 'throttle to '), tok('value', parts.join(', '))] : [tok('action', 'throttle'), tok('muted', ' (unlimited)')];
    }
    case 'highlight':
      return a.note != null
        ? [tok('action', 'highlight '), tok('color', a.color), tok('muted', ` (${truncate(a.note, 40)})`)]
        : [tok('action', 'highlight '), tok('color', a.color)];
    case 'script':
      return [tok('action', 'run script')];
    case 'tls-profile':
      if (a.profile === 'native') return [tok('action', "use Akev's native TLS fingerprint upstream")];
      if (a.profile === 'mirror-client') return [tok('action', "mirror the client's browser TLS fingerprint upstream")];
      return [tok('action', 'impersonate '), tok('value', tlsProfileLabel(a.profile)), tok('action', ' upstream'), tok('muted', ' (TLS + HTTP/2)')];
    case 'ws-breakpoint': {
      const what = a.direction === 'sent' ? 'sent WebSocket messages' : a.direction === 'received' ? 'received WebSocket messages' : 'WebSocket messages';
      return a.pattern ? [tok('action', `pause ${what}`), t(' '), ...stringMatch(a.pattern)] : [tok('action', `pause ${what}`)];
    }
    default:
      return [tok('action', 'unknown action')];
  }
}

export function describeAction(a: Action): string {
  return tokensToText(actionTokens(a));
}

function limitTokens(maxMatches: number | undefined): T {
  if (maxMatches == null) return [];
  return [tok('muted', maxMatches === 1 ? ' (once)' : ` (${maxMatches} times)`)];
}

/** Full summary tokens: `<what matches> → <what happens>[ (once|N times)]`. */
export function summarizeTokens(rule: Pick<Rule, 'matchers' | 'action' | 'maxMatches'>): T {
  return [...matchersTokens(rule.matchers), tok('arrow', ' → '), ...actionTokens(rule.action), ...limitTokens(rule.maxMatches)];
}

/** One-line summary, identical to the daemon's `describe::summarize`. */
export function summarize(rule: Pick<Rule, 'matchers' | 'action' | 'maxMatches'>): string {
  return tokensToText(summarizeTokens(rule));
}

/** Display title: the user's title, or the summary. */
export function ruleLabel(rule: Pick<Rule, 'title' | 'matchers' | 'action' | 'maxMatches'>): string {
  const title = rule.title?.trim();
  return title ? title : summarize(rule);
}
