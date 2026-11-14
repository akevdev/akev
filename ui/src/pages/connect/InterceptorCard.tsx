import clsx from 'clsx';
import { Ban, CircleCheck, TriangleAlert } from 'lucide-react';
import { memo, type MouseEvent } from 'react';
import type { InterceptorInfo, Platform } from '../../api/types';
import { ProBadge } from '../../components/pro';
import { AppIcon, Button, Tooltip } from '../../components/ui';
import { interceptorFeature, isEntitled } from '../../lib/license';
import { openUpgrade, useLicenseStore } from '../../state/license';
import { activateInterceptor, deactivateInterceptor, usePending } from './actions';
import { canUsePrimary, iconKey, opensSheet, primaryActionFor, statusOf, type CardStatus } from './catalog';
import styles from './InterceptorCard.module.css';

export interface InterceptorCardProps {
  info: InterceptorInfo;
  platform: Platform | null;
  onOpen: (id: string) => void;
}

function StatusLine({ status }: { status: CardStatus }) {
  const Icon = status.tone === 'ready' ? CircleCheck : status.tone === 'missing' ? TriangleAlert : status.tone === 'unsupported' ? Ban : null;
  const node = (
    <span className={styles.status} data-tone={status.tone} tabIndex={status.tooltip && status.tone !== 'ready' && status.tone !== 'active' ? 0 : undefined}>
      {status.tone === 'active' ? <span className={styles.liveDot} aria-hidden /> : Icon && <Icon size={13} strokeWidth={2} aria-hidden />}
      <span className={styles.statusText}>{status.label}</span>
    </span>
  );
  return status.tooltip ? (
    <Tooltip content={status.tooltip} multiline>
      {node}
    </Tooltip>
  ) : (
    node
  );
}

/** A Connect catalogue card (DESIGN.md §8.2). */
function InterceptorCardImpl({ info, platform, onOpen }: InterceptorCardProps) {
  const status = statusOf(info, platform);
  const action = primaryActionFor(info);
  const usable = canUsePrimary(info);
  const sheet = opensSheet(info);
  const busy = usePending(info.id);
  const stopping = usePending(`${info.id}:stop`);
  const dimmed = status.tone === 'missing' || status.tone === 'unsupported';
  const clickable = sheet && usable;
  const feature = interceptorFeature(info.id);
  const locked = useLicenseStore((s) => feature != null && !isEntitled(s.status, feature));

  const runPrimary = () => {
    if (sheet) onOpen(info.id);
    else if (locked && feature && action.kind !== 'deactivate') openUpgrade(feature);
    else if (action.kind === 'deactivate') void deactivateInterceptor(info);
    else void activateInterceptor(info);
  };

  const onCardClick = (e: MouseEvent) => {
    if (!clickable || (e.target as HTMLElement).closest('button, a, [tabindex="0"]')) return;
    onOpen(info.id);
  };

  const button = (
    <Button
      size="sm"
      disabled={!usable}
      loading={busy}
      onClick={runPrimary}
      className={styles.primary}
      data-card-action=""
      aria-label={`${action.label} ${info.name}`}
    >
      {action.label}
    </Button>
  );

  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents: a larger click target for the card button, which keyboard users reach directly
    <article
      className={clsx(styles.card, clickable && styles.clickable, dimmed && styles.dimmed, info.active && styles.active)}
      data-interceptor={info.id}
      aria-labelledby={`ic-${info.id}-name`}
      aria-describedby={`ic-${info.id}-desc`}
      onClick={onCardClick}
    >
      <div className={styles.head}>
        <AppIcon name={iconKey(info.icon)} size={40} tile className={styles.icon} />
        {feature && <ProBadge feature={feature} />}
      </div>
      <h3 className={styles.name} id={`ic-${info.id}-name`}>
        {info.name}
      </h3>
      <p className={styles.desc} id={`ic-${info.id}-desc`} title={info.description.length > 80 ? info.description : undefined}>
        {info.description}
      </p>
      <div className={styles.foot}>
        <StatusLine status={status} />
        <div className={styles.actions}>
          {info.active && !sheet && (
            <Tooltip content={`Stop intercepting ${info.name}`}>
              <Button size="sm" variant="ghost" loading={stopping} onClick={() => void deactivateInterceptor(info)} className={styles.stop}>
                Stop
              </Button>
            </Tooltip>
          )}
          {!usable && status.tooltip ? (
            <Tooltip content={status.tooltip} multiline>
              <span className={styles.disabledWrap}>{button}</span>
            </Tooltip>
          ) : (
            button
          )}
        </div>
      </div>
    </article>
  );
}

function sameInfo(a: InterceptorInfo, b: InterceptorInfo): boolean {
  return (
    a.id === b.id &&
    a.name === b.name &&
    a.description === b.description &&
    a.icon === b.icon &&
    a.category === b.category &&
    a.active === b.active &&
    a.available === b.available &&
    a.supported === b.supported &&
    a.needsConfig === b.needsConfig &&
    a.version === b.version &&
    a.unavailableReason === b.unavailableReason
  );
}

/** Memoised on the fields the card shows (the stream replaces interceptor objects wholesale). */
export const InterceptorCard = memo(InterceptorCardImpl, (prev, next) => prev.platform === next.platform && prev.onOpen === next.onOpen && sameInfo(prev.info, next.info));
