/**
 * Authentication detection for the request Auth tab: `Authorization` / `Proxy-Authorization`
 * schemes (Basic decoded, Bearer with JWT decoding), JWTs carried in cookies, API-key style
 * headers and token query parameters. Nothing is verified — this is an inspector.
 */

import type { Headers } from '../../../../api/types';
import { base64ToText } from '../../../../lib/encoding';
import { formatDateTime, formatRelativeTime } from '../../../../lib/format';
import { decodeJwt, parseCookieHeader, type DecodedJwt } from '../../../../lib/http';

export type AuthItem =
  | { kind: 'basic'; source: string; username: string; password: string; raw: string; decoded: boolean }
  | { kind: 'bearer'; source: string; token: string; jwt: DecodedJwt | null }
  | { kind: 'jwt'; source: string; token: string; jwt: DecodedJwt }
  | { kind: 'scheme'; source: string; scheme: string; credentials: string }
  | { kind: 'api-key'; source: string; value: string };

/** Compact JWS: three base64url segments (the signature may be empty for `alg: none`). */
const JWT_RE = /^[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]*$/;

const API_KEY_HEADERS = new Set([
  'x-api-key',
  'api-key',
  'apikey',
  'x-auth-token',
  'x-access-token',
  'x-csrf-token',
  'x-xsrf-token',
  'x-amz-security-token',
  'x-goog-api-key',
  'ocp-apim-subscription-key',
  'private-token',
  'x-shopify-access-token',
]);

const TOKEN_PARAMS = new Set(['access_token', 'token', 'api_key', 'apikey', 'key', 'auth', 'jwt', 'id_token']);

/** Whether a string looks like a JWT and decodes as one. */
export function asJwt(value: string, now?: number): DecodedJwt | null {
  const v = value.trim();
  if (!JWT_RE.test(v)) return null;
  return decodeJwt(v, now);
}

/** Decode `Basic <base64(user:pass)>` credentials. */
export function decodeBasic(credentials: string): { username: string; password: string; decoded: boolean } {
  let text: string | null = null;
  try {
    text = base64ToText(credentials.trim());
  } catch {
    text = null;
  }
  if (text == null) return { username: '', password: '', decoded: false };
  const i = text.indexOf(':');
  return i < 0 ? { username: text, password: '', decoded: true } : { username: text.slice(0, i), password: text.slice(i + 1), decoded: true };
}

function authorizationItem(source: string, value: string, now?: number): AuthItem {
  const trimmed = value.trim();
  const sp = trimmed.indexOf(' ');
  const scheme = sp < 0 ? trimmed : trimmed.slice(0, sp);
  const credentials = sp < 0 ? '' : trimmed.slice(sp + 1).trim();
  const lower = scheme.toLowerCase();
  if (lower === 'basic') {
    return { kind: 'basic', source, raw: credentials, ...decodeBasic(credentials) };
  }
  if (lower === 'bearer' || lower === 'token' || lower === 'jwt') {
    return { kind: 'bearer', source, token: credentials, jwt: asJwt(credentials, now) };
  }
  // A bare JWT without a scheme.
  if (sp < 0) {
    const jwt = asJwt(trimmed, now);
    if (jwt) return { kind: 'jwt', source, token: trimmed, jwt };
  }
  return { kind: 'scheme', source, scheme, credentials };
}

/**
 * Every credential found in the request. `query` (decoded `[name, value]` pairs) adds token
 * query parameters. Order: Authorization headers, API-key headers, JWT cookies, JWT headers,
 * query tokens.
 */
export function detectAuth(headers: Headers, query: readonly (readonly [string, string])[] = [], now?: number): AuthItem[] {
  const auth: AuthItem[] = [];
  const keys: AuthItem[] = [];
  const cookies: AuthItem[] = [];
  const others: AuthItem[] = [];
  for (const [name, value] of headers) {
    const lower = name.toLowerCase();
    if (lower === 'authorization' || lower === 'proxy-authorization') {
      auth.push(authorizationItem(name, value, now));
    } else if (lower === 'cookie') {
      for (const c of parseCookieHeader(value)) {
        const jwt = c.value ? asJwt(c.value, now) : null;
        if (jwt) cookies.push({ kind: 'jwt', source: `Cookie · ${c.name}`, token: c.value, jwt });
      }
    } else if (API_KEY_HEADERS.has(lower)) {
      const jwt = asJwt(value, now);
      keys.push(jwt ? { kind: 'jwt', source: name, token: value.trim(), jwt } : { kind: 'api-key', source: name, value });
    } else if (!lower.startsWith(':') && lower !== 'set-cookie') {
      const jwt = asJwt(value, now);
      if (jwt) others.push({ kind: 'jwt', source: name, token: value.trim(), jwt });
    }
  }
  const params: AuthItem[] = [];
  for (const [name, value] of query) {
    if (!value) continue;
    const jwt = asJwt(value, now);
    if (jwt) params.push({ kind: 'jwt', source: `Query · ${name}`, token: value, jwt });
    else if (TOKEN_PARAMS.has(name.toLowerCase())) params.push({ kind: 'api-key', source: `Query · ${name}`, value });
  }
  return [...auth, ...keys, ...cookies, ...others, ...params];
}

// =====================================================================================
// JWT claims
// =====================================================================================

export type JwtValidity = 'valid' | 'expired' | 'not-yet-valid' | 'no-expiry';

/** Time validity of a decoded token at `now`. */
export function jwtValidity(jwt: DecodedJwt, now: number = Date.now()): JwtValidity {
  const nbf = typeof jwt.payload.nbf === 'number' ? jwt.payload.nbf * 1000 : undefined;
  if (nbf != null && nbf > now) return 'not-yet-valid';
  if (jwt.expiresAt == null) return 'no-expiry';
  return jwt.expiresAt < now ? 'expired' : 'valid';
}

export interface ClaimRow {
  claim: string;
  /** Human label for registered claims (`Expires`, `Subject`…), else the claim name. */
  label: string;
  /** Display value (dates formatted, arrays joined). */
  value: string;
  /** Secondary text (relative time for date claims). */
  hint?: string;
  /** Whether this is a registered (RFC 7519) or common claim. */
  registered: boolean;
}

const CLAIM_LABELS: Readonly<Record<string, string>> = {
  iss: 'Issuer',
  sub: 'Subject',
  aud: 'Audience',
  exp: 'Expires',
  nbf: 'Not before',
  iat: 'Issued at',
  jti: 'Token ID',
  scope: 'Scope',
  scp: 'Scope',
  azp: 'Authorized party',
  email: 'Email',
  name: 'Name',
  sid: 'Session ID',
  roles: 'Roles',
};

const TIME_CLAIMS = new Set(['exp', 'nbf', 'iat', 'auth_time', 'updated_at']);
const CLAIM_ORDER = ['sub', 'iss', 'aud', 'exp', 'iat', 'nbf', 'jti', 'scope', 'scp', 'azp'];

function display(v: unknown): string {
  if (v == null) return String(v);
  if (typeof v === 'string') return v;
  if (Array.isArray(v) && v.every((x) => typeof x === 'string' || typeof x === 'number')) return v.join(', ');
  if (typeof v === 'object') return JSON.stringify(v);
  return String(v);
}

/** Claim rows: registered claims first (in a sensible order), then the rest as sent. */
export function describeClaims(payload: Record<string, unknown>, now: number = Date.now()): ClaimRow[] {
  const keys = Object.keys(payload);
  const ordered = [...CLAIM_ORDER.filter((k) => k in payload), ...keys.filter((k) => !CLAIM_ORDER.includes(k))];
  return ordered.map((claim) => {
    const v = payload[claim];
    const registered = claim in CLAIM_LABELS;
    const label = CLAIM_LABELS[claim] ?? claim;
    if (TIME_CLAIMS.has(claim) && typeof v === 'number' && Number.isFinite(v)) {
      const ms = v * 1000;
      const rel = formatRelativeTime(ms, now);
      // The relative hint only adds information while it isn't just the date again.
      return { claim, label, value: formatDateTime(ms), hint: /^\d{4}-/.test(rel) ? undefined : rel, registered: registered || claim === 'auth_time' };
    }
    return { claim, label, value: display(v), registered };
  });
}

/** Short human description of a token's lifetime (`expires in 3h`, `expired 2d ago`). */
export function describeExpiry(jwt: DecodedJwt, now: number = Date.now()): string {
  const validity = jwtValidity(jwt, now);
  if (validity === 'no-expiry') return 'No expiry';
  if (validity === 'not-yet-valid') {
    const nbf = (jwt.payload.nbf as number) * 1000;
    return `Valid ${formatRelativeTime(nbf, now)}`;
  }
  const rel = formatRelativeTime(jwt.expiresAt as number, now);
  // Far-away times come back as a date ("2025-11-08"): read "Expired on 2025-11-08".
  const on = /^\d{4}-/.test(rel) ? `on ${rel}` : rel;
  return validity === 'expired' ? `Expired ${on}` : `Expires ${on}`;
}
