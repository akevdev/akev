import * as RadixCheckbox from '@radix-ui/react-checkbox';
import clsx from 'clsx';
import { Check, Minus } from 'lucide-react';
import { useId, type ReactNode } from 'react';
import styles from './Checkbox.module.css';

export interface CheckboxProps {
  checked: boolean | 'indeterminate';
  onCheckedChange: (checked: boolean) => void;
  label?: ReactNode;
  description?: ReactNode;
  disabled?: boolean;
  className?: string;
  id?: string;
  'aria-label'?: string;
}

/** Checkbox with indeterminate support (Radix). */
export function Checkbox({ checked, onCheckedChange, label, description, disabled, className, id, ...aria }: CheckboxProps) {
  const autoId = useId();
  const boxId = id ?? autoId;
  const box = (
    <RadixCheckbox.Root
      id={boxId}
      checked={checked}
      onCheckedChange={(c) => onCheckedChange(c === true)}
      disabled={disabled}
      aria-label={aria['aria-label']}
      className={clsx(styles.root, !label && className)}
    >
      <RadixCheckbox.Indicator className={styles.indicator}>
        {checked === 'indeterminate' ? <Minus size={11} strokeWidth={3} aria-hidden /> : <Check size={11} strokeWidth={3} aria-hidden />}
      </RadixCheckbox.Indicator>
    </RadixCheckbox.Root>
  );
  if (!label) return box;
  return (
    <div className={clsx(styles.row, disabled && styles.disabled, className)}>
      {box}
      <label htmlFor={boxId} className={styles.labelWrap}>
        <span className={styles.label}>{label}</span>
        {description && <span className={styles.description}>{description}</span>}
      </label>
    </div>
  );
}
