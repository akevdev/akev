/**
 * Interceptor catalogue logic for the Connect page: categories, which configuration panel
 * an interceptor gets, card status/primary action, filtering and search ranking.
 *
 * The daemon owns the list (ids: chrome, chromium, brave, edge, opera, vivaldi, arc, firefox,
 * firefox-developer, firefox-nightly, fresh-terminal, existing-terminal, android-adb, docker,
 * system-proxy, electron, manual, ios — the mock adds a few aliases). Everything here is
 * defensive: unknown ids fall back to a generic card/panel.
 */

import type { InterceptorCategory, InterceptorInfo, Platform } from '../../api/types';
import { rankByQuery, type SearchField } from './search';

export type CategoryFilter = 'all' | InterceptorCategory;

export const CATEGORY_FILTERS: readonly { id: CategoryFilter; label: string }[] = [
  { id: 'all', label: 'All' },
  { id: 'browser', label: 'Browsers' },
  { id: 'terminal', label: 'Terminal' },
  { id: 'runtime', label: 'Runtimes' },
  { id: 'mobile', label: 'Mobile' },
  { id: 'container', label: 'Containers' },
  { id: 'system', label: 'System' },
  { id: 'manual', label: 'Manual' },
];

/** Singular label shown on cards / in search. */
export const CATEGORY_LABEL: Record<InterceptorCategory, string> = {
  browser: 'Browser',
  terminal: 'Terminal',
  runtime: 'Runtime',
  mobile: 'Mobile',
  container: 'Container',
  system: 'System',
  manual: 'Manual',
};

/** Display order of categories in the "All" grid. */
const CATEGORY_ORDER: readonly InterceptorCategory[] = ['browser', 'terminal', 'mobile', 'container', 'runtime', 'system', 'manual'];

/** Words people search for that the daemon's tags may not include. */
const CATEGORY_KEYWORDS: Record<InterceptorCategory, string[]> = {
  browser: ['browser', 'browsers', 'web', 'website', 'pwa'],
  terminal: ['terminal', 'shell', 'cli', 'command line', 'console', 'bash', 'zsh'],
  runtime: ['runtime', 'app', 'desktop app', 'process'],
  mobile: ['mobile', 'phone', 'device', 'tablet', 'app'],
  container: ['container', 'containers', 'docker', 'podman', 'compose'],
  system: ['system', 'everything', 'whole machine', 'global', 'os'],
  manual: ['manual', 'anything', 'other', 'custom', 'env vars', 'http_proxy'],
};

const ID_KEYWORDS: Record<string, string[]> = {
  'android-adb': ['android', 'adb', 'emulator', 'pixel', 'samsung', 'galaxy', 'phone', 'usb'],
  ios: ['ios', 'iphone', 'ipad', 'apple', 'safari mobile'],
  'ios-manual': ['ios', 'iphone', 'ipad', 'apple'],
  'ios-simulator': ['simulator', 'xcode', 'ios', 'iphone', 'ipad', 'apple', 'swift'],
  docker: ['docker', 'container', 'compose', 'podman', 'kubernetes'],
  electron: ['electron', 'slack', 'vscode', 'vs code', 'discord', 'postman', 'figma', 'desktop app'],
  'system-proxy': ['system proxy', 'everything', 'whole machine', 'global', 'os proxy', 'wifi'],
  manual: ['manual', 'anything', 'curl', 'node', 'python', 'java', 'go', 'rust', 'env', 'http_proxy', 'certificate'],
  'manual-setup': ['manual', 'anything', 'curl', 'node', 'python', 'java', 'go', 'rust', 'env', 'http_proxy', 'certificate'],
  'existing-terminal': ['existing', 'current shell', 'one-liner', 'eval', 'bash', 'zsh', 'fish', 'powershell', 'cmd'],
  'fresh-terminal': ['new terminal', 'shell', 'curl', 'node', 'python', 'npm'],
  jvm: ['java', 'jvm', 'kotlin', 'scala', 'spring', 'gradle', 'maven'],
  'transparent-linux': ['transparent', 'ignores proxy', 'no proxy support', 'iptables', 'nftables', 'redirect', 'per app', 'cgroup', 'firewall', 'linux'],
  'firefox-developer': ['developer edition', 'dev'],
  'firefox-nightly': ['nightly'],
  wireguard: ['wireguard', 'vpn', 'tunnel', 'qr code', 'iphone', 'ipad', 'ios', 'android', 'phone', 'tablet', 'whole device', 'every app'],
};

/** Configuration panel shown in the Sheet. `null` = one-click interceptor (no panel). */
export type PanelKind = 'android' | 'docker' | 'terminal' | 'electron' | 'manual' | 'ios' | 'system-proxy' | 'jvm' | 'wireguard' | 'transparent' | 'generic';

const PANEL_BY_ID: Record<string, PanelKind> = {
  'android-adb': 'android',
  android: 'android',
  docker: 'docker',
  'docker-attach': 'docker',
  'existing-terminal': 'terminal',
  electron: 'electron',
  manual: 'manual',
  'manual-setup': 'manual',
  ios: 'ios',
  'ios-manual': 'ios',
  'system-proxy': 'system-proxy',
  jvm: 'jvm',
  'attach-jvm': 'jvm',
  wireguard: 'wireguard',
  'transparent-linux': 'transparent',
};

/** Interceptors Akev can't stop by itself (e.g. firewall rules need root): "Stop" opens their panel. */
const STOPPED_FROM_PANEL: ReadonlySet<string> = new Set(['transparent-linux']);

/** Whether stopping `i` means following instructions in its panel rather than an API call. */
export function stopsFromPanel(i: Pick<InterceptorInfo, 'id'>): boolean {
  return STOPPED_FROM_PANEL.has(i.id);
}

/** Which tailored panel (if any) an interceptor opens. */
export function panelKindFor(i: Pick<InterceptorInfo, 'id' | 'needsConfig'>): PanelKind | null {
  return PANEL_BY_ID[i.id] ?? (i.needsConfig ? 'generic' : null);
}

/** Whether clicking the card's primary action opens the configuration Sheet. */
export function opensSheet(i: Pick<InterceptorInfo, 'id' | 'needsConfig'>): boolean {
  return panelKindFor(i) != null;
}

export interface PrimaryAction {
  kind: 'launch' | 'activate' | 'deactivate' | 'setup' | 'manage';
  label: string;
}

/** Label/kind of a card's primary button. */
export function primaryActionFor(i: Pick<InterceptorInfo, 'id' | 'needsConfig' | 'category' | 'active'>): PrimaryAction {
  if (opensSheet(i)) return i.active ? { kind: 'manage', label: 'Manage' } : { kind: 'setup', label: 'Set up' };
  if (i.category === 'browser' || i.category === 'terminal' || i.category === 'runtime') return { kind: 'launch', label: 'Launch' };
  return i.active ? { kind: 'deactivate', label: 'Deactivate' } : { kind: 'activate', label: 'Activate' };
}

export type StatusTone = 'active' | 'ready' | 'missing' | 'unsupported';

export interface CardStatus {
  tone: StatusTone;
  label: string;
  /** Longer explanation (reason / full version) for the tooltip. */
  tooltip?: string;
}

export const PLATFORM_NAME: Record<Platform, string> = { linux: 'Linux', macos: 'macOS', windows: 'Windows' };

/** `131.0.6778.86` → `131.0`; short versions pass through. */
export function shortVersion(version: string): string {
  const m = /^(\d+)\.(\d+)/.exec(version.trim());
  return m ? `${m[1]}.${m[2]}` : version.trim().slice(0, 16);
}

/** Card status line (✓ Available · 131.0 / ⚠ Not installed / ● Active / Not supported). */
export function statusOf(i: Pick<InterceptorInfo, 'active' | 'supported' | 'available' | 'version' | 'unavailableReason'>, platform?: Platform | null): CardStatus {
  if (i.active) return { tone: 'active', label: 'Active', tooltip: i.version ? `Version ${i.version}` : undefined };
  if (!i.supported) {
    return {
      tone: 'unsupported',
      label: platform ? `Not on ${PLATFORM_NAME[platform]}` : 'Not supported',
      tooltip: i.unavailableReason ?? (platform ? `Not supported on ${PLATFORM_NAME[platform]}` : 'Not supported on this platform'),
    };
  }
  if (!i.available) {
    const reason = i.unavailableReason ?? '';
    const label = /not (installed|found)|could not find|no .* found|missing/i.test(reason)
      ? 'Not installed'
      : /not running|is it running|daemon/i.test(reason)
        ? 'Not running'
        : 'Unavailable';
    return { tone: 'missing', label, tooltip: reason || undefined };
  }
  if (i.version) return { tone: 'ready', label: `Available · ${shortVersion(i.version)}`, tooltip: `Version ${i.version}` };
  return { tone: 'ready', label: 'Available' };
}

/** Whether the primary action can be used right now. Panels open even when unavailable (they explain why). */
export function canUsePrimary(i: Pick<InterceptorInfo, 'id' | 'needsConfig' | 'supported' | 'available' | 'active'>): boolean {
  if (opensSheet(i)) return i.supported;
  return i.supported && (i.available || i.active);
}

/** Within a category: usable first, browsers that aren't installed next, unsupported last. */
function availabilityRank(i: Pick<InterceptorInfo, 'supported' | 'category' | 'available' | 'active'>): number {
  if (!i.supported) return 2;
  // Browser installs rarely change, so this never reshuffles cards while you work; other
  // categories' availability flips (a phone is plugged in), so they keep daemon order.
  return i.category === 'browser' && !i.available && !i.active ? 1 : 0;
}

/**
 * Interceptors that can't be used on this computer for a lasting reason (browser not
 * installed, not supported on this OS). The "All" view tucks them away behind a disclosure.
 */
export function isDormant(i: Pick<InterceptorInfo, 'supported' | 'category' | 'available' | 'active'>): boolean {
  return availabilityRank(i) > 0;
}

/** Split the "All" view into usable cards and dormant ones (order kept). */
export function splitDormant<T extends Pick<InterceptorInfo, 'supported' | 'category' | 'available' | 'active'>>(list: readonly T[]): { main: T[]; dormant: T[] } {
  const main: T[] = [];
  const dormant: T[] = [];
  for (const i of list) (isDormant(i) ? dormant : main).push(i);
  return { main, dormant };
}

/**
 * Stable catalogue order: by category, then availability (see {@link availabilityRank}), then
 * daemon order. Activation state is never used, so launching something doesn't move cards.
 */
export function sortCatalog(list: readonly InterceptorInfo[]): InterceptorInfo[] {
  return list
    .map((item, index) => ({ item, index, rank: availabilityRank(item) }))
    .sort((a, b) => {
      const ca = CATEGORY_ORDER.indexOf(a.item.category);
      const cb = CATEGORY_ORDER.indexOf(b.item.category);
      return (ca < 0 ? 99 : ca) - (cb < 0 ? 99 : cb) || a.rank - b.rank || a.index - b.index;
    })
    .map((x) => x.item);
}

/** Daemon icon keys AppIcon doesn't know by that name. */
const ICON_ALIASES: Record<string, string> = {
  desktop: 'globe',
  ios: 'apple',
  'firefox-developer': 'firefox',
  'firefox-nightly': 'firefox',
};

/** AppIcon key for an interceptor's `icon`. */
export function iconKey(icon: string): string {
  return ICON_ALIASES[icon] ?? icon;
}

/** Search fields of an interceptor (name first). */
export function searchFieldsOf(i: InterceptorInfo): SearchField[] {
  return [
    { text: i.name, weight: 1 },
    { text: i.id.replace(/-/g, ' '), weight: 0.9 },
    { text: i.tags.join(' '), weight: 0.8 },
    { text: [...(ID_KEYWORDS[i.id] ?? []), ...(CATEGORY_KEYWORDS[i.category] ?? [])].join(' '), weight: 0.7 },
    { text: CATEGORY_LABEL[i.category] ?? '', weight: 0.7 },
    { text: i.description, weight: 0.5 },
  ];
}

/** Category + fuzzy query filter. Without a query the catalogue order is kept. */
export function filterCatalog(list: readonly InterceptorInfo[], category: CategoryFilter, query: string): InterceptorInfo[] {
  const inCategory = category === 'all' ? list : list.filter((i) => i.category === category);
  return rankByQuery(inCategory, query, searchFieldsOf, (i) => (i.active ? 2 : i.available ? 1 : 0));
}

/** Number of interceptors per category filter (for chip counts). */
export function countByCategory(list: readonly InterceptorInfo[]): Record<CategoryFilter, number> {
  const counts = { all: list.length } as Record<CategoryFilter, number>;
  for (const c of CATEGORY_FILTERS) if (c.id !== 'all') counts[c.id] = 0;
  for (const i of list) counts[i.category] = (counts[i.category] ?? 0) + 1;
  return counts;
}

/** The "works with anything" fallback interceptor, if the daemon provides one. */
export function manualInterceptor(list: readonly InterceptorInfo[]): InterceptorInfo | undefined {
  return list.find((i) => panelKindFor(i) === 'manual');
}
