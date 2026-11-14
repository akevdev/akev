/**
 * Pure builders for "Create rule ▸" (the traffic list context menu and the detail header,
 * DESIGN.md §6.3–6.4): mock this response, breakpoint on request or response, block this host,
 * Map Local, rewrite. Matchers are prefilled from the captured method + host (+ port) + path.
 *
 * The caller fetches what it needs (the editable response for a mock), builds the rule here,
 * prepends it with {@link prependRule} and saves via `useAppStore.saveRules`.
 */

import type { BodySource, ContentKind, ExchangeSummary, Headers, Matcher, Rule, RuleId, RuleItem, RuleSet } from '../api/types';
import { base64ToText } from './encoding';
import { actionInfo } from './rules/actions';
import { emptyRuleSet, insertAt, newId } from './rules/tree';

export type RuleTemplate = 'mock' | 'breakpoint-request' | 'breakpoint-response' | 'block-host' | 'map-local' | 'rewrite';

/** The parts of an exchange the builders look at. */
export type RuleSource = Pick<ExchangeSummary, 'method' | 'scheme' | 'host' | 'port' | 'path'> &
  Partial<Pick<ExchangeSummary, 'id' | 'contentKind' | 'contentType'>>;

/** Captured response used by "Mock this response". */
export interface CapturedResponse {
  status: number;
  statusMessage?: string;
  headers: Headers;
}

/**
 * Headers a mock must not copy: hop-by-hop (RFC 9110 §7.6.1), the original framing and
 * encoding (wrong for a re-encoded body), and capture-time headers (a frozen `Date`/`Age`
 * would be stale on every replay).
 */
const STRIPPED_HEADERS: ReadonlySet<string> = new Set([
  'connection',
  'keep-alive',
  'proxy-connection',
  'proxy-authenticate',
  'proxy-authorization',
  'te',
  'trailer',
  'transfer-encoding',
  'upgrade',
  'content-length',
  'content-encoding',
  'date',
  'age',
]);

/** `/a/b?x=1#f` → `/a/b` (`/` when empty). */
export function pathWithoutQuery(path: string): string {
  const cut = path.search(/[?#]/);
  const p = cut < 0 ? path : path.slice(0, cut);
  return p || '/';
}

function isDefaultPort(scheme: string, port: number): boolean {
  if (!port) return true;
  const s = scheme.toLowerCase();
  return ((s === 'http' || s === 'ws') && port === 80) || ((s === 'https' || s === 'wss') && port === 443);
}

/** Headers a mock response can safely replay: no hop-by-hop, framing, capture-time or pseudo headers. */
export function mockableHeaders(headers: Headers): Headers {
  return headers.filter(([name]) => !name.startsWith(':') && !STRIPPED_HEADERS.has(name.toLowerCase()));
}

/** Host matcher (exact, case-insensitive) plus a port matcher for non-default ports. */
export function hostMatchers(src: Pick<RuleSource, 'scheme' | 'host' | 'port'>): Matcher[] {
  const out: Matcher[] = [{ type: 'host', pattern: { op: 'equals', value: src.host, caseSensitive: false } }];
  if (!isDefaultPort(src.scheme, src.port)) out.push({ type: 'port', ports: [src.port] });
  return out;
}

/** Matchers for "requests like this one": method + host (+ port) + exact path (no query). */
export function requestMatchers(src: RuleSource, options: { method?: boolean } = {}): Matcher[] {
  const out: Matcher[] = [];
  if (options.method !== false && src.method) out.push({ type: 'method', methods: [src.method.toUpperCase()] });
  out.push(...hostMatchers(src));
  out.push({ type: 'path', pattern: { op: 'equals', value: pathWithoutQuery(src.path), caseSensitive: true } });
  return out;
}

/** A decoded base64 body as the most readable {@link BodySource}: UTF-8 text, else bytes. */
export function bodySourceFromBase64(b64: string): BodySource {
  if (!b64) return { type: 'empty' };
  let text: string | null = null;
  try {
    text = base64ToText(b64);
  } catch {
    text = null;
  }
  return text != null ? { type: 'text', text } : { type: 'base64', data: b64 };
}

function baseRule(src: RuleSource, id: RuleId, matchers: Matcher[]): Omit<Rule, 'action'> {
  return { id, enabled: true, matchers, ...(src.id != null ? { note: `Created from exchange #${src.id}` } : {}) };
}

/** "Mock this response": captured status + headers (cleaned) + decoded body. */
export function mockRule(src: RuleSource, response: CapturedResponse, body: BodySource, id: RuleId = newId()): Rule {
  return {
    ...baseRule(src, id, requestMatchers(src)),
    action: {
      type: 'mock',
      status: response.status,
      ...(response.statusMessage ? { statusMessage: response.statusMessage } : {}),
      headers: mockableHeaders(response.headers),
      body,
    },
  };
}

/** "Breakpoint on request" / "on response" for matching requests. */
export function breakpointRule(src: RuleSource, stages: { request: boolean; response: boolean }, id: RuleId = newId()): Rule {
  return { ...baseRule(src, id, requestMatchers(src)), action: { type: 'breakpoint', request: stages.request, response: stages.response } };
}

/** "Block this host": close every connection to the host. */
export function blockHostRule(src: RuleSource, id: RuleId = newId()): Rule {
  return { ...baseRule(src, id, hostMatchers(src)), action: { type: 'close' } };
}

const KIND_EXTENSIONS: Partial<Record<ContentKind, string>> = {
  json: 'json',
  'json-lines': 'jsonl',
  xml: 'xml',
  html: 'html',
  javascript: 'js',
  css: 'css',
  markdown: 'md',
  yaml: 'yaml',
  csv: 'csv',
  text: 'txt',
  'graph-ql': 'json',
  'event-stream': 'txt',
  pdf: 'pdf',
  wasm: 'wasm',
};

const MIME_EXTENSIONS: Readonly<Record<string, string>> = {
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/avif': 'avif',
  'image/svg+xml': 'svg',
  'image/x-icon': 'ico',
  'image/vnd.microsoft.icon': 'ico',
  'font/woff2': 'woff2',
  'font/woff': 'woff',
  'font/ttf': 'ttf',
  'font/otf': 'otf',
  'audio/mpeg': 'mp3',
  'video/mp4': 'mp4',
};

/** File extension for a response (`json`, `png`…), or `''` when unknown. */
export function extensionFor(src: Pick<RuleSource, 'contentKind' | 'contentType'>): string {
  const mime = src.contentType?.split(';')[0]?.trim().toLowerCase();
  if (mime && MIME_EXTENSIONS[mime]) return MIME_EXTENSIONS[mime] as string;
  return (src.contentKind && KIND_EXTENSIONS[src.contentKind]) || '';
}

/**
 * Suggested local file for Map Local: `<baseDir>/mocks/<host><path>` with an extension
 * derived from the response type when the path has none (`/` → `index.<ext>`).
 */
export function suggestLocalPath(src: RuleSource, baseDir?: string | null): string {
  const sep = baseDir && baseDir.includes('\\') && !baseDir.includes('/') ? '\\' : '/';
  const ext = extensionFor(src);
  let path = pathWithoutQuery(src.path);
  try {
    path = decodeURIComponent(path);
  } catch {
    // keep the encoded form
  }
  // Keep the path inside the mocks folder: no traversal, no empty segments.
  const segments = path.split('/').filter((s) => s && s !== '.' && s !== '..');
  if (path.endsWith('/') || segments.length === 0) segments.push(ext ? `index.${ext}` : 'index');
  else {
    const last = segments[segments.length - 1] as string;
    if (ext && !/\.[a-z0-9]{1,8}$/i.test(last)) segments[segments.length - 1] = `${last}.${ext}`;
  }
  const host = src.host.replace(/[:\\/]/g, '_');
  const rel = ['mocks', host, ...segments].join(sep);
  const base = baseDir?.replace(/[\\/]+$/, '');
  return base ? `${base}${sep}${rel}` : rel;
}

/**
 * Map Local: serve a local file for this path. Falls back to upstream while the file doesn't
 * exist yet, so the rule is safe to enable before the file is created.
 */
export function mapLocalRule(src: RuleSource, options: { baseDir?: string | null } = {}, id: RuleId = newId()): Rule {
  return {
    ...baseRule(src, id, requestMatchers(src)),
    action: { type: 'map-local', path: suggestLocalPath(src, options.baseDir), fallbackToUpstream: true, headers: [] },
  };
}

/** "Rewrite…": a transform with empty request/response transforms, ready to edit. */
export function rewriteRule(src: RuleSource, id: RuleId = newId()): Rule {
  return { ...baseRule(src, id, requestMatchers(src)), action: { type: 'transform', request: {}, response: {} } };
}

/** A new rule set with `rule` first (top = highest priority; a missing set starts empty). */
export function prependRule(set: RuleSet | null | undefined, rule: Rule): RuleSet {
  return insertAt(set ?? emptyRuleSet(), { type: 'rule', ...rule }, null, 0);
}

/** Short label for a rule chip: its title, else "<Action> rule"; unknown ids → "Rule". */
export function ruleChipLabel(item: RuleItem | undefined): string {
  if (!item) return 'Rule';
  if (item.title) return item.title;
  if (item.type === 'group') return 'Rule group';
  return `${actionInfo(item.action.type).label} rule`;
}
