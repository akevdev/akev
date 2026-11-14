/**
 * Inline validation for settings fields. Every validator returns `null` when valid or a
 * short, actionable message ("Leave out the scheme — just example.com").
 */

import { formatNumber } from '../../lib/format';

export function isIpv4(s: string): boolean {
  const m = /^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/.exec(s);
  return !!m && m.slice(1).every((p) => Number(p) <= 255 && (p.length === 1 || !p.startsWith('0')));
}

export function isIpv6(s: string): boolean {
  if (!s.includes(':') || !/^[0-9a-f:.]+$/i.test(s)) return false;
  const doubles = s.split('::').length - 1;
  if (doubles > 1) return false;
  const parts = s.split(':');
  // Embedded IPv4 tail (::ffff:1.2.3.4) counts as two groups.
  const last = parts[parts.length - 1] ?? '';
  const hasV4 = last.includes('.');
  if (hasV4 && !isIpv4(last)) return false;
  const groups = hasV4 ? parts.slice(0, -1) : parts;
  if (groups.some((g) => g.length > 4)) return false;
  const count = groups.filter(Boolean).length + (hasV4 ? 2 : 0);
  return doubles === 1 ? count < 8 : count === 8 && groups.every(Boolean);
}

export function isIpAddress(s: string): boolean {
  return isIpv4(s) || isIpv6(s.replace(/^\[|\]$/g, ''));
}

const LABEL = /^(?!-)[a-z0-9_-]{1,63}(?<!-)$/i;
const GLOB_LABEL = /^[a-z0-9_*-]{1,63}$/i;

/**
 * Normalise a host entry: trim, lowercase, drop a trailing dot — and reduce a pasted URL to
 * its host (`https://user@API.example.com/v1?q` → `api.example.com`), since that's what
 * people copy from the browser or the traffic list.
 */
export function normalizeHost(s: string): string {
  let h = s.trim().toLowerCase();
  const url = /^[a-z][a-z0-9+.-]*:\/\/([^/?#]*)/.exec(h);
  if (url) h = url[1] as string;
  if (url || /^[^/@\s]+@[^@\s]+$/.test(h)) h = h.replace(/^[^@]*@/, '');
  if (url) h = h.replace(/[/?#].*$/, '');
  return h.replace(/\.$/, '');
}

/**
 * Hostname, IP or glob (`*.example.com`, `api.*.internal`, `*`), with an optional `:port`.
 */
export function validateHostPattern(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter a hostname, e.g. api.example.com';
  if (/\s/.test(s)) return "Hostnames can't contain spaces";
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(s)) return 'Leave out the scheme — just example.com';
  if (s.includes('/')) return 'Leave out the path — just the host';
  if (s.includes('@')) return 'Leave out credentials — just the host';
  if (isIpAddress(s)) return null;
  let host = s;
  const bracketed = /^\[([^\]]+)\](?::(\d+))?$/.exec(s);
  if (bracketed) {
    if (!isIpv6(bracketed[1] as string)) return 'Not a valid IPv6 address';
    return bracketed[2] && !validPort(bracketed[2]) ? 'Port must be between 1 and 65535' : null;
  }
  const portMatch = /:(\d*)$/.exec(s);
  if (portMatch) {
    if (!validPort(portMatch[1] as string)) return 'Port must be between 1 and 65535';
    host = s.slice(0, portMatch.index);
  }
  if (!host) return 'Enter a hostname before the port';
  if (host.length > 253) return 'Hostname is too long';
  if (host.includes('..')) return 'Hostname has an empty label (..)';
  if (host === '*' || host === '**') return null;
  const labels = host.replace(/\.$/, '').split('.');
  for (const label of labels) {
    if (!label) return "Hostname can't start or end with a dot";
    if (label.includes('*')) {
      if (!GLOB_LABEL.test(label)) return `“${label}” has characters that aren't allowed`;
    } else if (!LABEL.test(label)) {
      return /^-|-$/.test(label) ? "Labels can't start or end with a hyphen" : `“${label}” has characters that aren't allowed`;
    }
  }
  return null;
}

function validPort(p: string): boolean {
  if (!/^\d{1,5}$/.test(p)) return false;
  const n = Number(p);
  return n >= 1 && n <= 65535;
}

export function validatePort(n: number | null | undefined): string | null {
  if (n == null || !Number.isFinite(n)) return 'Enter a port number';
  if (!Number.isInteger(n) || n < 1 || n > 65535) return 'Port must be between 1 and 65535';
  return null;
}

/** Port typed as text: the number, or null when it isn't a whole number. */
export function parsePort(text: string): number | null {
  const s = text.trim();
  return /^\d{1,6}$/.test(s) ? Number(s) : null;
}

/** Validation of a port as typed (never clamps — the user sees what they'll get). */
export function validatePortText(text: string): string | null {
  const s = text.trim();
  if (!s) return 'Enter a port number';
  if (!/^\d+$/.test(s)) return 'Use digits only, e.g. 8000';
  return validatePort(parsePort(s));
}

/** Non-blocking hint for privileged ports. */
export function portWarning(n: number | null | undefined): string | null {
  if (n != null && n > 0 && n < 1024) return 'Ports below 1024 usually need administrator rights';
  return null;
}

export function validateIpAddress(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter an IP address';
  return isIpAddress(s) ? null : 'Enter an IPv4 or IPv6 address, e.g. 127.0.0.1';
}

export function validateBindAddress(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter an address, e.g. 127.0.0.1 or 0.0.0.0';
  if (s === 'localhost' || isIpAddress(s)) return null;
  return 'Use an IP address (127.0.0.1, 0.0.0.0, ::) or localhost';
}

const PROXY_SCHEMES = ['http', 'https', 'socks4', 'socks4a', 'socks5', 'socks5h'];

/** Upstream proxy URL: `http://host:3128`, `socks5://user:pass@host:1080`. */
export function validateProxyUrl(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter the proxy URL, e.g. http://proxy.local:3128';
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(s);
  if (!scheme) return 'Start with a scheme, e.g. http://proxy.local:3128';
  if (!PROXY_SCHEMES.includes((scheme[1] as string).toLowerCase())) return `Use ${PROXY_SCHEMES.slice(0, 2).join(', ')}, or a socks4/5 scheme`;
  let url: URL;
  try {
    url = new URL(s);
  } catch {
    return "This isn't a valid URL";
  }
  if (!url.hostname) return 'Add the proxy host';
  if ((url.pathname && url.pathname !== '/') || url.search || url.hash) return 'Leave out the path — just scheme://host:port';
  return null;
}

export function validateHttpUrl(raw: string, opts: { httpsOnly?: boolean; allowFile?: boolean } = {}): string | null {
  const s = raw.trim();
  if (!s) return 'Enter a URL';
  let url: URL;
  try {
    url = new URL(s);
  } catch {
    return /^[a-z][a-z0-9+.-]*:\/\//i.test(s) ? "This isn't a valid URL" : `Start with ${opts.httpsOnly ? 'https://' : 'http:// or https://'}`;
  }
  if (opts.allowFile && url.protocol === 'file:') return null;
  if (opts.httpsOnly ? url.protocol !== 'https:' : url.protocol !== 'http:' && url.protocol !== 'https:') {
    return opts.httpsOnly ? 'DNS-over-HTTPS needs an https:// URL' : 'Use an http:// or https:// URL';
  }
  if (!url.hostname) return 'Add a host';
  return null;
}

/** DoH resolver: empty = system resolver. */
export function validateDohUrl(raw: string): string | null {
  return raw.trim() ? validateHttpUrl(raw, { httpsOnly: true }) : null;
}

export function validatePacUrl(raw: string): string | null {
  return validateHttpUrl(raw, { allowFile: true });
}

/** Absolute filesystem path (POSIX, `~/…`, Windows drive or UNC). */
export function validateAbsolutePath(raw: string): string | null {
  const s = raw.trim();
  if (!s) return 'Enter a file path';
  if (/^(\/|~\/|[a-z]:[\\/]|\\\\)/i.test(s)) return null;
  return 'Use an absolute path, e.g. /etc/ssl/my-ca.pem or C:\\certs\\ca.pem';
}

/** Timeouts in ms. `allowZero` = 0 means "no timeout". */
export function validateTimeout(n: number | null | undefined, opts: { allowZero?: boolean; max?: number } = {}): string | null {
  if (n == null || !Number.isFinite(n)) return 'Enter a duration in milliseconds';
  if (n < 0 || (!opts.allowZero && n === 0)) return opts.allowZero ? 'Use 0 for no timeout, or a positive value' : 'Must be greater than 0';
  if (opts.max != null && n > opts.max) return `Must be at most ${formatNumber(opts.max)} ms`;
  return null;
}

/** Index of an equal entry (after normalisation), excluding `exceptIndex`. */
export function findDuplicate(list: readonly string[], value: string, normalize: (s: string) => string = (s) => s.trim(), exceptIndex = -1): number {
  const v = normalize(value);
  return list.findIndex((x, i) => i !== exceptIndex && normalize(x) === v);
}

// ---- unit conversions for byte-sized settings shown in MB

const MB = 1024 * 1024;

export function bytesToMb(bytes: number): number {
  return Math.round((bytes / MB) * 10) / 10;
}

export function mbToBytes(mb: number): number {
  return Math.round(mb * MB);
}

/** A bind address the way the daemon stores and reports it (`localhost`/empty → `127.0.0.1`, no brackets). */
export function normalizeBind(bind: string): string {
  const b = bind.trim().replace(/^\[/, '').replace(/\]$/, '').toLowerCase();
  return b === '' || b === 'localhost' ? '127.0.0.1' : b;
}

/**
 * Stored listener settings that the running proxy doesn't use: `--proxy-port` /
 * `--proxy-bind` (never persisted) or a busy port at startup. `Status.proxy` reports the live
 * values; `null` fields agree. A stored port of 0 means "any port", so it is never overridden.
 */
export function listenerOverrides(stored: { port: number; bindAddress: string }, live: { port: number; bindAddress: string }): { port: number | null; bindAddress: string | null } {
  return {
    port: stored.port !== 0 && stored.port !== live.port ? stored.port : null,
    bindAddress: normalizeBind(stored.bindAddress) !== normalizeBind(live.bindAddress) ? stored.bindAddress.trim() : null,
  };
}

/**
 * Where "Use port N" moves the proxy (`POST /api/proxy/rebind`) to match the stored settings:
 * the overridden fields from the settings, the others as they are now. `null` when the proxy
 * already listens where the settings say. `what` says which fields move.
 */
export function storedListenerTarget(
  stored: { port: number; bindAddress: string },
  live: { port: number; bindAddress: string },
): { port: number; bindAddress: string; what: 'port' | 'bind' | 'both' } | null {
  const o = listenerOverrides(stored, live);
  if (o.port == null && o.bindAddress == null) return null;
  return {
    port: o.port ?? live.port,
    bindAddress: o.bindAddress != null ? normalizeBind(o.bindAddress) : normalizeBind(live.bindAddress),
    what: o.port != null && o.bindAddress != null ? 'both' : o.port != null ? 'port' : 'bind',
  };
}
