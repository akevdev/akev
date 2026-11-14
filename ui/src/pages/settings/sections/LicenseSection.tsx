import clsx from 'clsx';
import { BookOpen, Check, ExternalLink, KeyRound, LockKeyhole, Trash2 } from 'lucide-react';
import { useEffect, useState } from 'react';
import { errorMessage } from '../../../api/client';
import type { LicenseStatus } from '../../../api/types';
import { LicenseKeyField, openPricing, startProTrial, useLicenseKeyForm } from '../../../components/pro';
import { Badge, Button, confirm, CopyButton, EmptyState, Skeleton, toast, type BadgeVariant } from '../../../components/ui';
import { ACCOUNT_URL, describeLicense, FEATURES, formatLicenseDate, LICENSING_DOCS_URL, planLabel, PRO_PRICE, PRO_TERMS, RENEWAL_PRICE, TRIAL_DAYS, type LicenseTone } from '../../../lib/license';
import { openExternal } from '../../../lib/platform';
import { useAppStore } from '../../../state/app';
import { useLicense, useLicenseStore } from '../../../state/license';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';
import own from './LicenseSection.module.css';

const TONE_BADGE: Record<LicenseTone, BadgeVariant> = { accent: 'accent', success: 'success', warning: 'warning', danger: 'danger', neutral: 'neutral' };

function PlanCard({ status }: { status: LicenseStatus }) {
  const version = useAppStore((s) => s.status?.version ?? null);
  const [starting, setStarting] = useState(false);
  const summary = describeLicense(status);
  const label = planLabel(status) ?? 'Free';
  const license = status.license;
  const pro = status.state === 'active';
  const coversBuild = license ? license.updatesUntil >= status.releaseDate : null;

  const trial = async () => {
    setStarting(true);
    await startProTrial();
    setStarting(false);
  };

  return (
    <SettingsCard id="license" title="Plan">
      <SettingRow
        id="license-plan"
        title={summary.title}
        description={summary.detail}
        error={status.state === 'invalid' ? (status.error ?? "The stored license key couldn't be verified") : undefined}
        control={
          <Badge variant={TONE_BADGE[summary.tone]} dot={status.state === 'active' || status.state === 'trial'}>
            {label}
          </Badge>
        }
      />
      {license && (
        <>
          <SettingRow
            title="Licensed to"
            control={
              <span className={own.licensee}>
                {license.name && <span className={own.licenseeName}>{license.name}</span>}
                <span className={styles.muted}>{license.email}</span>
              </span>
            }
          />
          {license.seats && license.seats > 1 && (
            <SettingRow
              title="Seats"
              description="A team license: share this key with up to this many people."
              control={<span className={clsx(styles.muted, 'tnum')}>{license.seats} people</span>}
            />
          )}
          <SettingRow
            title="License ID"
            control={
              <span className={styles.inlineGroup}>
                <span className={styles.addr}>{license.id}</span>
                <CopyButton value={license.id} label="Copy license ID" toastMessage="License ID copied" />
              </span>
            }
          />
          <SettingRow
            title="Updates until"
            description={coversBuild ? 'Every Akev version released until then works with this license — and keeps working afterwards.' : undefined}
            warning={coversBuild === false ? `This version (released ${formatLicenseDate(status.releaseDate)}) is newer` : undefined}
            control={
              <>
                <span className={clsx(styles.muted, 'tnum')}>{formatLicenseDate(license.updatesUntil)}</span>
                {coversBuild === false && (
                  <Button size="sm" variant="primary" iconRight={ExternalLink} onClick={() => openExternal(ACCOUNT_URL)}>
                    Renew updates · {RENEWAL_PRICE}
                  </Button>
                )}
              </>
            }
          />
          <SettingRow
            title="Your account"
            description="See your keys and receipts, or renew updates. Sign in on akev.dev with the email you bought with."
            control={
              <Button size="sm" iconRight={ExternalLink} onClick={() => openExternal(ACCOUNT_URL)}>
                Manage license
              </Button>
            }
          />
        </>
      )}
      <SettingRow title="This version" control={<span className={clsx(styles.muted, 'tnum')}>{version ? `v${version} · ` : ''}released {formatLicenseDate(status.releaseDate)}</span>} />
      {!pro && status.state !== 'trial' && status.trialAvailable && (
        <SettingRow
          id="license-trial"
          title="Pro trial"
          description={`Try every Pro feature for ${TRIAL_DAYS} days — no account, no card. Once per computer.`}
          control={
            <Button size="sm" loading={starting} onClick={() => void trial()}>
              Start {TRIAL_DAYS}-day trial
            </Button>
          }
        />
      )}
      {!pro && status.state !== 'updates-ended' && (
        <SettingRow
          title="Buy Akev Pro"
          description={`${PRO_TERMS}. The key arrives by e-mail — paste it below.`}
          control={
            <Button size="sm" variant="primary" iconRight={ExternalLink} onClick={openPricing}>
              Buy Pro · {PRO_PRICE}
            </Button>
          }
        />
      )}
    </SettingsCard>
  );
}

function KeyCard({ status }: { status: LicenseStatus }) {
  const hasKey = status.license != null || status.state === 'invalid' || status.state === 'updates-ended' || status.state === 'active';
  const [editing, setEditing] = useState(!hasKey);
  const [removing, setRemoving] = useState(false);
  const form = useLicenseKeyForm((s) => {
    if (s.state === 'active') setEditing(false);
  });
  useEffect(() => {
    if (!hasKey) setEditing(true);
  }, [hasKey]);

  const remove = async () => {
    const ok = await confirm({
      title: 'Remove the license from this computer?',
      message: 'Akev goes back to Free here. Nothing is deleted — script rules, schemas and settings stay saved, and Pro features resume when you add the key again.',
      destructive: true,
      confirmLabel: 'Remove license',
    });
    if (!ok) return;
    setRemoving(true);
    try {
      await useLicenseStore.getState().remove();
      toast.success('License removed', { description: 'Akev runs as Free on this computer.' });
      setEditing(true);
    } catch (e) {
      toast.error("Couldn't remove the license", { description: errorMessage(e) });
    } finally {
      setRemoving(false);
    }
  };

  return (
    <SettingsCard
      id="license-key"
      title="License key"
      description="Keys are verified on this computer: no account, no activation server, nothing is sent anywhere."
      actions={
        <Button size="sm" variant="ghost" icon={BookOpen} iconRight={ExternalLink} onClick={() => openExternal(LICENSING_DOCS_URL)}>
          How licenses work
        </Button>
      }
    >
      {editing ? (
        <SettingRow id="license-key-input" title={hasKey ? 'Replace the license key' : 'Enter your license key'} layout="stacked">
          <LicenseKeyField form={form} hideLabel />
          <div className={own.keyActions}>
            <Button variant="primary" icon={KeyRound} loading={form.busy} onClick={() => void form.submit()} kbd="mod+enter">
              Activate
            </Button>
            {hasKey && (
              <Button variant="ghost" onClick={() => setEditing(false)} disabled={form.busy}>
                Cancel
              </Button>
            )}
          </div>
        </SettingRow>
      ) : (
        <SettingRow
          id="license-key-input"
          title="License key"
          description={status.state === 'active' ? 'Installed and verified.' : status.state === 'updates-ended' ? "Installed — genuine, but it doesn't cover this version." : "Installed, but it couldn't be verified."}
          control={
            <Button size="sm" icon={KeyRound} onClick={() => setEditing(true)}>
              Enter a different key…
            </Button>
          }
        />
      )}
      {hasKey && (
        <SettingRow
          id="license-remove"
          title="Remove license"
          description="Back to Free on this computer. Nothing is deleted."
          control={
            <Button size="sm" variant="subtle-danger" icon={Trash2} loading={removing} onClick={() => void remove()}>
              Remove…
            </Button>
          }
        />
      )}
    </SettingsCard>
  );
}

function FeaturesCard({ status }: { status: LicenseStatus }) {
  return (
    <SettingsCard
      id="license-features"
      title="What's in Pro"
      description={status.state === 'active' ? 'All unlocked on this computer.' : 'Capturing, inspecting, rules, breakpoints, Compose and the API are free. Without a license nothing is deleted — Pro features pause.'}
    >
      <ul className={own.features} id="setting-license-features" data-setting="license-features" aria-label="Pro features">
        {FEATURES.map((f) => {
          const on = status.features.includes(f.id);
          return (
            <li key={f.id} className={clsx(own.feature, on && own.featureOn)}>
              <span className={own.featureIcon} aria-hidden>
                {on ? <Check size={13} strokeWidth={2.5} /> : <LockKeyhole size={12} strokeWidth={2} />}
              </span>
              <span className={own.featureText}>
                <span className={own.featureLabel}>
                  {f.label}
                  <span className="sr-only">{on ? ' (unlocked)' : ' (locked)'}</span>
                </span>
                <span className={own.featureDesc}>{f.description}</span>
              </span>
            </li>
          );
        })}
      </ul>
    </SettingsCard>
  );
}

/** Settings → License: plan & state, license key, trial, what Pro includes. */
export function LicenseSection() {
  const status = useLicense();
  const connected = useAppStore((s) => s.hasConnected);
  const [loaded, setLoaded] = useState(false);

  // The stream keeps it current; refresh once so the trial countdown is exact.
  useEffect(() => {
    const ctrl = new AbortController();
    useLicenseStore
      .getState()
      .refresh(ctrl.signal)
      .catch(() => undefined)
      .finally(() => !ctrl.signal.aborted && setLoaded(true));
    return () => ctrl.abort();
  }, []);

  if (!status) {
    if (!loaded && !connected) {
      return (
        <div className={own.loading} aria-busy="true">
          <Skeleton width="30%" height={14} />
          <Skeleton lines={3} height={11} />
        </div>
      );
    }
    return <EmptyState icon={KeyRound} size="sm" title="No licensing in this version" description="This Akev service doesn't check licenses: every feature is available." />;
  }
  return (
    <>
      <PlanCard status={status} />
      <KeyCard status={status} />
      <FeaturesCard status={status} />
    </>
  );
}
