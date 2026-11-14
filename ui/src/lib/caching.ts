/**
 * HTTP caching analysis for the Cache tab (DESIGN.md §6.4) — a port of HTTP Toolkit's
 * `src/model/http/caching.ts` (https://github.com/httptoolkit/httptoolkit-ui, by Tim Perry,
 * AGPL-3.0-or-later like Akev) following RFC 9111, with its bugs fixed:
 *
 * - Cache-Control parsing lowercases directive names, honours quoted strings (commas inside
 *   `no-cache="a, b"` don't split), strips quotes from values (`max-age="60"`), and flags
 *   duplicate/conflicting directives (HTK: `parseInt` on raw parts, case-sensitive names).
 * - `Access-Control-Allow-Credentials` is read case-insensitively (HTK always read `false`).
 * - `Pragma: no-store` (not a real directive) is no longer treated as a cacheability signal;
 *   `Pragma: no-cache` on responses is reported as the deprecated no-op it is.
 * - 308 is heuristically cacheable (RFC 9111 §4.2.2), not only "permanent".
 * - Heuristic freshness needs `Last-Modified` (ETag alone gives no heuristic lifetime) and is
 *   estimated with the common 10% rule; invalid `Expires` values mean "already expired".
 * - The `Age` header is taken into account for the remaining freshness.
 *
 * Additions: 304 revalidation, ignored validators, conditional-request hints, request-side
 * directives, cookies on shared-cacheable responses, missing `Vary: Accept-Encoding` / `Origin`.
 *
 * Output is a list of findings with plain-text explanations using a tiny markdown subset
 * (`code`, **bold**, `- ` bullets, blank-line paragraphs). Pure: no React, no I/O.
 */

import type { ExchangeDetail, Headers } from '../api/types';

export type CacheFindingStatus = 'good' | 'warn' | 'info';

export type CacheCategory = 'cacheability' | 'storage' | 'matching' | 'lifetime' | 'validation' | 'request' | 'privacy' | 'headers';

export interface CacheFinding {
  /** Stable id (for React keys and tests), e.g. `cacheability.max-age`. */
  id: string;
  category: CacheCategory;
  status: CacheFindingStatus;
  summary: string;
  /** Markdown-ish plain text. */
  explanation: string;
}

export const CACHE_CATEGORY_LABELS: Readonly<Record<CacheCategory, string>> = {
  cacheability: 'Cacheability',
  storage: 'Who can store it',
  matching: 'Reuse',
  lifetime: 'Freshness',
  validation: 'Revalidation',
  request: 'Request',
  privacy: 'Privacy',
  headers: 'Headers',
};

// =====================================================================================
// Constants (RFC 9111 / Fetch)
// =====================================================================================

/** Methods whose responses may be stored (QUERY is the draft safe method with a body). */
const CACHEABLE_METHODS = new Set(['GET', 'HEAD', 'POST', 'QUERY']);
/** Heuristically cacheable status codes (RFC 9110 §15.1). */
export const HEURISTIC_STATUSES = new Set([200, 203, 204, 206, 300, 301, 308, 404, 405, 410, 414, 501]);
/** Statuses that clients typically keep indefinitely without explicit freshness. */
const PERMANENT_STATUSES = new Set([301, 308, 410]);
const CORS_SIMPLE_METHODS = ['GET', 'HEAD', 'POST'];
/** Browser caps on Access-Control-Max-Age (seconds). */
const PREFLIGHT_CAP_CHROMIUM = 7200;
const PREFLIGHT_CAP_FIREFOX = 86400;
const ONE_YEAR = 365 * 86400;

// =====================================================================================
// Cache-Control parsing
// =====================================================================================

export interface CacheDirectives {
  /** Directive name (lowercase) → value (`true` when valueless), first occurrence wins. */
  values: Map<string, string | true>;
  /** Directives that appeared more than once with different values. */
  conflicts: string[];
}

/** Split a header list on commas that aren't inside quoted strings. */
export function splitHeaderList(value: string): string[] {
  const out: string[] = [];
  let cur = '';
  let quoted = false;
  for (let i = 0; i < value.length; i++) {
    const ch = value[i] as string;
    if (quoted) {
      if (ch === '\\' && i + 1 < value.length) {
        cur += ch + (value[++i] as string);
        continue;
      }
      if (ch === '"') quoted = false;
      cur += ch;
    } else if (ch === '"') {
      quoted = true;
      cur += ch;
    } else if (ch === ',') {
      if (cur.trim()) out.push(cur.trim());
      cur = '';
    } else cur += ch;
  }
  if (cur.trim()) out.push(cur.trim());
  return out;
}

function unquote(v: string): string {
  const t = v.trim();
  if (t.length >= 2 && t.startsWith('"') && t.endsWith('"')) return t.slice(1, -1).replace(/\\(.)/g, '$1');
  return t;
}

/** Parse one or more `Cache-Control` header values. */
export function parseCacheControl(values: readonly string[]): CacheDirectives {
  const map = new Map<string, string | true>();
  const conflicts = new Set<string>();
  for (const value of values) {
    for (const part of splitHeaderList(value)) {
      const eq = part.indexOf('=');
      const name = (eq < 0 ? part : part.slice(0, eq)).trim().toLowerCase();
      if (!name) continue;
      const v: string | true = eq < 0 ? true : unquote(part.slice(eq + 1));
      if (map.has(name)) {
        if (map.get(name) !== v) conflicts.add(name);
        continue;
      }
      map.set(name, v);
    }
  }
  return { values: map, conflicts: [...conflicts] };
}

/** Parse a delta-seconds value. Returns `null` when absent, `NaN` when malformed. */
export function deltaSeconds(v: string | true | undefined): number | null {
  if (v === undefined) return null;
  if (v === true) return Number.NaN;
  if (!/^\d+$/.test(v.trim())) return Number.NaN;
  // RFC 9111 §1.2.2: values beyond 2^31 are clamped.
  return Math.min(Number(v.trim()), 2147483648);
}

/** Parse an HTTP-date; `NaN` when invalid. */
export function parseHttpDate(v: string | undefined): number {
  if (!v) return Number.NaN;
  const t = v.trim();
  // Bare numbers ("0", "-1") are invalid HTTP-dates (meaning "already expired").
  if (/^-?\d+$/.test(t)) return Number.NaN;
  return Date.parse(t);
}

// =====================================================================================
// Durations
// =====================================================================================

const UNITS: [number, string][] = [
  [ONE_YEAR, 'year'],
  [30 * 86400, 'month'],
  [86400, 'day'],
  [3600, 'hour'],
  [60, 'minute'],
  [1, 'second'],
];

function plural(n: number, unit: string): string {
  return `${n} ${unit}${n === 1 ? '' : 's'}`;
}

/** Human duration from seconds: `45 seconds`, `5 minutes`, `2 hours 30 minutes`, `1 year`. */
export function formatSeconds(totalSeconds: number): string {
  const s = Math.max(0, Math.round(totalSeconds));
  if (s === 0) return '0 seconds';
  for (let i = 0; i < UNITS.length; i++) {
    const [size, unit] = UNITS[i] as [number, string];
    if (s >= size) {
      const major = Math.floor(s / size);
      const rest = s - major * size;
      const next = UNITS[i + 1];
      // Secondary unit only for small majors (2 hours 30 minutes, not 14 days 3 hours).
      if (next && major < 3 && rest >= next[0] && size > 1) {
        const minor = Math.floor(rest / next[0]);
        if (minor > 0) return `${plural(major, unit)} ${plural(minor, next[1])}`;
      }
      return plural(major, unit);
    }
  }
  return plural(s, 'second');
}

// =====================================================================================
// Context
// =====================================================================================

interface Ctx {
  method: string;
  url: string;
  status: number;
  req: Headers;
  res: Headers;
  cc: CacheDirectives;
  reqCc: CacheDirectives;
  has: (d: string) => boolean;
  maxAge: number | null;
  sMaxAge: number | null;
  expires: number; // ms or NaN
  expiresHeader: string | undefined;
  date: number; // ms or NaN
  responseTime: number; // ms: Date header or capture time
  lastModified: number;
  etag: string | undefined;
  age: number | null;
  bodySize: number;
  isPreflight: boolean;
  now: number;
}

function header(headers: Headers, name: string): string | undefined {
  const lower = name.toLowerCase();
  for (const [k, v] of headers) if (k.toLowerCase() === lower) return v;
  return undefined;
}

function headerAll(headers: Headers, name: string): string[] {
  const lower = name.toLowerCase();
  const out: string[] = [];
  for (const [k, v] of headers) if (k.toLowerCase() === lower) out.push(v);
  return out;
}

function listValues(headers: Headers, name: string): string[] {
  return headerAll(headers, name).flatMap(splitHeaderList).map((s) => s.trim()).filter(Boolean);
}

function code(s: string): string {
  // Backticks inside values would end the code span early; fall back to quotes.
  return s.includes('`') ? `"${s}"` : `\`${s}\``;
}

function truncate(s: string, n = 80): string {
  return s.length > n ? `${s.slice(0, n - 1)}…` : s;
}

function stripFragment(url: string): string {
  const i = url.indexOf('#');
  return i < 0 ? url : url.slice(0, i);
}

function resolveUrl(value: string, base: string): string | null {
  try {
    return stripFragment(new URL(value, base).href);
  } catch {
    return null;
  }
}

// =====================================================================================
// Analysis
// =====================================================================================

interface Cacheability {
  finding: CacheFinding;
  cacheable: boolean;
  /** Only shared caches may store it (s-maxage without other freshness). */
  sharedOnly?: boolean;
  /** Lifetime is heuristic (no explicit freshness). */
  heuristic?: boolean;
  /** Kept indefinitely (permanent redirect/gone without freshness). */
  permanent?: boolean;
}

function f(id: string, category: CacheCategory, status: CacheFindingStatus, summary: string, explanation: string): CacheFinding {
  return { id, category, status, summary, explanation };
}

function explainCacheability(c: Ctx): Cacheability {
  const { method, status } = c;

  if (c.isPreflight) {
    const maxAge = header(c.res, 'access-control-max-age');
    const n = maxAge == null ? null : Number(maxAge.trim());
    if (n == null || Number.isNaN(n)) {
      return {
        cacheable: true,
        finding: f(
          'cacheability.preflight-default',
          'cacheability',
          'info',
          'Preflight cached very briefly',
          'This is a CORS preflight (`OPTIONS` with an `Origin` header). Without `Access-Control-Max-Age`, browsers cache the preflight result for **5 seconds**, so most cross-origin requests will send a new preflight.\n\nAdd `Access-Control-Max-Age` (e.g. `600`) to avoid a preflight round trip before each request.',
        ),
      };
    }
    if (n <= 0) {
      return {
        cacheable: false,
        finding: f(
          'cacheability.preflight-disabled',
          'cacheability',
          'info',
          'Preflight not cached',
          `\`Access-Control-Max-Age: ${maxAge?.trim()}\` disables the preflight cache, so every matching cross-origin request is preceded by a new preflight.`,
        ),
      };
    }
    return {
      cacheable: true,
      finding: f('cacheability.preflight', 'cacheability', 'good', 'Preflight cacheable', `Browsers may reuse this CORS preflight result for up to ${formatSeconds(n)} (\`Access-Control-Max-Age: ${n}\`).`),
    };
  }

  if (!CACHEABLE_METHODS.has(method)) {
    const unsafe = !['OPTIONS', 'TRACE', 'CONNECT'].includes(method);
    return {
      cacheable: false,
      finding: f(
        'cacheability.method',
        'cacheability',
        'info',
        'Not cacheable',
        `Responses to ${code(method)} requests are never stored by HTTP caches.${unsafe && status >= 200 && status < 400 ? `\n\nA successful ${code(method)} also **invalidates** any cached copies of this URL (and of its \`Location\` / \`Content-Location\`).` : ''}`,
      ),
    };
  }

  if (c.reqCc.values.has('no-store')) {
    return {
      cacheable: false,
      finding: f('cacheability.request-no-store', 'cacheability', 'info', 'Not cacheable', 'The **request** carried `Cache-Control: no-store`, so caches must not store this response, whatever its own headers say.'),
    };
  }

  if (c.has('no-store')) {
    const overridden = ['max-age', 's-maxage', 'public', 'immutable'].filter((d) => c.has(d));
    return {
      cacheable: false,
      finding: f(
        'cacheability.no-store',
        'cacheability',
        'info',
        'Not cacheable',
        `\`Cache-Control: no-store\` tells every cache — browsers, proxies and CDNs — not to store any part of this response.${overridden.length ? `\n\n\`no-store\` overrides ${overridden.map(code).join(', ')}, which have no effect here.` : ''}`,
      ),
    };
  }

  if (listValues(c.res, 'vary').includes('*')) {
    return {
      cacheable: false,
      finding: f('cacheability.vary-star', 'cacheability', 'info', 'Not reusable', '`Vary: *` means no future request can ever match this response, so caches never reuse it (some don\'t store it at all).'),
    };
  }

  const explicit = c.maxAge != null || c.sMaxAge != null || !Number.isNaN(c.expires) || c.expiresHeader != null;

  if (method === 'POST' || method === 'QUERY') {
    const loc = header(c.res, 'content-location');
    const sameUri = loc != null && resolveUrl(loc, c.url) === stripFragment(c.url);
    if (explicit && sameUri) {
      return {
        cacheable: true,
        finding: f(
          'cacheability.post',
          'cacheability',
          'good',
          'Cacheable for future GET/HEAD requests',
          `${code(method)} responses can be stored when they carry explicit freshness **and** a \`Content-Location\` equal to the request URL — both are present. A later \`GET\` to this URL may be answered from cache.\n\nIn practice browsers rarely do this; don't rely on it.`,
        ),
      };
    }
    return {
      cacheable: false,
      finding: f(
        'cacheability.post',
        'cacheability',
        'info',
        'Not cacheable',
        `${code(method)} responses are only cacheable with explicit freshness (\`max-age\` or \`Expires\`) **and** a \`Content-Location\` header matching the request URL${explicit ? ' — the `Content-Location` is missing or different' : ''}.`,
      ),
    };
  }

  if (status === 304) {
    return {
      cacheable: true,
      finding: f(
        'cacheability.not-modified',
        'cacheability',
        'good',
        'Cached copy revalidated',
        'The server answered `304 Not Modified` to a conditional request: the client reuses its cached copy instead of downloading the body again. Freshness headers in this response update the stored copy.',
      ),
    };
  }

  if (c.maxAge != null) {
    if (Number.isNaN(c.maxAge)) {
      return {
        cacheable: true,
        finding: f('cacheability.max-age-invalid', 'cacheability', 'warn', 'Cacheable, but max-age is malformed', `\`max-age=${String(c.cc.values.get('max-age'))}\` isn't a valid number of seconds, so caches treat the response as already stale.`),
      };
    }
    const notes: string[] = [];
    let status_: CacheFindingStatus = 'good';
    if (Number.isNaN(c.date)) {
      status_ = 'warn';
      notes.push('There is no valid `Date` header, so caches compute the response\'s age from when they received it, which can be inaccurate behind other caches.');
    } else if (!Number.isNaN(c.expires) && Math.abs(c.expires - (c.date + c.maxAge * 1000)) > 60_000) {
      notes.push(`\`Expires\` (${code(c.expiresHeader ?? '')}) disagrees with \`Date\` + \`max-age\`. Modern caches use \`max-age\` and ignore \`Expires\`; HTTP/1.0 caches use \`Expires\`.`);
    }
    return {
      cacheable: true,
      finding: f('cacheability.max-age', 'cacheability', status_, 'Cacheable', `\`max-age=${c.maxAge}\` gives this response an explicit lifetime.${notes.length ? `\n\n${notes.join('\n\n')}` : ''}`),
    };
  }

  if (c.expiresHeader != null) {
    return {
      cacheable: true,
      finding: f(
        'cacheability.expires',
        'cacheability',
        'good',
        'Cacheable',
        `The \`Expires\` header gives this response an explicit lifetime.${Number.isNaN(c.expires) ? ' Its value isn\'t a valid date, which means it is **already expired**.' : ''}`,
      ),
    };
  }

  if (c.sMaxAge != null) {
    return {
      cacheable: true,
      sharedOnly: !HEURISTIC_STATUSES.has(status) && !c.has('public'),
      finding: f(
        'cacheability.s-maxage',
        'cacheability',
        'info',
        'Cacheable by shared caches',
        `Only \`s-maxage=${c.sMaxAge}\` is set: CDNs and proxies get an explicit lifetime, while private caches (browsers) ${HEURISTIC_STATUSES.has(status) ? 'fall back to heuristic freshness' : "won't treat it as fresh"}. Add \`max-age\` to control browser caching too.`,
      ),
    };
  }

  if (HEURISTIC_STATUSES.has(status) || c.has('public')) {
    if (c.has('no-cache')) {
      return {
        cacheable: true,
        finding: f('cacheability.no-cache', 'cacheability', 'good', 'Cacheable, always revalidated', '`Cache-Control: no-cache` lets caches store the response but requires them to revalidate with the server before **every** reuse.'),
      };
    }
    if (PERMANENT_STATUSES.has(status) && !c.has('public')) {
      return {
        cacheable: true,
        permanent: true,
        finding: f(
          'cacheability.permanent',
          'cacheability',
          'info',
          'Cached indefinitely',
          `\`${status}\` responses without explicit freshness are cached heuristically, and most browsers keep ${status === 410 ? 'them' : 'permanent redirects'} **forever**. If this might change, send \`Cache-Control: max-age=…\` to limit it.`,
        ),
      };
    }
    if (!Number.isNaN(c.lastModified)) {
      return {
        cacheable: true,
        heuristic: true,
        finding: f(
          'cacheability.heuristic',
          'cacheability',
          'warn',
          'Probably cacheable',
          'There is no explicit freshness (`max-age` or `Expires`), so caches estimate one from `Last-Modified` (typically 10% of the time since it was last modified). Behaviour varies between caches — set `Cache-Control` explicitly.',
        ),
      };
    }
    if (c.etag) {
      return {
        cacheable: true,
        heuristic: true,
        finding: f(
          'cacheability.etag-only',
          'cacheability',
          'warn',
          'Stored, but stale immediately',
          'There is no freshness information and no `Last-Modified`, so caches can\'t estimate a lifetime: the response may be stored, but is **revalidated with its `ETag` before every reuse**. Set `Cache-Control` to make this explicit.',
        ),
      };
    }
    return {
      cacheable: false,
      finding: f(
        'cacheability.no-freshness',
        'cacheability',
        'warn',
        'Typically not cached',
        `\`${status}\` responses may be cached heuristically, but with no \`Cache-Control\`, \`Expires\`, \`Last-Modified\` or \`ETag\` most caches won't reuse it. Set \`Cache-Control\` to say what you want.`,
      ),
    };
  }

  return {
    cacheable: false,
    finding: f('cacheability.status', 'cacheability', 'info', 'Not cacheable', `\`${status}\` responses aren't cacheable by default — they'd need explicit freshness (\`Cache-Control: max-age\` or \`Expires\`).`),
  };
}

type Storage = 'private' | 'shared' | 'both';

function explainStorage(c: Ctx, cb: Cacheability): { finding: CacheFinding; storage: Storage } {
  if (c.isPreflight) {
    return { storage: 'private', finding: f('storage.preflight', 'storage', 'info', 'Browser only', 'Preflight results are cached by the browser in its CORS preflight cache, never by shared caches.') };
  }
  const priv = c.cc.values.get('private');
  if (c.has('public') && priv === true) {
    return {
      storage: 'private',
      finding: f('storage.conflict', 'storage', 'warn', 'Conflicting directives', 'Both `public` and `private` are set. Caches treat this as `private` (browser only), but the intent is unclear — remove one.'),
    };
  }
  if (priv === true) {
    return { storage: 'private', finding: f('storage.private', 'storage', 'good', 'Browser cache only', '`private` allows only the client\'s own cache (e.g. the browser) to store it. CDNs and proxies won\'t.') };
  }
  if (typeof priv === 'string') {
    return {
      storage: 'both',
      finding: f('storage.private-fields', 'storage', 'info', 'Private and shared caches (some fields private)', `\`private="${priv}"\`: shared caches may store the response but must drop ${priv.split(',').map((s) => code(s.trim())).join(', ')}.`),
    };
  }
  if (c.has('public')) {
    return { storage: 'both', finding: f('storage.public', 'storage', 'info', 'Private and shared caches', '`public` allows browsers, proxies and CDNs to store it — even for authenticated requests.') };
  }
  if (header(c.req, 'authorization') != null && c.sMaxAge == null && !c.has('must-revalidate')) {
    return {
      storage: 'private',
      finding: f(
        'storage.authorization',
        'storage',
        'info',
        'Browser cache only',
        'The request carried an `Authorization` header, so shared caches won\'t store the response unless it is marked `public`, `s-maxage` or `must-revalidate`. Browsers may still cache it.',
      ),
    };
  }
  if (cb.sharedOnly) {
    return { storage: 'shared', finding: f('storage.shared', 'storage', 'info', 'Shared caches only', '`s-maxage` gives shared caches (CDNs, proxies) a lifetime; private caches have no freshness information for this response.') };
  }
  return { storage: 'both', finding: f('storage.both', 'storage', 'info', 'Private and shared caches', 'Browsers, proxies and CDNs may all store this response. Add `private` if it contains user-specific data.') };
}

function explainMatching(c: Ctx): CacheFinding {
  if (c.isPreflight) {
    const origin = header(c.req, 'origin') ?? '';
    const credentials = (header(c.res, 'access-control-allow-credentials') ?? '').trim().toLowerCase() === 'true';
    const methods = listValues(c.res, 'access-control-allow-methods');
    const headers = listValues(c.res, 'access-control-allow-headers');
    const allMethods = methods.includes('*') && !credentials;
    const allHeaders = headers.includes('*') && !credentials;
    const lines = [
      `- from the origin ${code(origin || '(none)')}`,
      credentials ? '- with or without credentials (cookies, HTTP auth)' : '- only **without** credentials (no `Access-Control-Allow-Credentials: true`)',
      allMethods ? '- using any method' : `- using ${[...new Set([...CORS_SIMPLE_METHODS, ...methods.filter((m) => m !== '*').map((m) => m.toUpperCase())])].map(code).join(', ')}`,
      allHeaders
        ? '- with any request headers (except `Authorization`)'
        : `- sending only safelisted headers${headers.filter((h) => h !== '*').length ? ` or ${headers.filter((h) => h !== '*').map(code).join(', ')}` : ''}`,
    ];
    return f('matching.preflight', 'matching', 'info', 'Reused for matching CORS requests to this URL', `The cached preflight applies to future requests to this URL:\n\n${lines.join('\n')}${credentials && (methods.includes('*') || headers.includes('*')) ? '\n\nWildcards (`*`) are treated literally when credentials are allowed.' : ''}`);
  }
  const vary = listValues(c.res, 'vary').filter((v) => v !== '*');
  if (vary.length === 0) {
    return f('matching.url', 'matching', 'info', 'Reused for GET & HEAD requests to this URL', 'Any future `GET` or `HEAD` request to this exact URL can be answered from cache, regardless of its headers (there is no `Vary`).');
  }
  const lines = vary.map((name) => {
    const v = header(c.req, name);
    return v == null ? `- no ${code(name)} header (like this request)` : `- ${code(name)}: ${code(truncate(v, 60))}`;
  });
  return f(
    'matching.vary',
    'matching',
    'info',
    `Reused for requests with the same ${vary.map(code).join(' & ')}`,
    `\`Vary\` limits reuse to future \`GET\`/\`HEAD\` requests to this URL that send the same values as this request:\n\n${lines.join('\n')}`,
  );
}

function revalidationSuffix(c: Ctx): string {
  const parts: string[] = [];
  if (c.has('must-revalidate')) parts.push('then must be revalidated before reuse');
  else if (c.has('proxy-revalidate') || c.sMaxAge != null) parts.push('then shared caches must revalidate');
  const swr = deltaSeconds(c.cc.values.get('stale-while-revalidate'));
  if (swr != null && !Number.isNaN(swr) && swr > 0) parts.push(`may be served stale for ${formatSeconds(swr)} while revalidating in the background`);
  const sie = deltaSeconds(c.cc.values.get('stale-if-error'));
  if (sie != null && !Number.isNaN(sie) && sie > 0) parts.push(`may be served stale for ${formatSeconds(sie)} if the server errors`);
  return parts.length ? `; ${parts.join('; ')}` : '';
}

function relative(ms: number, now: number): string {
  const diff = Math.round((ms - now) / 1000);
  return diff >= 0 ? `in ${formatSeconds(diff)}` : `${formatSeconds(-diff)} ago`;
}

function explainLifetime(c: Ctx, cb: Cacheability): CacheFinding[] {
  const out: CacheFinding[] = [];
  if (c.isPreflight) {
    const n = Number((header(c.res, 'access-control-max-age') ?? '').trim());
    if (!Number.isFinite(n) || header(c.res, 'access-control-max-age') == null) {
      out.push(f('lifetime.preflight', 'lifetime', 'info', 'Expires after about 5 seconds', 'Without `Access-Control-Max-Age` the preflight cache entry lasts roughly 5 seconds.'));
    } else {
      const capped = n > PREFLIGHT_CAP_CHROMIUM;
      out.push(
        f(
          'lifetime.preflight',
          'lifetime',
          capped ? 'warn' : 'good',
          `Expires after ${formatSeconds(capped ? PREFLIGHT_CAP_CHROMIUM : n)}${capped ? ' in Chromium' : ''}`,
          capped
            ? `\`Access-Control-Max-Age: ${n}\` (${formatSeconds(n)}) exceeds browser limits: Chromium caps it at ${formatSeconds(PREFLIGHT_CAP_CHROMIUM)}, Firefox at ${formatSeconds(PREFLIGHT_CAP_FIREFOX)}.`
            : `Browsers reuse the preflight result for ${formatSeconds(n)}.`,
        ),
      );
    }
    return out;
  }

  if (c.has('no-cache')) {
    out.push(
      f(
        'lifetime.no-cache',
        'lifetime',
        'info',
        'Revalidated on every use',
        `\`no-cache\` means a stored copy must be confirmed with the server (a conditional request) before each reuse${c.maxAge != null ? `; it overrides \`max-age=${c.maxAge}\`` : ''}.${typeof c.cc.values.get('no-cache') === 'string' ? ` Here it only applies to the ${String(c.cc.values.get('no-cache')).split(',').map((s) => code(s.trim())).join(', ')} field(s).` : ''}`,
      ),
    );
    return out;
  }

  let lifetime: number | null = null;
  if (c.maxAge != null) lifetime = Number.isNaN(c.maxAge) ? 0 : c.maxAge;
  else if (c.expiresHeader != null) lifetime = Number.isNaN(c.expires) ? -1 : (c.expires - c.responseTime) / 1000;
  const shared = c.sMaxAge != null && !Number.isNaN(c.sMaxAge) ? c.sMaxAge : lifetime;

  if (lifetime == null && shared == null) {
    if (cb.permanent) {
      out.push(f('lifetime.forever', 'lifetime', 'info', 'Never expires', 'With no explicit lifetime, clients typically keep this response indefinitely.'));
      return out;
    }
    if (cb.heuristic && !Number.isNaN(c.lastModified)) {
      const since = Math.max(0, (c.responseTime - c.lastModified) / 1000);
      const estimate = since * 0.1;
      out.push(
        f(
          'lifetime.heuristic',
          'lifetime',
          'warn',
          `Expires unpredictably (≈ ${formatSeconds(estimate)})`,
          `Last modified ${formatSeconds(since)} before this response, so a typical cache considers it fresh for about **${formatSeconds(estimate)}** (10% of that). Firefox caps heuristic freshness at one week; other caches differ.`,
        ),
      );
      return out;
    }
    if (cb.heuristic) {
      out.push(f('lifetime.stale', 'lifetime', 'warn', 'Stale immediately', 'There is no way for caches to estimate a lifetime, so every reuse needs revalidation.'));
    }
    return out;
  }

  const life = lifetime ?? shared ?? 0;
  const age = c.age ?? 0;
  const ageNote = age > 0 ? `\n\nIt was already **${formatSeconds(age)} old** when received (\`Age: ${age}\` from an upstream cache).` : '';

  if (life <= 0) {
    out.push(
      f(
        'lifetime.expired',
        'lifetime',
        'info',
        c.has('must-revalidate') ? 'Stale immediately, must revalidate' : 'Expires immediately',
        `${c.maxAge === 0 ? '`max-age=0`' : Number.isNaN(c.expires) && c.expiresHeader != null ? `\`Expires: ${c.expiresHeader}\` (an invalid date)` : 'The `Expires` date is not after the response date, which'} means a stored copy is stale right away and must be revalidated before reuse${c.has('must-revalidate') ? '' : ' (caches may serve it stale when disconnected)'}.`,
      ),
    );
    return out;
  }

  const sharedDiffers = shared != null && lifetime != null && shared !== lifetime;
  const freshUntil = c.responseTime + (life - age) * 1000;
  const remaining = freshUntil - c.now;
  const durationText = formatSeconds(life);
  out.push(
    f(
      'lifetime.fresh',
      'lifetime',
      'good',
      `Fresh for ${durationText}${sharedDiffers ? ` (${formatSeconds(shared as number)} in shared caches)` : ''}`,
      `Expires after **${durationText}**${sharedDiffers ? ` in private caches and ${formatSeconds(shared as number)} in shared caches` : ''}${revalidationSuffix(c)}.${ageNote}\n\nCaptured copies stay fresh until ${new Date(freshUntil).toISOString().replace('T', ' ').slice(0, 19)} UTC — ${remaining > 0 ? `**${relative(freshUntil, c.now)}**` : `that was ${relative(freshUntil, c.now)}`}.`,
    ),
  );

  if (life >= ONE_YEAR && !c.has('immutable')) {
    out.push(
      f(
        'lifetime.immutable',
        'lifetime',
        'info',
        'Consider adding immutable',
        'This response is fresh for a year or more. If its URL is versioned (e.g. contains a content hash), add `immutable` so browsers skip revalidation even when the user reloads.',
      ),
    );
  } else if (c.has('immutable')) {
    out.push(f('lifetime.immutable-set', 'lifetime', 'good', 'Immutable', '`immutable` tells browsers not to revalidate while fresh — even on reload. Make sure the URL changes when the content does.'));
  }
  return out;
}

function explainValidation(c: Ctx, cb: Cacheability): CacheFinding[] {
  const out: CacheFinding[] = [];
  const inm = header(c.req, 'if-none-match');
  const ims = header(c.req, 'if-modified-since');
  const conditional = inm != null || ims != null;
  const lastModifiedHeader = header(c.res, 'last-modified');

  if (conditional && c.status === 200) {
    const etagMatches = inm != null && c.etag != null && splitHeaderList(inm).some((t) => t.replace(/^W\//, '') === c.etag?.replace(/^W\//, ''));
    if (etagMatches) {
      out.push(
        f(
          'validation.ignored',
          'validation',
          'warn',
          'Server ignored a matching If-None-Match',
          `The request sent \`If-None-Match: ${truncate(inm ?? '', 60)}\` and the response has the same \`ETag\`, yet the server returned the full body with \`200\` instead of \`304 Not Modified\`.`,
        ),
      );
    } else {
      out.push(f('validation.changed', 'validation', 'info', 'Conditional request, content changed', `The client revalidated (${[inm != null ? '`If-None-Match`' : '', ims != null ? '`If-Modified-Since`' : ''].filter(Boolean).join(' and ')}), but the resource had changed, so the full body was sent.`));
    }
  }

  if (c.status === 304) return out;

  if (c.etag || lastModifiedHeader) {
    const validators = [c.etag ? `\`ETag: ${truncate(c.etag, 50)}\`` : '', lastModifiedHeader ? '`Last-Modified`' : ''].filter(Boolean);
    const weak = c.etag?.startsWith('W/');
    out.push(
      f(
        'validation.validators',
        'validation',
        'good',
        'Can be revalidated cheaply',
        `${validators.join(' and ')} let${validators.length === 1 ? 's' : ''} caches revalidate an expired copy with ${c.etag ? '`If-None-Match`' : '`If-Modified-Since`'} and get a body-less \`304 Not Modified\` if nothing changed.${weak ? ' The ETag is weak (`W/`), so it can\'t be used for range requests.' : ''}${!conditional && c.status === 200 && cb.cacheable ? '\n\nThis request wasn\'t conditional — a client holding a cached copy could have sent ' + (c.etag ? `\`If-None-Match: ${truncate(c.etag, 50)}\`` : '`If-Modified-Since`') + ' instead.' : ''}`,
      ),
    );
  } else if (cb.cacheable && c.bodySize > 0 && !c.has('immutable') && !c.isPreflight) {
    out.push(
      f(
        'validation.missing',
        'validation',
        'info',
        'No validators',
        'Without an `ETag` or `Last-Modified`, an expired copy can\'t be revalidated: caches must download the whole body again. Add an `ETag` so they can get a `304 Not Modified` instead.',
      ),
    );
  }
  return out;
}

function explainRequest(c: Ctx): CacheFinding[] {
  const out: CacheFinding[] = [];
  const names = [...c.reqCc.values.keys()].filter((k) => k !== 'no-store');
  const pragma = header(c.req, 'pragma');
  const bypass = c.reqCc.values.has('no-cache') || deltaSeconds(c.reqCc.values.get('max-age')) === 0 || (pragma != null && /no-cache/i.test(pragma));
  if (bypass) {
    out.push(
      f(
        'request.bypass',
        'request',
        'info',
        'Client asked to revalidate',
        `The request sent ${[...(names.length ? [`\`Cache-Control: ${header(c.req, 'cache-control')}\``] : []), ...(pragma ? [`\`Pragma: ${pragma}\``] : [])].join(' and ')}, so caches had to check with the server instead of answering from cache — typical of a hard reload or \`fetch(url, { cache: 'no-cache' })\`.`,
      ),
    );
  } else if (names.length) {
    out.push(f('request.directives', 'request', 'info', 'Request cache directives', `The request sent \`Cache-Control: ${header(c.req, 'cache-control')}\`, which constrains how caches may answer it.`));
  }
  return out;
}

function explainHeaders(c: Ctx, cb: Cacheability, storage: Storage | null): CacheFinding[] {
  const out: CacheFinding[] = [];
  const ccHeader = header(c.res, 'cache-control');
  const pragma = header(c.res, 'pragma');
  if (pragma && /no-cache/i.test(pragma) && ccHeader == null) {
    out.push(
      f(
        'headers.pragma',
        'headers',
        'warn',
        'Pragma: no-cache has no effect here',
        '`Pragma` is an HTTP/1.0 *request* header; caches ignore it on responses. Use `Cache-Control: no-cache` (or `no-store`) instead.',
      ),
    );
  }
  for (const name of c.cc.conflicts) {
    out.push(f(`headers.duplicate.${name}`, 'headers', 'warn', `Conflicting ${name} directives`, `\`${name}\` appears more than once with different values. Caches use the first one or treat the response as stale — send it once.`));
  }
  if (cb.cacheable && !c.isPreflight && c.status !== 304) {
    const vary = listValues(c.res, 'vary').map((v) => v.toLowerCase());
    const encoding = header(c.res, 'content-encoding');
    if (encoding && !/^identity$/i.test(encoding.trim()) && !vary.includes('accept-encoding') && !vary.includes('*') && storage !== 'private') {
      out.push(
        f(
          'headers.vary-encoding',
          'headers',
          'warn',
          'Missing Vary: Accept-Encoding',
          `The body is compressed (\`Content-Encoding: ${encoding}\`) but \`Vary\` doesn't include \`Accept-Encoding\`. A shared cache could serve this compressed copy to a client that can't decode it.`,
        ),
      );
    }
    const acao = header(c.res, 'access-control-allow-origin');
    if (acao && acao.trim() !== '*' && !vary.includes('origin') && !vary.includes('*')) {
      out.push(
        f(
          'headers.vary-origin',
          'headers',
          'warn',
          'Missing Vary: Origin',
          `\`Access-Control-Allow-Origin: ${truncate(acao, 60)}\` names a specific origin, but \`Vary\` doesn't include \`Origin\`. A cached copy could be served to a different origin and fail its CORS check.`,
        ),
      );
    }
  }
  if (cb.cacheable && (storage === 'both' || storage === 'shared') && headerAll(c.res, 'set-cookie').length > 0) {
    out.push(
      f(
        'privacy.set-cookie',
        'privacy',
        'warn',
        'Sets cookies but may be cached by shared caches',
        'This response sets a cookie and isn\'t marked `private`, so a CDN or proxy could store it and hand the same cookie to other users. Add `Cache-Control: private` (or `no-store`).',
      ),
    );
  }
  return out;
}

/**
 * Analyse how caches will treat an exchange. Returns findings in display order: cacheability,
 * storage, reuse, freshness, revalidation, request directives, header issues.
 */
export function analyzeCaching(detail: ExchangeDetail, now: number = Date.now()): CacheFinding[] {
  const response = detail.response;
  if (!response) {
    const failed = detail.summary.state === 'aborted' || detail.error != null;
    return [
      f(
        'cacheability.no-response',
        'cacheability',
        'info',
        failed ? 'No response to analyse' : 'Waiting for the response',
        failed ? 'The exchange failed before a response arrived, so there is nothing a cache could store.' : 'Caching can be analysed once the response headers arrive.',
      ),
    ];
  }

  const req = detail.request.headers;
  const res = response.headers;
  const method = detail.request.method.toUpperCase();
  const cc = parseCacheControl(headerAll(res, 'cache-control'));
  const reqCc = parseCacheControl(headerAll(req, 'cache-control'));
  const expiresHeader = header(res, 'expires');
  const date = parseHttpDate(header(res, 'date'));
  const captureTime = detail.timing.start / 1000 + (detail.timing.responseHeaders ?? 0);
  const ageRaw = header(res, 'age');
  const ageNum = ageRaw == null ? null : deltaSeconds(ageRaw);

  const ctx: Ctx = {
    method,
    url: detail.request.url,
    status: response.status,
    req,
    res,
    cc,
    reqCc,
    has: (d) => cc.values.has(d),
    maxAge: deltaSeconds(cc.values.get('max-age')),
    sMaxAge: deltaSeconds(cc.values.get('s-maxage')),
    expires: parseHttpDate(expiresHeader),
    expiresHeader,
    date,
    responseTime: Number.isNaN(date) ? captureTime : date,
    lastModified: parseHttpDate(header(res, 'last-modified')),
    etag: header(res, 'etag')?.trim(),
    age: ageNum == null || Number.isNaN(ageNum) ? null : ageNum,
    bodySize: detail.responseBody.decodedSize ?? detail.responseBody.capturedSize ?? detail.responseBody.wireSize ?? 0,
    // HTK's definition: any OPTIONS with an Origin (Access-Control-Request-Method is usually there too).
    isPreflight: method === 'OPTIONS' && header(req, 'origin') != null,
    now,
  };

  const findings: CacheFinding[] = [];
  const cb = explainCacheability(ctx);
  findings.push(cb.finding);

  let storage: Storage | null = null;
  if (cb.cacheable && ctx.status !== 304) {
    const s = explainStorage(ctx, cb);
    storage = s.storage;
    findings.push(s.finding);
    findings.push(explainMatching(ctx));
  }
  if (cb.cacheable) findings.push(...explainLifetime(ctx, cb));
  findings.push(...explainValidation(ctx, cb));
  findings.push(...explainRequest(ctx));
  findings.push(...explainHeaders(ctx, cb, storage));
  return findings;
}

/** Response headers relevant to caching, in a stable display order (for the Cache tab). */
export const CACHE_HEADER_NAMES = ['Cache-Control', 'Expires', 'Date', 'Age', 'Last-Modified', 'ETag', 'Vary', 'Pragma'] as const;

/** `[name, value]` pairs of caching-related response headers present on an exchange. */
export function cachingHeaders(detail: ExchangeDetail): [string, string][] {
  const res = detail.response?.headers ?? [];
  const out: [string, string][] = [];
  for (const name of CACHE_HEADER_NAMES) for (const v of headerAll(res, name)) out.push([name, v]);
  return out;
}

