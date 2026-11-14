import * as RadixSwitch from '@radix-ui/react-switch';
import clsx from 'clsx';
import { useId, type ReactNode } from 'react';
import styles from './Switch.module.css';

export interface SwitchProps {
  checked: boolean;
  onCheckedChange: (checked: boolean) => void;
  /** Visible label rendered after the switch (clickable). */
  label?: ReactNode;
  /** Muted description under the label. */
  description?: ReactNode;
  size?: 'sm' | 'md';
  disabled?: boolean;
  className?: string;
  id?: string;
  'aria-label'?: string;
}

/** On/off toggle (Radix Switch). */
export function Switch({ checked, onCheckedChange, label, description, size = 'md', disabled, className, id, ...aria }: SwitchProps) {
  const autoId = useId();
  const switchId = id ?? autoId;
  const control = (
    <RadixSwitch.Root
      id={switchId}
      checked={checked}
      onCheckedChange={onCheckedChange}
      disabled={disabled}
      aria-label={aria['aria-label']}
      className={clsx(styles.root, styles[size], !label && className)}
    >
      <RadixSwitch.Thumb className={styles.thumb} />
    </RadixSwitch.Root>
  );
  if (!label) return control;
  return (
    <div className={clsx(styles.row, disabled && styles.disabled, className)}>
      {control}
      <label htmlFor={switchId} className={styles.labelWrap}>
        <span className={styles.label}>{label}</span>
        {description && <span className={styles.description}>{description}</span>}
      </label>
    </div>
  );
}
