import { ChevronDown, ChevronUp } from 'lucide-react';
import { useEffect, useState, type KeyboardEvent } from 'react';
import { Input, type InputProps } from './Input';
import styles from './Input.module.css';

export interface NumberInputProps extends Omit<InputProps, 'value' | 'onChange' | 'type' | 'min' | 'max' | 'step' | 'defaultValue'> {
  value: number | null;
  onValueChange: (value: number | null) => void;
  min?: number;
  max?: number;
  /** Arrow-key step (Shift ×10). Default 1. */
  step?: number;
  /** Unit suffix (`ms`, `KB`, `%`). */
  unit?: string;
  /** Allow an empty field (`null`). Default false: empty reverts on blur. */
  allowEmpty?: boolean;
  /** Decimal places allowed (default 0 = integers). */
  precision?: number;
  /** Show the ▲▼ stepper. */
  stepper?: boolean;
}

function clamp(n: number, min?: number, max?: number): number {
  let v = n;
  if (min != null) v = Math.max(min, v);
  if (max != null) v = Math.min(max, v);
  return v;
}

/**
 * Numeric input with unit suffix, keyboard stepping (↑/↓, Shift ×10) and clamping.
 * Text is edited freely; the value commits on blur/Enter (invalid text reverts).
 */
export function NumberInput({ value, onValueChange, min, max, step = 1, unit, allowEmpty = false, precision = 0, stepper = false, onBlur, onKeyDown, ...rest }: NumberInputProps) {
  const format = (v: number | null) => (v == null ? '' : precision > 0 ? String(Number(v.toFixed(precision))) : String(Math.round(v)));
  const [text, setText] = useState(format(value));
  const [editing, setEditing] = useState(false);

  // Reflect external value changes unless the user is mid-edit.
  useEffect(() => {
    if (!editing) setText(value == null ? '' : precision > 0 ? String(Number(value.toFixed(precision))) : String(Math.round(value)));
  }, [value, editing, precision]);

  const parse = (t: string): number | null | undefined => {
    const s = t.trim().replace(',', '.');
    if (s === '') return allowEmpty ? null : undefined;
    const n = Number(s);
    if (!Number.isFinite(n)) return undefined;
    const factor = 10 ** precision;
    return clamp(Math.round(n * factor) / factor, min, max);
  };

  const commit = () => {
    const parsed = parse(text);
    setEditing(false);
    if (parsed === undefined) {
      setText(format(value));
      return;
    }
    setText(format(parsed));
    if (parsed !== value) onValueChange(parsed);
  };

  const bump = (dir: 1 | -1, big: boolean) => {
    const base = parse(text) ?? value ?? min ?? 0;
    const next = clamp((base ?? 0) + dir * step * (big ? 10 : 1), min, max);
    const rounded = Number(next.toFixed(precision));
    setText(format(rounded));
    onValueChange(rounded);
  };

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
      e.preventDefault();
      bump(e.key === 'ArrowUp' ? 1 : -1, e.shiftKey);
    } else if (e.key === 'Enter') {
      commit();
    } else if (e.key === 'Escape') {
      setText(format(value));
      setEditing(false);
    }
  };

  const invalid = rest.invalid ?? (editing && parse(text) === undefined && text.trim() !== '');

  return (
    <Input
      {...rest}
      invalid={invalid}
      inputMode={precision > 0 ? 'decimal' : 'numeric'}
      value={text}
      onChange={(e) => {
        setEditing(true);
        setText(e.target.value);
      }}
      onBlur={(e) => {
        commit();
        onBlur?.(e);
      }}
      onKeyDown={handleKeyDown}
      className={rest.className}
      inputClassName="tnum"
      trailing={
        unit || stepper ? (
          <>
            {unit && <span className={styles.unit}>{unit}</span>}
            {stepper && (
              <span className={styles.stepper}>
                <button type="button" tabIndex={-1} className={styles.step} aria-label="Increase" onMouseDown={(e) => e.preventDefault()} onClick={() => bump(1, false)}>
                  <ChevronUp size={11} strokeWidth={2} aria-hidden />
                </button>
                <button type="button" tabIndex={-1} className={styles.step} aria-label="Decrease" onMouseDown={(e) => e.preventDefault()} onClick={() => bump(-1, false)}>
                  <ChevronDown size={11} strokeWidth={2} aria-hidden />
                </button>
              </span>
            )}
          </>
        ) : undefined
      }
    />
  );
}
