import clsx from 'clsx';
import { memo, type CSSProperties } from 'react';
import { tagColorVar, methodColorVar, statusColorVar } from '../../lib/http';
import type { Token } from '../../lib/rules/describe';
import styles from './Tokens.module.css';

export interface TokensProps {
  tokens: readonly Token[];
  className?: string;
  /** Hide UI-only detail tokens. */
  plain?: boolean;
}

/** Renders a rule summary with coloured methods, targets, values, statuses and actions. */
export const Tokens = memo(function Tokens({ tokens, className, plain }: TokensProps) {
  return (
    <span className={clsx(styles.tokens, className)}>
      {tokens.map((t, i) => {
        if (plain && t.extra) return null;
        switch (t.kind) {
          case 'method':
            return (
              <span key={i} className={styles.method} style={{ '--tok-color': methodColorVar(t.text) } as CSSProperties}>
                {t.text}
              </span>
            );
          case 'status':
            return (
              <span key={i} className={styles.status} style={{ '--tok-color': statusColorVar(Number(t.text)) } as CSSProperties}>
                {t.text}
              </span>
            );
          case 'color':
            return (
              <span key={i} className={styles.color} style={{ '--tok-color': tagColorVar(t.text) } as CSSProperties}>
                <span className={styles.swatch} aria-hidden />
                {t.text}
              </span>
            );
          case 'target':
            return (
              <span key={i} className={styles.target}>
                {t.text}
              </span>
            );
          case 'value':
            return (
              <span key={i} className={styles.value}>
                {t.text}
              </span>
            );
          case 'action':
            return (
              <span key={i} className={styles.action}>
                {t.text}
              </span>
            );
          case 'arrow':
            return (
              <span key={i} className={styles.arrow}>
                {t.text}
              </span>
            );
          case 'muted':
            return (
              <span key={i} className={clsx(styles.muted, t.extra && styles.extra)}>
                {t.text}
              </span>
            );
          default:
            return <span key={i}>{t.text}</span>;
        }
      })}
    </span>
  );
});
