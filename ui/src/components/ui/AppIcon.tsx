import clsx from 'clsx';
import { useId, useSyncExternalStore, type CSSProperties } from 'react';
import styles from './AppIcon.module.css';

/** Icon keys understood by {@link AppIcon} (interceptor `icon` values and detected sources). */
export type AppIconName =
  | 'chrome'
  | 'chromium'
  | 'firefox'
  | 'edge'
  | 'brave'
  | 'opera'
  | 'vivaldi'
  | 'safari'
  | 'arc'
  | 'terminal'
  | 'node'
  | 'python'
  | 'curl'
  | 'android'
  | 'apple'
  | 'docker'
  | 'java'
  | 'go'
  | 'rust'
  | 'electron'
  | 'globe'
  | 'linux'
  | 'manual'
  | 'akev'
  | 'wireguard'
  | 'app';

/** Brand-ish tint per icon (the tile background). The artwork loads separately. */
const TILE_COLORS: Record<AppIconName, string> = {
  chrome: '#1da462',
  chromium: '#4489f4',
  firefox: '#e66000',
  edge: '#1b8ad6',
  brave: '#fb542b',
  opera: '#ff1b2d',
  vivaldi: '#ef3939',
  safari: '#1a8cff',
  arc: '#ff536a',
  terminal: '#20c20e',
  node: '#5fa04e',
  python: '#3776ab',
  curl: '#1f9bb0',
  android: '#3ddc84',
  apple: '#8b93a7',
  docker: '#2496ed',
  java: '#e76f00',
  go: '#00add8',
  rust: '#e3784b',
  electron: '#47848f',
  globe: '#6b8afd',
  linux: '#f2b705',
  manual: '#a0a6b3',
  akev: '#7556ff',
  wireguard: '#88171a',
  app: '#8b93a7',
};

type Art = typeof import('./appIconArt')['ICON_ART'];

let art: Art | null = null;
const artListeners = new Set<() => void>();
// Start loading the artwork as soon as the app starts; icons render empty (same size) until it
// arrives, a moment after the first paint.
void import('./appIconArt').then((m) => {
  art = m.ICON_ART;
  for (const l of artListeners) l();
});

function subscribeArt(l: () => void): () => void {
  artListeners.add(l);
  return () => artListeners.delete(l);
}

/** All icon names (kitchen sink, pickers). */
export const APP_ICON_NAMES = Object.keys(TILE_COLORS) as AppIconName[];

const ALIASES: [RegExp, AppIconName][] = [
  [/chromium/i, 'chromium'],
  [/chrome/i, 'chrome'],
  [/firefox|gecko|librewolf|waterfox/i, 'firefox'],
  [/edge|msedge/i, 'edge'],
  [/brave/i, 'brave'],
  [/opera/i, 'opera'],
  [/vivaldi/i, 'vivaldi'],
  [/safari|webkit/i, 'safari'],
  [/^arc$|arc browser/i, 'arc'],
  [/electron|vscode|slack|discord|code helper/i, 'electron'],
  [/terminal|bash|zsh|fish|shell|powershell|pwsh|cmd\.exe|iterm|konsole|alacritty|kitty|wezterm/i, 'terminal'],
  [/node|deno|bun|npm|yarn|pnpm|axios|got\b/i, 'node'],
  [/python|pip|httpx|requests|urllib|aiohttp/i, 'python'],
  [/curl|wget|httpie/i, 'curl'],
  [/android|okhttp|adb|dalvik/i, 'android'],
  [/ios|iphone|ipad|darwin|macos|apple|cfnetwork/i, 'apple'],
  [/docker|container|podman/i, 'docker'],
  [/java|jvm|kotlin|gradle|maven|intellij/i, 'java'],
  [/^go$|golang|go-http/i, 'go'],
  [/rust|reqwest|cargo|hyper/i, 'rust'],
  [/linux|tux|nftables|iptables|transparent/i, 'linux'],
  [/system|proxy|network|globe|desktop/i, 'globe'],
  [/manual|setup/i, 'manual'],
  [/akev|composer/i, 'akev'],
  [/wireguard/i, 'wireguard'],
];

/** Best icon for an interceptor icon key or a detected process/source name. */
export function appIconFor(nameOrKey: string | null | undefined): AppIconName {
  if (!nameOrKey) return 'app';
  const key = nameOrKey.toLowerCase().trim();
  if (key in TILE_COLORS) return key as AppIconName;
  for (const [re, icon] of ALIASES) if (re.test(key)) return icon;
  return 'app';
}

export interface AppIconProps {
  /** Icon key or any app/process name (resolved with {@link appIconFor}). */
  name: AppIconName | string | null | undefined;
  size?: number;
  /** Render on a softly tinted rounded tile (Connect cards). */
  tile?: boolean;
  className?: string;
  /** Accessible label; omit when the name is shown next to the icon. */
  label?: string;
}

/** Inline-SVG app/interceptor/process icon. */
export function AppIcon({ name, size = 16, tile, className, label }: AppIconProps) {
  const key = appIconFor(name);
  const loaded = useSyncExternalStore(subscribeArt, () => art, () => art);
  const uid = `ai${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`;
  const svg = (
    <svg
      width={tile ? Math.round(size * 0.62) : size}
      height={tile ? Math.round(size * 0.62) : size}
      viewBox="0 0 24 24"
      role={label ? 'img' : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
      className={clsx(!tile && styles.icon, !tile && className)}
    >
      {loaded?.[key].render(uid)}
    </svg>
  );
  if (!tile) return svg;
  return (
    <span className={clsx(styles.tile, className)} style={{ '--tile-color': TILE_COLORS[key], width: size, height: size } as CSSProperties}>
      {svg}
    </span>
  );
}
