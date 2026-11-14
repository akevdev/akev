/**
 * Open state of the keyboard shortcuts overlay. Kept apart from the overlay component so the
 * shell can open it without loading the overlay.
 */

import { useSyncExternalStore } from 'react';

let open = false;
const listeners = new Set<() => void>();

/** Show / hide the keyboard shortcuts overlay. */
export function setShortcutsOverlayOpen(value: boolean): void {
  if (open === value) return;
  open = value;
  for (const l of listeners) l();
}

function subscribe(l: () => void): () => void {
  listeners.add(l);
  return () => listeners.delete(l);
}

export function useShortcutsOverlayOpen(): boolean {
  return useSyncExternalStore(subscribe, () => open, () => open);
}
