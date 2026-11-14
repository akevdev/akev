/**
 * "Repeat…": send one request many times (Compose, the traffic list). This module is small and
 * eager; the dialog itself loads when first opened. Mount {@link RepeatHost} once per page that
 * offers it.
 */

import { lazy, Suspense, useSyncExternalStore } from 'react';
import type { EditableRequest, ExchangeId, SendOptions } from '../../api/types';

/** What to repeat: a built request (Compose) or a captured exchange (resent as captured). */
export type RepeatTarget = { kind: 'request'; request: EditableRequest; options?: SendOptions; label: string } | { kind: 'exchange'; id: ExchangeId; label: string };

let target: RepeatTarget | null = null;
const listeners = new Set<() => void>();

function set(next: RepeatTarget | null): void {
  target = next;
  for (const l of listeners) l();
}

export function openRepeatDialog(next: RepeatTarget): void {
  set(next);
}

export function closeRepeatDialog(): void {
  set(null);
}

const RepeatDialog = lazy(() => import('./RepeatDialog'));

/** Renders the Repeat dialog while one is open. */
export function RepeatHost() {
  const current = useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => target,
    () => target,
  );
  if (!current) return null;
  return (
    <Suspense fallback={null}>
      <RepeatDialog target={current} onClose={closeRepeatDialog} />
    </Suspense>
  );
}
