/**
 * Theme, density and UI-scale application.
 *
 * `data-theme` (dark | light | midnight | contrast), `data-density` and `--ui-scale` live on
 * `<html>`. `system` follows `prefers-color-scheme`. A `?theme=<name>` URL parameter overrides
 * the preference for the session without persisting (handy for screenshots and reviews).
 * `public/theme-boot.js` applies the same logic before first paint.
 */

import { useEffect, useSyncExternalStore } from 'react';
import { useUiStore, type ResolvedTheme, type ThemePref } from '../state/ui';

const VALID: readonly ResolvedTheme[] = ['dark', 'light', 'midnight', 'contrast'];

function readUrlOverride(): ResolvedTheme | null {
  try {
    const t = new URLSearchParams(window.location.search).get('theme');
    return VALID.includes(t as ResolvedTheme) ? (t as ResolvedTheme) : null;
  } catch {
    return null;
  }
}

/** Session-only override from `?theme=`. */
export const urlThemeOverride: ResolvedTheme | null = readUrlOverride();

const media = window.matchMedia('(prefers-color-scheme: light)');

function subscribeSystem(cb: () => void): () => void {
  media.addEventListener('change', cb);
  return () => media.removeEventListener('change', cb);
}

function systemPrefersLight(): boolean {
  return media.matches;
}

/** Resolve a preference to a concrete theme. */
export function resolveTheme(pref: ThemePref, prefersLight: boolean): ResolvedTheme {
  if (pref === 'system') return prefersLight ? 'light' : 'dark';
  return pref;
}

/** The concrete theme currently applied. */
export function useResolvedTheme(): ResolvedTheme {
  const pref = useUiStore((s) => s.theme);
  const prefersLight = useSyncExternalStore(subscribeSystem, systemPrefersLight, () => false);
  return urlThemeOverride ?? resolveTheme(pref, prefersLight);
}

const THEME_COLORS: Record<ResolvedTheme, string> = {
  dark: '#0a0b0e',
  midnight: '#000000',
  contrast: '#000000',
  light: '#eceef2',
};

/** Apply theme/density/scale to `<html>`. Call once from the app shell. */
export function useApplyTheme(): void {
  const theme = useResolvedTheme();
  const density = useUiStore((s) => s.density);
  const scale = useUiStore((s) => s.scale);

  useEffect(() => {
    const root = document.documentElement;
    root.dataset.theme = theme;
    root.style.colorScheme = theme === 'light' ? 'light' : 'dark';
    const meta = document.querySelector<HTMLMetaElement>('meta[name="theme-color"]');
    if (meta) meta.content = THEME_COLORS[theme];
  }, [theme]);

  useEffect(() => {
    document.documentElement.dataset.density = density;
  }, [density]);

  useEffect(() => {
    document.documentElement.style.setProperty('--ui-scale', String(scale));
  }, [scale]);
}
