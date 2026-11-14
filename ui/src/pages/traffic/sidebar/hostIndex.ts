/**
 * The Traffic sidebar's model: request counts per host and per client app, kept up to date
 * incrementally from the traffic store, grouped into registrable domains, and the filter clauses
 * the sidebar adds. No React.
 *
 * Counting is incremental: the store's `order` (ascending ids) grows at the end while traffic
 * flows and at the front while a paged `hello` delivers older exchanges, so each sync finds the
 * counted run by binary search and visits just the ids around it (O(new + log n)). Anything else
 * — deletions, a clear, a resync or an id inserted inside the run (imports) — triggers one
 * rebuild (O(n), ~10 ms at 100k).
 */

import type { ExchangeId, ExchangeSummary } from '../../../api/types';
import { indexOfSorted } from '../../../state/traffic';
import { clause } from '../filter/filterEdit';

export { hasClause, removeClause } from './clauses';

// =====================================================================================
// Registrable domains
// =====================================================================================

/**
 * Second-level labels under two-letter country TLDs that act as public suffixes
 * (`co.uk`, `com.au`, `ne.jp`…). A small stand-in for the Public Suffix List, enough to group a
 * sidebar sensibly without shipping the list.
 */
const CC_SECOND_LEVEL = new Set(['co', 'com', 'net', 'org', 'gov', 'edu', 'ac', 'or', 'ne', 'go', 'gob', 'mil', 'nic', 'ltd', 'plc', 'sch']);

/** Hosting suffixes where every subdomain belongs to someone else. */
const SHARED_SUFFIXES = new Set([
  'github.io',
  'gitlab.io',
  'vercel.app',
  'netlify.app',
  'pages.dev',
  'workers.dev',
  'herokuapp.com',
  'web.app',
  'firebaseapp.com',
  'appspot.com',
  'azurewebsites.net',
  'onrender.com',
  'fly.dev',
  'ngrok.io',
  'ngrok-free.app',
  'blogspot.com',
]);

const IPV4 = /^\d{1,3}(\.\d{1,3}){3}$/;

/**
 * The registrable domain a host belongs to (`api.example.co.uk` → `example.co.uk`). IP addresses,
 * single-label hosts (`localhost`) and public suffixes themselves are their own group.
 */
export function registrableDomain(host: string): string {
  const h = host.toLowerCase().replace(/\.$/, '');
  if (!h || h.includes(':') || IPV4.test(h) || h.startsWith('[')) return h;
  const labels = h.split('.');
  if (labels.length <= 2) return h;
  const last2 = labels.slice(-2).join('.');
  const tld = labels[labels.length - 1] as string;
  const second = labels[labels.length - 2] as string;
  const suffixLabels = SHARED_SUFFIXES.has(last2) || (tld.length === 2 && CC_SECOND_LEVEL.has(second)) ? 2 : 1;
  return labels.slice(-(suffixLabels + 1)).join('.');
}

// =====================================================================================
// Incremental counts
// =====================================================================================

export class HostIndex {
  /** Exchanges per host. */
  readonly hosts = new Map<string, number>();
  /** Exchanges per client app (`''` = unknown). */
  readonly sources = new Map<string, number>();
  /** Increments whenever counts change. */
  version = 0;
  /** How many ids are counted: a contiguous run of `order`, from `firstId` to `lastId`. */
  private counted = 0;
  private firstId: ExchangeId | null = null;
  private lastId: ExchangeId | null = null;
  private stale = true;

  /** Recount from scratch on the next {@link sync} (store reset, clear or deletions). */
  invalidate(): void {
    this.stale = true;
  }

  /**
   * Bring the counts up to date with the store (`order` ascending). Returns whether they
   * changed. Visits only ids added since the last sync, unless a rebuild is needed.
   */
  sync(order: readonly ExchangeId[], summaries: ReadonlyMap<ExchangeId, ExchangeSummary>): boolean {
    // Where the counted run sits now: unchanged inside means only its ends grew.
    let from = 0;
    let to = 0;
    let incremental = false;
    if (!this.stale) {
      if (this.counted === 0) incremental = true;
      else if (this.firstId != null && this.lastId != null) {
        from = indexOfSorted(order, this.firstId);
        to = indexOfSorted(order, this.lastId) + 1;
        incremental = from >= 0 && to - from === this.counted;
      }
    }
    if (!incremental) {
      this.hosts.clear();
      this.sources.clear();
      this.counted = 0;
      this.stale = false;
      from = to = 0;
    } else if (order.length === this.counted) {
      return false;
    }
    this.add(order, summaries, 0, from);
    this.add(order, summaries, to, order.length);
    this.counted = order.length;
    this.firstId = order.length ? (order[0] as ExchangeId) : null;
    this.lastId = order.length ? (order[order.length - 1] as ExchangeId) : null;
    this.version++;
    return true;
  }

  private add(order: readonly ExchangeId[], summaries: ReadonlyMap<ExchangeId, ExchangeSummary>, start: number, end: number): void {
    for (let i = start; i < end; i++) {
      const s = summaries.get(order[i] as ExchangeId);
      if (!s) continue;
      this.hosts.set(s.host, (this.hosts.get(s.host) ?? 0) + 1);
      const src = s.source ?? '';
      this.sources.set(src, (this.sources.get(src) ?? 0) + 1);
    }
  }
}

// =====================================================================================
// Tree rows
// =====================================================================================

export type SidebarRow =
  | { kind: 'heading'; key: string; label: string; count?: number }
  | { kind: 'domain'; key: string; domain: string; count: number; hosts: number; expanded: boolean; clause: string }
  | { kind: 'host'; key: string; host: string; count: number; depth: 0 | 1; pinned: boolean; clause: string }
  | { kind: 'source'; key: string; source: string; count: number; clause: string }
  | { kind: 'empty'; key: string; label: string };

export type SidebarSort = 'name' | 'count';

/** Filter clause for one host. */
export function hostClause(host: string): string {
  return clause('host', '=', host);
}

/** Filter clause for a registrable domain and every subdomain. */
export function domainClause(domain: string): string {
  return `(${clause('host', '=', domain)} | host:*.${domain})`;
}

/** Filter clause for a client app (`''` = exchanges without one). */
export function sourceClause(source: string): string {
  return source ? clause('source', '=', source) : '-source:*';
}

function byName(a: string, b: string): number {
  return a.replace(/^www\./, '').localeCompare(b.replace(/^www\./, ''));
}

export interface TreeOptions {
  view: 'hosts' | 'apps';
  pinned: readonly string[];
  /** Expanded domains. */
  expanded: ReadonlySet<string>;
  sort: SidebarSort;
  /** Search text (matches hosts / apps, case-insensitive). */
  query?: string;
}

/**
 * The visible rows: pinned hosts first, then domains (a domain with several hosts expands into
 * them; a domain with one host is that host), or the apps.
 */
export function buildRows(index: HostIndex, opts: TreeOptions): SidebarRow[] {
  const q = opts.query?.trim().toLowerCase() ?? '';
  const rows: SidebarRow[] = [];
  const order = <T extends { name: string; count: number }>(list: T[]): T[] => list.sort((a, b) => (opts.sort === 'count' ? b.count - a.count : 0) || byName(a.name, b.name));

  if (opts.view === 'apps') {
    const apps = order([...index.sources].map(([name, count]) => ({ name, count })).filter((a) => !q || (a.name || 'unknown app').toLowerCase().includes(q)));
    rows.push({ kind: 'heading', key: 'h-apps', label: 'Apps', count: apps.length });
    for (const a of apps) rows.push({ kind: 'source', key: `s-${a.name}`, source: a.name, count: a.count, clause: sourceClause(a.name) });
    if (!apps.length) rows.push({ kind: 'empty', key: 'e-apps', label: q ? 'No app matches' : 'No traffic yet' });
    return rows;
  }

  const pinned = opts.pinned.filter((h) => !q || h.includes(q));
  if (pinned.length) {
    rows.push({ kind: 'heading', key: 'h-pinned', label: 'Pinned', count: pinned.length });
    for (const host of pinned) rows.push({ kind: 'host', key: `p-${host}`, host, count: index.hosts.get(host) ?? 0, depth: 0, pinned: true, clause: hostClause(host) });
  }

  const domains = new Map<string, { name: string; count: number; hosts: { name: string; count: number }[] }>();
  for (const [host, count] of index.hosts) {
    if (q && !host.toLowerCase().includes(q)) continue;
    const name = registrableDomain(host);
    const d = domains.get(name) ?? { name, count: 0, hosts: [] };
    d.count += count;
    d.hosts.push({ name: host, count });
    domains.set(name, d);
  }
  const pinnedSet = new Set(opts.pinned);
  const list = order([...domains.values()]);
  rows.push({ kind: 'heading', key: 'h-domains', label: 'Domains', count: list.length });
  for (const d of list) {
    const only = d.hosts.length === 1 ? d.hosts[0] : undefined;
    if (only) {
      rows.push({ kind: 'host', key: `h-${only.name}`, host: only.name, count: only.count, depth: 0, pinned: pinnedSet.has(only.name), clause: hostClause(only.name) });
      continue;
    }
    // Searching opens every matching domain.
    const expanded = q !== '' || opts.expanded.has(d.name);
    rows.push({ kind: 'domain', key: `d-${d.name}`, domain: d.name, count: d.count, hosts: d.hosts.length, expanded, clause: domainClause(d.name) });
    if (!expanded) continue;
    for (const h of order(d.hosts)) rows.push({ kind: 'host', key: `h-${h.name}`, host: h.name, count: h.count, depth: 1, pinned: pinnedSet.has(h.name), clause: hostClause(h.name) });
  }
  if (!list.length) rows.push({ kind: 'empty', key: 'e-domains', label: q ? 'No host matches' : 'No traffic yet' });
  return rows;
}
