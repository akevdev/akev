import { ShieldCheck, ShieldOff } from 'lucide-react';
import { useEffect } from 'react';
import type { TrustStore } from '../../../api/types';
import { Badge, Button, Skeleton } from '../../../components/ui';
import { trustWithConfirm, untrustWithConfirm, useCaTrust } from '../../../state/caTrust';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';

function stateBadge(store: TrustStore) {
  if (store.trusted === true) return <Badge variant="success" dot>Trusted</Badge>;
  if (store.trusted === false) return <Badge variant="warning">Not trusted</Badge>;
  return <Badge variant="neutral">Unknown</Badge>;
}

/** One click to trust the Akev CA on this computer (the operating system asks to confirm). */
export function CaTrustCard() {
  const report = useCaTrust((s) => s.report);
  const busy = useCaTrust((s) => s.busy);
  const { refresh } = useCaTrust.getState();

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return (
    <SettingsCard
      id="trust-this-computer"
      title="Trust on this computer"
      description="Browsers and terminals Akev launches trust it automatically. For everything else on this computer, add the certificate to the system's store — Akev only does this when you click Trust."
    >
      {!report && (
        <SettingRow title={<Skeleton width={180} height={12} />} control={<Skeleton width={90} height={22} />} />
      )}
      {report && report.stores.length === 0 && (
        <SettingRow title="Not available on this system" description="Use the instructions below to trust the certificate by hand." />
      )}
      {report?.stores.map((store) => (
        <SettingRow
          key={store.id}
          id={`trust-${store.id}`}
          title={
            <span className={styles.inlineGroup}>
              {store.label} {stateBadge(store)}
            </span>
          }
          description={store.unavailableReason ? `${store.description}. ${store.unavailableReason}` : store.description}
          control={
            store.canChange &&
            (store.trusted === true ? (
              <Button size="sm" variant="subtle-danger" icon={ShieldOff} loading={busy === store.id} disabled={busy != null} onClick={() => void untrustWithConfirm(store)}>
                Remove
              </Button>
            ) : (
              <Button size="sm" variant="primary" icon={ShieldCheck} loading={busy === store.id} disabled={busy != null} onClick={() => void trustWithConfirm(store)}>
                Trust
              </Button>
            ))
          }
        />
      ))}
    </SettingsCard>
  );
}
