import * as RadixDialog from '@radix-ui/react-dialog';
import clsx from 'clsx';
import { X } from 'lucide-react';
import type { ReactElement, ReactNode } from 'react';
import styles from './Dialog.module.css';

export const DialogClose = RadixDialog.Close;

export interface DialogProps {
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Optional trigger element (uncontrolled usage). */
  trigger?: ReactElement;
  title: ReactNode;
  description?: ReactNode;
  /** Footer actions (right-aligned). */
  footer?: ReactNode;
  /** Max width in px (default 560). */
  width?: number;
  children?: ReactNode;
  className?: string;
  /** Hide the × button. */
  hideClose?: boolean;
  /** Body without padding (lists, editors). */
  flush?: boolean;
  /** Prevent closing via Escape / outside click (e.g. while saving). */
  dismissible?: boolean;
  /** Called when the dialog opens; call `event.preventDefault()` to manage focus yourself. */
  onOpenAutoFocus?: (event: Event) => void;
  /**
   * Focus the dialog surface instead of its first control on open — for read-only dialogs
   * (help, shortcuts) where a focus ring on the close button would be noise.
   */
  focusContent?: boolean;
}

/** Radix dispatches the mount auto-focus event on the content element. */
function autoFocusHandler(focusContent: boolean | undefined, custom: ((event: Event) => void) | undefined): ((event: Event) => void) | undefined {
  if (!focusContent) return custom;
  return (event) => {
    custom?.(event);
    if (event.defaultPrevented) return;
    event.preventDefault();
    if (event.target instanceof HTMLElement) event.target.focus({ preventScroll: true });
  };
}

/**
 * Centered modal dialog (scrim blur, scale .98→1 + fade). Focus is trapped while open and
 * restored on close.
 */
export function Dialog({ open, onOpenChange, trigger, title, description, footer, width = 560, children, className, hideClose, flush, dismissible = true, onOpenAutoFocus, focusContent }: DialogProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger && <RadixDialog.Trigger asChild>{trigger}</RadixDialog.Trigger>}
      <RadixDialog.Portal>
        <RadixDialog.Overlay className={styles.overlay} />
        <RadixDialog.Content
          className={clsx(styles.content, className)}
          style={{ maxWidth: width }}
          onOpenAutoFocus={autoFocusHandler(focusContent, onOpenAutoFocus)}
          onEscapeKeyDown={(e) => !dismissible && e.preventDefault()}
          onPointerDownOutside={(e) => !dismissible && e.preventDefault()}
        >
          <header className={styles.header}>
            <div className={styles.titles}>
              <RadixDialog.Title className={styles.title}>{title}</RadixDialog.Title>
              {description ? (
                <RadixDialog.Description className={styles.description}>{description}</RadixDialog.Description>
              ) : (
                <RadixDialog.Description className="sr-only">{typeof title === 'string' ? title : 'Dialog'}</RadixDialog.Description>
              )}
            </div>
            {!hideClose && (
              <RadixDialog.Close className={styles.close} aria-label="Close">
                <X size={16} strokeWidth={1.75} aria-hidden />
              </RadixDialog.Close>
            )}
          </header>
          {children != null && <div className={clsx(styles.body, flush && styles.flush)}>{children}</div>}
          {footer && <footer className={styles.footer}>{footer}</footer>}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}

export interface SheetProps extends Omit<DialogProps, 'width'> {
  /** Width in px (default 440). */
  width?: number;
  /** Side to slide from (default right). */
  side?: 'right' | 'left';
}

/** Side drawer (interceptor configuration, details). Same API as {@link Dialog}. */
export function Sheet({ open, onOpenChange, trigger, title, description, footer, width = 440, side = 'right', children, className, hideClose, flush, dismissible = true, onOpenAutoFocus, focusContent }: SheetProps) {
  return (
    <RadixDialog.Root open={open} onOpenChange={onOpenChange}>
      {trigger && <RadixDialog.Trigger asChild>{trigger}</RadixDialog.Trigger>}
      <RadixDialog.Portal>
        <RadixDialog.Overlay className={clsx(styles.overlay, styles.sheetOverlay)} />
        <RadixDialog.Content
          className={clsx(styles.sheet, styles[side], className)}
          style={{ width: `min(${width}px, calc(100vw - 48px))` }}
          onOpenAutoFocus={autoFocusHandler(focusContent, onOpenAutoFocus)}
          onEscapeKeyDown={(e) => !dismissible && e.preventDefault()}
          onPointerDownOutside={(e) => !dismissible && e.preventDefault()}
        >
          <header className={clsx(styles.header, styles.sheetHeader)}>
            <div className={styles.titles}>
              <RadixDialog.Title className={styles.title}>{title}</RadixDialog.Title>
              {description ? (
                <RadixDialog.Description className={styles.description}>{description}</RadixDialog.Description>
              ) : (
                <RadixDialog.Description className="sr-only">{typeof title === 'string' ? title : 'Panel'}</RadixDialog.Description>
              )}
            </div>
            {!hideClose && (
              <RadixDialog.Close className={styles.close} aria-label="Close">
                <X size={16} strokeWidth={1.75} aria-hidden />
              </RadixDialog.Close>
            )}
          </header>
          <div className={clsx(styles.sheetBody, flush && styles.flush)}>{children}</div>
          {footer && <footer className={clsx(styles.footer, styles.sheetFooter)}>{footer}</footer>}
        </RadixDialog.Content>
      </RadixDialog.Portal>
    </RadixDialog.Root>
  );
}
