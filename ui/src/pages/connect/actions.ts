/**
 * Side-effecting actions shared by the Connect page and Settings: interceptor
 * activation/deactivation (with a tiny pending-state store so cards and sheets agree),
 * LAN access toggling, status/CA refreshes.
 */

import { useSyncExternalStore } from 'react';
import { api, isApiError } from '../../api/client';
import type { ActivationResult, CaInfo, InterceptorInfo, Settings, Status } from '../../api/types';
import { navigate } from '../../app/router';
import { toast } from '../../components/ui';
import { useAppStore } from '../../state/app';
import { primaryActionFor, stopsFromPanel } from './catalog';

// =====================================================================================
// Pending state (per interceptor or per target, e.g. `android-adb:emulator-5554`)
// =====================================================================================

const pending = new Set<string>();
const listeners = new Set<() => void>();
let version = 0;

function setPending(key: string, on: boolean): void {
  if (on === pending.has(key)) return;
  if (on) pending.add(key);
  else pending.delete(key);
  version++;
  for (const l of listeners) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

/** Whether an activation/deactivation for `key` is in flight. */
export function usePending(key: string): boolean {
  return useSyncExternalStore(subscribe, () => pending.has(key), () => false);
}

/** Re-renders whenever any pending state changes; returns a lookup. */
export function usePendingSet(): (key: string) => boolean {
  useSyncExternalStore(subscribe, () => version, () => 0);
  return (key) => pending.has(key);
}

export function pendingKey(id: string, target?: string | number | null): string {
  return target == null ? id : `${id}:${target}`;
}

// =====================================================================================
// Last successful activation result (per pending key) — panels show what it returned
// (Docker's exec command, Android's install steps…) until the target is stopped.
// =====================================================================================

const results = new Map<string, ActivationResult>();

function setResult(key: string, result: ActivationResult | null): void {
  if (result == null ? !results.has(key) : results.get(key) === result) return;
  if (result == null) results.delete(key);
  else results.set(key, result);
  version++;
  for (const l of listeners) l();
}

/** The last successful activation of `key` (cleared when it's deactivated). */
export function useActivationResult(key: string): ActivationResult | null {
  return useSyncExternalStore(subscribe, () => results.get(key) ?? null, () => null);
}

/** Forget a stored result (e.g. the user dismissed it). */
export function clearActivationResult(key: string): void {
  setResult(key, null);
}

/** `docker:stop:abc` → `docker:abc`; `docker:stop` → `docker`. */
export function activationKeyOf(stopKey: string, id: string): string {
  return stopKey === `${id}:stop` ? id : stopKey.replace(`${id}:stop:`, `${id}:`);
}

// =====================================================================================
// Errors
// =====================================================================================

/** Plain-language explanation for a failed interceptor/API call. */
export function explainError(e: unknown): string {
  if (isApiError(e)) {
    if (e.isNetworkError) return "Akev's background service isn't reachable. Make sure Akev is running, then try again.";
    if (e.code === 'timeout') return "The interceptor didn't respond in time. The app may still be starting — try again in a moment.";
    if (e.isNotFound) return "This interceptor isn't available in this version of Akev.";
    if (e.isUnauthorized) return 'Your session expired. Reopen Akev from the app or the link in your terminal.';
    return e.message;
  }
  return e instanceof Error ? e.message : String(e);
}

// =====================================================================================
// Interceptors
// =====================================================================================

export interface ActivateOptions {
  /** Pending key (defaults to the interceptor id). */
  key?: string;
  /** Human target for messages ("Pixel 8", "api-server"). */
  target?: string;
  /** Verb for error messages ("launch", "attach to"). */
  verb?: string;
  /** Skip the success toast (panels that show their own status). */
  quiet?: boolean;
}

function defaultVerb(info: InterceptorInfo): string {
  const kind = primaryActionFor(info).kind;
  return kind === 'launch' ? 'launch' : 'activate';
}

/** Activate an interceptor; reports success/failure with toasts. Resolves null on error. */
export async function activateInterceptor(info: InterceptorInfo, options?: unknown, opts: ActivateOptions = {}): Promise<ActivationResult | null> {
  const key = opts.key ?? info.id;
  if (pending.has(key)) return null;
  const subject = opts.target ?? info.name;
  const verb = opts.verb ?? defaultVerb(info);
  const retry = { label: 'Retry', onClick: () => void activateInterceptor(info, options, opts) };
  setPending(key, true);
  try {
    const res = await api.interceptors.activate(info.id, options);
    if (!res.success) {
      toast.error(`Couldn't ${verb} ${subject}`, { description: res.message ?? 'The interceptor reported a failure without details.', action: retry });
      return res;
    }
    setResult(key, res);
    if (!opts.quiet) {
      toast.success(res.message ?? `${subject} is now intercepted`, {
        description: opts.target ? info.name : undefined,
        action: { label: 'View traffic', onClick: () => navigate('/traffic') },
      });
    }
    // The stream broadcasts the new state; refresh too in case it's lagging.
    void useAppStore.getState().refreshInterceptors().catch(() => undefined);
    return res;
  } catch (e) {
    toast.error(`Couldn't ${verb} ${subject}`, { description: explainError(e), action: retry });
    return null;
  } finally {
    setPending(key, false);
  }
}

/** Deactivate an interceptor (or one target of it). */
export async function deactivateInterceptor(info: InterceptorInfo, options?: unknown, opts: ActivateOptions = {}): Promise<boolean> {
  const key = opts.key ?? `${info.id}:stop`;
  if (pending.has(key)) return false;
  setPending(key, true);
  try {
    await api.interceptors.deactivate(info.id, options);
    if (key === `${info.id}:stop`) {
      // Stopping the whole interceptor forgets every per-target result too.
      for (const k of [...results.keys()]) if (k === info.id || k.startsWith(`${info.id}:`)) setResult(k, null);
    } else {
      setResult(activationKeyOf(key, info.id), null);
    }
    if (!opts.quiet) toast.success(`Stopped intercepting ${opts.target ?? info.name}`);
    void useAppStore.getState().refreshInterceptors().catch(() => undefined);
    return true;
  } catch (e) {
    toast.error(`Couldn't stop ${opts.target ?? info.name}`, { description: explainError(e), action: { label: 'Retry', onClick: () => void deactivateInterceptor(info, options, opts) } });
    return false;
  } finally {
    setPending(key, false);
  }
}

/** Stop every active interceptor. */
export async function deactivateAll(list: readonly InterceptorInfo[]): Promise<void> {
  // Some can only be stopped by the user (firewall rules need root): their panel says how.
  const active = list.filter((i) => i.active && !stopsFromPanel(i));
  if (active.length === 0) return;
  const results = await Promise.all(active.map((i) => deactivateInterceptor(i, undefined, { quiet: true })));
  const ok = results.filter(Boolean).length;
  if (ok === active.length) toast.success(ok === 1 ? `Stopped intercepting ${active[0]?.name}` : `Stopped ${ok} interceptors`);
}

// =====================================================================================
// Proxy status, LAN access, CA
// =====================================================================================

/**
 * Re-fetch `/api/status` and update the app store (the stream has no status message, so a
 * rebind or CA regeneration would otherwise leave the status bar stale).
 */
export async function refreshStatus(): Promise<Status | null> {
  try {
    const status = await api.status();
    useAppStore.setState({ status, stats: status.stats });
    return status;
  } catch {
    return null;
  }
}

/** Patch `status.ca` in the store after the CA changed. */
export function applyCaInfo(ca: CaInfo): void {
  const status = useAppStore.getState().status;
  if (status) useAppStore.setState({ status: { ...status, ca } });
}

export const LAN_BIND = '0.0.0.0';
export const LOCAL_BIND = '127.0.0.1';

/** Allow / disallow connections from other devices (rebinds the proxy). */
export async function setLanAccess(enabled: boolean): Promise<Settings> {
  const settings = useAppStore.getState().settings ?? (await api.settings.get());
  const bindAddress = enabled ? LAN_BIND : LOCAL_BIND;
  const saved = await useAppStore.getState().saveSettings({ ...settings, proxy: { ...settings.proxy, bindAddress } });
  await refreshStatus();
  return saved;
}
