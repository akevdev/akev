import clsx from 'clsx';
import { useEffect, useSyncExternalStore } from 'react';
import { api } from '../../../api/client';
import type { ExchangeSummary } from '../../../api/types';
import { trafficStore } from '../../../state/traffic';
import type { HeaderColumnId, TrafficColumnId } from '../../../state/ui';
import { HeaderValueCache } from './headerValues';
import styles from './TrafficList.module.css';

/** The app-wide cache of header column values (rows on screen only). */
export const headerValueCache = new HeaderValueCache((ids, names, signal) => api.exchanges.headerValues(ids, names, signal));

trafficStore.onLifecycle((e) => {
  if (e.type === 'removed') headerValueCache.forget(e.ids);
  else headerValueCache.reset();
});

/** Tell the cache which header columns are shown (call when the column set changes). */
export function setHeaderColumns(columns: readonly TrafficColumnId[]): void {
  headerValueCache.setColumns(columns);
}

/** Whether an exchange's response headers are final (or will never come). */
function hasResponse(s: ExchangeSummary): boolean {
  return s.status != null || s.error != null || s.state === 'complete' || s.state === 'aborted';
}

/** A custom header column cell: the header's value, loaded with the other rows on screen. */
export function HeaderCell({ s, column }: { s: ExchangeSummary; column: HeaderColumnId }) {
  const version = useSyncExternalStore(headerValueCache.subscribe, headerValueCache.getVersion, headerValueCache.getVersion);
  const httpLike = s.kind !== 'tunnel' && s.kind !== 'tls-failure';
  const complete = hasResponse(s);
  // `version` re-asks after the cache was reset (cheap: a no-op when the value is loaded).
  // biome-ignore lint/correctness/useExhaustiveDependencies: version is the re-run trigger
  useEffect(() => {
    if (httpLike) headerValueCache.ensure(s.id, complete);
  }, [s.id, complete, httpLike, version]);
  const value = httpLike ? headerValueCache.get(s.id, column) : null;
  if (value == null) {
    return (
      <div role="gridcell" className={clsx(styles.cell, styles.subtleText)} aria-busy={value === undefined || undefined}>
        {value === undefined ? '…' : '—'}
      </div>
    );
  }
  return (
    <div role="gridcell" className={clsx(styles.cell, styles.headerValueCell)}>
      <span className="truncate" data-tip={value} data-tip-overflow="">
        {value}
      </span>
    </div>
  );
}
