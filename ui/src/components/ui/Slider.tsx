import * as RadixSlider from '@radix-ui/react-slider';
import clsx from 'clsx';
import styles from './Slider.module.css';

export interface SliderProps {
  value: number;
  onValueChange: (value: number) => void;
  /** Fired when the user releases the thumb (commit expensive work here). */
  onValueCommit?: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  className?: string;
  'aria-label'?: string;
  /** Render the current value to the right (`(v) => `${v} ms``). */
  formatValue?: (value: number) => string;
}

/** Single-thumb range slider (Radix). */
export function Slider({ value, onValueChange, onValueCommit, min = 0, max = 100, step = 1, disabled, className, formatValue, ...aria }: SliderProps) {
  return (
    <div className={clsx(styles.wrap, className)}>
      <RadixSlider.Root
        className={styles.root}
        value={[value]}
        min={min}
        max={max}
        step={step}
        disabled={disabled}
        onValueChange={(v) => v[0] != null && onValueChange(v[0])}
        onValueCommit={(v) => v[0] != null && onValueCommit?.(v[0])}
      >
        <RadixSlider.Track className={styles.track}>
          <RadixSlider.Range className={styles.range} />
        </RadixSlider.Track>
        <RadixSlider.Thumb className={styles.thumb} aria-label={aria['aria-label']} />
      </RadixSlider.Root>
      {formatValue && <span className={clsx(styles.value, 'tnum')}>{formatValue(value)}</span>}
    </div>
  );
}
