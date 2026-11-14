/** Certificate & connection display helpers for ConnectionView. No React. */

import type { CertificateSummary, ProxyMode } from '../../../api/types';
import { formatNumber } from '../../../lib/format';

/** Parse an RFC 4514-ish distinguished name (`CN=x, O=Acme\, Inc, C=US`) into ordered pairs. */
export function parseDn(dn: string): [string, string][] {
  const out: [string, string][] = [];
  let key = '';
  let val = '';
  let inValue = false;
  let quoted = false;
  const push = () => {
    if (key.trim()) out.push([key.trim(), val.trim()]);
    key = '';
    val = '';
    inValue = false;
  };
  for (let i = 0; i < dn.length; i++) {
    const ch = dn[i] as string;
    if (!inValue) {
      if (ch === '=') inValue = true;
      else if (ch === ',' || ch === '/') {
        if (key.trim()) push();
        key = '';
      } else key += ch;
      continue;
    }
    if (ch === '\\' && i + 1 < dn.length) {
      val += dn[++i];
      continue;
    }
    if (ch === '"') {
      quoted = !quoted;
      continue;
    }
    if ((ch === ',' || ch === '+') && !quoted) {
      push();
      continue;
    }
    val += ch;
  }
  if (inValue || key.trim()) push();
  return out;
}

/** First value of an attribute (`CN`, `O`…) in a DN. */
export function dnField(dn: string, field: string): string | undefined {
  const f = field.toLowerCase();
  return parseDn(dn).find(([k]) => k.toLowerCase() === f)?.[1];
}

export type ValidityStatus = 'valid' | 'expiring' | 'expired' | 'not-yet-valid' | 'unknown';

export interface Validity {
  status: ValidityStatus;
  /** Whole days until expiry (negative when expired). */
  daysLeft: number;
  /** Fraction of the validity period elapsed (0..1, clamped). */
  elapsed: number;
  notBefore: number;
  notAfter: number;
}

/** Certificates expiring within this many days are flagged. */
export const EXPIRING_DAYS = 14;

export function certValidity(cert: Pick<CertificateSummary, 'notBefore' | 'notAfter'>, now: number = Date.now()): Validity {
  const nb = Date.parse(cert.notBefore);
  const na = Date.parse(cert.notAfter);
  if (!Number.isFinite(nb) || !Number.isFinite(na)) return { status: 'unknown', daysLeft: Number.NaN, elapsed: 0, notBefore: nb, notAfter: na };
  const daysLeft = Math.floor((na - now) / 86_400_000);
  const span = Math.max(1, na - nb);
  const elapsed = Math.min(1, Math.max(0, (now - nb) / span));
  let status: ValidityStatus = 'valid';
  if (now < nb) status = 'not-yet-valid';
  else if (now > na) status = 'expired';
  else if (daysLeft < EXPIRING_DAYS) status = 'expiring';
  return { status, daysLeft, elapsed, notBefore: nb, notAfter: na };
}

/** Human validity sentence: `Expires in 50 days`, `Expired 3 days ago`, `Valid from …`. */
export function validityLabel(v: Validity, now: number = Date.now()): string {
  const days = (ms: number) => {
    const d = Math.round(Math.abs(ms) / 86_400_000);
    if (d === 0) {
      const h = Math.max(1, Math.round(Math.abs(ms) / 3_600_000));
      return `${h} hour${h === 1 ? '' : 's'}`;
    }
    return `${formatNumber(d)} day${d === 1 ? '' : 's'}`;
  };
  switch (v.status) {
    case 'expired':
      return `Expired ${days(now - v.notAfter)} ago`;
    case 'not-yet-valid':
      return `Not valid for another ${days(v.notBefore - now)}`;
    case 'unknown':
      return 'Validity unknown';
    default:
      return `Expires in ${days(v.notAfter - now)}`;
  }
}

/** Uppercase colon-separated hex (accepts any input spacing/case). */
export function formatFingerprint(hex: string): string {
  const clean = hex.replace(/[^0-9a-f]/gi, '').toUpperCase();
  return clean.match(/.{1,2}/g)?.join(':') ?? hex;
}

/** TLS protocol version → display label and tone (TLS 1.3 good, ≤ 1.1 deprecated). */
export function tlsVersionInfo(version: string | undefined): { label: string; tone: 'success' | 'neutral' | 'warning' } | null {
  if (!version) return null;
  const v = version.replace(/^tls\s*v?/i, '').replace('_', '.').trim();
  const label = /^\d/.test(v) ? `TLS ${v}` : version;
  if (v.startsWith('1.3')) return { label, tone: 'success' };
  if (v.startsWith('1.2')) return { label, tone: 'neutral' };
  return { label, tone: 'warning' };
}

export const PROXY_MODE_LABELS: Readonly<Record<ProxyMode, string>> = {
  'http-proxy': 'HTTP proxy',
  connect: 'CONNECT tunnel',
  socks: 'SOCKS proxy',
  direct: 'Direct (transparent)',
  composer: 'Akev Compose',
  wireguard: 'WireGuard tunnel',
  transparent: 'Transparent capture (redirected)',
};

/** ALPN protocol id → readable name. */
export function alpnLabel(id: string): string {
  switch (id) {
    case 'h2':
      return 'HTTP/2';
    case 'h3':
      return 'HTTP/3';
    case 'http/1.1':
      return 'HTTP/1.1';
    case 'http/1.0':
      return 'HTTP/1.0';
    default:
      return id;
  }
}
