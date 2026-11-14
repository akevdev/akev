/**
 * Header-table logic: ordering (wire vs sorted, pseudo-headers first), copy text, special value
 * classification (cookies, JWT, Basic auth, content types, dates) and helpers. No React.
 */

import type { Headers } from '../../../api/types';
import { decodeJwt, type DecodedJwt } from '../../../lib/http';
import { base64ToText } from '../../../lib/encoding';

export interface HeaderEntry {
  name: string;
  value: string;
  /** Position on the wire (stable React key together with name). */
  index: number;
  pseudo: boolean;
}

export function isPseudoHeader(name: string): boolean {
  return name.startsWith(':');
}

/**
 * Display order: HTTP/2 pseudo-headers always first (in wire order), then either the wire order
 * or a case-insensitive sort by name (stable for duplicates).
 */
export function orderHeaders(headers: Headers, sorted: boolean): HeaderEntry[] {
  const entries = headers.map(([name, value], index) => ({ name, value, index, pseudo: isPseudoHeader(name) }));
  const pseudo = entries.filter((e) => e.pseudo);
  const rest = entries.filter((e) => !e.pseudo);
  if (sorted) rest.sort((a, b) => a.name.toLowerCase().localeCompare(b.name.toLowerCase()) || a.index - b.index);
  return [...pseudo, ...rest];
}

/** `Name: value` lines joined by `\n` — no trailing newline. Pseudo-headers excluded by default. */
export function headersToText(entries: readonly Pick<HeaderEntry, 'name' | 'value' | 'pseudo'>[], includePseudo = false): string {
  return entries
    .filter((e) => includePseudo || !e.pseudo)
    .map((e) => `${e.name}: ${e.value}`)
    .join('\n');
}

export type HeaderSpecial = 'cookie' | 'set-cookie' | 'jwt' | 'basic' | 'bearer' | 'content-type' | 'date' | 'none';

const DATE_HEADERS = new Set(['date', 'expires', 'last-modified', 'if-modified-since', 'if-unmodified-since', 'retry-after', 'sunset', 'if-range']);
const AUTH_HEADERS = new Set(['authorization', 'proxy-authorization']);

/** Which special renderer applies to a header. */
export function classifyHeader(name: string, value: string): HeaderSpecial {
  const n = name.toLowerCase();
  if (n === 'cookie') return 'cookie';
  if (n === 'set-cookie') return 'set-cookie';
  if (n === 'content-type') return 'content-type';
  if (AUTH_HEADERS.has(n)) {
    if (/^basic\s+/i.test(value)) return 'basic';
    if (/^bearer\s+/i.test(value)) return decodeJwt(value) ? 'jwt' : 'bearer';
    return 'none';
  }
  if (DATE_HEADERS.has(n) && !Number.isNaN(parseHeaderDate(value))) return 'date';
  return 'none';
}

/** Parse an HTTP date header value (rejects bare numbers like `Retry-After: 120`). */
export function parseHeaderDate(value: string): number {
  const v = value.trim();
  if (!v || /^-?\d+$/.test(v)) return Number.NaN;
  return Date.parse(v);
}

export interface BasicCredentials {
  user: string;
  password: string;
}

/** Decode `Basic base64(user:password)`; `null` if malformed. */
export function decodeBasicAuth(value: string): BasicCredentials | null {
  const m = /^basic\s+([A-Za-z0-9+/=_-]+)\s*$/i.exec(value.trim());
  if (!m?.[1]) return null;
  let text: string | null;
  try {
    text = base64ToText(m[1].replace(/-/g, '+').replace(/_/g, '/'));
  } catch {
    return null;
  }
  if (text == null) return null;
  const i = text.indexOf(':');
  if (i < 0) return { user: text, password: '' };
  return { user: text.slice(0, i), password: text.slice(i + 1) };
}

export type JwtStatus = 'valid' | 'expired' | 'not-yet-valid' | 'no-expiry';

export interface JwtSummary {
  jwt: DecodedJwt;
  status: JwtStatus;
  /** Unix ms of `nbf`, if present. */
  notBefore?: number;
  algorithm: string | null;
}

/** Decode a bearer JWT and compute its validity window. */
export function summarizeJwt(value: string, now: number = Date.now()): JwtSummary | null {
  const jwt = decodeJwt(value, now);
  if (!jwt) return null;
  const nbf = typeof jwt.payload.nbf === 'number' ? jwt.payload.nbf * 1000 : undefined;
  let status: JwtStatus = 'no-expiry';
  if (nbf != null && nbf > now) status = 'not-yet-valid';
  else if (jwt.expiresAt != null) status = jwt.expiresAt < now ? 'expired' : 'valid';
  const alg = typeof jwt.header.alg === 'string' ? jwt.header.alg : null;
  return { jwt, status, notBefore: nbf, algorithm: alg };
}

/** Registered JWT claims with friendly labels, in display order. */
export const JWT_CLAIMS: readonly [string, string][] = [
  ['iss', 'Issuer'],
  ['sub', 'Subject'],
  ['aud', 'Audience'],
  ['exp', 'Expires'],
  ['nbf', 'Not before'],
  ['iat', 'Issued at'],
  ['jti', 'Token id'],
  ['scope', 'Scope'],
];

/** Values longer than this are clamped with a "Show all" toggle. */
export const LONG_HEADER_VALUE = 280;
