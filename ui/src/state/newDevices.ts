/**
 * `client-connected`: a device on another machine connected to the proxy for the first time
 * this run. Shown as a toast with a Block action that adds it to `proxy.access.block`.
 */

import { errorMessage } from '../api/client';
import type { Settings } from '../api/types';
import { toast } from '../components/ui/toast';
import { useAppStore } from './app';

/** Settings with `address` added to the blocked clients. */
export function withBlockedClient(settings: Settings, address: string): Settings {
  const { access } = settings.proxy;
  if (access.block.includes(address)) return settings;
  return { ...settings, proxy: { ...settings.proxy, access: { ...access, block: [...access.block, address] } } };
}

export async function blockClient(address: string): Promise<void> {
  const { settings, saveSettings } = useAppStore.getState();
  if (!settings) return;
  try {
    await saveSettings(withBlockedClient(settings, address));
    toast.success(`Blocked ${address}`, { description: 'Unblock it in Settings > Proxy > Who can connect.' });
  } catch (e) {
    toast.error(`Could not block ${address}`, { description: errorMessage(e) });
  }
}

export function announceNewDevice(address: string): void {
  toast.info(`New device connected: ${address}`, {
    description: "It is using the proxy. Block it if you don't recognise it.",
    duration: 15_000,
    action: { label: 'Block', onClick: () => void blockClient(address) },
  });
}
