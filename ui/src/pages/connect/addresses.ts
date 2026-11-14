/** Proxy address helpers (local vs LAN addresses, device setup URL). Pure. */

import type { Status } from '../../api/types';

export interface HostPort {
  host: string;
  port: number | null;
}

/** Split `host:port`, `[v6]:port`, bare hosts and bare IPv6 addresses. */
export function splitHostPort(address: string): HostPort {
  const a = address.trim();
  const bracket = /^\[([^\]]+)\](?::(\d+))?$/.exec(a);
  if (bracket) return { host: bracket[1] as string, port: bracket[2] ? Number(bracket[2]) : null };
  const colons = a.split(':').length - 1;
  if (colons === 1) {
    const [host, port] = a.split(':') as [string, string];
    const n = Number(port);
    return { host, port: Number.isInteger(n) && n > 0 && n < 65536 ? n : null };
  }
  return { host: a, port: null };
}

/** `host:port`, bracketing IPv6 hosts. */
export function formatHostPort(host: string, port: number): string {
  return host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`;
}

export function isLoopbackHost(host: string): boolean {
  const h = host.toLowerCase();
  return h === 'localhost' || h === '::1' || /^127\.\d+\.\d+\.\d+$/.test(h);
}

export function isUnspecifiedHost(host: string): boolean {
  return host === '0.0.0.0' || host === '::' || host === '[::]';
}

/** True when the proxy accepts connections from other machines. */
export function isLanBind(bindAddress: string | null | undefined): boolean {
  if (!bindAddress) return false;
  return !isLoopbackHost(bindAddress);
}

function isIpv4(host: string): boolean {
  return /^\d{1,3}(\.\d{1,3}){3}$/.test(host);
}

/** Link-local / docker-ish addresses phones can't use. */
function isUnusableForDevices(host: string): boolean {
  return /^169\.254\./.test(host) || /^fe80:/i.test(host) || /^172\.17\./.test(host);
}

/**
 * Non-loopback `host:port` addresses from `status.proxy.addresses`, best first (private
 * IPv4 before IPv6; link-local and docker bridge addresses last), deduplicated.
 */
export function lanAddresses(status: Pick<Status, 'proxy'> | null | undefined): string[] {
  if (!status) return [];
  const seen = new Set<string>();
  const out: { addr: string; rank: number; index: number }[] = [];
  status.proxy.addresses.forEach((raw, index) => {
    const { host, port } = splitHostPort(raw);
    if (!host || isLoopbackHost(host) || isUnspecifiedHost(host)) return;
    const addr = formatHostPort(host, port ?? status.proxy.port);
    if (seen.has(addr)) return;
    seen.add(addr);
    const rank = isUnusableForDevices(host) ? 3 : isIpv4(host) ? (/^(10|192\.168|172\.(1[6-9]|2\d|3[01]))\./.test(host) ? 0 : 1) : 2;
    out.push({ addr, rank, index });
  });
  return out.sort((a, b) => a.rank - b.rank || a.index - b.index).map((x) => x.addr);
}

/** Device setup page served by the proxy port (`http://192.168.1.23:8000/`). */
export function deviceSetupUrl(address: string): string {
  return `http://${address}/`;
}

/** `http://host:port` proxy URL. */
export function proxyUrl(address: string): string {
  return `http://${address}`;
}
