import clsx from 'clsx';
import { LockKeyhole } from 'lucide-react';
import type { ReactNode } from 'react';
import type { Feature } from '../../api/types';
import { featureInfo, PRO_TERMS } from '../../lib/license';
import { openUpgrade, useFeature } from '../../state/license';
import { Button } from '../ui';
import { ProBadge } from './ProBadge';
import styles from './pro.module.css';

export interface ProCalloutProps {
  feature: Feature;
  /** Default: "<Feature> is part of Akev Pro". */
  title?: ReactNode;
  /** Default: the feature's one-line description. */
  children?: ReactNode;
  /** `inline` (default): a compact strip; `panel`: a centred block that stands in for a whole view. */
  variant?: 'inline' | 'panel';
  /** Extra buttons after "Unlock". */
  actions?: ReactNode;
  className?: string;
}

/** A locked Pro feature: what it is and an "Unlock" button that opens the upgrade dialog. */
export function ProCallout({ feature, title, children, variant = 'inline', actions, className }: ProCalloutProps) {
  const info = featureInfo(feature);
  const panel = variant === 'panel';
  return (
    <div className={clsx(styles.callout, panel && styles.panel, className)} role="note" data-pro-feature={feature}>
      {panel ? (
        <span className={styles.panelIcon} aria-hidden>
          <LockKeyhole size={18} strokeWidth={1.75} />
        </span>
      ) : (
        <LockKeyhole size={15} strokeWidth={1.9} className={styles.calloutIcon} aria-hidden />
      )}
      <div className={styles.calloutBody}>
        <div className={styles.calloutTitle}>
          {title ?? `${info.label} is part of Akev Pro`}
          {!panel && <ProBadge always />}
        </div>
        <div className={styles.calloutText}>{children ?? info.description}</div>
      </div>
      <div className={styles.calloutActions}>
        <Button size="sm" variant="primary" onClick={() => openUpgrade(feature)}>
          Unlock
        </Button>
        {actions}
      </div>
      {panel && <span className={styles.terms}>{PRO_TERMS}</span>}
    </div>
  );
}

export interface ProGateProps {
  feature: Feature;
  /**
   * `disable` (default): children stay visible but every control inside is disabled, with the
   * callout above. `replace`: the callout stands in for the children.
   */
  mode?: 'disable' | 'replace';
  title?: ReactNode;
  /** Callout text (default: the feature's description). */
  description?: ReactNode;
  /** `disable` mode without the callout (the call site shows its own). */
  hideCallout?: boolean;
  variant?: ProCalloutProps['variant'];
  children?: ReactNode;
  className?: string;
}

/**
 * Wraps the controls that turn a Pro feature on. Licensed (or no licensing at all): renders
 * the children untouched. Existing data is never hidden in `disable` mode — only its controls
 * are locked.
 */
export function ProGate({ feature, mode = 'disable', title, description, hideCallout, variant, children, className }: ProGateProps) {
  const entitled = useFeature(feature);
  const callout = <ProCallout feature={feature} title={title} variant={variant}>{description}</ProCallout>;
  if (mode === 'replace') return entitled ? <>{children}</> : callout;
  return (
    <div className={clsx(styles.gate, className)}>
      {!entitled && !hideCallout && callout}
      {/* Always a fieldset (only `disabled` toggles) so children keep their state on unlock. */}
      <fieldset disabled={!entitled} className={clsx(styles.fieldset, !entitled && styles.locked)}>
        {children}
      </fieldset>
    </div>
  );
}
