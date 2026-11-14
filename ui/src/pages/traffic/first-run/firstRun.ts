/**
 * The first-run setup (trust the CA → pick what to capture → wait for the first request).
 *
 * This module is the small, eager part: whether the setup is showing, which step it is on and
 * the persisted "done" flag (a UI preference, so the desktop app and the browser UI share it
 * through the daemon). The flow itself (`FirstRunFlow.tsx`) is loaded lazily.
 *
 * The setup opens by itself once per app load, when the flag isn't set and the first traffic
 * snapshot is empty. The palette ("Run the setup guide") and Settings → General reopen it.
 */

import { useEffect } from 'react';
import { create } from 'zustand';
import { trafficStore, useTraffic } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';

/** `pages` key of the UI preference recording that the setup was finished or skipped. */
export const FIRST_RUN_DONE_PREF = 'firstRun.done';

export type FirstRunStep = 0 | 1 | 2;
export const FIRST_RUN_STEPS: readonly { id: FirstRunStep; label: string }[] = [
  { id: 0, label: 'Trust the certificate' },
  { id: 1, label: 'Choose what to capture' },
  { id: 2, label: 'See your first request' },
];

export interface FirstRunState {
  /** `pending` until the first traffic snapshot has been checked. */
  status: 'pending' | 'open' | 'closed';
  step: FirstRunStep;
  /** Exchanges with a higher id count as "the first request" (the newest id when opened). */
  baseline: number;
}

export const useFirstRun = create<FirstRunState>(() => ({ status: 'pending', step: 0, baseline: 0 }));

/** Whether the setup should open by itself: never finished, and nothing captured yet. */
export function shouldAutoOpen(prefs: { pages: Record<string, unknown> }, captured: number): boolean {
  return prefs.pages[FIRST_RUN_DONE_PREF] !== true && captured === 0;
}

function newestId(): number {
  const order = trafficStore.getSnapshot().order;
  return order.length ? (order[order.length - 1] as number) : 0;
}

/** Decide once per app load, after the first traffic snapshot (`hello`) arrived. */
export function evaluateFirstRun(): void {
  if (useFirstRun.getState().status !== 'pending') return;
  const snap = trafficStore.getSnapshot();
  if (!snap.loaded) return;
  const open = shouldAutoOpen(useUiStore.getState(), snap.order.length);
  useFirstRun.setState({ status: open ? 'open' : 'closed', step: 0, baseline: newestId() });
}

/** Show the setup again (palette, Settings). Waits for a request newer than the current ones. */
export function openFirstRun(): void {
  useFirstRun.setState({ status: 'open', step: 0, baseline: newestId() });
}

export function setFirstRunStep(step: FirstRunStep): void {
  useFirstRun.setState({ step });
}

/** Finish or skip the setup: it won't open by itself again. */
export function closeFirstRun(): void {
  useFirstRun.setState({ status: 'closed' });
  const ui = useUiStore.getState();
  if (ui.pages[FIRST_RUN_DONE_PREF] !== true) ui.setPagePref(FIRST_RUN_DONE_PREF, true);
}

/** The first exchange captured since the setup opened, if any (`order` is ascending). */
export function firstNewExchange(order: readonly number[], baseline: number): number | null {
  // Binary search: the first id above the baseline.
  let lo = 0;
  let hi = order.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((order[mid] as number) <= baseline) lo = mid + 1;
    else hi = mid;
  }
  return lo < order.length ? (order[lo] as number) : null;
}

/** Whether the setup shows in place of the traffic list (decides on the first snapshot). */
export function useFirstRunOpen(): boolean {
  const loaded = useTraffic((s) => s.loaded);
  const status = useFirstRun((s) => s.status);
  useEffect(() => {
    if (loaded) evaluateFirstRun();
  }, [loaded]);
  return status === 'open';
}
