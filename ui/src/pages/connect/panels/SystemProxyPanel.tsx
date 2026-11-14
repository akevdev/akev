import clsx from 'clsx';
import { RefreshCw } from 'lucide-react';
import { useState } from 'react';
import type { Platform } from '../../../api/types';
import { Button, SegmentedControl, Spinner, Switch } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { activateInterceptor, deactivateInterceptor, usePending } from '../actions';
import { GuideView, TrustView } from '../GuideView';
import { commonName, TRUST_TARGETS, trustGuide, type TrustTargetId } from '../snippets';
import { Callout, PanelSection } from './kit';
import type { PanelProps } from './types';
import styles from './panels.module.css';

const WHAT_CHANGES: Record<Platform, string> = {
  macos: 'Sets the HTTP and HTTPS proxy of your active network services (System Settings → Network).',
  windows: 'Sets the Windows proxy (Settings → Network & Internet → Proxy), used by Edge, Chrome and most apps.',
  linux: 'Sets the GNOME / KDE system proxy. Apps that ignore desktop settings need the env vars from Manual setup.',
};

/** System proxy: explanation, on/off toggle, trust instructions per OS. */
export function SystemProxyPanel({ info, meta, proxy }: PanelProps) {
  const caSubject = useAppStore((s) => s.status?.ca.subject ?? null);
  const starting = usePending(info.id);
  const stopping = usePending(`${info.id}:stop`);
  const busy = starting || stopping;
  const platform = proxy.platform ?? 'linux';
  const desktopTargets = TRUST_TARGETS.filter((t) => t.id === 'macos' || t.id === 'windows' || t.id === 'linux' || t.id === 'firefox');
  const [target, setTarget] = useState<TrustTargetId>(platform);
  const expected = meta.proxy ?? proxy.local;
  const stale = info.active && meta.configuredProxy != null && expected != null && meta.configuredProxy !== expected;
  const backendNames = meta.backends.map((b) => b.name).join(' and ');

  const toggle = (on: boolean) => {
    if (on) void activateInterceptor(info, undefined, { verb: 'enable' });
    else void deactivateInterceptor(info, undefined, { target: 'the system proxy' });
  };

  return (
    <>
      {!info.available && !info.active && info.unavailableReason && <Callout tone="warning">{info.unavailableReason}</Callout>}
      <div className={clsx(styles.toggleCard, info.active && styles.toggleCardOn)}>
        <Switch
          checked={info.active}
          onCheckedChange={toggle}
          disabled={busy || (!info.available && !info.active)}
          label={
            <span className={styles.row}>
              Use Akev as the system proxy
              {busy && <Spinner size={12} tone="muted" />}
            </span>
          }
          description={
            info.active
              ? `Apps that follow the system settings now send traffic to ${meta.configuredProxy ?? proxy.local ?? 'Akev'}.`
              : `Your previous proxy settings are restored when you turn this off${backendNames ? ` (${backendNames})` : ''}.`
          }
        />
      </div>
      {stale && (
        <Callout
          tone="warning"
          title="The system still points at the old address"
          action={
            <Button size="sm" icon={RefreshCw} loading={starting} onClick={() => void activateInterceptor(info, undefined, { verb: 'update' })}>
              Point it at {expected}
            </Button>
          }
        >
          The proxy moved, but your system proxy is still set to {meta.configuredProxy}.
        </Callout>
      )}
      <PanelSection title="What this changes">
        <ul className={styles.bullets}>
          <li>{WHAT_CHANGES[platform]}</li>
          <li>Browsers and most desktop apps follow it — everything on this machine shows up in Traffic. Many command-line tools read environment variables instead: use a Fresh terminal for those.</li>
          <li>For HTTPS, apps must also trust the Akev certificate system-wide (below). Launched browsers and terminals don't need this.</li>
        </ul>
      </PanelSection>
      <PanelSection title="Trust the certificate" description="Akev never changes your trust store itself — run these once, and remove the certificate when you're done.">
        <SegmentedControl<TrustTargetId> value={target} onValueChange={setTarget} items={desktopTargets.map((t) => ({ value: t.id, label: t.label }))} size="sm" aria-label="Operating system" className={styles.selfStart} />
        {target === platform && meta.trust ? (
          <TrustView trust={meta.trust} />
        ) : (
          <GuideView guide={trustGuide(target, { caName: commonName(caSubject), providedCaPath: target === platform ? meta.caPath : null, setupUrl: proxy.setupUrl })} />
        )}
      </PanelSection>
    </>
  );
}
