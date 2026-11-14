import { useEffect, useRef, useState, type ChangeEvent, type KeyboardEvent } from 'react';

export interface TextFieldOptions {
  /** The saved value (draft). */
  value: string;
  validate?: (text: string) => string | null;
  /** Called with valid text on every change (debounce the save in the caller). */
  onCommit: (text: string) => void;
  /** Called on blur with valid text (flush pending saves). */
  onSettle?: () => void;
  /** Delay before an error appears while typing (ms). Errors always show on blur. */
  errorDelay?: number;
}

/**
 * Local text state for a validated settings field: valid edits commit as you type, invalid
 * ones show an inline error (after a short pause, or on blur) and are never saved. External
 * value changes replace the text unless the user is mid-edit. Escape reverts.
 */
export function useTextField({ value, validate, onCommit, onSettle, errorDelay = 700 }: TextFieldOptions) {
  const [text, setText] = useState(value);
  const [error, setError] = useState<string | null>(null);
  const [errorVisible, setErrorVisible] = useState(false);
  const focused = useRef(false);
  const dirty = useRef(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => {
    if (!focused.current || !dirty.current) {
      setText(value);
      setError(null);
      setErrorVisible(false);
      dirty.current = false;
    }
  }, [value]);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const onChange = (e: ChangeEvent<HTMLInputElement>) => {
    const t = e.target.value;
    setText(t);
    dirty.current = true;
    const err = validate?.(t) ?? null;
    setError(err);
    setErrorVisible(false);
    if (timer.current) clearTimeout(timer.current);
    if (err) timer.current = setTimeout(() => setErrorVisible(true), errorDelay);
    else onCommit(t);
  };

  const onFocus = () => {
    focused.current = true;
  };

  const onBlur = () => {
    focused.current = false;
    if (error) {
      setErrorVisible(true);
      return;
    }
    dirty.current = false;
    onSettle?.();
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === 'Escape' && dirty.current) {
      e.preventDefault();
      e.stopPropagation();
      setText(value);
      setError(null);
      setErrorVisible(false);
      dirty.current = false;
    } else if (e.key === 'Enter') {
      if (error) setErrorVisible(true);
      else onSettle?.();
    }
  };

  return {
    text,
    error: errorVisible ? error : null,
    /** Whether the current text is invalid (even if the message isn't shown yet). */
    invalid: error != null,
    inputProps: { value: text, onChange, onFocus, onBlur, onKeyDown, invalid: errorVisible && error != null },
  };
}
