import { ChevronDown, RotateCcw, SlidersHorizontal } from 'lucide-react';
import { useState } from 'react';
import type { HttpVersionChoice } from '../../api/types';
import { Button, NumberInput, Popover, PopoverContent, PopoverTrigger, Select, type SelectOption, Switch, Tooltip } from '../../components/ui';
import { DEFAULT_OPTIONS, type RequestOptions } from './model';
import styles from './RequestLine.module.css';

export interface OptionsPopoverProps {
  value: RequestOptions;
  onChange: (options: RequestOptions) => void;
}

function changedCount(o: RequestOptions): number {
  return (Object.keys(DEFAULT_OPTIONS) as (keyof RequestOptions)[]).filter((k) => o[k] !== DEFAULT_OPTIONS[k]).length;
}

const HTTP_VERSIONS: SelectOption<HttpVersionChoice>[] = [
  { value: 'auto', label: 'Auto', description: 'Settings decide' },
  { value: 'http1', label: 'HTTP/1.1' },
  { value: 'http2', label: 'HTTP/2', description: 'TCP, falls back to HTTP/1.1' },
  { value: 'http3', label: 'HTTP/3', description: 'QUIC, falls back to TCP' },
];

/** What each `httpVersion` choice does (the hint under "HTTP version"). */
export const HTTP_VERSION_HINTS: Record<HttpVersionChoice, string> = {
  auto: 'HTTP/2 when the server offers it; HTTP/3 as set in Settings → Proxy.',
  http1: 'HTTP/1.1 only — never HTTP/2 or HTTP/3.',
  http2: 'Over TCP: HTTP/2 when the server offers it, else HTTP/1.1.',
  http3: "Over QUIC first; falls back to TCP when the server doesn't answer.",
};

/** Per-request send options (`SendOptions`): rules, redirects, TLS, HTTP version, timeout. */
export function OptionsPopover({ value, onChange }: OptionsPopoverProps) {
  const [open, setOpen] = useState(false);
  const changed = changedCount(value);
  const set = <K extends keyof RequestOptions>(key: K, v: RequestOptions[K]) => onChange({ ...value, [key]: v });
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip content={changed ? `Request options · ${changed} changed` : 'Request options'}>
        <PopoverTrigger asChild>
          <button type="button" className={styles.optionsButton} aria-label="Request options" data-open={open || undefined}>
            <SlidersHorizontal size={15} strokeWidth={1.75} aria-hidden />
            <ChevronDown size={12} strokeWidth={2} aria-hidden className={styles.optionsChevron} />
            {changed > 0 && <span className={styles.optionsDot} aria-hidden />}
          </button>
        </PopoverTrigger>
      </Tooltip>
      <PopoverContent align="end" width={340} className={styles.optionsPanel}>
        <div className={styles.optionsHead}>
          <span className={styles.optionsTitle}>Request options</span>
          {changed > 0 && (
            <Button size="sm" variant="ghost" icon={RotateCcw} onClick={() => onChange({ ...DEFAULT_OPTIONS })}>
              Reset
            </Button>
          )}
        </div>
        <div className={styles.optionsList}>
          <Switch
            size="sm"
            checked={value.applyRules}
            onCheckedChange={(c) => set('applyRules', c)}
            label="Apply rules"
            description="Send through your rules — mocks, rewrites and breakpoints apply."
          />
          <Switch size="sm" checked={value.followRedirects} onCheckedChange={(c) => set('followRedirects', c)} label="Follow redirects" description="Follow 3xx responses and show the final one." />
          <Switch size="sm" checked={value.insecure} onCheckedChange={(c) => set('insecure', c)} label="Ignore TLS errors" description="Accept expired, self-signed or mismatched certificates." />
        </div>
        <div className={styles.optionsTimeout}>
          <div className={styles.optionsTimeoutText}>
            <span className={styles.optionsLabel}>HTTP version</span>
            <span className={styles.optionsHint}>{HTTP_VERSION_HINTS[value.httpVersion]}</span>
          </div>
          <Select<HttpVersionChoice> size="sm" className={styles.timeoutInput} value={value.httpVersion} onValueChange={(v) => set('httpVersion', v)} options={HTTP_VERSIONS} aria-label="HTTP version" renderValue={(o) => o?.label} />
        </div>
        <div className={styles.optionsTimeout}>
          <div className={styles.optionsTimeoutText}>
            <span className={styles.optionsLabel}>Timeout</span>
            <span className={styles.optionsHint}>Empty uses the proxy's response timeout.</span>
          </div>
          <NumberInput
            size="sm"
            className={styles.timeoutInput}
            value={value.timeoutMs > 0 ? value.timeoutMs / 1000 : null}
            onValueChange={(s) => set('timeoutMs', s == null ? 0 : Math.round(s * 1000))}
            allowEmpty
            min={0.1}
            max={3600}
            step={1}
            precision={1}
            unit="s"
            placeholder="Default"
            aria-label="Timeout in seconds"
          />
        </div>
      </PopoverContent>
    </Popover>
  );
}
