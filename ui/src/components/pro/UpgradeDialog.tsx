/**
 * The upgrade dialog (mounted once in the App shell): what Pro includes, the price, and the
 * three ways forward — start the trial, enter a license key, buy. Opened with
 * `openUpgrade(feature?)`, from Pro badges/callouts and from any `pro-required` API error.
 */

import clsx from 'clsx';
import { ArrowLeft, Check, ExternalLink, KeyRound, TriangleAlert } from 'lucide-react';
import { useMemo, useState } from 'react';
import { errorMessage } from '../../api/client';
import type { Feature, LicenseStatus } from '../../api/types';
import { describeLicense, FEATURES, featureInfo, formatLicenseDate, PRICING_URL, PRO_PRICE, PRO_TERMS, TRIAL_DAYS } from '../../lib/license';
import { openExternal } from '../../lib/platform';
import { closeUpgrade, setUpgradeView, useLicense, useLicenseStore, useUpgradeDialog } from '../../state/license';
import { Button, Dialog, toast } from '../ui';
import { LicenseKeyField, useLicenseKeyForm } from './LicenseKeyForm';
import styles from './pro.module.css';

/** Start the trial (shared with Settings → License and the palette). Resolves to success. */
export async function startProTrial(): Promise<boolean> {
  try {
    const status = await useLicenseStore.getState().startTrial();
    toast.success(`Your ${TRIAL_DAYS}-day Pro trial has started`, {
      description: status.trialEndsAt != null ? `Every Pro feature is unlocked until ${formatLicenseDate(status.trialEndsAt)}.` : 'Every Pro feature is unlocked.',
    });
    return true;
  } catch (e) {
    toast.error("Couldn't start the trial", { description: errorMessage(e) });
    return false;
  }
}

/** Opens the pricing page in the browser. */
export function openPricing(): void {
  openExternal(PRICING_URL);
}

/** Features in display order, the one that prompted the dialog first. */
function orderedFeatures(feature: Feature | null) {
  if (!feature) return FEATURES;
  return [featureInfo(feature), ...FEATURES.filter((f) => f.id !== feature)];
}

/** Why the user is here when Pro was theirs before (trial over, key not covering this build…). */
function StateNotice({ status }: { status: LicenseStatus | null }) {
  if (!status || !['trial-ended', 'updates-ended', 'invalid'].includes(status.state)) return null;
  const s = describeLicense(status);
  return (
    <p className={styles.notice} role="note">
      <TriangleAlert size={14} strokeWidth={1.9} aria-hidden />
      <span>{s.detail}</span>
    </p>
  );
}

function Offer({ feature, status }: { feature: Feature | null; status: LicenseStatus | null }) {
  const features = useMemo(() => orderedFeatures(feature), [feature]);
  return (
    <>
      <StateNotice status={status} />
      <ul className={styles.features} aria-label="Included in Akev Pro">
        {features.map((f) => (
          <li key={f.id} className={clsx(styles.feature, f.id === feature && styles.featureHighlighted)}>
            <Check size={14} strokeWidth={2.25} className={styles.featureCheck} aria-hidden />
            <span className={styles.featureText}>
              <span className={styles.featureLabel}>{f.label}</span>
              <span className={styles.featureDesc}>{f.description}</span>
            </span>
          </li>
        ))}
      </ul>
      <p className={styles.freeNote}>Capturing, inspecting, rules, breakpoints, Compose and the API stay free. Without a license nothing is deleted — Pro features just pause.</p>
    </>
  );
}

function UpgradeDialogContent() {
  const { feature, view } = useUpgradeDialog();
  const status = useLicense();
  const [starting, setStarting] = useState(false);
  const form = useLicenseKeyForm((s) => {
    if (s.state === 'active') closeUpgrade();
  });
  const info = feature ? featureInfo(feature) : null;

  const trial = async () => {
    setStarting(true);
    const ok = await startProTrial();
    setStarting(false);
    if (ok) closeUpgrade();
  };

  const buy = () => {
    openPricing();
    // Coming back with a key: the form is ready.
    setUpgradeView('key');
  };

  if (view === 'key') {
    return (
      <Dialog
        open
        onOpenChange={(o) => !o && closeUpgrade()}
        title="Enter your license key"
        description="It's verified on this computer: no account, no activation server, nothing is sent anywhere."
        width={560}
        className={styles.dialog}
        dismissible={!form.busy}
        footer={
          <>
            <button type="button" className={styles.footerLink} onClick={() => setUpgradeView('offer')}>
              <ArrowLeft size={13} strokeWidth={1.75} aria-hidden />
              What's in Pro
            </button>
            <Button variant="ghost" onClick={closeUpgrade} disabled={form.busy}>
              Cancel
            </Button>
            <Button variant="primary" icon={KeyRound} loading={form.busy} onClick={() => void form.submit()} kbd="mod+enter">
              Activate
            </Button>
          </>
        }
      >
        <LicenseKeyField form={form} autoFocus hideLabel />
      </Dialog>
    );
  }

  return (
    <Dialog
      open
      onOpenChange={(o) => !o && closeUpgrade()}
      title={info ? `${info.label} is part of Akev Pro` : 'Upgrade to Akev Pro'}
      description={PRO_TERMS}
      width={640}
      className={styles.dialog}
      focusContent
      footer={
        <>
          <button type="button" className={styles.footerLink} onClick={() => setUpgradeView('key')}>
            <KeyRound size={13} strokeWidth={1.75} aria-hidden />
            Enter license key
          </button>
          {status?.trialAvailable && (
            <Button loading={starting} onClick={() => void trial()}>
              Start {TRIAL_DAYS}-day trial
            </Button>
          )}
          <Button variant="primary" iconRight={ExternalLink} onClick={buy}>
            Buy Pro · {PRO_PRICE}
          </Button>
        </>
      }
    >
      <Offer feature={feature} status={status} />
    </Dialog>
  );
}

/** Mount once (App shell). */
export function UpgradeDialog() {
  const { open } = useUpgradeDialog();
  if (!open) return null;
  return <UpgradeDialogContent />;
}
