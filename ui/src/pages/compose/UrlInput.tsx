import * as RadixPopover from '@radix-ui/react-popover';
import clsx from 'clsx';
import { useId, useLayoutEffect, useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent, type Ref } from 'react';
import { urlSpans } from './highlight';
import { applyVarCompletion, DYNAMIC_VARS, varCompletionAt, type VarCompletion, type VarMap } from './variables';
import styles from './UrlInput.module.css';

export interface UrlInputProps {
  value: string;
  onChange: (value: string) => void;
  vars: VarMap;
  /** Return true when the paste was handled (e.g. a cURL command). */
  onPasteText?: (text: string) => boolean;
  /** A large chunk of text was inserted at once (drop, IME, automation) — same contract. */
  onBulkInsert?: (value: string) => boolean;
  onSubmit?: () => void;
  placeholder?: string;
  inputRef?: Ref<HTMLInputElement>;
  'aria-label'?: string;
  invalid?: boolean;
}

interface Suggest {
  completion: VarCompletion;
  caret: number;
  items: { name: string; detail: string }[];
  active: number;
  /** Viewport x/y of the completion start (anchor). */
  x: number;
  y: number;
  h: number;
}

let charWidthCache: number | null = null;
function measureCharWidth(input: HTMLInputElement): number {
  if (charWidthCache) return charWidthCache;
  const canvas = document.createElement('canvas');
  const ctx = canvas.getContext('2d');
  if (!ctx) return 7.8;
  const cs = getComputedStyle(input);
  ctx.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
  charWidthCache = ctx.measureText('0000000000').width / 10;
  return charWidthCache;
}

/**
 * Monospace URL field with syntax colouring and `{{variable}}` highlighting (a transparent
 * input over a mirrored backdrop), `{{` completion of variable names, and paste hooks.
 */
export function UrlInput({ value, onChange, vars, onPasteText, onBulkInsert, onSubmit, placeholder, inputRef, invalid, ...aria }: UrlInputProps) {
  const local = useRef<HTMLInputElement | null>(null);
  const backdrop = useRef<HTMLDivElement | null>(null);
  const [suggest, setSuggest] = useState<Suggest | null>(null);
  const listId = useId();
  const spans = useMemo(() => urlSpans(value, vars), [value, vars]);

  const syncScroll = () => {
    const el = local.current;
    if (el && backdrop.current) backdrop.current.style.transform = `translateX(${-el.scrollLeft}px)`;
  };
  useLayoutEffect(syncScroll, [value]);

  const names = useMemo(() => {
    const out: { name: string; detail: string }[] = [];
    for (const [k, v] of vars) out.push({ name: k, detail: v });
    for (const d of DYNAMIC_VARS) out.push({ name: d.name, detail: d.description });
    return out;
  }, [vars]);

  const updateSuggest = (text: string, caret: number | null) => {
    const el = local.current;
    if (!el || caret == null) return setSuggest(null);
    const completion = varCompletionAt(text, caret);
    if (!completion) return setSuggest(null);
    const q = completion.partial.toLowerCase();
    const items = names.filter((n) => n.name.toLowerCase().includes(q)).sort((a, b) => Number(!a.name.toLowerCase().startsWith(q)) - Number(!b.name.toLowerCase().startsWith(q))).slice(0, 8);
    if (!items.length) return setSuggest(null);
    const rect = el.getBoundingClientRect();
    const x = rect.left + completion.start * measureCharWidth(el) - el.scrollLeft;
    setSuggest((prev) => ({ completion, caret, items, active: prev && prev.completion.start === completion.start ? Math.min(prev.active, items.length - 1) : 0, x: Math.max(rect.left, Math.min(x, rect.right - 40)), y: rect.top, h: rect.height }));
  };

  const accept = (name: string) => {
    const el = local.current;
    if (!el || !suggest) return;
    const next = applyVarCompletion(value, suggest.caret, suggest.completion, name);
    onChange(next.text);
    setSuggest(null);
    requestAnimationFrame(() => {
      el.setSelectionRange(next.caret, next.caret);
      syncScroll();
    });
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (suggest) {
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault();
        const n = suggest.items.length;
        setSuggest({ ...suggest, active: (suggest.active + (e.key === 'ArrowDown' ? 1 : -1) + n) % n });
        return;
      }
      if ((e.key === 'Enter' && !e.metaKey && !e.ctrlKey) || e.key === 'Tab') {
        const item = suggest.items[suggest.active];
        if (item) {
          e.preventDefault();
          accept(item.name);
          return;
        }
      }
      if (e.key === 'Escape') {
        e.preventDefault();
        e.stopPropagation();
        setSuggest(null);
        return;
      }
    }
    if (e.key === 'Enter' && !e.metaKey && !e.ctrlKey && !e.altKey && !e.shiftKey) {
      e.preventDefault();
      onSubmit?.();
    }
  };

  const onPaste = (e: ClipboardEvent<HTMLInputElement>) => {
    const text = e.clipboardData.getData('text');
    if (text && onPasteText?.(text)) e.preventDefault();
  };

  return (
    <div className={clsx(styles.field, invalid && styles.invalid)}>
      <div className={styles.clip} aria-hidden>
        <div ref={backdrop} className={styles.backdrop}>
          {spans.map((s, i) => (
            <span key={i} className={styles[s.kind]}>
              {s.text}
            </span>
          ))}
        </div>
      </div>
      <input
        ref={(el) => {
          local.current = el;
          if (typeof inputRef === 'function') inputRef(el);
          else if (inputRef) inputRef.current = el;
        }}
        className={styles.input}
        value={value}
        placeholder={placeholder}
        spellCheck={false}
        autoComplete="off"
        autoCapitalize="off"
        autoCorrect="off"
        aria-label={aria['aria-label']}
        aria-invalid={invalid || undefined}
        aria-autocomplete="list"
        aria-expanded={!!suggest}
        aria-controls={suggest ? listId : undefined}
        aria-activedescendant={suggest ? `${listId}-${suggest.active}` : undefined}
        role="combobox"
        onChange={(e) => {
          const next = e.target.value;
          const inserted = next.length - value.length;
          if (inserted > 6 && onBulkInsert?.(next)) return;
          onChange(next);
          updateSuggest(next, e.target.selectionStart);
        }}
        onScroll={syncScroll}
        onSelect={(e) => {
          syncScroll();
          if (suggest) updateSuggest(e.currentTarget.value, e.currentTarget.selectionStart);
        }}
        onKeyUp={syncScroll}
        onBlur={() => setSuggest(null)}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
      />
      <RadixPopover.Root open={!!suggest}>
        <RadixPopover.Anchor
          virtualRef={{ current: { getBoundingClientRect: () => DOMRect.fromRect({ x: suggest?.x ?? 0, y: suggest?.y ?? 0, width: 1, height: suggest?.h ?? 0 }) } }}
        />
        <RadixPopover.Portal>
          <RadixPopover.Content className={styles.suggest} side="bottom" align="start" sideOffset={6} collisionPadding={8} onOpenAutoFocus={(e) => e.preventDefault()} onCloseAutoFocus={(e) => e.preventDefault()}>
            <div className={styles.suggestHead}>Variables</div>
            <div role="listbox" id={listId} aria-label="Variables">
              {suggest?.items.map((item, i) => (
                // biome-ignore lint/a11y/useFocusableInteractive lint/a11y/useKeyWithClickEvents: focus stays in the URL field, which drives the list (aria-activedescendant)
                <div
                  key={item.name}
                  id={`${listId}-${i}`}
                  role="option"
                  aria-selected={i === suggest.active}
                  className={clsx(styles.option, i === suggest.active && styles.optionActive)}
                  onMouseDown={(e) => e.preventDefault()}
                  onMouseMove={() => i !== suggest.active && setSuggest({ ...suggest, active: i })}
                  onClick={() => accept(item.name)}
                >
                  <span className={clsx(styles.optName, item.name.startsWith('$') && styles.optDynamic)}>{item.name}</span>
                  <span className={styles.optDetail}>{item.detail || '(empty)'}</span>
                </div>
              ))}
            </div>
          </RadixPopover.Content>
        </RadixPopover.Portal>
      </RadixPopover.Root>
    </div>
  );
}
