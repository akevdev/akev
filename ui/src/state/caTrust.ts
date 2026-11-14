/**
 * Whether the operating system trusts the Akev CA (`GET /api/ca/trust`), shared by the status
 * bar and Settings → Certificates. Refreshed when the window regains focus: the user may have
 * changed the trust store outside Akev.
 */
import { create } from 'zustand';
import { api, ApiError } from '../api/client';
import type { TrustReport, TrustStore } from '../api/types';
import { confirm } from '../components/ui/confirm';
import { toast } from '../components/ui/toast';

interface CaTrustState {
  report: TrustReport | null;
  /** Store id being changed. */
  busy: string | null;
  refresh: () => Promise<void>;
  install: (store: string) => Promise<TrustReport>;
  remove: (store: string) => Promise<TrustReport>;
}

let inflight: Promise<void> | null = null;

export const useCaTrust = create<CaTrustState>((set) => ({
  report: null,
  busy: null,
  refresh: () => {
    inflight ??= api.ca
      .trust()
      .then(
        (report) => set({ report }),
        () => undefined,
      )
      .finally(() => {
        inflight = null;
      });
    return inflight;
  },
  install: async (store) => {
    set({ busy: store });
    try {
      const report = await api.ca.trustInstall(store);
      set({ report });
      return report;
    } finally {
      set({ busy: null });
    }
  },
  remove: async (store) => {
    set({ busy: store });
    try {
      const report = await api.ca.trustRemove(store);
      set({ report });
      return report;
    } finally {
      set({ busy: null });
    }
  },
}));

function message(e: unknown): string {
  return e instanceof ApiError || e instanceof Error ? e.message : String(e);
}

/**
 * Ask, then add the Akev CA to `store` (the operating system asks to confirm too). Shared by
 * Settings → Certificates and the first-run setup. Resolves whether the store trusts it now.
 */
export async function trustWithConfirm(store: TrustStore): Promise<boolean> {
  const ok = await confirm({
    title: `Trust the Akev certificate in “${store.label}”?`,
    message: `${store.description} will accept HTTPS certificates that Akev makes, so Akev can show their traffic. Only this computer is affected, and you can remove it in Settings → Certificates at any time.${store.needsAdmin ? ' Your system will ask for your password.' : ' Your system will ask you to confirm.'}`,
    confirmLabel: 'Trust',
  });
  if (!ok) return false;
  try {
    const after = await useCaTrust.getState().install(store.id);
    const now = after.stores.find((s) => s.id === store.id);
    if (now?.trusted === false) {
      toast.warning("The certificate was added, but the store still doesn't trust it. Restart the apps you use, or check the system's certificate settings.");
      return false;
    }
    toast.success('The Akev certificate is trusted', { description: 'Restart apps that were already open so they pick it up.' });
    return true;
  } catch (e) {
    toast.error("Couldn't trust the certificate", { description: message(e) });
    return false;
  }
}

/** Ask, then remove the Akev CA from `store`. */
export async function untrustWithConfirm(store: TrustStore): Promise<void> {
  const ok = await confirm({
    title: `Remove the Akev certificate from “${store.label}”?`,
    message: "Apps using this store will stop accepting Akev's certificates. Clients Akev launches itself keep working.",
    destructive: true,
    confirmLabel: 'Remove',
  });
  if (!ok) return;
  try {
    await useCaTrust.getState().remove(store.id);
    toast.success('The Akev certificate was removed');
  } catch (e) {
    toast.error("Couldn't remove the certificate", { description: message(e) });
  }
}

/** Load the report once, and again whenever the window regains focus. */
export function watchCaTrust(): () => void {
  const refresh = () => void useCaTrust.getState().refresh();
  refresh();
  window.addEventListener('focus', refresh);
  return () => window.removeEventListener('focus', refresh);
}
