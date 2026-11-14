import { describe, expect, test } from 'bun:test';
import type { ExchangeDetail, Headers } from '../api/types';
import { analyzeCaching, cachingHeaders, deltaSeconds, formatSeconds, parseCacheControl, parseHttpDate, splitHeaderList, type CacheFinding } from './caching';

const NOW = Date.parse('2026-11-08T12:00:00Z');
const DATE = 'Sun, 08 Nov 2026 12:00:00 GMT';

function detail(opts: { method?: string; status?: number | null; req?: Headers; res?: Headers; url?: string; bodySize?: number; state?: ExchangeDetail['summary']['state'] }): ExchangeDetail {
  const url = opts.url ?? 'https://example.com/app.js';
  const status = opts.status === undefined ? 200 : opts.status;
  return {
    summary: {
      id: 1,
      kind: 'http',
      state: opts.state ?? 'complete',
      startedAt: NOW,
      method: opts.method ?? 'GET',
      scheme: 'https',
      host: 'example.com',
      port: 443,
      path: '/app.js',
      httpVersion: 'HTTP/1.1',
      status: status ?? undefined,
      contentKind: 'javascript',
      requestSize: 0,
      responseSize: opts.bodySize ?? 100,
    },
    request: { method: opts.method ?? 'GET', url, scheme: 'https', host: 'example.com', port: 443, path: '/app.js', httpVersion: 'HTTP/1.1', headers: opts.req ?? [] },
    requestBody: { wireSize: 0, capturedSize: 0, truncated: false, encodings: [] },
    response: status == null ? undefined : { status, statusMessage: 'OK', httpVersion: 'HTTP/1.1', headers: opts.res ?? [] },
    responseBody: { wireSize: opts.bodySize ?? 100, capturedSize: opts.bodySize ?? 100, truncated: false, encodings: [], decodedSize: opts.bodySize ?? 100 },
    timing: { start: NOW * 1000, responseHeaders: 10 },
    client: { remoteAddr: '127.0.0.1:5000', connectionId: 1, via: 'connect' },
  };
}

const byId = (fs: CacheFinding[], prefix: string) => fs.find((x) => x.id.startsWith(prefix));
const ids = (fs: CacheFinding[]) => fs.map((x) => x.id);

describe('parsing helpers', () => {
  test('splitHeaderList respects quoted strings', () => {
    expect(splitHeaderList('no-cache="Set-Cookie, X-Foo", max-age=10')).toEqual(['no-cache="Set-Cookie, X-Foo"', 'max-age=10']);
    expect(splitHeaderList(' a , ,b ')).toEqual(['a', 'b']);
    expect(splitHeaderList('a="x\\"y,z", b')).toEqual(['a="x\\"y,z"', 'b']);
  });

  test('parseCacheControl lowercases names, unquotes values, flags conflicts', () => {
    const d = parseCacheControl(['Max-Age="60", PUBLIC', 'no-cache="Set-Cookie, X-Foo"', 'max-age=120']);
    expect(d.values.get('max-age')).toBe('60');
    expect(d.values.get('public')).toBe(true);
    expect(d.values.get('no-cache')).toBe('Set-Cookie, X-Foo');
    expect(d.conflicts).toEqual(['max-age']);
  });

  test('deltaSeconds and parseHttpDate', () => {
    expect(deltaSeconds(undefined)).toBeNull();
    expect(deltaSeconds('60')).toBe(60);
    expect(deltaSeconds(' 60 ')).toBe(60);
    expect(deltaSeconds('abc')).toBeNaN();
    expect(deltaSeconds(true)).toBeNaN();
    expect(deltaSeconds('99999999999')).toBe(2147483648);
    expect(parseHttpDate(DATE)).toBe(NOW);
    expect(parseHttpDate('0')).toBeNaN();
    expect(parseHttpDate('-1')).toBeNaN();
    expect(parseHttpDate(undefined)).toBeNaN();
  });

  test('formatSeconds', () => {
    expect(formatSeconds(0)).toBe('0 seconds');
    expect(formatSeconds(1)).toBe('1 second');
    expect(formatSeconds(45)).toBe('45 seconds');
    expect(formatSeconds(90)).toBe('1 minute 30 seconds');
    expect(formatSeconds(600)).toBe('10 minutes');
    expect(formatSeconds(9000)).toBe('2 hours 30 minutes');
    expect(formatSeconds(86400 * 14)).toBe('14 days');
    expect(formatSeconds(31536000)).toBe('1 year');
  });
});

describe('analyzeCaching', () => {
  test('no response', () => {
    const fs = analyzeCaching(detail({ status: null, state: 'aborted' }), NOW);
    expect(fs).toHaveLength(1);
    expect(fs[0]?.summary).toBe('No response to analyse');
    expect(analyzeCaching(detail({ status: null, state: 'waiting' }), NOW)[0]?.summary).toBe('Waiting for the response');
  });

  test('long-lived immutable asset', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'public, max-age=31536000, immutable'], ['ETag', '"abc"'], ['Vary', 'Accept-Encoding'], ['Content-Encoding', 'br']] }), NOW);
    expect(byId(fs, 'cacheability')?.status).toBe('good');
    expect(byId(fs, 'cacheability')?.summary).toBe('Cacheable');
    expect(byId(fs, 'storage')?.id).toBe('storage.public');
    expect(byId(fs, 'lifetime.fresh')?.summary).toBe('Fresh for 1 year');
    expect(byId(fs, 'lifetime.immutable-set')).toBeDefined();
    expect(byId(fs, 'lifetime.immutable')?.id).not.toBe('lifetime.immutable');
    expect(byId(fs, 'headers.vary-encoding')).toBeUndefined();
  });

  test('case-insensitive, quoted max-age (HTK bug) is understood', () => {
    const fs = analyzeCaching(detail({ res: [['date', DATE], ['cache-control', 'Max-Age="600"']] }), NOW);
    expect(byId(fs, 'lifetime.fresh')?.summary).toBe('Fresh for 10 minutes');
  });

  test('suggests immutable for year-long lifetimes and warns without Date', () => {
    const fs = analyzeCaching(detail({ res: [['Cache-Control', 'max-age=31536000']] }), NOW);
    expect(byId(fs, 'cacheability.max-age')?.status).toBe('warn');
    expect(byId(fs, 'cacheability.max-age')?.explanation).toContain('Date');
    expect(byId(fs, 'lifetime.immutable')?.summary).toBe('Consider adding immutable');
  });

  test('Expires that disagrees with max-age is reported', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['Expires', 'Sun, 08 Nov 2026 13:00:00 GMT']] }), NOW);
    expect(byId(fs, 'cacheability.max-age')?.explanation).toContain('disagrees');
  });

  test('no-store wins and mentions overridden directives', () => {
    const fs = analyzeCaching(detail({ res: [['Cache-Control', 'no-store, max-age=60']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.no-store');
    expect(fs[0]?.explanation).toContain('`max-age`');
    expect(byId(fs, 'storage')).toBeUndefined();
  });

  test('request no-store prevents storage', () => {
    const fs = analyzeCaching(detail({ req: [['Cache-Control', 'no-store']], res: [['Cache-Control', 'max-age=60']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.request-no-store');
  });

  test('Vary: * is never reused', () => {
    expect(analyzeCaching(detail({ res: [['Cache-Control', 'max-age=60'], ['Vary', 'Accept, *']] }), NOW)[0]?.id).toBe('cacheability.vary-star');
  });

  test('unsafe methods are not cacheable and invalidate', () => {
    const fs = analyzeCaching(detail({ method: 'DELETE', res: [['Cache-Control', 'max-age=60']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.method');
    expect(fs[0]?.explanation).toContain('invalidates');
  });

  test('POST needs freshness and a matching Content-Location', () => {
    const url = 'https://example.com/search?q=1';
    const ok = analyzeCaching(detail({ method: 'POST', url, res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['Content-Location', '/search?q=1#top']] }), NOW);
    expect(ok[0]?.summary).toBe('Cacheable for future GET/HEAD requests');
    const bad = analyzeCaching(detail({ method: 'POST', url, res: [['Cache-Control', 'max-age=60']] }), NOW);
    expect(bad[0]?.summary).toBe('Not cacheable');
    expect(bad[0]?.explanation).toContain('Content-Location');
  });

  test('CORS preflight reads Access-Control-Allow-Credentials case-insensitively (HTK bug)', () => {
    const fs = analyzeCaching(
      detail({
        method: 'OPTIONS',
        status: 204,
        req: [['Origin', 'https://app.example'], ['Access-Control-Request-Method', 'PUT']],
        res: [['access-control-max-age', '600'], ['access-control-allow-credentials', 'true'], ['access-control-allow-methods', 'PUT, DELETE'], ['access-control-allow-headers', 'X-Token']],
      }),
      NOW,
    );
    expect(fs[0]?.id).toBe('cacheability.preflight');
    expect(byId(fs, 'storage')?.id).toBe('storage.preflight');
    const m = byId(fs, 'matching.preflight');
    expect(m?.explanation).toContain('with or without credentials');
    expect(m?.explanation).toContain('`PUT`');
    expect(m?.explanation).toContain('`X-Token`');
    expect(byId(fs, 'lifetime.preflight')?.summary).toBe('Expires after 10 minutes');
  });

  test('preflight defaults and browser caps', () => {
    const base = { method: 'OPTIONS', status: 204, req: [['Origin', 'https://a.example']] as Headers };
    expect(analyzeCaching(detail({ ...base, res: [] }), NOW)[0]?.summary).toBe('Preflight cached very briefly');
    expect(analyzeCaching(detail({ ...base, res: [['Access-Control-Max-Age', '0']] }), NOW)[0]?.summary).toBe('Preflight not cached');
    const capped = analyzeCaching(detail({ ...base, res: [['Access-Control-Max-Age', '86400']] }), NOW);
    expect(byId(capped, 'lifetime.preflight')?.status).toBe('warn');
    expect(byId(capped, 'lifetime.preflight')?.summary).toBe('Expires after 2 hours in Chromium');
  });

  test('Expires only, and invalid Expires means already expired', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Expires', 'Sun, 08 Nov 2026 14:00:00 GMT']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.expires');
    expect(byId(fs, 'lifetime.fresh')?.summary).toBe('Fresh for 2 hours');
    const expired = analyzeCaching(detail({ res: [['Date', DATE], ['Expires', '0']] }), NOW);
    expect(expired[0]?.explanation).toContain('already expired');
    expect(byId(expired, 'lifetime.expired')).toBeDefined();
  });

  test('heuristic freshness from Last-Modified (10% rule)', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Last-Modified', 'Thu, 29 Oct 2026 12:00:00 GMT']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.heuristic');
    expect(fs[0]?.status).toBe('warn');
    expect(byId(fs, 'lifetime.heuristic')?.summary).toBe('Expires unpredictably (≈ 1 day)');
  });

  test('ETag only: stored but stale immediately; nothing: typically not cached', () => {
    expect(analyzeCaching(detail({ res: [['ETag', '"x"']] }), NOW)[0]?.id).toBe('cacheability.etag-only');
    const none = analyzeCaching(detail({ res: [['Content-Type', 'text/plain']] }), NOW);
    expect(none[0]?.id).toBe('cacheability.no-freshness');
    expect(none[0]?.summary).toBe('Typically not cached');
  });

  test('statuses: 302 needs explicit freshness, 308 is permanent, 404 heuristic', () => {
    expect(analyzeCaching(detail({ status: 302, res: [] }), NOW)[0]?.id).toBe('cacheability.status');
    expect(analyzeCaching(detail({ status: 302, res: [['Date', DATE], ['Cache-Control', 'max-age=60']] }), NOW)[0]?.summary).toBe('Cacheable');
    const perm = analyzeCaching(detail({ status: 308, res: [] }), NOW);
    expect(perm[0]?.id).toBe('cacheability.permanent');
    expect(byId(perm, 'lifetime.forever')).toBeDefined();
  });

  test('Authorization requests are private unless public', () => {
    const fs = analyzeCaching(detail({ req: [['Authorization', 'Bearer x']], res: [['Date', DATE], ['Cache-Control', 'max-age=60']] }), NOW);
    expect(byId(fs, 'storage')?.id).toBe('storage.authorization');
    const pub = analyzeCaching(detail({ req: [['Authorization', 'Bearer x']], res: [['Date', DATE], ['Cache-Control', 'public, max-age=60']] }), NOW);
    expect(byId(pub, 'storage')?.id).toBe('storage.public');
  });

  test('private, public+private conflict and private fields', () => {
    expect(byId(analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'private, max-age=60']] }), NOW), 'storage')?.id).toBe('storage.private');
    expect(byId(analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'public, private, max-age=60']] }), NOW), 'storage')?.status).toBe('warn');
    expect(byId(analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'private="Set-Cookie", max-age=60']] }), NOW), 'storage')?.id).toBe('storage.private-fields');
  });

  test('s-maxage only: shared caches', () => {
    const fs = analyzeCaching(detail({ status: 302, res: [['Date', DATE], ['Cache-Control', 's-maxage=300']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.s-maxage');
    expect(byId(fs, 'storage')?.id).toBe('storage.shared');
    expect(byId(fs, 'lifetime.fresh')?.explanation).toContain('shared caches must revalidate');
  });

  test('no-cache: cacheable, revalidated every time', () => {
    const fs = analyzeCaching(detail({ res: [['Cache-Control', 'no-cache'], ['ETag', '"1"']] }), NOW);
    expect(fs[0]?.id).toBe('cacheability.no-cache');
    expect(byId(fs, 'lifetime.no-cache')?.summary).toBe('Revalidated on every use');
  });

  test('stale-while-revalidate and must-revalidate phrasing', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60, must-revalidate, stale-while-revalidate=30, stale-if-error=86400']] }), NOW);
    const e = byId(fs, 'lifetime.fresh')?.explanation ?? '';
    expect(e).toContain('then must be revalidated before reuse');
    expect(e).toContain('stale for 30 seconds while revalidating');
    expect(e).toContain('stale for 1 day if the server errors');
  });

  test('Age reduces the remaining freshness', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=3600'], ['Age', '600']] }), NOW);
    const e = byId(fs, 'lifetime.fresh')?.explanation ?? '';
    expect(e).toContain('10 minutes old');
    expect(e).toContain('in 50 minutes');
  });

  test('max-age=0 expires immediately', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'private, max-age=0']] }), NOW);
    expect(byId(fs, 'lifetime.expired')?.summary).toBe('Expires immediately');
  });

  test('304 revalidation and ignored validators', () => {
    const nm = analyzeCaching(detail({ status: 304, req: [['If-None-Match', '"v1"']], res: [['ETag', '"v1"'], ['Cache-Control', 'max-age=60'], ['Date', DATE]] }), NOW);
    expect(nm[0]?.id).toBe('cacheability.not-modified');
    expect(byId(nm, 'storage')).toBeUndefined();
    const ignored = analyzeCaching(detail({ req: [['If-None-Match', 'W/"v1"']], res: [['ETag', '"v1"'], ['Cache-Control', 'max-age=60'], ['Date', DATE]] }), NOW);
    expect(byId(ignored, 'validation.ignored')?.status).toBe('warn');
    const changed = analyzeCaching(detail({ req: [['If-Modified-Since', DATE]], res: [['Last-Modified', DATE], ['Cache-Control', 'max-age=60'], ['Date', DATE]] }), NOW);
    expect(byId(changed, 'validation.changed')).toBeDefined();
  });

  test('validators: hint for conditional requests, suggestion when missing', () => {
    const v = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['ETag', 'W/"9"']] }), NOW);
    const val = byId(v, 'validation.validators');
    expect(val?.explanation).toContain('weak');
    expect(val?.explanation).toContain("wasn't conditional");
    const missing = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60']] }), NOW);
    expect(byId(missing, 'validation.missing')).toBeDefined();
    const empty = analyzeCaching(detail({ bodySize: 0, res: [['Date', DATE], ['Cache-Control', 'max-age=60']] }), NOW);
    expect(byId(empty, 'validation.missing')).toBeUndefined();
  });

  test('client revalidation directives', () => {
    const fs = analyzeCaching(detail({ req: [['Cache-Control', 'no-cache'], ['Pragma', 'no-cache']], res: [['Date', DATE], ['Cache-Control', 'max-age=60']] }), NOW);
    expect(byId(fs, 'request.bypass')?.explanation).toContain('hard reload');
  });

  test('header issues: Pragma on responses, duplicates, Vary, cookies', () => {
    expect(byId(analyzeCaching(detail({ res: [['Pragma', 'no-cache']] }), NOW), 'headers.pragma')?.status).toBe('warn');
    expect(byId(analyzeCaching(detail({ res: [['Cache-Control', 'max-age=60'], ['Cache-Control', 'max-age=3600']] }), NOW), 'headers.duplicate.max-age')).toBeDefined();
    const enc = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['Content-Encoding', 'gzip'], ['Access-Control-Allow-Origin', 'https://app.example']] }), NOW);
    expect(ids(enc)).toContain('headers.vary-encoding');
    expect(ids(enc)).toContain('headers.vary-origin');
    const ok = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['Content-Encoding', 'gzip'], ['Vary', 'Accept-Encoding, Origin'], ['Access-Control-Allow-Origin', 'https://app.example']] }), NOW);
    expect(ids(ok)).not.toContain('headers.vary-encoding');
    expect(ids(ok)).not.toContain('headers.vary-origin');
    const cookie = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'public, max-age=60'], ['Set-Cookie', 'a=1']] }), NOW);
    expect(byId(cookie, 'privacy.set-cookie')?.status).toBe('warn');
    const priv = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'private, max-age=60'], ['Set-Cookie', 'a=1']] }), NOW);
    expect(byId(priv, 'privacy.set-cookie')).toBeUndefined();
  });

  test('findings are ordered: cacheability first, then storage, matching, lifetime', () => {
    const fs = analyzeCaching(detail({ res: [['Date', DATE], ['Cache-Control', 'max-age=60'], ['Vary', 'Accept-Encoding']], req: [['Accept-Encoding', 'gzip']] }), NOW);
    expect(fs.slice(0, 4).map((x) => x.category)).toEqual(['cacheability', 'storage', 'matching', 'lifetime']);
    expect(byId(fs, 'matching.vary')?.explanation).toContain('`gzip`');
  });

  test('cachingHeaders lists relevant headers in a stable order', () => {
    const d = detail({ res: [['etag', '"1"'], ['content-type', 'x'], ['cache-control', 'max-age=1'], ['date', DATE]] });
    expect(cachingHeaders(d).map(([k]) => k)).toEqual(['Cache-Control', 'Date', 'ETag']);
  });
});
