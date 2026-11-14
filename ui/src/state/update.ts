/**
 * New versions of Akev (`/api/update`). The daemon only contacts the release server when the
 * UI asks: once on connecting (and daily after that) while automatic checks are on, or when
 * the user presses "Check now".
 */
import { create } from 'zustand';
import { api, errorMessage } from '../api/client';
import type { UpdateInfo } from '../api/types';
import { toast } from '../components/ui';
import { openExternal, tauriBridge } from '../lib/platform';

const DISMISSED_KEY = 'akev.update.dismissed';
const DAY = 24 * 60 * 60 * 1000;

interface UpdateState {
  info: UpdateInfo | null;
  checking: boolean;
  installing: boolean;
  /** `GET /api/update`. */
  load: () => Promise<UpdateInfo | null>;
  /** `POST /api/update/check`. */
  checkNow: () => Promise<UpdateInfo | null>;
  /** Desktop: install and restart. Browser: open the download page. */
  install: () => Promise<void>;
}

export const useUpdate = create<UpdateState>((set, get) => ({
  info: null,
  checking: false,
  installing: false,
  load: async () => {
    try {
      const info = await api.updates.get();
      set({ info });
      return info;
    } catch {
      return null;
    }
  },
  checkNow: async () => {
    set({ checking: true });
    try {
      const info = await api.updates.check();
      set({ info });
      return info;
    } catch (e) {
      toast.error(`Couldn't check for updates: ${errorMessage(e)}`);
      return null;
    } finally {
      set({ checking: false });
    }
  },
  install: async () => {
    const info = get().info;
    const bridge = tauriBridge();
    if (!bridge?.installUpdate) {
      if (info) void openExternal(info.downloadUrl);
      return;
    }
    set({ installing: true });
    try {
      const installed = await bridge.installUpdate();
      if (!installed) toast.success('Akev is up to date');
    } catch (e) {
      toast.error(errorMessage(e), { description: 'You can also download the new version from the release page.' });
    } finally {
      set({ installing: false });
    }
  },
}));

function dismissed(version: string): boolean {
  try {
    return localStorage.getItem(DISMISSED_KEY) === version;
  } catch {
    return false;
  }
}

function dismiss(version: string): void {
  try {
    localStorage.setItem(DISMISSED_KEY, version);
  } catch {
    // Private windows: the notice comes back next time.
  }
}

/** Show a notice once per new version. */
export function announce(info: UpdateInfo | null): void {
  if (!info?.available || !info.latest || dismissed(info.latest)) return;
  const latest = info.latest;
  const desktop = tauriBridge()?.installUpdate != null;
  toast(`Akev ${latest} is available`, {
    id: 'akev-update',
    description: `You have ${info.current}.${desktop ? ' Updating restarts Akev.' : ''}`,
    duration: Number.POSITIVE_INFINITY,
    closeButton: true,
    onDismiss: () => dismiss(latest),
    action: {
      label: desktop ? 'Update and restart' : 'Download',
      onClick: () => {
        dismiss(latest);
        void useUpdate.getState().install();
      },
    },
  });
}

/** Check on connect and once a day while the UI stays open. */
export function watchUpdates(): () => void {
  const run = () => void useUpdate.getState().load().then(announce);
  run();
  const timer = window.setInterval(run, DAY);
  return () => window.clearInterval(timer);
}
