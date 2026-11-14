import clsx from 'clsx';
import { Star, Trash2 } from 'lucide-react';
import { useEffect, useRef, useState } from 'react';
import { Button, IconButton, Input, Popover, PopoverContent, PopoverTrigger, toast, Tooltip } from '../../../components/ui';
import { useUiStore, type SavedFilter } from '../../../state/ui';
import { HighlightedQuery } from './HighlightedQuery';
import styles from './FilterBar.module.css';

export interface SavedFiltersProps {
  /** Current filter text. */
  text: string;
  /** The saved filter equal to the current text, if any. */
  match: SavedFilter | undefined;
  onApply: (query: string) => void;
  buttonClassName?: string;
}

/** Suggest a name from the query: the first value-ish words (`status:4xx host:github` → "4xx github"). */
function suggestName(query: string): string {
  const words = query
    .trim()
    .split(/\s+/)
    .map((w) => w.replace(/^-/, 'not ').replace(/^[a-z.-]+[:=~<>!^$*]+/i, '').replace(/["']/g, ''))
    .filter(Boolean);
  return (words.join(' ') || query).slice(0, 40);
}

/**
 * Star button + popover: save the current filter under a name, and the list of saved filters
 * (click to apply, trash to remove). Filled star = the current filter is saved.
 */
export function SavedFilters({ text, match, onApply, buttonClassName }: SavedFiltersProps) {
  const saved = useUiStore((s) => s.savedFilters);
  const [open, setOpen] = useState(false);
  const [name, setName] = useState('');
  const inputRef = useRef<HTMLInputElement | null>(null);
  const canSave = text.trim().length > 0 && !match;

  useEffect(() => {
    if (open) setName(match?.name ?? suggestName(text));
  }, [open, match, text]);

  const save = () => {
    const n = name.trim();
    if (!n || !text.trim()) return;
    const replacing = saved.some((f) => f.name === n);
    useUiStore.getState().saveFilter({ name: n, query: text.trim() });
    toast.success(replacing ? `Updated saved filter “${n}”` : `Saved filter “${n}”`);
    setOpen(false);
  };

  const remove = (f: SavedFilter) => {
    useUiStore.getState().removeSavedFilter(f.name);
    toast(`Removed saved filter “${f.name}”`, { action: { label: 'Undo', onClick: () => useUiStore.getState().saveFilter(f) } });
  };

  const label = match ? `Saved as “${match.name}”` : 'Saved filters';

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <Tooltip content={label}>
        <PopoverTrigger asChild>
          <IconButton size="sm" label={label} tooltip={false} className={clsx(buttonClassName, match && styles.starred)} active={open || undefined}>
            <Star size={14} strokeWidth={1.75} fill={match ? 'currentColor' : 'none'} aria-hidden />
          </IconButton>
        </PopoverTrigger>
      </Tooltip>
      <PopoverContent
        align="end"
        width={380}
        flush
        className={styles.savedPopover}
        onOpenAutoFocus={(e) => {
          if (!canSave) return;
          e.preventDefault();
          requestAnimationFrame(() => inputRef.current?.select());
        }}
        onCloseAutoFocus={(e) => e.preventDefault()}
      >
        <div className={styles.savedHeader}>Saved filters</div>
        {canSave && (
          <form
            className={styles.saveForm}
            onSubmit={(e) => {
              e.preventDefault();
              save();
            }}
          >
            <div className={styles.saveQuery}>
              <HighlightedQuery text={text.trim()} />
            </div>
            <div className={styles.saveRow}>
              <Input ref={inputRef} size="sm" value={name} onChange={(e) => setName(e.target.value)} placeholder="Name this filter" aria-label="Saved filter name" className={styles.saveName} />
              <Button size="sm" variant="primary" type="submit" icon={Star} disabled={!name.trim()}>
                Save
              </Button>
            </div>
          </form>
        )}
        {match && (
          <div className={styles.savedCurrent}>
            <Star size={13} strokeWidth={1.75} fill="currentColor" className={styles.starred} aria-hidden />
            <span>
              Current filter is saved as <strong>{match.name}</strong>
            </span>
          </div>
        )}
        {saved.length === 0 ? (
          <p className={styles.savedEmpty}>{canSave ? 'Saved filters appear here and in the filter dropdown.' : 'Type a filter, then save it here to reuse it in one click.'}</p>
        ) : (
          <ul className={styles.savedList} aria-label="Saved filters">
            {saved.map((f) => (
              <li key={f.name} className={clsx(styles.savedItem, match?.name === f.name && styles.savedItemCurrent)}>
                <button
                  type="button"
                  className={styles.savedApply}
                  onClick={() => {
                    setOpen(false);
                    onApply(f.query);
                  }}
                >
                  <span className={styles.savedItemName}>{f.name}</span>
                  <span className={styles.savedItemQuery}>
                    <HighlightedQuery text={f.query} />
                  </span>
                </button>
                <IconButton size="sm" variant="danger" icon={Trash2} label={`Remove “${f.name}”`} className={styles.savedRemove} onClick={() => remove(f)} />
              </li>
            ))}
          </ul>
        )}
      </PopoverContent>
    </Popover>
  );
}
