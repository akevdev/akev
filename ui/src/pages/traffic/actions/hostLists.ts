/**
 * Proxy host-list settings changed from the Traffic page (TLS passthrough, ignore certificate
 * errors), e.g. from an error banner or a TLS-failure layout.
 */

import { api, errorMessage } from '../../../api/client';
import type { Settings } from '../../../api/types';
import { toast } from '../../../components/ui';
import { useAppStore } from '../../../state/app';

type HostListKey = 'tlsPassthrough' | 'ignoreUpstreamCertErrors';

const HOST_LIST_LABEL: Record<HostListKey, string> = {
  tlsPassthrough: 'TLS passthrough',
  ignoreUpstreamCertErrors: 'Ignore certificate errors',
};

/** Add or remove a host in a proxy host list setting. */
export async function setHostListed(key: HostListKey, host: string, listed: boolean): Promise<boolean> {
  const store = useAppStore.getState();
  let settings: Settings | null = store.settings;
  try {
    settings ??= await api.settings.get();
    const list = settings.proxy[key];
    const has = list.some((h) => h.toLowerCase() === host.toLowerCase());
    if (has === listed) return true;
    const next = listed ? [...list, host] : list.filter((h) => h.toLowerCase() !== host.toLowerCase());
    await store.saveSettings({ ...settings, proxy: { ...settings.proxy, [key]: next } });
    toast.success(listed ? `Added ${host} to ${HOST_LIST_LABEL[key]}` : `Removed ${host} from ${HOST_LIST_LABEL[key]}`, {
      description: key === 'tlsPassthrough' ? (listed ? 'New connections to it are tunnelled without interception.' : 'New connections to it will be intercepted.') : undefined,
      action: { label: 'Undo', onClick: () => void setHostListed(key, host, !listed) },
    });
    return true;
  } catch (e) {
    toast.error("Couldn't update settings", { description: errorMessage(e) });
    return false;
  }
}

/** Whether a host is in a proxy host list (glob-free exact match, case-insensitive). */
export function isHostListed(settings: Settings | null, key: HostListKey, host: string): boolean {
  return settings?.proxy[key].some((h) => h.toLowerCase() === host.toLowerCase()) ?? false;
}
