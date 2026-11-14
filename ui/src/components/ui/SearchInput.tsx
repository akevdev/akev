import { Search, X } from 'lucide-react';
import { useRef, useState, type KeyboardEvent, type Ref } from 'react';
import type { ControlSize } from './Button';
import { Input, type InputProps } from './Input';
import { Kbd } from './Kbd';
import styles from './Input.module.css';

export interface SearchInputProps extends Omit<InputProps, 'value' | 'onChange' | 'leading' | 'trailing' | 'type'> {
  value: string;
  onValueChange: (value: string) => void;
  /** Shortcut hint shown while empty and unfocused (`mod+f`). Bind it with `useShortcut`. */
  shortcut?: string;
  size?: ControlSize;
  /** Extra trailing content (rendered before the clear button). */
  trailing?: InputProps['trailing'];
  ref?: Ref<HTMLInputElement>;
}

/**
 * Search field with a leading magnifier, a clear button and an optional shortcut hint.
 * Escape clears the text, or blurs when already empty.
 */
export function SearchInput({ value, onValueChange, shortcut, size = 'md', trailing, onKeyDown, onFocus, onBlur, ref, placeholder = 'Search', className, ...rest }: SearchInputProps) {
  const [focused, setFocused] = useState(false);
  const local = useRef<HTMLInputElement | null>(null);
  const iconSize = size === 'sm' ? 13 : 14;

  const handleKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    onKeyDown?.(e);
    if (e.defaultPrevented) return;
    if (e.key === 'Escape') {
      if (value) {
        e.preventDefault();
        e.stopPropagation();
        onValueChange('');
      } else {
        e.currentTarget.blur();
      }
    }
  };

  return (
    <Input
      {...rest}
      ref={(el) => {
        local.current = el;
        if (typeof ref === 'function') ref(el);
        else if (ref) ref.current = el;
      }}
      type="search"
      size={size}
      className={className}
      value={value}
      placeholder={placeholder}
      onChange={(e) => onValueChange(e.target.value)}
      onKeyDown={handleKeyDown}
      onFocus={(e) => {
        setFocused(true);
        onFocus?.(e);
      }}
      onBlur={(e) => {
        setFocused(false);
        onBlur?.(e);
      }}
      leading={<Search size={iconSize} strokeWidth={1.75} aria-hidden />}
      trailing={
        <>
          {trailing}
          {value ? (
            <button
              type="button"
              className={styles.clear}
              aria-label="Clear search"
              tabIndex={-1}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                onValueChange('');
                local.current?.focus();
              }}
            >
              <X size={13} strokeWidth={2} aria-hidden />
            </button>
          ) : (
            shortcut && !focused && <Kbd combo={shortcut} size="sm" />
          )}
        </>
      }
    />
  );
}
