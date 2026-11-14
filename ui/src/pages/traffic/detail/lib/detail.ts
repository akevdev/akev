/**
 * Pure helpers for the single-exchange detail view: tab sets & remembered-tab resolution,
 * cookie counts, size descriptions, protocol labels, URL splitting and paused timers.
 */

import type { BodyMeta, ExchangeDetail, ExchangeKind, ExchangeSummary, Headers, HttpVersion, RequestHead, ResponseHead } from '../../../../api/types';
import { formatBytes } from '../../../../lib/format';
import { formatAuthority, parseCookieHeader } from '../../../../lib/http';
import { appendClause, clause, quoteFilterValue as quoteValue } from '../../filter/filterEdit';

// =====================================================================================
// Tabs
// =====================================================================================

export type RequestTab = 'headers' | 'query' | 'body' | 'cookies' | 'auth' | 'raw' | 'code';
export type ResponseTab = 'llm' | 'messages' | 'headers' | 'body' | 'cookies' | 'raw' | 'timing' | 'connection' | 'cache';

export function isStreamKind(kind: ExchangeKind): boolean {
  return kind === 'web-socket' || kind === 'event-stream';
}

/** Request tabs in display order; Auth only when credentials are present. */
export function requestTabs(opts: { hasAuth: boolean }): RequestTab[] {
  const tabs: RequestTab[] = ['headers', 'query', 'body', 'cookies'];
  if (opts.hasAuth) tabs.push('auth');
  tabs.push('raw', 'code');
  return tabs;
}

/**
 * Response tabs in display order; WebSocket/SSE exchanges get Messages first, recognised LLM
 * API calls get the LLM conversation view before everything.
 */
export function responseTabs(opts: { stream: boolean; llm?: boolean }): ResponseTab[] {
  const base: ResponseTab[] = ['headers', 'body', 'cookies', 'raw', 'timing', 'connection', 'cache'];
  const tabs: ResponseTab[] = opts.stream ? ['messages', ...base] : base;
  return opts.llm ? ['llm', ...tabs] : tabs;
}

/** The remembered tab when it exists for this exchange, else the first available tab. */
export function resolveTab<T extends string>(available: readonly T[], preferred: string | undefined): T {
  if (preferred && (available as readonly string[]).includes(preferred)) return preferred as T;
  return available[0] as T;
}

/** UI-pref group under which the last response tab is remembered (streams and LLM calls separately). */
export function responseTabGroup(kind: ExchangeKind, llm = false): string {
  if (llm) return 'traffic.response.llm';
  return isStreamKind(kind) ? 'traffic.response.stream' : 'traffic.response';
}

// =====================================================================================
// Counts
// =====================================================================================

/** Number of cookies sent in `Cookie` headers. */
export function countRequestCookies(headers: Headers): number {
  let n = 0;
  for (const [k, v] of headers) if (k.toLowerCase() === 'cookie') n += parseCookieHeader(v).filter((c) => c.name || c.value).length;
  return n;
}

/** Number of `Set-Cookie` headers. */
export function countSetCookies(headers: Headers): number {
  let n = 0;
  for (const [k] of headers) if (k.toLowerCase() === 'set-cookie') n++;
  return n;
}

/** Header count excluding HTTP/2 pseudo-headers. */
export function countHeaders(headers: Headers): number {
  let n = 0;
  for (const [k] of headers) if (!k.startsWith(':')) n++;
  return n;
}

// =====================================================================================
// Sizes & protocol
// =====================================================================================

export interface SizeInfo {
  /** Bytes on the wire. */
  wire: number;
  /** Decoded size when it differs from the wire size (compressed bodies). */
  decoded: number | null;
  encodings: string[];
}

/** Wire size plus decoded size when compressed. */
export function sizeInfo(meta: BodyMeta | undefined, fallback = 0): SizeInfo {
  if (!meta) return { wire: fallback, decoded: null, encodings: [] };
  const decoded = meta.decodedSize != null && meta.decodedSize !== meta.wireSize ? meta.decodedSize : null;
  return { wire: meta.wireSize || fallback, decoded, encodings: meta.encodings };
}

/** `"3.1 KB"` or `"3.1 KB → 12.8 KB"` (with the encoding when known). */
export function describeSize(info: SizeInfo): string {
  if (info.decoded == null) return formatBytes(info.wire);
  return `${formatBytes(info.wire)} → ${formatBytes(info.decoded)}`;
}

/** Size used for tab badges: decoded when known, else captured/wire. */
export function bodyDisplaySize(meta: BodyMeta | undefined): number {
  if (!meta) return 0;
  return meta.decodedSize ?? (meta.capturedSize || meta.wireSize);
}

/** Protocol label for the meta line (`HTTP/2`, `WebSocket · HTTP/1.1`, `SSE · HTTP/2`). */
export function protocolLabel(kind: ExchangeKind, httpVersion: HttpVersion): string {
  if (kind === 'web-socket') return `WebSocket · ${httpVersion}`;
  if (kind === 'event-stream') return `SSE · ${httpVersion}`;
  return httpVersion;
}

/** `TLSv1.3` → `TLS 1.3`. */
export function tlsLabel(version: string | undefined): string | null {
  if (!version) return null;
  const m = /^TLSv?(\d(?:\.\d)?)$/i.exec(version.trim());
  return m ? `TLS ${m[1]}` : version;
}

// =====================================================================================
// URL
// =====================================================================================

export interface UrlParts {
  /** `https://` */
  scheme: string;
  /** `api.example.com:8443` */
  authority: string;
  /** `/v1/users` */
  path: string;
  /** `?page=2` (with `?`) or `''`. */
  query: string;
}

/** Split a URL for emphasis: scheme + authority, path, query. */
export function splitUrl(s: Pick<ExchangeSummary, 'scheme' | 'host' | 'port' | 'path'>): UrlParts {
  const q = s.path.indexOf('?');
  return {
    scheme: `${s.scheme}://`,
    authority: formatAuthority(s.scheme, s.host, s.port),
    path: q < 0 ? s.path : s.path.slice(0, q),
    query: q < 0 ? '' : s.path.slice(q),
  };
}

// =====================================================================================
// Views (original vs modified)
// =====================================================================================

export interface HeadsView {
  request: RequestHead;
  response: ResponseHead | undefined;
  /** Bodies should be fetched with `original: true`. */
  original: boolean;
}

/** Request/response heads to display for the Original/Modified toggle. */
export function headsFor(detail: ExchangeDetail, showOriginal: boolean): HeadsView {
  if (!showOriginal) return { request: detail.request, response: detail.response, original: false };
  return {
    request: detail.originalRequest ?? detail.request,
    response: detail.originalResponse ?? detail.response,
    original: true,
  };
}

/** Whether an exchange has anything to show in the Original view. */
export function hasOriginal(detail: ExchangeDetail): boolean {
  return detail.originalRequest != null || detail.originalResponse != null || detail.summary.modified === true;
}

// =====================================================================================
// Breakpoint timer
// =====================================================================================

/** When the exchange started waiting at its breakpoint (unix ms). */
export function pausedSince(detail: Pick<ExchangeDetail, 'timing'> | undefined, summary: Pick<ExchangeSummary, 'startedAt' | 'paused'>): number {
  const t = detail?.timing;
  if (summary.paused === 'response') {
    const at = t?.responseHeaders ?? t?.requestSent;
    return summary.startedAt + (at ?? 0);
  }
  return summary.startedAt + (t?.requestBody ?? t?.requestHeaders ?? 0);
}

/** Waiting long enough that clients typically give up (warn in the banner). */
export const PAUSE_WARN_MS = 30_000;

// =====================================================================================
// Filter clauses ("filter by this header")
// =====================================================================================

/** Quote a filter value when needed (same rules as the filter bar's context menus). */
export const quoteFilterValue = quoteValue;

/** Bytes a `header.<name>` subkey may contain unbracketed (akev-filter `is_subkey_byte`). */
const SUBKEY = /^[A-Za-z0-9\-_.+#%&'`]+$/;

/**
 * "Filter by this header": `header.<name>=<value>` — an exact (case-insensitive) match, so a
 * `*` in the value (curl's `Accept` header) is literal rather than a wildcard. An empty value filters on the
 * header being present (`header:<name>`). Names with characters a dotted subkey can't hold use
 * the bracket form (`header[x!y]=…`).
 */
export function headerFilterClause(name: string, value: string): string {
  const n = name.toLowerCase();
  if (value === '') return `header:${quoteValue(n)}`;
  const field = SUBKEY.test(n) ? `header.${n}` : `header[${n}]`;
  return clause(field, '=', value);
}

/**
 * Append a clause to an existing filter query (AND), skipping duplicates. A query with a
 * top-level OR is parenthesised first, so the clause applies to all of it.
 */
export function appendFilterClause(existing: string, newClause: string): string {
  return appendClause(existing, newClause);
}
