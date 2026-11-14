import { CircleAlert } from 'lucide-react';
import { useCallback, useId, useState } from 'react';
import { errorMessage, isApiError } from '../../api/client';
import type { LicenseStatus } from '../../api/types';
import { activationResult, LICENSE_KEY_PREFIX, validateLicenseKeyText } from '../../lib/license';
import { useLicenseStore } from '../../state/license';
import { TextArea, toast } from '../ui';
import styles from './pro.module.css';

export interface LicenseKeyFormState {
  text: string;
  setText: (text: string) => void;
  error: string | null;
  busy: boolean;
  /** Validate and `PUT /api/license`; resolves to the new status, or null on failure (shown inline). */
  submit: () => Promise<LicenseStatus | null>;
}

/** State of a license key field: the daemon's `invalid-license` reason is kept for inline display. */
export function useLicenseKeyForm(onActivated?: (status: LicenseStatus) => void): LicenseKeyFormState {
  const [text, setTextRaw] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const setText = useCallback((next: string) => {
    setTextRaw(next);
    setError(null);
  }, []);

  const submit = useCallback(async () => {
    const invalid = validateLicenseKeyText(text);
    if (invalid) {
      setError(invalid);
      return null;
    }
    setBusy(true);
    setError(null);
    try {
      const status = await useLicenseStore.getState().activate(text);
      const result = activationResult(status);
      if (result.ok) {
        toast.success(result.title, { description: result.description });
        setTextRaw('');
      } else {
        toast.warning(result.title, { description: result.description, duration: 12_000 });
      }
      onActivated?.(status);
      return status;
    } catch (e) {
      setError(isApiError(e) && e.code === 'invalid-license' ? e.message : `Couldn't activate the license: ${errorMessage(e)}`);
      return null;
    } finally {
      setBusy(false);
    }
  }, [text, onActivated]);

  return { text, setText, error, busy, submit };
}

/** The key textarea with its inline error (buttons are the caller's). Mod+Enter submits. */
export function LicenseKeyField({ form, autoFocus, label = 'License key', hideLabel }: { form: LicenseKeyFormState; autoFocus?: boolean; label?: string; hideLabel?: boolean }) {
  const id = useId();
  return (
    <div className={styles.keyForm}>
      <label className={hideLabel ? 'sr-only' : styles.keyLabel} htmlFor={id}>
        {label}
      </label>
      <TextArea
        id={id}
        mono
        autoFocus={autoFocus}
        autoResize
        minRows={3}
        maxRows={8}
        value={form.text}
        invalid={!!form.error}
        disabled={form.busy}
        placeholder={`${LICENSE_KEY_PREFIX}…`}
        onChange={(e) => form.setText(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void form.submit();
          }
        }}
        aria-describedby={form.error ? `${id}-error` : `${id}-hint`}
        className={styles.keyInput}
      />
      {form.error ? (
        <p className={styles.keyError} id={`${id}-error`} role="alert">
          <CircleAlert size={12} strokeWidth={2} aria-hidden />
          <span>{form.error}</span>
        </p>
      ) : (
        <p className={styles.keyHint} id={`${id}-hint`}>
          Paste the whole key from your purchase e-mail — line breaks don't matter.
        </p>
      )}
    </div>
  );
}
