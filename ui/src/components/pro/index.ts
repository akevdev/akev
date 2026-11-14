/**
 * Akev Pro UI: the "Pro" badge, locked-feature callouts/gates, the license key form and the
 * app-level upgrade dialog. State lives in `state/license.ts`, wording in `lib/license.ts`.
 */

export { LicenseKeyField, useLicenseKeyForm, type LicenseKeyFormState } from './LicenseKeyForm';
export { ProBadge, type ProBadgeProps } from './ProBadge';
export { ProCallout, ProGate, type ProCalloutProps, type ProGateProps } from './ProGate';
export { openPricing, startProTrial, UpgradeDialog } from './UpgradeDialog';
