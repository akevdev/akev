/**
 * Akev Pro license state (zustand): the daemon's `LicenseStatus` and the upgrade dialog.
 *
 *   hello (`status.license`) ──┐
 *   `license` stream message ──┼─▶ licenseReducers.set ─▶ useLicense() / useFeature(f)
 *   PUT/DELETE/trial calls ────┘
 *   any `pro-required` (402) from a user action ─▶ openUpgrade(feature)
 *
 * Gating is the daemon's job; the UI only explains it. A missing status (not connected yet)
 * gates nothing.
 */

import { create } from 'zustand';
import type { ExternalToast } from 'sonner';
import { api, type ApiClient, type ApiError } from '../api/client';
import type { Feature, LicenseStatus } from '../api/types';
import { setErrorToastFilter } from '../components/ui/toast';
import { isEntitled, isFeature, normalizeLicenseKey, showProBadge } from '../lib/license';
import { useAppStore } from './app';
import { loadTlsProfiles } from './tlsProfiles';

/** What the upgrade dialog shows: the offer, or the license key form. */
export type UpgradeView = 'offer' | 'key';

export interface UpgradeDialogState {
  open: boolean;
  /** The feature that prompted it (listed first, named in the title). */
  feature: Feature | null;
  view: UpgradeView;
}

export interface LicenseStoreState {
  status: LicenseStatus | null;
  upgrade: UpgradeDialogState;

  /** `PUT /api/license`. Throws the daemon's `invalid-license` error (message says why). */
  activate(key: string): Promise<LicenseStatus>;
  /** `DELETE /api/license`. */
  remove(): Promise<LicenseStatus>;
  /** `POST /api/license/trial` (409 `trial-unavailable` once used). */
  startTrial(): Promise<LicenseStatus>;
  /** `GET /api/license` (e.g. when Settings → License opens). */
  refresh(signal?: AbortSignal): Promise<LicenseStatus>;
}

const CLOSED: UpgradeDialogState = { open: false, feature: null, view: 'offer' };

export const useLicenseStore = create<LicenseStoreState>()(() => ({
  status: null,
  upgrade: CLOSED,

  async activate(key) {
    const status = await api.license.activate(normalizeLicenseKey(key));
    licenseReducers.set(status);
    return status;
  },
  async remove() {
    const status = await api.license.remove();
    licenseReducers.set(status);
    return status;
  },
  async startTrial() {
    const status = await api.license.startTrial();
    licenseReducers.set(status);
    return status;
  },
  async refresh(signal) {
    const status = await api.license.get(signal);
    licenseReducers.set(status);
    return status;
  },
}));

function sameFeatures(a: LicenseStatus | null, b: LicenseStatus | null): boolean {
  if (a === b) return true;
  if (!a || !b || a.features.length !== b.features.length) return false;
  return a.features.every((f) => b.features.includes(f));
}

// =====================================================================================
// Stream reducers (state/connection.ts)
// =====================================================================================

export const licenseReducers = {
  /** A new status from `hello`, the `license` message or an API call (`null` = forget it). */
  set(next: LicenseStatus | null): void {
    const prev = useLicenseStore.getState().status;
    useLicenseStore.setState({ status: next });
    // Keep `Status.license` in the app store in step (it arrived with `hello`).
    const app = useAppStore.getState().status;
    if (app && next && app.license !== next) useAppStore.setState({ status: { ...app, license: next } });
    // What the daemon may do changed: refresh state derived from the license elsewhere.
    if (prev && !sameFeatures(prev, next)) void loadTlsProfiles();
  },
};

// =====================================================================================
// Hooks & selectors
// =====================================================================================

/** The current license status (`null` before `hello`). */
export function useLicense(): LicenseStatus | null {
  return useLicenseStore((s) => s.status);
}

export function getLicense(): LicenseStatus | null {
  return useLicenseStore.getState().status;
}

/** Whether `feature` works right now (true when the daemon doesn't gate at all). */
export function useFeature(feature: Feature): boolean {
  return useLicenseStore((s) => isEntitled(s.status, feature));
}

/** Whether to mark `feature` (or Pro in general) with a "Pro" badge. */
export function useShowProBadge(feature?: Feature): boolean {
  return useLicenseStore((s) => showProBadge(s.status, feature));
}

// =====================================================================================
// Upgrade dialog
// =====================================================================================

/** Open the upgrade dialog, optionally for the feature the user just tried to use. */
export function openUpgrade(feature?: Feature | null, view: UpgradeView = 'offer'): void {
  useLicenseStore.setState({ upgrade: { open: true, feature: feature ?? null, view } });
}

/** Open the dialog straight at the license key form. */
export function openLicenseKeyEntry(): void {
  openUpgrade(null, 'key');
}

export function setUpgradeView(view: UpgradeView): void {
  const cur = useLicenseStore.getState().upgrade;
  useLicenseStore.setState({ upgrade: { ...cur, view } });
}

export function closeUpgrade(): void {
  useLicenseStore.setState({ upgrade: CLOSED });
}

export function useUpgradeDialog(): UpgradeDialogState {
  return useLicenseStore((s) => s.upgrade);
}

// =====================================================================================
// `pro-required` (HTTP 402) → upgrade dialog
// =====================================================================================

/** How long an error toast repeating a `pro-required` message counts as redundant. */
const REDUNDANT_TOAST_MS = 3000;

let lastProRequired: { message: string; at: number } | null = null;

/**
 * React to an API error: a `pro-required` answer to something the user did (any non-GET
 * request) opens the upgrade dialog for the named feature. Background reads (GET polls) never
 * pop a dialog — the UI gates those itself and shows the daemon's message inline. Returns
 * whether the dialog was opened.
 */
export function handleProRequired(error: ApiError, now: number = Date.now()): boolean {
  if (!error.isProRequired) return false;
  if ((error.method ?? 'GET').toUpperCase() === 'GET') return false;
  lastProRequired = { message: error.message, at: now };
  const feature = error.feature;
  openUpgrade(isFeature(feature) ? feature : null);
  return true;
}

/**
 * Whether an error toast only repeats the `pro-required` failure the upgrade dialog is
 * already explaining (call sites toast `errorMessage(e)` as the description).
 */
export function isRedundantErrorToast(message: unknown, options: ExternalToast | undefined, now: number = Date.now()): boolean {
  const last = lastProRequired;
  if (!last || now - last.at > REDUNDANT_TOAST_MS) return false;
  if (!useLicenseStore.getState().upgrade.open) return false;
  return options?.description === last.message || message === last.message;
}

/** Route `pro-required` errors to the upgrade dialog. Called once at startup; returns an undo. */
export function installLicenseHandlers(client: ApiClient = api): () => void {
  const off = client.onError((e) => void handleProRequired(e));
  setErrorToastFilter((message, options) => isRedundantErrorToast(message, options));
  return () => {
    off();
    setErrorToastFilter(null);
  };
}
