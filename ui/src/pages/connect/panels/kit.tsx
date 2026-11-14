import clsx from 'clsx';
import { CircleAlert, CircleCheck, Info, TriangleAlert, Wifi, X, type LucideIcon } from 'lucide-react';
import type { ReactNode } from 'react';
import { Button, IconButton, Spinner } from '../../../components/ui';
import { useLanToggle } from '../useProxyInfo';
import styles from './kit.module.css';

export function PanelSection({ title, description, actions, children, className }: { title?: ReactNode; description?: ReactNode; actions?: ReactNode; children?: ReactNode; className?: string }) {
  return (
    <section className={clsx(styles.section, className)}>
      {(title || actions) && (
        <header className={styles.sectionHeader}>
          <div className={styles.sectionTitles}>
            {title && <h3 className={styles.sectionTitle}>{title}</h3>}
            {description && <p className={styles.sectionDesc}>{description}</p>}
          </div>
          {actions && <div className={styles.sectionActions}>{actions}</div>}
        </header>
      )}
      {children}
    </section>
  );
}

const CALLOUT_ICONS: Record<'info' | 'warning' | 'danger' | 'success', LucideIcon> = {
  info: Info,
  warning: TriangleAlert,
  danger: CircleAlert,
  success: CircleCheck,
};

export function Callout({ tone = 'info', title, children, action, className }: { tone?: 'info' | 'warning' | 'danger' | 'success'; title?: ReactNode; children?: ReactNode; action?: ReactNode; className?: string }) {
  const Icon = CALLOUT_ICONS[tone];
  return (
    <div className={clsx(styles.callout, styles[tone], className)} role={tone === 'danger' || tone === 'warning' ? 'alert' : 'note'}>
      <Icon size={15} strokeWidth={1.9} className={styles.calloutIcon} aria-hidden />
      <div className={styles.calloutBody}>
        {title && <div className={styles.calloutTitle}>{title}</div>}
        {children && <div className={styles.calloutText}>{children}</div>}
        {action && <div className={styles.calloutAction}>{action}</div>}
      </div>
    </div>
  );
}

/** Live status row ("Waiting for a terminal…" → "Terminal connected"). */
export function LiveStatus({ active, activeText, waitingText }: { active: boolean; activeText: ReactNode; waitingText: ReactNode }) {
  return (
    <div className={clsx(styles.live, active && styles.liveActive)} role="status" aria-live="polite">
      {active ? <CircleCheck size={15} strokeWidth={2} aria-hidden /> : <Spinner size={14} tone="muted" />}
      <span>{active ? activeText : waitingText}</span>
    </div>
  );
}

export interface TargetRowProps {
  icon: ReactNode;
  title: ReactNode;
  subtitle?: ReactNode;
  badge?: ReactNode;
  hint?: ReactNode;
  action?: ReactNode;
  active?: boolean;
  disabled?: boolean;
  /** Full-width content under the row (e.g. what an activation returned). */
  children?: ReactNode;
}

/** A device / container / process row with an action on the right. */
export function TargetRow({ icon, title, subtitle, badge, hint, action, active, disabled, children }: TargetRowProps) {
  return (
    <li className={clsx(styles.target, active && styles.targetActive, disabled && styles.targetDisabled)}>
      <span className={styles.targetIcon} aria-hidden>
        {icon}
      </span>
      <span className={styles.targetText}>
        <span className={styles.targetTitleRow}>
          <span className={styles.targetTitle}>{title}</span>
          {badge}
        </span>
        {subtitle && (
          <span className={styles.targetSubtitle} title={typeof subtitle === 'string' ? subtitle : undefined}>
            {subtitle}
          </span>
        )}
        {hint && <span className={styles.targetHint}>{hint}</span>}
      </span>
      {action && <span className={styles.targetAction}>{action}</span>}
      {children && <div className={styles.targetExtra}>{children}</div>}
    </li>
  );
}

/** Message + details an activation returned for one target. */
export function ResultBlock({ message, children, onDismiss }: { message?: ReactNode; children?: ReactNode; onDismiss?: () => void }) {
  return (
    <div className={styles.result} role="status">
      {message && (
        <div className={styles.resultHead}>
          <CircleCheck size={14} strokeWidth={2} aria-hidden />
          <span className={styles.resultText}>{message}</span>
          {onDismiss && <IconButton label="Dismiss" icon={X} size="sm" onClick={onDismiss} className={styles.resultDismiss} />}
        </div>
      )}
      {children}
    </div>
  );
}

export function TargetList({ children, label }: { children: ReactNode; label: string }) {
  return (
    <ul className={styles.targets} aria-label={label}>
      {children}
    </ul>
  );
}

/** Empty list placeholder inside a panel. */
export function PanelEmpty({ icon: Icon, title, children, action }: { icon: LucideIcon; title: ReactNode; children?: ReactNode; action?: ReactNode }) {
  return (
    <div className={styles.empty}>
      <span className={styles.emptyIcon} aria-hidden>
        <Icon size={18} strokeWidth={1.6} />
      </span>
      <div className={styles.emptyTitle}>{title}</div>
      {children && <div className={styles.emptyText}>{children}</div>}
      {action && <div className={styles.emptyAction}>{action}</div>}
    </div>
  );
}

/** Callout asking to enable LAN access (devices need it). */
export function LanRequired({ what = 'Devices' }: { what?: string }) {
  const lan = useLanToggle();
  if (lan.checked) return null;
  return (
    <Callout
      tone="info"
      title="Allow other devices first"
      action={
        <Button size="sm" variant="primary" icon={Wifi} loading={lan.pending} disabled={lan.disabled} onClick={() => lan.toggle(true)}>
          Allow other devices
        </Button>
      }
    >
      {what} connect to Akev over your network, but the proxy only accepts connections from this computer right now.
    </Callout>
  );
}

/** Small key/value line (`Proxy  192.168.1.23:8000 [copy]`). */
export function KeyLine({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.keyLine}>
      <span className={styles.keyLabel}>{label}</span>
      <span className={styles.keyValue}>{children}</span>
    </div>
  );
}
