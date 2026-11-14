import clsx from 'clsx';
import type { Feature } from '../../api/types';
import { useShowProBadge } from '../../state/license';
import { Badge } from '../ui';
import styles from './pro.module.css';

export interface ProBadgeProps {
  /** The feature it marks: hidden once that feature is licensed (shown during the trial). */
  feature?: Feature;
  /** Show regardless of the license (dialogs, the License settings). */
  always?: boolean;
  size?: 'sm' | 'md';
  className?: string;
}

/** Small "Pro" pill next to a Pro feature's name. */
export function ProBadge({ feature, always, size = 'sm', className }: ProBadgeProps) {
  const show = useShowProBadge(feature);
  if (!show && !always) return null;
  return (
    <Badge variant="accent" size={size} className={clsx(styles.badge, className)} title="Part of Akev Pro">
      Pro
    </Badge>
  );
}
