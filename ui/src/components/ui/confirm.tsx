import { useSyncExternalStore, type ReactNode } from 'react';
import { Button } from './Button';
import { Dialog } from './Dialog';

export interface ConfirmOptions {
  title: ReactNode;
  message?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Red confirm button (destructive actions). */
  destructive?: boolean;
}

interface PendingConfirm extends ConfirmOptions {
  resolve: (ok: boolean) => void;
}

let pending: PendingConfirm | null = null;
const listeners = new Set<() => void>();
const emit = () => listeners.forEach((l) => l());

/**
 * Ask the user to confirm. Prefer undo toasts for reversible actions (DESIGN.md §1.6) and
 * reserve this for genuinely destructive ones (e.g. clearing pinned exchanges).
 *
 * ```ts
 * if (await confirm({ title: 'Clear all traffic?', message: '3 pinned exchanges will be deleted too.', destructive: true, confirmLabel: 'Clear all' })) …
 * ```
 */
export function confirm(options: ConfirmOptions): Promise<boolean> {
  pending?.resolve(false);
  return new Promise((resolve) => {
    pending = { ...options, resolve };
    emit();
  });
}

function settle(ok: boolean): void {
  const p = pending;
  pending = null;
  emit();
  p?.resolve(ok);
}

/** Answer "no" to an open confirm dialog (tests clean up with it). */
export function dismissConfirm(): void {
  if (pending) settle(false);
}

/** Renders the active confirm dialog. Mount once (App does). */
export function ConfirmHost() {
  const current = useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => pending,
    () => pending,
  );
  return (
    <Dialog
      open={current != null}
      onOpenChange={(open) => !open && settle(false)}
      title={current?.title ?? ''}
      description={current?.message}
      width={440}
      hideClose
      footer={
        <>
          <Button variant="ghost" onClick={() => settle(false)}>
            {current?.cancelLabel ?? 'Cancel'}
          </Button>
          <Button variant={current?.destructive ? 'danger' : 'primary'} onClick={() => settle(true)} autoFocus>
            {current?.confirmLabel ?? 'Confirm'}
          </Button>
        </>
      }
    />
  );
}
