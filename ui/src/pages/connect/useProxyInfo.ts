import { useMemo, useSyncExternalStore } from 'react';
import type { Platform } from '../../api/types';
import { toast } from '../../components/ui';
import { proxyAddress, useAppStore } from '../../state/app';
import { explainError, setLanAccess } from './actions';
import { deviceSetupUrl, isLanBind, lanAddresses } from './addresses';
import { panelKindFor } from './catalog';
import { parseMetadata, type TrustInstructions } from './metadata';

export interface CaFiles {
  /** The CA certificate (PEM) on this machine, when the daemon reports it. */
  caPath: string | null;
  /** System roots + Akev CA bundle. */
  bundlePath: string | null;
  /** Trust instructions for this OS (exact paths, install + remove). */
  trust: TrustInstructions | null;
}

/**
 * Where the daemon keeps the CA and how to trust it on this OS — read from the Manual setup
 * and System proxy interceptors' metadata (no dedicated endpoint exists).
 */
export function useCaFiles(): CaFiles {
  const interceptors = useAppStore((s) => s.interceptors);
  return useMemo(() => {
    let caPath: string | null = null;
    let bundlePath: string | null = null;
    let trust: TrustInstructions | null = null;
    for (const i of interceptors) {
      const kind = panelKindFor(i);
      if (kind !== 'manual' && kind !== 'system-proxy') continue;
      const meta = parseMetadata(i.metadata);
      caPath ??= meta.caPath ?? null;
      bundlePath ??= meta.bundlePath ?? null;
      trust ??= meta.trust;
    }
    return { caPath, bundlePath, trust };
  }, [interceptors]);
}

export interface ProxyInfo {
  /** `127.0.0.1:8000`, null before the first `hello`. */
  local: string | null;
  port: number | null;
  bindAddress: string | null;
  /** Non-loopback addresses (best first), regardless of the bind setting. */
  lan: string[];
  /** The proxy accepts connections from other devices. */
  lanEnabled: boolean;
  /** Best LAN address when LAN access is enabled. */
  primaryLan: string | null;
  /** Device setup page (`http://<lan>:<port>/`) when LAN access is enabled. */
  setupUrl: string | null;
  platform: Platform | null;
}

/**
 * Proxy addresses for the Connect page & panels. Selects nested objects (not `status`, which
 * is replaced every 2 s by stats updates) so consumers re-render only when they change.
 */
export function useProxyInfo(): ProxyInfo {
  const proxy = useAppStore((s) => s.status?.proxy ?? null);
  const platform = useAppStore((s) => s.status?.platform ?? null);
  const settingsBind = useAppStore((s) => s.settings?.proxy.bindAddress ?? null);
  return useMemo(() => {
    const status = proxy ? { proxy } : null;
    const bindAddress = settingsBind ?? proxy?.bindAddress ?? null;
    const lan = lanAddresses(status);
    const lanEnabled = isLanBind(bindAddress);
    const primaryLan = lanEnabled ? (lan[0] ?? null) : null;
    return {
      local: proxy ? proxyAddress({ proxy } as Parameters<typeof proxyAddress>[0]) : null,
      port: proxy?.port ?? null,
      bindAddress,
      lan,
      lanEnabled,
      primaryLan,
      setupUrl: primaryLan ? deviceSetupUrl(primaryLan) : null,
      platform,
    };
  }, [proxy, platform, settingsBind]);
}

// Optimistic "Allow other devices" value while the rebind is in flight — module-level so the
// Proxy card, the device setup card and every panel agree (one toggle, many switches).
let optimisticLan: boolean | null = null;
const lanListeners = new Set<() => void>();

function setOptimisticLan(value: boolean | null): void {
  optimisticLan = value;
  for (const l of lanListeners) l();
}

function subscribeLan(l: () => void): () => void {
  lanListeners.add(l);
  return () => lanListeners.delete(l);
}

/** Allow / disallow LAN connections with optimistic state, toasts and undo. */
export function toggleLanAccess(on: boolean): void {
  if (optimisticLan != null) return;
  setOptimisticLan(on);
  setLanAccess(on).then(
    () => {
      setOptimisticLan(null);
      const lan = lanAddresses(useAppStore.getState().status)[0];
      toast.success(on ? 'Other devices can now connect' : 'Only this computer can connect', {
        description: on ? (lan ? `Proxy reachable at ${lan}` : 'The proxy now listens on all network interfaces.') : 'The proxy listens on 127.0.0.1 only.',
        action: { label: 'Undo', onClick: () => toggleLanAccess(!on) },
      });
    },
    (e: unknown) => {
      setOptimisticLan(null);
      toast.error("Couldn't change network access", { description: explainError(e), action: { label: 'Retry', onClick: () => toggleLanAccess(on) } });
    },
  );
}

/** "Allow other devices" switch state with optimistic toggling, toasts and undo. */
export function useLanToggle(): { checked: boolean; pending: boolean; toggle: (on: boolean) => void; disabled: boolean } {
  const { lanEnabled } = useProxyInfo();
  const hasSettings = useAppStore((s) => s.settings != null);
  const optimistic = useSyncExternalStore(subscribeLan, () => optimisticLan, () => null);
  return { checked: optimistic ?? lanEnabled, pending: optimistic != null, toggle: toggleLanAccess, disabled: !hasSettings || optimistic != null };
}
