import clsx from 'clsx';
import { AlertCircle, CircleHelp, Clock, Equal, Flag, Lightbulb, Search, Star, Tag, Type, X, type LucideIcon } from 'lucide-react';
import { useCallback, useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import { api } from '../../../api/client';
import type { Suggestion } from '../../../api/types';
import { IconButton, Kbd, Popover, PopoverContent, PopoverTrigger, Spinner, Tooltip } from '../../../components/ui';
import { byteOffsetToIndex, indexToByteOffset } from '../../../lib/encoding';
import { formatNumber } from '../../../lib/format';
import { trafficStore, useFilterState, useTraffic } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { trafficView } from '../trafficView';
import { applySuggestion, historyStep, suggestionContinues } from './filterEdit';
import { FilterHelp, loadFilterDocs } from './FilterHelp';
import { HighlightedQuery } from './HighlightedQuery';
import { SavedFilters } from './SavedFilters';
import styles from './FilterBar.module.css';

const SUGGEST_DEBOUNCE_MS = 60;

type Option =
  | { type: 'suggestion'; s: Suggestion }
  | { type: 'recent'; query: string }
  | { type: 'saved'; name: string; query: string };

const KIND_ICONS: Record<Suggestion['kind'], LucideIcon> = {
  field: Tag,
  operator: Equal,
  value: Type,
  flag: Flag,
  example: Lightbulb,
};

function optionKey(o: Option): string {
  if (o.type === 'suggestion') return `s:${o.s.kind}:${o.s.label}:${o.s.replaceStart}`;
  return `${o.type}:${o.type === 'saved' ? o.name : o.query}`;
}

/** Commit the current filter text to history (when valid and non-empty). */
function rememberFilter(): void {
  const f = trafficStore.getSnapshot().filter;
  if (f.text.trim() && !f.error) useUiStore.getState().pushRecentFilter(f.text);
}

/**
 * The traffic filter (DESIGN.md §6.2): a native `<input>` (caret, selection, IME, undo all
 * native) layered over a syntax-highlighted mirror, with the daemon's parse error as a wavy
 * underline + tooltip, debounced autocomplete, recent-filter history (↑ in an empty input),
 * saved filters and a field reference.
 */
export function FilterBar({ className }: { className?: string }) {
  const filter = useFilterState();
  const text = filter.text;
  const recent = useUiStore((s) => s.recentFilters);
  const saved = useUiStore((s) => s.savedFilters);
  const matchCount = useTraffic((s) => s.visible.length);

  const inputRef = useRef<HTMLInputElement | null>(null);
  const mirrorRef = useRef<HTMLDivElement | null>(null);
  const listId = useId();

  const [focused, setFocused] = useState(false);
  const [caret, setCaret] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const [suggestions, setSuggestions] = useState<Suggestion[]>([]);
  const [suggestFor, setSuggestFor] = useState<string | null>(null);
  const [highlight, setHighlight] = useState(-1);
  const [helpOpen, setHelpOpen] = useState(false);
  const [errorHover, setErrorHover] = useState(false);
  const [showPending, setShowPending] = useState(false);
  const history = useRef({ index: -1 });

  // ---- focus handle for Ctrl/⌘F and "/"
  useEffect(() => {
    trafficView.registerFilter({
      focus(selectAll = true) {
        const el = inputRef.current;
        if (!el) return;
        el.focus();
        if (selectAll) el.select();
      },
    });
    return () => trafficView.registerFilter(null);
  }, []);

  // Prefetch the field reference so the help popover opens instantly.
  useEffect(() => {
    if (focused) loadFilterDocs().catch(() => undefined);
  }, [focused]);

  // ---- error span (UTF-8 bytes → UTF-16 indices)
  const errorRange = useMemo(() => {
    const e = filter.error;
    if (!e) return null;
    return { start: byteOffsetToIndex(text, e.start), end: byteOffsetToIndex(text, e.end) };
  }, [filter.error, text]);

  // Avoid flashing a spinner for fast filter results.
  useEffect(() => {
    if (!filter.pending || !filter.active) {
      setShowPending(false);
      return;
    }
    const t = setTimeout(() => setShowPending(true), 180);
    return () => clearTimeout(t);
  }, [filter.pending, filter.active]);

  // ---- mirror scroll sync (the input scrolls horizontally on long queries)
  const syncScroll = useCallback(() => {
    const input = inputRef.current;
    const mirror = mirrorRef.current;
    if (input && mirror) mirror.style.transform = `translateX(${-input.scrollLeft}px)`;
  }, []);
  useLayoutEffect(() => {
    syncScroll();
    const id = requestAnimationFrame(syncScroll);
    return () => cancelAnimationFrame(id);
  }, [text, caret, syncScroll]);

  // ---- suggestions (debounced, cancelled when superseded)
  const wantSuggestions = focused && !dismissed && history.current.index < 0;
  useEffect(() => {
    if (!wantSuggestions) return;
    const ctrl = new AbortController();
    const query = text;
    const cursor = indexToByteOffset(query, caret);
    const t = setTimeout(() => {
      api.filter.suggest(query, cursor, ctrl.signal).then(
        (res) => {
          setSuggestions(res);
          setSuggestFor(`${query}\u0000${caret}`);
        },
        () => undefined,
      );
    }, SUGGEST_DEBOUNCE_MS);
    return () => {
      clearTimeout(t);
      ctrl.abort();
    };
  }, [text, caret, wantSuggestions]);

  const options = useMemo<Option[]>(() => {
    const out: Option[] = [];
    const trimmed = text.trim();
    if (!trimmed) {
      for (const f of saved.slice(0, 5)) out.push({ type: 'saved', name: f.name, query: f.query });
      for (const q of recent.slice(0, 6)) if (!saved.some((f) => f.query === q)) out.push({ type: 'recent', query: q });
    } else {
      const needle = trimmed.toLowerCase();
      for (const f of saved) {
        if (out.length >= 3) break;
        if (f.query !== trimmed && (f.name.toLowerCase().includes(needle) || f.query.toLowerCase().startsWith(needle))) out.push({ type: 'saved', name: f.name, query: f.query });
      }
    }
    const fresh = suggestFor === `${text}\u0000${caret}` || (suggestFor?.startsWith(`${text}\u0000`) ?? false);
    // An empty input shows a short, calm list (the "?" reference has every field).
    const limit = trimmed ? 30 : 8;
    if (fresh || !trimmed) for (const s of suggestions.slice(0, limit)) out.push({ type: 'suggestion', s });
    return out;
  }, [text, caret, saved, recent, suggestions, suggestFor]);

  const open = wantSuggestions && (options.length > 0 || !!filter.error);

  // Keep the highlight in range as options change.
  useEffect(() => {
    if (highlight >= options.length) setHighlight(options.length ? options.length - 1 : -1);
  }, [options.length, highlight]);

  // ---- editing helpers
  const setText = (next: string, nextCaret: number, opts: { immediate?: boolean } = {}) => {
    trafficStore.setFilterText(next);
    if (opts.immediate) trafficStore.flushFilter();
    setCaret(nextCaret);
    requestAnimationFrame(() => {
      const el = inputRef.current;
      if (!el) return;
      el.setSelectionRange(nextCaret, nextCaret);
      syncScroll();
    });
  };

  const accept = (o: Option) => {
    history.current.index = -1;
    if (o.type === 'suggestion') {
      const edit = applySuggestion(text, o.s);
      const continues = suggestionContinues(o.s.insert, o.s.kind);
      setText(edit.text, edit.caret, { immediate: !continues });
      setHighlight(-1);
      setDismissed(!continues);
      if (!continues) rememberFilter();
    } else {
      setText(o.query, o.query.length, { immediate: true });
      setHighlight(-1);
      setDismissed(true);
      useUiStore.getState().pushRecentFilter(o.query);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing) return;
    const el = e.currentTarget;
    const browsing = history.current.index >= 0;
    switch (e.key) {
      case 'ArrowDown': {
        e.preventDefault();
        if (browsing) {
          const step = historyStep(recent, history.current.index, -1);
          history.current.index = step.index;
          const next = step.text ?? '';
          setText(next, next.length, { immediate: true });
          if (step.index < 0) setDismissed(false);
          return;
        }
        if (!open) {
          setDismissed(false);
          setHighlight(0);
          return;
        }
        setHighlight((h) => (options.length === 0 ? -1 : (h + 1) % options.length));
        return;
      }
      case 'ArrowUp': {
        if ((text === '' || browsing) && (highlight <= 0 || !open) && recent.length > 0) {
          e.preventDefault();
          const step = historyStep(recent, history.current.index, 1);
          history.current.index = step.index;
          const next = step.text ?? '';
          setText(next, next.length, { immediate: true });
          setHighlight(-1);
          return;
        }
        if (open) {
          e.preventDefault();
          setHighlight((h) => (options.length === 0 ? -1 : h <= 0 ? options.length - 1 : h - 1));
        }
        return;
      }
      case 'Tab': {
        if (!open || e.shiftKey) return;
        const target = options[highlight >= 0 ? highlight : 0];
        if (!target) return;
        e.preventDefault();
        accept(target);
        return;
      }
      case 'Enter': {
        e.preventDefault();
        if (open && highlight >= 0 && options[highlight]) {
          accept(options[highlight] as Option);
          return;
        }
        trafficStore.flushFilter();
        rememberFilter();
        history.current.index = -1;
        setDismissed(true);
        return;
      }
      case 'Escape': {
        e.preventDefault();
        e.stopPropagation();
        if (open) {
          setDismissed(true);
          setHighlight(-1);
          return;
        }
        el.blur();
        trafficView.focusList();
        return;
      }
      default:
    }
  };

  const onMouseMove = (e: ReactMouseEvent<HTMLInputElement>) => {
    if (!errorRange || !mirrorRef.current) {
      if (errorHover) setErrorHover(false);
      return;
    }
    let hit = false;
    for (const el of mirrorRef.current.querySelectorAll<HTMLElement>('[data-filter-error]')) {
      const r = el.getBoundingClientRect();
      const pad = r.width < 6 ? 6 : 0;
      if (e.clientX >= r.left - pad && e.clientX <= r.right + pad && e.clientY >= r.top - 4 && e.clientY <= r.bottom + 4) hit = true;
    }
    if (hit !== errorHover) setErrorHover(hit);
  };

  const applyExample = (example: string, append: boolean) => {
    const next = append && text.trim() ? `${text.trimEnd()} ${example}` : example;
    setHelpOpen(false);
    history.current.index = -1;
    setDismissed(true);
    setText(next, next.length, { immediate: true });
    useUiStore.getState().pushRecentFilter(next);
    requestAnimationFrame(() => inputRef.current?.focus());
  };

  const savedMatch = saved.find((f) => f.query.trim() === text.trim() && text.trim() !== '');

  const renderOption = (o: Option, i: number) => {
    const active = i === highlight;
    let icon: LucideIcon;
    let label: ReactNode;
    let detail: string | undefined;
    if (o.type === 'suggestion') {
      icon = KIND_ICONS[o.s.kind] ?? Tag;
      label = <HighlightedQuery text={o.s.label} />;
      detail = o.s.detail;
    } else if (o.type === 'recent') {
      icon = Clock;
      label = <HighlightedQuery text={o.query} />;
      detail = 'Recent';
    } else {
      icon = Star;
      label = (
        <>
          <span className={styles.savedName}>{o.name}</span>
          <span className={styles.savedQuery}>
            <HighlightedQuery text={o.query} />
          </span>
        </>
      );
      detail = 'Saved';
    }
    const Icon = icon;
    return (
      // biome-ignore lint/a11y/useFocusableInteractive lint/a11y/useKeyWithClickEvents: focus stays in the filter field, which drives the list (aria-activedescendant)
      <div
        key={optionKey(o)}
        id={`${listId}-o${i}`}
        role="option"
        aria-selected={active}
        className={clsx(styles.option, active && styles.optionActive, o.type === 'saved' && styles.optionSaved)}
        onMouseDown={(e) => e.preventDefault()}
        onMouseMove={() => !active && setHighlight(i)}
        onClick={() => accept(o)}
      >
        <Icon size={13} strokeWidth={1.9} className={clsx(styles.optionIcon, o.type === 'saved' && styles.optionIconSaved)} aria-hidden />
        <span className={styles.optionLabel}>{label}</span>
        {detail && <span className={styles.optionDetail}>{detail}</span>}
        {i === (highlight >= 0 ? highlight : 0) && <Kbd combo="tab" size="sm" variant="subtle" className={styles.optionKbd} />}
      </div>
    );
  };

  // Group headers for the empty-input dropdown.
  const sections = useMemo(() => {
    const out: { title: string | null; start: number; end: number }[] = [];
    let cur: string | null | undefined;
    options.forEach((o, i) => {
      const title = o.type === 'saved' ? 'Saved filters' : o.type === 'recent' ? 'Recent' : text.trim() ? null : 'Fields';
      if (title !== cur) {
        out.push({ title, start: i, end: i + 1 });
        cur = title;
      } else (out[out.length - 1] as { end: number }).end = i + 1;
    });
    return out;
  }, [options, text]);

  return (
    <div className={clsx(styles.root, className)}>
      <div className={styles.bar} data-focused={focused || undefined} data-invalid={filter.error ? true : undefined} data-active={filter.active || undefined}>
        <span className={styles.leading} aria-hidden>
          {showPending ? <Spinner size={12} tone="muted" /> : <Search size={14} strokeWidth={1.75} />}
        </span>
        <div className={styles.field}>
          <Tooltip content={filter.error?.message} open={errorHover && !!filter.error} side="bottom" align="start">
            <div className={styles.mirrorClip} aria-hidden>
              <div ref={mirrorRef} className={styles.mirror}>
                <HighlightedQuery text={text} error={errorRange} />
              </div>
            </div>
          </Tooltip>
          <input
            ref={inputRef}
            className={styles.input}
            value={text}
            spellCheck={false}
            autoComplete="off"
            autoCapitalize="off"
            autoCorrect="off"
            placeholder="Filter  ·  host:github.com  status:4xx  -is:ws"
            aria-label="Filter traffic"
            aria-invalid={filter.error ? true : undefined}
            aria-describedby={filter.error ? `${listId}-err` : undefined}
            role="combobox"
            aria-expanded={open}
            aria-controls={`${listId}-list`}
            aria-autocomplete="list"
            aria-activedescendant={open && highlight >= 0 ? `${listId}-o${highlight}` : undefined}
            onChange={(e) => {
              history.current.index = -1;
              setDismissed(false);
              setHighlight(-1);
              trafficStore.setFilterText(e.target.value);
              setCaret(e.target.selectionStart ?? e.target.value.length);
            }}
            onSelect={(e) => setCaret(e.currentTarget.selectionStart ?? 0)}
            onScroll={syncScroll}
            onKeyDown={onKeyDown}
            onKeyUp={syncScroll}
            onMouseMove={onMouseMove}
            onMouseLeave={() => setErrorHover(false)}
            onFocus={(e) => {
              setFocused(true);
              setCaret(e.currentTarget.selectionStart ?? 0);
            }}
            onBlur={() => {
              setFocused(false);
              setDismissed(false);
              setHighlight(-1);
              history.current.index = -1;
              trafficStore.flushFilter();
              rememberFilter();
            }}
          />
          {filter.error && (
            <span id={`${listId}-err`} className="sr-only">
              {filter.error.message}
            </span>
          )}
        </div>
        <span className={styles.trailing}>
          {filter.error ? (
            <Tooltip content={filter.error.message} side="bottom">
              <span className={styles.errorIcon} role="img" aria-label="Filter error" tabIndex={-1}>
                <AlertCircle size={14} strokeWidth={2} />
              </span>
            </Tooltip>
          ) : filter.active && !showPending ? (
            <span className={clsx(styles.count, 'tnum')} aria-live="polite">
              {formatNumber(matchCount)} {matchCount === 1 ? 'match' : 'matches'}
            </span>
          ) : null}
          {text && (
            <IconButton
              size="sm"
              icon={X}
              label="Clear filter"
              className={styles.adornButton}
              onMouseDown={(e) => e.preventDefault()}
              onClick={() => {
                history.current.index = -1;
                setText('', 0, { immediate: true });
                inputRef.current?.focus();
              }}
            />
          )}
          {!text && !focused && <Kbd combo="mod+f" size="sm" className={styles.kbd} />}
          <span className={styles.divider} aria-hidden />
          <SavedFilters text={text} match={savedMatch} onApply={(q) => applyExample(q, false)} buttonClassName={styles.adornButton} />
          <Popover open={helpOpen} onOpenChange={setHelpOpen}>
            <Tooltip content="Filter syntax & fields">
              <PopoverTrigger asChild>
                <IconButton size="sm" icon={CircleHelp} label="Filter syntax & fields" tooltip={false} className={styles.adornButton} active={helpOpen || undefined} />
              </PopoverTrigger>
            </Tooltip>
            <PopoverContent align="end" width={640} flush className={styles.helpPopover} onCloseAutoFocus={(e) => e.preventDefault()}>
              <FilterHelp onExample={applyExample} />
            </PopoverContent>
          </Popover>
        </span>
      </div>

      {open && (
        <div className={styles.dropdown} id={`${listId}-list`} role="listbox" aria-label="Filter suggestions">
          {filter.error && (
            <div className={styles.dropdownError} role="alert">
              <AlertCircle size={13} strokeWidth={2} aria-hidden />
              <span>{filter.error.message}</span>
            </div>
          )}
          {sections.map((sec) => (
            <div key={`${sec.title}-${sec.start}`} role="group" aria-label={sec.title ?? 'Suggestions'}>
              {sec.title && <div className={styles.groupTitle}>{sec.title}</div>}
              {options.slice(sec.start, sec.end).map((o, j) => renderOption(o, sec.start + j))}
            </div>
          ))}
          <div className={styles.dropdownFooter} aria-hidden>
            <span>
              <Kbd combo="arrowup" size="sm" variant="subtle" />
              <Kbd combo="arrowdown" size="sm" variant="subtle" /> navigate
            </span>
            <span>
              <Kbd combo="tab" size="sm" variant="subtle" /> complete
            </span>
            <span>
              <Kbd combo="enter" size="sm" variant="subtle" /> apply
            </span>
            <span>
              <Kbd combo="escape" size="sm" variant="subtle" /> close
            </span>
          </div>
        </div>
      )}
    </div>
  );
}
