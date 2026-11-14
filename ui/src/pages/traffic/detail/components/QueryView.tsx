// biome-ignore-all lint/a11y/useFocusableInteractive: static ARIA table: rows and cells are not interactive and take no focus
import clsx from 'clsx';
import { Search } from 'lucide-react';
import { useMemo } from 'react';
import { CopyButton, EmptyState } from '../../../../components/ui';
import { pluralize } from '../../../../lib/format';
import { extractQuery, queryParamsToText } from '../lib/query';
import styles from './views.module.css';

export interface QueryViewProps {
  /** Request path (with query) or absolute URL. */
  path: string;
}

/** Decoded query parameters (copy each) + the raw query string (DESIGN.md §6.4 Query tab). */
export function QueryView({ path }: QueryViewProps) {
  const info = useMemo(() => extractQuery(path), [path]);

  if (info.params.length === 0) {
    return <EmptyState icon={Search} size="sm" title="No query parameters" description="This request's URL has no ?query string." />;
  }

  return (
    <div className={styles.stack}>
      <div className={styles.toolbar}>
        <span className={styles.toolbarText}>{pluralize(info.params.length, 'parameter')}</span>
        <CopyButton value={() => queryParamsToText(info.params)} label="Copy all as name=value lines" toastMessage="Parameters copied" />
      </div>
      <div className={styles.kvTable} role="table" aria-label="Query parameters">
        {info.params.map((p) => (
          <div key={p.index} className={styles.kvRow} role="row">
            <div className={styles.kvName} role="cell">
              <span className={clsx('selectable', styles.breakAll)}>{p.name || <span className={styles.dim}>(empty name)</span>}</span>
              {info.duplicates.has(p.name) && <span className={styles.dup}>repeated</span>}
            </div>
            <div className={clsx(styles.kvValue, 'selectable')} role="cell">
              {p.rawValue == null ? <span className={styles.dim}>no value</span> : p.value === '' ? <span className={styles.dim}>empty</span> : p.value}
            </div>
            <div className={styles.kvActions} role="cell">
              <CopyButton value={p.value} label={`Copy ${p.name || 'value'}`} />
            </div>
          </div>
        ))}
      </div>
      <div className={styles.rawBlock}>
        <div className={styles.rawHead}>
          <span className={styles.caps}>Raw query</span>
          <CopyButton value={`?${info.raw}`} label="Copy raw query string" />
        </div>
        <code className={clsx(styles.rawCode, 'selectable')}>?{info.raw}</code>
      </div>
    </div>
  );
}
