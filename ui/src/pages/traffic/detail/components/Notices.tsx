import clsx from 'clsx';
import { FileX, Info } from 'lucide-react';
import type { BodyMeta, Side } from '../../../../api/types';
import { EmptyState } from '../../../../components/ui';
import { formatBytes } from '../../../../lib/format';
import styles from './Banners.module.css';

/**
 * Why Akev handled this exchange differently from usual (`ExchangeDetail.notices`): a limit was
 * reached, capture was overloaded, a breakpoint released itself… `stacked` when it follows
 * another banner in the header.
 */
export function NoticesBanner({ notices, stacked = false }: { notices: readonly string[]; stacked?: boolean }) {
  if (notices.length === 0) return null;
  return (
    <div className={clsx(styles.banner, styles.warnSoft, stacked && styles.stacked)} role="status">
      <Info size={14} strokeWidth={2} className={styles.icon} aria-hidden />
      <div className={styles.text}>
        {notices.map((n) => (
          <div key={n} className={styles.titleRegular}>
            {n}
          </div>
        ))}
      </div>
    </div>
  );
}

/** Body tab of a body whose bytes were discarded to respect a limit (`BodyMeta.dropped`). */
export function DroppedBody({ meta, side }: { meta: BodyMeta; side: Side }) {
  const size = meta.wireSize > 0 ? ` It was ${formatBytes(meta.wireSize)}.` : '';
  return (
    <EmptyState
      icon={FileX}
      size="sm"
      title={`${side === 'request' ? 'Request' : 'Response'} body not kept`}
      description={`Akev discarded this body to stay within a limit (disk space for bodies, or capture overload); only its size was kept.${size} The exchange notes say which limit.`}
    />
  );
}
