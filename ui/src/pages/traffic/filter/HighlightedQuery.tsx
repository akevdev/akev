import clsx from 'clsx';
import { Fragment, useMemo, type ReactNode } from 'react';
import { tokenizeFilter, type FilterToken } from './filterTokens';
import styles from './FilterBar.module.css';

const KIND_CLASS: Record<FilterToken['kind'], string | undefined> = {
  space: undefined,
  paren: styles.tParen,
  keyword: styles.tKeyword,
  negation: styles.tNegation,
  field: styles.tField,
  operator: styles.tOperator,
  value: styles.tValue,
  string: styles.tString,
  regex: styles.tRegex,
  punct: styles.tOperator,
  text: styles.tText,
};

export interface HighlightedQueryProps {
  text: string;
  /** UTF-16 range to underline as an error (`start === end` draws a caret marker). */
  error?: { start: number; end: number } | null;
}

/**
 * Syntax-highlighted filter text (fields accent, operators muted, negation danger…), with an
 * optional wavy error underline. Renders exactly the input's characters, so it can sit under
 * a transparent `<input>` as a highlight mirror.
 */
export function HighlightedQuery({ text, error }: HighlightedQueryProps) {
  const tokens = useMemo(() => tokenizeFilter(text), [text]);
  const nodes: ReactNode[] = [];
  const errStart = error ? Math.max(0, Math.min(text.length, error.start)) : -1;
  const errEnd = error ? Math.max(errStart, Math.min(text.length, error.end)) : -1;
  let markerPlaced = !error || errEnd > errStart;

  const push = (t: FilterToken, from: number, to: number, isError: boolean, key: string) => {
    if (to <= from) return;
    const cls = KIND_CLASS[t.kind];
    const content = text.slice(from, to);
    nodes.push(
      isError ? (
        <span key={key} data-filter-error="" className={clsx(cls, styles.tError)}>
          {content}
        </span>
      ) : cls ? (
        <span key={key} className={cls}>
          {content}
        </span>
      ) : (
        <Fragment key={key}>{content}</Fragment>
      ),
    );
  };

  for (let i = 0; i < tokens.length; i++) {
    const t = tokens[i] as FilterToken;
    if (!markerPlaced && errStart <= t.start) {
      nodes.push(<span key={`m${i}`} data-filter-error="" className={styles.tErrorMarker} />);
      markerPlaced = true;
    }
    if (!error || errEnd <= t.start || errStart >= t.end || errEnd === errStart) {
      push(t, t.start, t.end, false, `t${i}`);
      continue;
    }
    push(t, t.start, Math.max(t.start, errStart), false, `a${i}`);
    push(t, Math.max(t.start, errStart), Math.min(t.end, errEnd), true, `e${i}`);
    push(t, Math.min(t.end, errEnd), t.end, false, `b${i}`);
  }
  if (!markerPlaced) nodes.push(<span key="m-end" data-filter-error="" className={styles.tErrorMarker} />);
  return <>{nodes}</>;
}
