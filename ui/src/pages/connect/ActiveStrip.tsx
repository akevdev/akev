import clsx from 'clsx';
import { Settings2, X } from 'lucide-react';
import type { InterceptorInfo } from '../../api/types';
import { AppIcon, Button, Spinner, Tooltip } from '../../components/ui';
import { deactivateAll, deactivateInterceptor, usePendingSet } from './actions';
import { iconKey, opensSheet, stopsFromPanel } from './catalog';
import styles from './ActiveStrip.module.css';

export interface ActiveStripProps {
  active: readonly InterceptorInfo[];
  all: readonly InterceptorInfo[];
  onOpen: (id: string) => void;
  /** Proxy address for the empty-state hint. */
  proxyAddress?: string | null;
  /** The interceptor list hasn't loaded yet (keeps the strip's space without claiming "none"). */
  loading?: boolean;
}

/**
 * "Connected sources" strip (HTTP Toolkit #598): every active interceptor as a chip with a
 * Stop button, plus Stop all. Always rendered (quiet when empty) so launching the first
 * source doesn't push the catalogue down under the pointer.
 */
export function ActiveStrip({ active, all, onOpen, proxyAddress, loading }: ActiveStripProps) {
  const isPending = usePendingSet();
  if (active.length === 0) {
    return (
      <section className={clsx(styles.strip, styles.emptyStrip)} aria-label="Active interceptors">
        <span className={clsx(styles.label, styles.labelIdle)}>
          <span className={clsx(styles.dot, styles.dotIdle)} aria-hidden />
          None active
        </span>
        {!loading && (
          <span className={styles.emptyHint}>
            Launch a browser or terminal below{proxyAddress ? (
              <>
                {' '}
                — or point any client at <span className={styles.emptyAddr}>{proxyAddress}</span>
              </>
            ) : null}
            . Everything you connect shows up here.
          </span>
        )}
      </section>
    );
  }
  const stoppingAll = active.filter((i) => !stopsFromPanel(i)).every((i) => isPending(`${i.id}:stop`));
  return (
    <section className={styles.strip} aria-label="Active interceptors">
      <span className={styles.label}>
        <span className={styles.dot} aria-hidden />
        <span className="tnum">{active.length}</span> active
      </span>
      <ul className={styles.chips}>
        {active.map((i) => {
          const stopping = isPending(`${i.id}:stop`);
          const configurable = opensSheet(i);
          return (
            <li key={i.id} className={styles.chip}>
              <AppIcon name={iconKey(i.icon)} size={16} />
              {configurable ? (
                <Tooltip content={`Manage ${i.name}`}>
                  <button type="button" className={styles.name} onClick={() => onOpen(i.id)}>
                    {i.name}
                    <Settings2 size={12} strokeWidth={1.9} className={styles.manage} aria-hidden />
                  </button>
                </Tooltip>
              ) : (
                <span className={styles.name}>{i.name}</span>
              )}
              <Tooltip content={stopsFromPanel(i) ? `How to stop ${i.name}` : `Stop ${i.name}`}>
                <button type="button" className={styles.stop} aria-label={`Stop ${i.name}`} disabled={stopping} onClick={() => (stopsFromPanel(i) ? onOpen(i.id) : void deactivateInterceptor(i))}>
                  {stopping ? <Spinner size={11} /> : <X size={12} strokeWidth={2.2} aria-hidden />}
                </button>
              </Tooltip>
            </li>
          );
        })}
      </ul>
      {active.length > 1 && (
        <Button size="sm" variant="ghost" onClick={() => void deactivateAll(all)} loading={stoppingAll} className={styles.stopAll}>
          Stop all
        </Button>
      )}
    </section>
  );
}
