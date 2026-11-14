/**
 * Transparent capture (Linux): pure helpers shared by the Connect panel and Settings → Proxy —
 * settings patching, firewall-mark parsing, the listener status line, and the panel's options
 * → `POST /api/transparent/plan` request mapping.
 */

import type {
  Settings,
  TransparentBackend,
  TransparentCaptureMode,
  TransparentExclusion,
  TransparentLauncher,
  TransparentPlanRequest,
  TransparentPortScope,
  TransparentSettings,
  TransparentStatus,
} from '../../api/types';
import { formatRelativeTime } from '../../lib/format';

/** Interceptor id of the Connect card. */
export const TRANSPARENT_ID = 'transparent-linux';

/** Mark suggested when the user picks mark-based exclusion and none is set. */
export const SUGGESTED_MARK = 0x1a4b;

/** Settings with `proxy.transparent` patched. */
export function withTransparent<T extends Pick<Settings, 'proxy'>>(settings: T, patch: Partial<TransparentSettings>): T {
  return { ...settings, proxy: { ...settings.proxy, transparent: { ...settings.proxy.transparent, ...patch } } };
}

const MAX_MARK = 0xffff_ffff;

/** Parse a firewall mark: decimal or `0x` hex, 0 … 2³²−1. Empty = 0. `null` when invalid. */
export function parseMark(text: string): number | null {
  const t = text.trim().toLowerCase();
  if (t === '') return 0;
  const n = /^0x[0-9a-f]{1,8}$/.test(t) ? Number.parseInt(t.slice(2), 16) : /^\d{1,10}$/.test(t) ? Number(t) : Number.NaN;
  return Number.isInteger(n) && n >= 0 && n <= MAX_MARK ? n : null;
}

/** `0` → `0`, `6731` → `0x1a4b` (marks are usually written in hex). */
export function formatMark(mark: number): string {
  return mark === 0 ? '0' : `0x${mark.toString(16)}`;
}

export function validateMarkText(text: string): string | null {
  return parseMark(text) == null ? 'A number (e.g. 0x1a4b), 0 to turn marking off' : null;
}

/** Parse `8080, 8443 3000` into ports. `error` when any token isn't a port. */
export function parsePortList(text: string): { ports: number[]; error: string | null } {
  const tokens = text.split(/[\s,]+/).filter(Boolean);
  const ports: number[] = [];
  for (const t of tokens) {
    const n = /^\d{1,5}$/.test(t) ? Number(t) : Number.NaN;
    if (!Number.isInteger(n) || n < 1 || n > 65535) return { ports, error: `“${t}” isn't a port (1–65535)` };
    if (!ports.includes(n)) ports.push(n);
  }
  if (ports.length === 0) return { ports, error: 'Enter at least one port' };
  return { ports, error: null };
}

/** Parse comma/space separated IPs or CIDRs (loose check; the daemon validates exactly). */
export function parseDestinations(text: string): { items: string[]; error: string | null } {
  const items = text.split(/[\s,]+/).filter(Boolean);
  for (const d of items) {
    const [addr, prefix, ...rest] = d.split('/');
    const v4 = /^\d{1,3}(\.\d{1,3}){3}$/.test(addr ?? '');
    const v6 = !v4 && /^[0-9a-f:]+$/i.test(addr ?? '') && (addr ?? '').includes(':');
    const max = v4 ? 32 : 128;
    const okPrefix = prefix == null || (/^\d{1,3}$/.test(prefix) && Number(prefix) <= max);
    if ((!v4 && !v6) || !okPrefix || rest.length > 0) return { items, error: `“${d}” isn't an IP address or CIDR range` };
  }
  return { items, error: null };
}

/** Panel options (persisted per user as a page preference). */
export interface TransparentOptions {
  mode: TransparentCaptureMode;
  ports: TransparentPortScope;
  customPorts: string;
  backend: TransparentBackend;
  command: string;
  user: string;
  exclude: TransparentExclusion;
  launcher: TransparentLauncher;
  excludeDestinations: string;
  trustCa: boolean;
  sudo: boolean;
}

export const DEFAULT_TRANSPARENT_OPTIONS: TransparentOptions = {
  mode: 'app',
  ports: 'web',
  customPorts: '8080, 8443',
  backend: 'nft',
  command: '',
  user: '',
  exclude: 'uid',
  launcher: 'systemd',
  excludeDestinations: '',
  trustCa: true,
  sudo: true,
};

/** Options from storage, with unknown/missing fields replaced by defaults. */
export function normalizeOptions(raw: unknown): TransparentOptions {
  const o = (raw && typeof raw === 'object' ? raw : {}) as Partial<Record<keyof TransparentOptions, unknown>>;
  const pick = <K extends keyof TransparentOptions>(k: K, allowed?: readonly TransparentOptions[K][]): TransparentOptions[K] => {
    const v = o[k];
    const d = DEFAULT_TRANSPARENT_OPTIONS[k];
    if (typeof v !== typeof d) return d;
    if (allowed && !allowed.includes(v as TransparentOptions[K])) return d;
    return v as TransparentOptions[K];
  };
  return {
    mode: pick('mode', ['app', 'user', 'machine']),
    ports: pick('ports', ['web', 'all', 'custom']),
    customPorts: pick('customPorts'),
    backend: pick('backend', ['nft', 'iptables']),
    command: pick('command'),
    user: pick('user'),
    exclude: pick('exclude', ['uid', 'mark']),
    launcher: pick('launcher', ['systemd', 'cgroupfs']),
    excludeDestinations: pick('excludeDestinations'),
    trustCa: pick('trustCa'),
    sudo: pick('sudo'),
  };
}

/** The plan request for the options, or the first input error (nothing is sent then). */
export function planRequestOf(o: TransparentOptions): { request: TransparentPlanRequest | null; error: string | null } {
  const request: TransparentPlanRequest = { mode: o.mode, ports: o.ports, backend: o.backend, sudo: o.sudo };
  if (o.ports === 'custom') {
    const { ports, error } = parsePortList(o.customPorts);
    if (error) return { request: null, error };
    request.customPorts = ports;
  }
  const dest = parseDestinations(o.excludeDestinations);
  if (dest.error) return { request: null, error: dest.error };
  if (dest.items.length) request.excludeDestinations = dest.items;
  if (o.mode === 'app') {
    request.launcher = o.launcher;
    request.trustCa = o.trustCa;
    if (o.command.trim()) request.command = o.command.trim();
  } else if (o.mode === 'user') {
    if (!o.user.trim()) return { request: null, error: 'Enter the user name or id to capture' };
    request.user = o.user.trim();
  } else {
    request.exclude = o.exclude;
  }
  return { request, error: null };
}

export type ListenerTone = 'off' | 'error' | 'waiting' | 'active';

export interface ListenerSummary {
  tone: ListenerTone;
  title: string;
  detail?: string;
}

/** One-line state of the listener + redirect detection. */
export function listenerSummary(s: TransparentStatus | null | undefined, now: number = Date.now()): ListenerSummary {
  if (!s) return { tone: 'off', title: 'Checking the transparent listener…' };
  if (!s.enabled) return { tone: 'off', title: 'Transparent listener is off', detail: 'Turn it on so redirected connections have somewhere to go.' };
  if (!s.listening) return { tone: 'error', title: "Transparent listener isn't running", detail: s.error ?? "It couldn't open its port." };
  const where = s.port != null ? `port ${s.port}` : 'its port';
  const fallback = s.port != null && s.requestedPort !== 0 && s.port !== s.requestedPort ? ` (${s.requestedPort} was busy)` : '';
  if (s.lastRedirectAt != null) {
    const ago = formatRelativeTime(s.lastRedirectAt, now);
    const count = `${s.redirected} redirected connection${s.redirected === 1 ? '' : 's'}`;
    const stale = now - s.lastRedirectAt > 10 * 60_000;
    return {
      tone: stale ? 'waiting' : 'active',
      title: stale ? `No redirected connections lately (last ${ago})` : `Redirection is working — last connection ${ago}`,
      detail: `${count} on ${where}${fallback}.`,
    };
  }
  return { tone: 'waiting', title: `Waiting for redirected connections on ${where}${fallback}`, detail: 'Load the rules below, then run the check command.' };
}

/** Rows for the "recent connections" list. */
export function recentRows(s: TransparentStatus | null | undefined, now: number = Date.now(), limit = 6): { key: string; destination: string; who: string; when: string; redirected: boolean }[] {
  return (s?.recent ?? []).slice(0, limit).map((c, i) => ({
    key: `${c.at}-${c.client}-${i}`,
    destination: c.originalDestination ?? 'direct to the port',
    who: c.process ?? c.client,
    when: formatRelativeTime(c.at, now),
    redirected: c.originalDestination != null,
  }));
}

/** Mode descriptions shown under the picker. */
export const MODE_HELP: Record<TransparentCaptureMode, string> = {
  app: "Only programs you launch in Akev's capture cgroup are redirected — everything else on this machine is untouched. Recommended.",
  user: 'Every connection of another user account is redirected (e.g. a dedicated account for the app under test).',
  machine: "Every connection on this machine is redirected, except Akev's own.",
};
