import { CornerDownLeft } from 'lucide-react';
import { Fragment, useEffect, useMemo, useState } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { FieldDoc } from '../../../api/types';
import { SearchInput, Skeleton } from '../../../components/ui';
import { HighlightedQuery } from './HighlightedQuery';
import styles from './FilterBar.module.css';

let docsPromise: Promise<FieldDoc[]> | null = null;

/** `/api/filter/docs`, fetched once per session. */
export function loadFilterDocs(): Promise<FieldDoc[]> {
  if (!docsPromise) {
    docsPromise = api.filter.docs().catch((e: unknown) => {
      docsPromise = null;
      throw e;
    });
  }
  return docsPromise;
}

const SYNTAX: readonly { example: string; meaning: string }[] = [
  { example: 'github', meaning: 'URL contains text' },
  { example: 'host:api.github.com', meaning: 'field contains' },
  { example: 'status=404', meaning: 'exact match' },
  { example: 'method:get,post', meaning: 'any of' },
  { example: 'status:4xx', meaning: 'class or range' },
  { example: 'size:>1mb', meaning: 'compare with units' },
  { example: 'path^=/v1', meaning: 'prefix · $= suffix' },
  { example: 'url~/users\\/\\d+/', meaning: 'regular expression' },
  { example: '-is:ws', meaning: 'negate (or NOT)' },
  { example: 'host:a OR host:b', meaning: 'either (AND default)' },
  { example: '(is:error OR status:5xx)', meaning: 'group with ( )' },
  { example: 'body:"not found"', meaning: 'quoted value' },
];

export interface FilterHelpProps {
  /** Apply an example (`append` when Shift is held). */
  onExample: (example: string, append: boolean) => void;
}

/** The "?" popover: syntax cheat sheet and every field with clickable examples. */
export function FilterHelp({ onExample }: FilterHelpProps) {
  const [docs, setDocs] = useState<FieldDoc[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [q, setQ] = useState('');

  useEffect(() => {
    let live = true;
    loadFilterDocs().then(
      (d) => live && setDocs(d),
      (e: unknown) => live && setError(errorMessage(e)),
    );
    return () => {
      live = false;
    };
  }, []);

  const shown = useMemo(() => {
    if (!docs) return [];
    const needle = q.trim().toLowerCase();
    if (!needle) return docs;
    return docs.filter(
      (d) =>
        d.name.includes(needle) ||
        d.aliases.some((a) => a.includes(needle)) ||
        d.description.toLowerCase().includes(needle) ||
        d.examples.some((e) => e.toLowerCase().includes(needle)),
    );
  }, [docs, q]);

  const example = (ex: string, key: string) => (
    <button
      key={key}
      type="button"
      className={styles.example}
      onClick={(e) => onExample(ex, e.shiftKey)}
      aria-label={`Use filter ${ex}`}
    >
      <span className={styles.exampleText}>
        <HighlightedQuery text={ex} />
      </span>
      <CornerDownLeft size={11} strokeWidth={2} className={styles.exampleIcon} aria-hidden />
    </button>
  );

  return (
    <div className={styles.help}>
      <div className={styles.helpHeader}>
        <div className={styles.helpTitle}>
          <span>Filter syntax</span>
          <span className={styles.helpHint}>Click an example to use it · Shift-click to add it</span>
        </div>
        <SearchInput size="sm" value={q} onValueChange={setQ} placeholder="Search fields" className={styles.helpSearch} autoFocus aria-label="Search filter fields" />
      </div>
      <div className={styles.helpBody}>
        {!q && (
          <section className={styles.helpSection} aria-label="Syntax">
            {/* Four aligned columns: example · meaning · example · meaning. */}
            <div className={styles.syntaxGrid}>
              {SYNTAX.map((s) => (
                <Fragment key={s.example}>
                  {example(s.example, s.example)}
                  <span className={styles.syntaxMeaning} title={s.meaning}>
                    {s.meaning}
                  </span>
                </Fragment>
              ))}
            </div>
          </section>
        )}
        <section className={styles.helpSection} aria-label="Fields">
          <h3 className={styles.helpSectionTitle}>Fields{docs ? ` · ${shown.length}` : ''}</h3>
          {error && <p className={styles.helpError}>Couldn't load the field reference: {error}</p>}
          {!docs && !error && (
            <div className={styles.helpLoading}>
              <Skeleton lines={4} height={10} />
            </div>
          )}
          {docs && shown.length === 0 && <p className={styles.helpEmpty}>No field matches “{q}”.</p>}
          <dl className={styles.fields}>
            {shown.map((d) => (
              <div key={d.name} className={styles.fieldRow}>
                <dt className={styles.fieldName}>
                  <span className={styles.fieldKey}>{d.name}</span>
                  {d.aliases.length > 0 && <span className={styles.fieldAliases}>{d.aliases.join(', ')}</span>}
                </dt>
                <dd className={styles.fieldDesc}>
                  <span>{d.description}</span>
                  <span className={styles.examples}>{d.examples.map((ex, i) => example(ex, `${d.name}-${i}`))}</span>
                </dd>
              </div>
            ))}
          </dl>
        </section>
      </div>
    </div>
  );
}
