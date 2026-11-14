import clsx from 'clsx';
import { CircleAlert, TriangleAlert } from 'lucide-react';
import { useId, type ReactNode } from 'react';
import styles from './SettingRow.module.css';

export interface SettingRowProps {
  /** Anchor id (`setting-<id>`) used by settings search. */
  id?: string;
  title: ReactNode;
  description?: ReactNode;
  /** The control (switch, input, select…). */
  control?: ReactNode;
  /** `inline`: control on the right. `stacked`: full-width control under the text (lists, tables). */
  layout?: 'inline' | 'stacked';
  error?: ReactNode;
  warning?: ReactNode;
  /** Extra content under the row (e.g. a contextual callout). */
  children?: ReactNode;
  className?: string;
  /** Associates the visible title with a control id. */
  htmlFor?: string;
}

/** One setting: title + description on the left, control on the right (or below). */
export function SettingRow({ id, title, description, control, layout = 'inline', error, warning, children, className, htmlFor }: SettingRowProps) {
  const autoId = useId();
  const descId = `${autoId}-desc`;
  return (
    <div className={clsx(styles.row, styles[layout], className)} id={id ? `setting-${id}` : undefined} data-setting={id}>
      <div className={styles.text}>
        {htmlFor ? (
          <label className={styles.title} htmlFor={htmlFor}>
            {title}
          </label>
        ) : (
          <div className={styles.title}>{title}</div>
        )}
        {description && (
          <p className={styles.description} id={descId}>
            {description}
          </p>
        )}
      </div>
      {control != null && <div className={styles.control}>{control}</div>}
      {(error || warning) && (
        <p className={clsx(styles.message, error ? styles.error : styles.warning)} role={error ? 'alert' : undefined}>
          {error ? <CircleAlert size={12} strokeWidth={2} aria-hidden /> : <TriangleAlert size={12} strokeWidth={2} aria-hidden />}
          <span>{error ?? warning}</span>
        </p>
      )}
      {children && <div className={styles.extra}>{children}</div>}
    </div>
  );
}

/** A card of setting rows with dividers. */
export function SettingsCard({ id, title, description, actions, children, className }: { id?: string; title: ReactNode; description?: ReactNode; actions?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={clsx(styles.card, className)} id={id ? `card-${id}` : undefined} aria-labelledby={id ? `card-${id}-title` : undefined}>
      <header className={styles.cardHeader}>
        <div className={styles.cardTitles}>
          <h3 className={styles.cardTitle} id={id ? `card-${id}-title` : undefined}>
            {title}
          </h3>
          {description && <p className={styles.cardDescription}>{description}</p>}
        </div>
        {actions && <div className={styles.cardActions}>{actions}</div>}
      </header>
      <div className={styles.rows}>{children}</div>
    </section>
  );
}
