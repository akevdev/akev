import clsx from 'clsx';
import { AlertTriangle, CheckCircle2, Database, DatabaseZap, Info } from 'lucide-react';
import { memo, useMemo } from 'react';
import type { ExchangeDetail } from '../../api/types';
import { analyzeCaching, CACHE_CATEGORY_LABELS, cachingHeaders, type CacheFinding, type CacheFindingStatus } from '../../lib/caching';
import { Badge } from '../ui';
import { parseMarkdown } from './model/markdown';
import { Markdown, MarkdownInline } from './parts/Parts';
import styles from './CacheView.module.css';

export interface CacheViewProps {
  detail: ExchangeDetail;
  className?: string;
}

const ICONS: Record<CacheFindingStatus, typeof Info> = { good: CheckCircle2, warn: AlertTriangle, info: Info };
const STATUS_LABEL: Record<CacheFindingStatus, string> = { good: 'Good', warn: 'Warning', info: 'Note' };

const Finding = memo(function Finding({ f }: { f: CacheFinding }) {
  const Icon = ICONS[f.status];
  const blocks = useMemo(() => parseMarkdown(f.explanation), [f.explanation]);
  return (
    <li className={styles.finding}>
      <Icon size={16} strokeWidth={2} className={clsx(styles.icon, styles[f.status])} aria-label={STATUS_LABEL[f.status]} />
      <div>
        <div className={styles.findingHead}>
          <MarkdownInline text={f.summary} className={styles.summary} />
          <span className={styles.category}>{CACHE_CATEGORY_LABELS[f.category]}</span>
        </div>
        <Markdown blocks={blocks} compact className={styles.explanation} />
      </div>
    </li>
  );
});

/**
 * Cache analysis (DESIGN.md §6.4): how browsers, proxies and CDNs will treat the response —
 * cacheability, who may store it, reuse (Vary), freshness, revalidation and header issues —
 * from `lib/caching.ts` (HTTP Toolkit's analysis, fixed and extended).
 */
export function CacheView({ detail, className }: CacheViewProps) {
  // Recomputed per render of a new detail; the analysis is cheap (header scans only).
  const findings = useMemo(() => analyzeCaching(detail), [detail]);
  const headers = useMemo(() => cachingHeaders(detail), [detail]);
  const head = findings[0];
  const lifetime = findings.find((f) => f.category === 'lifetime');
  const storage = findings.find((f) => f.category === 'storage');
  const warnings = findings.filter((f) => f.status === 'warn').length;
  const heroStatus: CacheFindingStatus = head?.status === 'good' ? 'good' : head?.status === 'warn' ? 'warn' : 'info';
  const HeroIcon = heroStatus === 'info' ? Database : DatabaseZap;

  return (
    <div className={clsx(styles.root, className)}>
      {head && (
        <div className={styles.hero}>
          <span className={clsx(styles.heroIcon, styles[heroStatus])} aria-hidden>
            <HeroIcon size={18} strokeWidth={1.9} />
          </span>
          <div className={styles.heroText}>
            <div className={styles.heroTitle}>
              <MarkdownInline text={head.summary} />
            </div>
            {(lifetime || storage) && (
              <div className={styles.heroSub}>
                <MarkdownInline text={[storage?.summary, lifetime?.summary].filter(Boolean).join(' · ')} />
              </div>
            )}
          </div>
          {detail.response && (
            <div className={styles.counts}>
              {warnings > 0 ? (
                <Badge variant="warning" icon={AlertTriangle}>
                  {warnings} {warnings === 1 ? 'issue' : 'issues'}
                </Badge>
              ) : (
                <Badge variant="success" icon={CheckCircle2}>
                  No issues
                </Badge>
              )}
            </div>
          )}
        </div>
      )}

      {detail.response && (
        <div className={styles.headers}>
          <div className={styles.headersTitle}>Caching headers</div>
          {headers.length ? (
            headers.map(([k, v], i) => (
              <div key={i} className={styles.headerRow}>
                <span className={styles.headerName}>{k}</span>
                <span className={styles.headerValue}>{v}</span>
              </div>
            ))
          ) : (
            <div className={styles.noHeaders}>None — no Cache-Control, Expires, ETag or Last-Modified.</div>
          )}
        </div>
      )}

      <ul className={styles.list} aria-label="Caching analysis">
        {findings.map((f) => (
          <Finding key={f.id} f={f} />
        ))}
      </ul>
      <div className={styles.footnote}>Based on RFC 9111 and common browser behaviour. Heuristic lifetimes vary between caches.</div>
    </div>
  );
}
