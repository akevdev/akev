import clsx from 'clsx';
import { CircleAlert, Pencil, Plus, X } from 'lucide-react';
import { useEffect, useMemo, useReducer, useRef, useState, type KeyboardEvent } from 'react';
import { Button, IconButton, Input, undoToast } from '../../components/ui';
import { draftError, initialListState, makeListReducer, type ListOptions } from './stringList';
import styles from './StringListEditor.module.css';

export interface StringListEditorProps extends ListOptions {
  items: readonly string[];
  onChange: (items: string[]) => void;
  /** Placeholder of the add field. */
  placeholder?: string;
  /** Noun for messages ("host", "path"). */
  noun?: string;
  /** Shown when the list is empty. */
  emptyText?: string;
  mono?: boolean;
  'aria-label': string;
  disabled?: boolean;
}

function sameList(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

/**
 * Editable list of strings (hosts, globs, paths): add (paste several at once), inline edit
 * (double-click / Enter / pencil), remove with undo, validation and duplicate detection.
 */
export function StringListEditor({ items, onChange, validate, normalize, splitPaste, placeholder = 'Add…', noun = 'entry', emptyText, mono = true, disabled, ...aria }: StringListEditorProps) {
  const opts = useMemo<ListOptions>(() => ({ validate, normalize, splitPaste }), [validate, normalize, splitPaste]);
  const reducer = useMemo(() => makeListReducer(opts), [opts]);
  const [state, dispatch] = useReducer(reducer, items as string[], initialListState);
  const [draftTouched, setDraftTouched] = useState(false);
  const emitted = useRef<readonly string[]>(items);
  const addRef = useRef<HTMLInputElement | null>(null);
  const listRef = useRef<HTMLUListElement | null>(null);

  // Local changes → parent.
  useEffect(() => {
    if (!sameList(state.items, items) && state.items !== emitted.current) {
      emitted.current = state.items;
      onChange(state.items);
    }
  }, [state.items]);

  // Parent changes (server echo, revert after a failed save) → local.
  useEffect(() => {
    emitted.current = items;
    if (!sameList(items, state.items)) dispatch({ type: 'reset', items: items as string[] });
  }, [items]);

  const live = draftError(state.items, state.draft, opts);
  const shownError = state.draftError ?? (draftTouched ? live : null);

  const add = () => {
    setDraftTouched(true);
    dispatch({ type: 'add' });
  };

  const remove = (index: number) => {
    const value = state.items[index];
    if (value == null) return;
    dispatch({ type: 'remove', index });
    undoToast(`Removed ${value}`, () => dispatch({ type: 'restore', index, value }));
    // Keep keyboard focus in the list.
    requestAnimationFrame(() => {
      const rows = listRef.current?.querySelectorAll<HTMLElement>('[data-row]');
      const next = rows?.[Math.min(index, (rows?.length ?? 1) - 1)];
      (next ?? addRef.current)?.focus();
    });
  };

  const onRowKeyDown = (e: KeyboardEvent<HTMLLIElement>, index: number) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === 'Enter' || e.key === 'F2') {
      e.preventDefault();
      dispatch({ type: 'edit-start', index });
    } else if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      remove(index);
    } else if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault();
      const rows = listRef.current?.querySelectorAll<HTMLElement>('[data-row]');
      const next = rows?.[index + (e.key === 'ArrowDown' ? 1 : -1)];
      if (next) next.focus();
      else if (e.key === 'ArrowDown') addRef.current?.focus();
    }
  };

  return (
    <div className={clsx(styles.editor, disabled && styles.disabled)} aria-label={aria['aria-label']} role="group">
      {state.items.length > 0 ? (
        <ul className={styles.list} ref={listRef}>
          {state.items.map((item, index) =>
            state.editing?.index === index ? (
              <li key={`${item}-${index}`} className={clsx(styles.row, styles.editingRow)}>
                <Input
                  size="sm"
                  mono={mono}
                  value={state.editing.text}
                  invalid={state.editing.error != null}
                  autoFocus
                  aria-label={`Edit ${item}`}
                  onChange={(e) => dispatch({ type: 'edit-text', text: e.target.value })}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      e.preventDefault();
                      dispatch({ type: 'edit-commit' });
                    } else if (e.key === 'Escape') {
                      e.preventDefault();
                      e.stopPropagation();
                      dispatch({ type: 'edit-cancel' });
                    }
                  }}
                  onBlur={() => dispatch({ type: 'edit-commit' })}
                  className={styles.editInput}
                />
                {state.editing.error && (
                  <span className={styles.rowError}>
                    <CircleAlert size={12} strokeWidth={2} aria-hidden />
                    {state.editing.error}
                  </span>
                )}
              </li>
            ) : (
              <li
                key={`${item}-${index}`}
                className={styles.row}
                // biome-ignore lint/a11y/noNoninteractiveTabindex: list row that edits on Enter and removes on Delete
                tabIndex={0}
                data-row=""
                onDoubleClick={() => !disabled && dispatch({ type: 'edit-start', index })}
                onKeyDown={(e) => !disabled && onRowKeyDown(e, index)}
                aria-label={`${item}. Press Enter to edit, Delete to remove.`}
              >
                <span className={clsx(styles.value, mono && styles.mono)}>{item}</span>
                <span className={styles.rowActions}>
                  <IconButton label="Edit" icon={Pencil} size="sm" tabIndex={-1} disabled={disabled} onClick={() => dispatch({ type: 'edit-start', index })} />
                  <IconButton label="Remove" icon={X} size="sm" variant="danger" tabIndex={-1} disabled={disabled} onClick={() => remove(index)} />
                </span>
              </li>
            ),
          )}
        </ul>
      ) : (
        <div className={styles.empty}>{emptyText ?? `No ${noun}s yet.`}</div>
      )}
      <div className={styles.addRow}>
        <Input
          ref={addRef}
          size="sm"
          mono={mono}
          value={state.draft}
          placeholder={placeholder}
          invalid={shownError != null}
          disabled={disabled}
          aria-label={`Add ${noun}`}
          onChange={(e) => {
            setDraftTouched(false);
            dispatch({ type: 'draft', text: e.target.value });
          }}
          onBlur={() => state.draft.trim() && setDraftTouched(true)}
          onKeyDown={(e) => {
            if (e.key === 'Enter') {
              e.preventDefault();
              add();
            } else if (e.key === 'ArrowUp' && !state.draft) {
              const rows = listRef.current?.querySelectorAll<HTMLElement>('[data-row]');
              rows?.[rows.length - 1]?.focus();
            }
          }}
          onPaste={(e) => {
            const text = e.clipboardData.getData('text');
            if (/[\n,;]/.test(text.trim()) && splitPaste !== false) {
              e.preventDefault();
              dispatch({ type: 'draft', text });
              setDraftTouched(true);
              dispatch({ type: 'add' });
            }
          }}
          className={styles.addInput}
        />
        <Button size="sm" icon={Plus} onClick={add} disabled={disabled || !state.draft.trim() || live != null}>
          Add
        </Button>
      </div>
      {shownError && (
        <p className={styles.error} role="alert">
          <CircleAlert size={12} strokeWidth={2} aria-hidden />
          {shownError}
        </p>
      )}
    </div>
  );
}
