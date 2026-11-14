import clsx from 'clsx';
import { ChevronDown, ChevronUp } from 'lucide-react';
import { Fragment, useState, type ReactNode } from 'react';
import { splitUrl } from '../lib/detail';
import { useOverflow } from '../hooks/misc';
import styles from './UrlTitle.module.css';

export interface UrlTitleProps {
  scheme: string;
  host: string;
  port: number;
  /** Path + query. */
  path: string;
  /** Badges rendered inline before the URL (method, status). */
  leading?: ReactNode;
  /** Identity of the exchange: collapses the expanded state when it changes. */
  resetKey?: string | number;
  /** Override the rendered URL (CONNECT/tunnel authorities). */
  plain?: string;
}

/**
 * The URL line of the detail header: badges, then the URL in its own column (hanging indent) —
 * scheme+host emphasised, query dimmed, selectable, wrapping to at most three lines with a
 * "Full URL" toggle when it overflows.
 */
export function UrlTitle({ scheme, host, port, path, leading, resetKey, plain }: UrlTitleProps) {
  const [expandedFor, setExpandedFor] = useState<string | number | undefined | null>(null);
  const expanded = expandedFor === (resetKey ?? '');
  const parts = splitUrl({ scheme, host, port, path });
  const [ref, overflow] = useOverflow<HTMLDivElement>([scheme, host, port, path, expanded, plain]);

  return (
    <div className={styles.wrap}>
      {leading && <span className={styles.leading}>{leading}</span>}
      <div className={styles.urlCol}>
        <div ref={ref} className={clsx(styles.url, !expanded && styles.clamped, 'selectable')} title={expanded ? undefined : `${parts.scheme}${parts.authority}${path}`}>
          {plain != null ? (
            <span className={styles.host}>{plain}</span>
          ) : (
            <>
              <span className={styles.scheme}>{parts.scheme}</span>
              <span className={styles.host}>{parts.authority}</span>
              <span className={styles.path}>
                <Breakable text={parts.path} />
              </span>
              {parts.query && (
                <span className={styles.query}>
                  <Breakable text={parts.query} />
                </span>
              )}
            </>
          )}
        </div>
        {(overflow || expanded) && (
          <button
            type="button"
            className={styles.toggle}
            onClick={() => setExpandedFor(expanded ? null : (resetKey ?? ''))}
            aria-expanded={expanded}
            aria-label={expanded ? 'Collapse URL' : 'Show the full URL'}
          >
            {expanded ? <ChevronUp size={12} strokeWidth={2} aria-hidden /> : <ChevronDown size={12} strokeWidth={2} aria-hidden />}
            <span>{expanded ? 'Less' : 'Full URL'}</span>
          </button>
        )}
      </div>
    </div>
  );
}

/** Text with preferred line-break opportunities after URL separators (`/ ? & = ,`). */
function Breakable({ text }: { text: string }) {
  const parts = text.split(/(?<=[/?&=,;])/);
  return (
    <>
      {parts.map((p, i) => (
        <Fragment key={i}>
          {i > 0 && <wbr />}
          {p}
        </Fragment>
      ))}
    </>
  );
}
