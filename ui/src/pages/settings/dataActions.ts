/**
 * Settings → Data actions (export/import configuration, reset UI prefs, clear traffic), shared
 * by the Data section's buttons and the Settings page's palette commands. Busy flags live in
 * a tiny external store so both stay in sync.
 */

import { useSyncExternalStore } from 'react';
import { api } from '../../api/client';
import type { RuleSet, Settings } from '../../api/types';
import { confirm, toast, undoToast } from '../../components/ui';
import { pluralize } from '../../lib/format';
import { pickFiles, saveData } from '../../lib/platform';
import { useAppStore } from '../../state/app';
import { trafficStore } from '../../state/traffic';
import { defaultPrefs, sanitizePrefs, useUiStore } from '../../state/ui';
import { explainError, refreshStatus } from '../connect/actions';
import type { SettingsSync } from './settingsSync';
import { buildConfigExport, configFilename, mergeSettings, omittedPassphrases, parseConfigImport } from './transfer';

type Busy = { importing: boolean; clearing: boolean };
let busy: Busy = { importing: false, clearing: false };
const listeners = new Set<() => void>();

function setBusy(patch: Partial<Busy>): void {
  busy = { ...busy, ...patch };
  for (const l of listeners) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useDataBusy(): Busy {
  return useSyncExternalStore(subscribe, () => busy, () => busy);
}

/** Replace all UI prefs (goes through the store's normal persistence + daemon sync). */
function replaceUiPrefs(prefs: unknown): void {
  useUiStore.setState({ ...sanitizePrefs(prefs), updatedAt: Date.now() });
}

function currentUiPrefs() {
  return sanitizePrefs(useUiStore.getState());
}

export async function exportConfig(sync: SettingsSync<Settings>): Promise<void> {
  const { settings, rules, status } = useAppStore.getState();
  const current = sync.getSnapshot().draft ?? settings;
  const data = buildConfigExport({ settings: current, rules, ui: currentUiPrefs(), appVersion: status?.version });
  const ok = await saveData(JSON.stringify(data, null, 2), configFilename(), 'application/json');
  const secrets = omittedPassphrases(current);
  if (ok) {
    toast.success('Configuration exported', {
      description: secrets > 0 ? `Settings, rules and interface preferences. ${pluralize(secrets, 'client certificate passphrase')} left out on purpose.` : 'Settings, rules and interface preferences.',
    });
  }
}

export async function importConfig(sync: SettingsSync<Settings>): Promise<void> {
  if (busy.importing) return;
  const files = await pickFiles({ accept: ['.json', 'application/json'] });
  const file = files?.[0];
  if (!file) return;
  const parsed = parseConfigImport(new TextDecoder().decode(file.data));
  if (!parsed.ok) {
    toast.error(`Couldn't import ${file.name}`, { description: parsed.error });
    return;
  }
  const ok = await confirm({
    title: `Import ${file.name}?`,
    message: `This replaces your ${parsed.summary.join(', ')}. You can undo right after.`,
    confirmLabel: 'Import',
  });
  if (!ok) return;

  const app = useAppStore.getState();
  const prevSettings: Settings | null = sync.getSnapshot().draft ?? app.settings;
  const prevRules: RuleSet | null = app.rules;
  const prevUi = currentUiPrefs();
  setBusy({ importing: true });
  try {
    const { config } = parsed;
    if (config.settings && prevSettings) {
      const merged = mergeSettings(prevSettings, config.settings);
      await sync.edit(() => merged);
      // A different port or bind address rebinds the proxy: refresh the addresses shown everywhere.
      if (merged.proxy.port !== prevSettings.proxy.port || merged.proxy.bindAddress !== prevSettings.proxy.bindAddress) void refreshStatus();
    }
    if (config.rules) {
      let rules = config.rules;
      // HTTP Toolkit exports are converted by the daemon.
      if (rules.version !== 1) rules = await api.rules.import(rules);
      await app.saveRules(rules);
    }
    if (config.ui) replaceUiPrefs(config.ui);
    undoToast(`Imported ${parsed.summary.join(', ')}`, () => {
      void (async () => {
        try {
          if (config.settings && prevSettings) {
            await sync.edit(() => prevSettings);
            void refreshStatus();
          }
          if (config.rules && prevRules) await useAppStore.getState().saveRules(prevRules);
          if (config.ui) replaceUiPrefs(prevUi);
          toast.success('Import undone');
        } catch (e) {
          toast.error("Couldn't undo the import", { description: explainError(e) });
        }
      })();
    });
  } catch (e) {
    toast.error(`Couldn't import ${file.name}`, { description: explainError(e) });
  } finally {
    setBusy({ importing: false });
  }
}

export function resetUiPrefs(): void {
  const prev = currentUiPrefs();
  replaceUiPrefs(defaultPrefs());
  undoToast('Interface preferences reset', () => replaceUiPrefs(prev), { description: 'Theme, layout, columns, splits and saved filters.' });
}

export async function clearAllTraffic(): Promise<void> {
  if (busy.clearing) return;
  const order = trafficStore.getSnapshot().order;
  let pinned = 0;
  for (const id of order) if (trafficStore.get(id)?.pinned) pinned++;
  const ok = await confirm({
    title: `Delete ${pluralize(order.length, 'exchange')}?`,
    message: pinned > 0 ? `${pluralize(pinned, 'pinned exchange')} will be kept. This can't be undone.` : "This can't be undone. Export a HAR first if you might need them.",
    destructive: true,
    confirmLabel: 'Delete traffic',
  });
  if (!ok) return;
  setBusy({ clearing: true });
  try {
    await api.exchanges.clear({ keepPinned: pinned > 0 });
    toast.success(pinned > 0 ? `Traffic cleared — kept ${pluralize(pinned, 'pinned exchange')}` : 'Traffic cleared');
  } catch (e) {
    toast.error("Couldn't clear traffic", { description: explainError(e) });
  } finally {
    setBusy({ clearing: false });
  }
}
