import clsx from 'clsx';
import type { ReactNode } from 'react';
import styles from './Section.module.css';

export interface SectionProps {
  title: ReactNode;
  description?: ReactNode;
  /** Actions aligned right of the title. */
  actions?: ReactNode;
  children?: ReactNode;
  /** Card surface (settings) vs plain block. Default `card`. */
  variant?: 'card' | 'plain';
  className?: string;
  id?: string;
}

/** Titled block used for settings cards and grouped forms. */
export function Section({ title, description, actions, children, variant = 'card', className, id }: SectionProps) {
  return (
    <section className={clsx(styles.section, styles[variant], className)} id={id} aria-labelledby={id ? `${id}-title` : undefined}>
      <header className={styles.header}>
        <div className={styles.titles}>
          <h3 className={styles.title} id={id ? `${id}-title` : undefined}>
            {title}
          </h3>
          {description && <p className={styles.description}>{description}</p>}
        </div>
        {actions && <div className={styles.actions}>{actions}</div>}
      </header>
      {children != null && <div className={styles.body}>{children}</div>}
    </section>
  );
}
