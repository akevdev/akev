import { toast as sonner, Toaster as SonnerToaster, type ExternalToast } from 'sonner';
import { useResolvedTheme } from '../../app/theme';
import styles from './Toast.module.css';

/**
 * Toast helpers on top of sonner (bottom-right, max 3 visible, themed with tokens).
 *
 * ```ts
 * toast.success('Proxy address copied');
 * toast.error("Couldn't save rules", { description: err.message, action: { label: 'Retry', onClick: save } });
 * undoToast('Deleted 3 exchanges', () => restore());
 * ```
 *
 * Error toasts have a close button and stay for 6 s, or until dismissed when they carry an
 * action (so a Retry can't time out while the user reads the message).
 */
/**
 * Decides whether an error toast is redundant because another surface already reports the
 * failure (e.g. the upgrade dialog for `pro-required`). See {@link setErrorToastFilter}.
 */
export type ErrorToastFilter = (message: unknown, options: ExternalToast | undefined) => boolean;

let errorFilter: ErrorToastFilter | null = null;

/** Install (or clear) the app-wide filter for redundant error toasts. */
export function setErrorToastFilter(filter: ErrorToastFilter | null): void {
  errorFilter = filter;
}

function suppressed(message: unknown, options: ExternalToast | undefined): boolean {
  try {
    return errorFilter?.(message, options) ?? false;
  } catch {
    return false;
  }
}

const error: typeof sonner.error = (message, data) => {
  if (suppressed(message, data)) return '';
  return sonner.error(message, { duration: data?.action ? Infinity : 6000, closeButton: true, ...data });
};

export const toast: typeof sonner = Object.assign((...args: Parameters<typeof sonner>) => sonner(...args), sonner, { error });

/** Toast with an Undo action (DESIGN.md §1.6: reversible instead of confirmed). */
export function undoToast(message: string, onUndo: () => void, options: ExternalToast = {}): string | number {
  return sonner(message, {
    duration: 6000,
    ...options,
    action: { label: 'Undo', onClick: onUndo },
  });
}

/** Mount once at the root (App does). */
export function Toaster() {
  const theme = useResolvedTheme();
  return (
    <SonnerToaster
      position="bottom-right"
      visibleToasts={3}
      theme={theme === 'light' ? 'light' : 'dark'}
      offset={{ bottom: 36, right: 16 }}
      gap={8}
      toastOptions={{
        classNames: {
          toast: styles.toast,
          title: styles.title,
          description: styles.description,
          actionButton: styles.action,
          cancelButton: styles.cancel,
          closeButton: styles.close,
          icon: styles.icon,
          success: styles.success,
          error: styles.error,
          warning: styles.warning,
          info: styles.info,
        },
      }}
    />
  );
}
