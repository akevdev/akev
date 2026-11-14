/**
 * State machine of the string-list editor (TLS passthrough hosts, ignored hosts, trusted CA
 * paths, no-proxy list…): an "add" draft, inline editing of one entry, validation and
 * duplicate detection. Pure; the component dispatches and reports `items` changes upward.
 */

export interface ListOptions {
  /** Return an error message or null. */
  validate?: (value: string) => string | null;
  /** Normalise an entry before validation/storage (e.g. lowercase hosts). */
  normalize?: (value: string) => string;
  /** Allow pasting several entries separated by newlines / commas / spaces. Default true. */
  splitPaste?: boolean;
}

export interface ListState {
  items: string[];
  draft: string;
  draftError: string | null;
  /** Entry being edited inline. */
  editing: { index: number; text: string; error: string | null } | null;
}

export type ListAction =
  | { type: 'reset'; items: string[] }
  | { type: 'draft'; text: string }
  | { type: 'add' }
  | { type: 'remove'; index: number }
  | { type: 'restore'; index: number; value: string }
  | { type: 'edit-start'; index: number }
  | { type: 'edit-text'; text: string }
  | { type: 'edit-commit' }
  | { type: 'edit-cancel' };

export function initialListState(items: readonly string[]): ListState {
  return { items: [...items], draft: '', draftError: null, editing: null };
}

const defaultNormalize = (s: string) => s.trim();

/** Split pasted text into entries (newlines, commas, whitespace). */
export function splitEntries(input: string): string[] {
  return input
    .split(/[\n,;\s]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export interface AddResult {
  items: string[];
  added: string[];
  /** Error describing rejected entries (null when everything was added). */
  error: string | null;
  /** Entries that were rejected, to keep in the draft for fixing. */
  rejected: string[];
}

/** Add one or many entries, skipping invalid ones and duplicates. */
export function addEntries(items: readonly string[], input: string, opts: ListOptions = {}): AddResult {
  const normalize = opts.normalize ?? defaultNormalize;
  const candidates = opts.splitPaste === false ? [input.trim()].filter(Boolean) : splitEntries(input);
  const next = [...items];
  const seen = new Set(items.map((i) => normalize(i)));
  const added: string[] = [];
  const rejected: string[] = [];
  let firstError: string | null = null;
  for (const raw of candidates) {
    const value = normalize(raw);
    const error = opts.validate?.(value) ?? (seen.has(value) ? 'Already in the list' : null);
    if (error) {
      rejected.push(raw);
      firstError ??= error;
      continue;
    }
    seen.add(value);
    next.push(value);
    added.push(value);
  }
  let error: string | null = null;
  if (candidates.length === 0) error = null;
  else if (rejected.length === 1 && candidates.length === 1) error = firstError;
  else if (rejected.length > 0) error = `${rejected.length} of ${candidates.length} entries skipped — ${firstError}`;
  return { items: next, added, error, rejected };
}

/** Replace the entry at `index`; returns an error instead when invalid / duplicate. */
export function replaceEntry(items: readonly string[], index: number, input: string, opts: ListOptions = {}): { items: string[]; error: string | null } {
  const normalize = opts.normalize ?? defaultNormalize;
  const value = normalize(input);
  if (!value) return { items: items.filter((_, i) => i !== index), error: null }; // emptied → removed
  const error = opts.validate?.(value) ?? null;
  if (error) return { items: [...items], error };
  if (items.some((x, i) => i !== index && normalize(x) === value)) return { items: [...items], error: 'Already in the list' };
  const next = [...items];
  next[index] = value;
  return { items: next, error: null };
}

export function removeEntry(items: readonly string[], index: number): string[] {
  return items.filter((_, i) => i !== index);
}

/** Reducer for the editor (pass validation options via the factory). */
export function makeListReducer(opts: ListOptions = {}) {
  return function reducer(state: ListState, action: ListAction): ListState {
    switch (action.type) {
      case 'reset':
        return { ...state, items: [...action.items], editing: state.editing && state.editing.index < action.items.length ? state.editing : null };
      case 'draft':
        return { ...state, draft: action.text, draftError: null };
      case 'add': {
        if (!state.draft.trim()) return { ...state, draftError: null };
        const res = addEntries(state.items, state.draft, opts);
        return { ...state, items: res.items, draft: res.rejected.join(' '), draftError: res.error };
      }
      case 'remove':
        return { ...state, items: removeEntry(state.items, action.index), editing: null };
      case 'restore': {
        const items = [...state.items];
        items.splice(Math.min(action.index, items.length), 0, action.value);
        return { ...state, items };
      }
      case 'edit-start': {
        const text = state.items[action.index];
        return text == null ? state : { ...state, editing: { index: action.index, text, error: null } };
      }
      case 'edit-text':
        return state.editing ? { ...state, editing: { ...state.editing, text: action.text, error: null } } : state;
      case 'edit-commit': {
        if (!state.editing) return state;
        const res = replaceEntry(state.items, state.editing.index, state.editing.text, opts);
        if (res.error) return { ...state, editing: { ...state.editing, error: res.error } };
        return { ...state, items: res.items, editing: null };
      }
      case 'edit-cancel':
        return { ...state, editing: null };
    }
  };
}

/** Live validation of the draft while typing (does not block typing). */
export function draftError(items: readonly string[], draft: string, opts: ListOptions = {}): string | null {
  const parts = opts.splitPaste === false ? [draft.trim()] : splitEntries(draft);
  if (parts.length !== 1) return null; // multi-entry pastes are validated on add
  const normalize = opts.normalize ?? defaultNormalize;
  const value = normalize(parts[0] as string);
  if (!value) return null;
  return opts.validate?.(value) ?? (items.some((x) => normalize(x) === value) ? 'Already in the list' : null);
}
