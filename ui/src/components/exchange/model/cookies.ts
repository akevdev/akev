/** Cookie helpers for HeadersTable / CookiesTable (expiry, size, spec warnings). No React. */

import type { Headers } from '../../../api/types';
import { parseCookieHeader, parseSetCookie, type RequestCookie, type SetCookie } from '../../../lib/http';
import { utf8Length } from '../../../lib/encoding';

export type CookieExpiry =
  | { kind: 'session' }
  | { kind: 'expires'; at: number; source: 'max-age' | 'expires' }
  | { kind: 'expired'; at: number; source: 'max-age' | 'expires' }
  | { kind: 'invalid'; raw: string };

/**
 * When a Set-Cookie expires: `Max-Age` wins over `Expires` (RFC 6265 §5.3); `Max-Age<=0` deletes
 * immediately. `base` is the response time (Date header) used for Max-Age.
 */
export function cookieExpiry(c: Pick<SetCookie, 'maxAge' | 'expires'>, now: number = Date.now(), base: number = now): CookieExpiry {
  if (c.maxAge != null) {
    const at = base + c.maxAge * 1000;
    return c.maxAge <= 0 || at <= now ? { kind: 'expired', at, source: 'max-age' } : { kind: 'expires', at, source: 'max-age' };
  }
  if (c.expires != null) {
    const at = Date.parse(c.expires);
    if (!Number.isFinite(at)) return { kind: 'invalid', raw: c.expires };
    return at <= now ? { kind: 'expired', at, source: 'expires' } : { kind: 'expires', at, source: 'expires' };
  }
  return { kind: 'session' };
}

/** Bytes counted against browser cookie limits (name + value). */
export function cookieSize(name: string, value: string): number {
  return utf8Length(name) + utf8Length(value);
}

/** Spec problems that make browsers reject or mis-scope a Set-Cookie. */
export function cookieWarnings(c: SetCookie): string[] {
  const out: string[] = [];
  const sameSite = c.sameSite?.toLowerCase();
  if (sameSite === 'none' && !c.secure) out.push('SameSite=None without Secure — browsers reject this cookie.');
  if (sameSite && !['lax', 'strict', 'none'].includes(sameSite)) out.push(`Unknown SameSite value "${c.sameSite}" — treated as Lax.`);
  if (c.name.startsWith('__Host-')) {
    if (!c.secure) out.push('__Host- cookies must be Secure.');
    if (c.domain) out.push('__Host- cookies must not set Domain.');
    if ((c.path ?? '') !== '/') out.push('__Host- cookies must use Path=/.');
  } else if (c.name.startsWith('__Secure-') && !c.secure) out.push('__Secure- cookies must be Secure.');
  if (c.partitioned && !c.secure) out.push('Partitioned cookies must be Secure.');
  if (cookieSize(c.name, c.value) > 4096) out.push('Larger than 4 KB — most browsers will drop it.');
  return out;
}

/** All request cookies from every `Cookie` header, in order. */
export function requestCookies(headers: Headers): RequestCookie[] {
  return headers.filter(([k]) => k.toLowerCase() === 'cookie').flatMap(([, v]) => parseCookieHeader(v));
}

/** All `Set-Cookie` headers parsed, in order. */
export function responseCookies(headers: Headers): SetCookie[] {
  return headers.filter(([k]) => k.toLowerCase() === 'set-cookie').map(([, v]) => parseSetCookie(v));
}

/** Percent-decoded value when it differs (for tooltips), else null. */
export function decodedCookieValue(value: string): string | null {
  if (!/%[0-9a-f]{2}/i.test(value)) return null;
  try {
    const d = decodeURIComponent(value);
    return d === value ? null : d;
  } catch {
    return null;
  }
}
