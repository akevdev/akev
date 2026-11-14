import clsx from 'clsx';
import type { LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import styles from './EmptyState.module.css';

export interface EmptyStateProps {
  icon?: LucideIcon;
  title: ReactNode;
  description?: ReactNode;
  /** Buttons / links offering the next step. */
  actions?: ReactNode;
  /** Extra content under the actions (cards, tips). */
  children?: ReactNode;
  size?: 'sm' | 'md' | 'lg';
  /** Accent-tinted icon circle. */
  tone?: 'neutral' | 'accent' | 'warning' | 'danger';
  className?: string;
}

/** Teaching empty state: icon in a soft circle, title, description, next actions. */
export function EmptyState({ icon: Icon, title, description, actions, children, size = 'md', tone = 'neutral', className }: EmptyStateProps) {
  const iconSize = size === 'sm' ? 18 : size === 'lg' ? 28 : 22;
  return (
    <div className={clsx(styles.root, styles[size], className)}>
      {Icon && (
        <div className={clsx(styles.iconWrap, styles[tone])} aria-hidden>
          <Icon size={iconSize} strokeWidth={1.6} />
        </div>
      )}
      <div className={styles.text}>
        <h2 className={styles.title}>{title}</h2>
        {description && <p className={styles.description}>{description}</p>}
      </div>
      {actions && <div className={styles.actions}>{actions}</div>}
      {children}
    </div>
  );
}
