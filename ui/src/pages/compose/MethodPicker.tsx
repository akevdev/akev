import { ChevronDown } from 'lucide-react';
import { useId, type CSSProperties } from 'react';
import { Combobox, MethodBadge, type ComboboxOption } from '../../components/ui';
import { HTTP_METHODS, methodColorVar } from '../../lib/http';
import styles from './RequestLine.module.css';

const METHOD_HINTS: Record<string, string> = {
  GET: 'Read',
  POST: 'Create',
  PUT: 'Replace',
  PATCH: 'Update',
  DELETE: 'Remove',
  HEAD: 'Headers only',
  OPTIONS: 'Capabilities',
  CONNECT: 'Tunnel',
  TRACE: 'Echo',
};

const OPTIONS: readonly ComboboxOption[] = HTTP_METHODS.map((m) => ({ value: m, label: <MethodBadge method={m} />, detail: METHOD_HINTS[m] }));

function suggestions(query: string): readonly ComboboxOption[] {
  const q = query.trim().toUpperCase();
  if (!q || (HTTP_METHODS as readonly string[]).includes(q)) return OPTIONS;
  return OPTIONS.filter((o) => o.value.startsWith(q));
}

export interface MethodPickerProps {
  value: string;
  onChange: (method: string) => void;
}

/** Editable method combobox (standard methods + any custom token), coloured by method. */
export function MethodPicker({ value, onChange }: MethodPickerProps) {
  const id = useId();
  return (
    <Combobox
      id={id}
      value={value}
      onValueChange={(v) => onChange(v.toUpperCase().replace(/[^A-Z0-9!#$%&'*+.^_`|~-]/g, '').slice(0, 24))}
      options={suggestions}
      filter={false}
      size="lg"
      className={styles.methodWrap}
      inputClassName={styles.methodInput}
      style={{ '--method-color': methodColorVar(value) } as CSSProperties}
      aria-label="HTTP method"
      onFocus={(e) => e.currentTarget.select()}
      onBlur={() => {
        if (!value.trim()) onChange('GET');
      }}
      trailing={
        <span
          className={styles.methodChevron}
          onMouseDown={(e) => {
            e.preventDefault();
            const input = document.getElementById(id);
            if (input instanceof HTMLInputElement) {
              if (document.activeElement === input) input.blur();
              else input.focus();
            }
          }}
          aria-hidden
        >
          <ChevronDown size={13} strokeWidth={2} />
        </span>
      }
    />
  );
}
