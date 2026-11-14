import { createContext, useContext, useSyncExternalStore } from 'react';
import type { BreakpointSettings, SessionSettings, Settings, UpdateSettings } from '../../api/types';
import type { EditOptions, SettingsSync, SyncSnapshot } from './settingsSync';

export const SettingsSyncContext = createContext<SettingsSync<Settings> | null>(null);

export function useSettingsSync(): SettingsSync<Settings> {
  const sync = useContext(SettingsSyncContext);
  if (!sync) throw new Error('useSettingsSync must be used inside the Settings page');
  return sync;
}

/** Current draft + save phase (re-renders on every draft change). */
export function useSettingsSnapshot(): SyncSnapshot<Settings> {
  const sync = useSettingsSync();
  return useSyncExternalStore(sync.subscribe, sync.getSnapshot, sync.getSnapshot);
}

/** The draft settings (what controls show). `null` until loaded. */
export function useDraft(): Settings | null {
  return useSettingsSnapshot().draft;
}

export type Mutator = (draft: Settings) => Settings;

/** `edit(mutator, { debounce })` bound to the page's sync controller. */
export function useEdit(): (mutate: Mutator, options?: EditOptions) => Promise<Settings> {
  const sync = useSettingsSync();
  return (mutate, options) => sync.edit(mutate, options);
}

/** Helpers producing mutators for nested settings. */
export const setProxy =
  <K extends keyof Settings['proxy']>(key: K, value: Settings['proxy'][K]): Mutator =>
  (d) => ({ ...d, proxy: { ...d.proxy, [key]: value } });

export const setCapture =
  <K extends keyof Settings['capture']>(key: K, value: Settings['capture'][K]): Mutator =>
  (d) => ({ ...d, capture: { ...d.capture, [key]: value } });

export const setSessions =
  <K extends keyof SessionSettings>(key: K, value: SessionSettings[K]): Mutator =>
  (d) => ({ ...d, sessions: { ...d.sessions, [key]: value } });

export const setUpdates =
  <K extends keyof UpdateSettings>(key: K, value: UpdateSettings[K]): Mutator =>
  (d) => ({ ...d, updates: { ...d.updates, [key]: value } });

export const setBreakpoints =
  <K extends keyof BreakpointSettings>(key: K, value: BreakpointSettings[K]): Mutator =>
  (d) => ({ ...d, breakpoints: { ...d.breakpoints, [key]: value } });

export const setUpstream =
  (patch: Partial<Settings['proxy']['upstreamProxy']>): Mutator =>
  (d) => ({ ...d, proxy: { ...d.proxy, upstreamProxy: { ...d.proxy.upstreamProxy, ...patch } } });
