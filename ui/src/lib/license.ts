/**
 * Akev Pro: the feature catalogue, plan/state wording and entitlement checks over
 * `LicenseStatus` (`GET /api/license`). Pure — no React; the live status lives in
 * `state/license.ts`.
 *
 * A missing status (before the first `hello`) means "no gating": everything is available and
 * no Pro badges are shown.
 */

import type { Action, ActionType, Feature, LicenseInfo, LicenseState, LicenseStatus } from '../api/types';

export const PRICING_URL = 'https://akev.dev/pricing/';
/** Where buyers see their keys and renew updates (sign in with the checkout email). */
export const ACCOUNT_URL = 'https://akev.dev/account/';
export const LICENSING_DOCS_URL = 'https://akev.dev/docs/licensing/';
export const PRO_PRICE = '$49';
export const RENEWAL_PRICE = '$25';
export const TRIAL_DAYS = 14;
/** The one-line offer under every upgrade prompt. */
export const PRO_TERMS = `${PRO_PRICE} once · 1 year of updates · no subscription · no account`;
/** Every key starts with this (keys pasted from an e-mail may be wrapped). */
export const LICENSE_KEY_PREFIX = 'AKEV1.';

export interface FeatureInfo {
  id: Feature;
  label: string;
  /** One line: what it does for you. */
  description: string;
}

/** Display order everywhere (upgrade dialog, Settings → License). */
export const FEATURES: readonly FeatureInfo[] = [
  { id: 'wireguard', label: 'WireGuard capture', description: 'Intercept a whole phone or tablet with one QR code — every app, no proxy settings.' },
  { id: 'transparent', label: 'Transparent capture', description: 'Capture apps that ignore proxy settings, per app, per user or machine-wide (Linux).' },
  { id: 'jvm-attach', label: 'JVM attach', description: 'Intercept running Java, Kotlin and Scala apps without restarting them.' },
  { id: 'tls-impersonation', label: 'TLS fingerprints', description: 'Look like Chrome, Firefox or Safari to servers that check JA3/JA4 fingerprints.' },
  { id: 'scripting', label: 'Script rules', description: 'Rewrite requests and responses with JavaScript in a sandbox.' },
  { id: 'proto-schemas', label: 'Protobuf schemas', description: 'Decode protobuf and gRPC with your .proto files: real field and message names.' },
  { id: 'llm-lens', label: 'LLM lens', description: 'Read LLM API calls as conversations: prompts, tool calls, tokens and latency.' },
  { id: 'mcp-write', label: 'MCP write tools', description: 'Let AI agents add mocks and rules through the MCP server.' },
];

const FEATURE_BY_ID = new Map(FEATURES.map((f) => [f.id, f]));

export function featureInfo(id: Feature): FeatureInfo {
  return FEATURE_BY_ID.get(id) ?? { id, label: id, description: '' };
}

/** Whether `id` is a known feature (narrows the untyped `feature` of a 402 body). */
export function isFeature(id: unknown): id is Feature {
  return typeof id === 'string' && FEATURE_BY_ID.has(id as Feature);
}

/** Interceptor ids (Connect) whose activation needs Pro. */
const INTERCEPTOR_FEATURES: Readonly<Record<string, Feature>> = {
  wireguard: 'wireguard',
  'transparent-linux': 'transparent',
  jvm: 'jvm-attach',
};

export function interceptorFeature(id: string): Feature | undefined {
  return INTERCEPTOR_FEATURES[id];
}

/** Rule actions that only run with Pro (rules with them are still saved). */
const ACTION_FEATURES: Partial<Record<ActionType, Feature>> = {
  script: 'scripting',
  'tls-profile': 'tls-impersonation',
};

export function actionFeature(type: ActionType): Feature | undefined {
  return ACTION_FEATURES[type];
}

/** The Pro feature a configured action needs to run (a `native` TLS profile needs none). */
export function actionNeedsPro(action: Action): Feature | undefined {
  if (action.type === 'tls-profile' && action.profile === 'native') return undefined;
  return actionFeature(action.type);
}

// =====================================================================================
// Entitlement
// =====================================================================================

/** Whether `feature` works right now. Unknown status = no gating. */
export function isEntitled(status: LicenseStatus | null | undefined, feature: Feature): boolean {
  if (!status) return true;
  return status.features.includes(feature);
}

/**
 * Whether a "Pro" marker belongs next to `feature`: always while it's locked, and during the
 * trial (so it's clear what stops working afterwards). Hidden for licensed users.
 */
export function showProBadge(status: LicenseStatus | null | undefined, feature?: Feature): boolean {
  if (!status) return false;
  if (status.state === 'active') return feature != null && !status.features.includes(feature);
  return true;
}

const DAY_MS = 86_400_000;

/** Whole days left in the trial (rounded up: "1 day left" until it ends), or null. */
export function trialDaysLeft(status: LicenseStatus | null | undefined, now: number = Date.now()): number | null {
  if (!status || status.state !== 'trial' || status.trialEndsAt == null) return null;
  return Math.max(0, Math.ceil((status.trialEndsAt - now) / DAY_MS));
}

export type PlanLabel = 'Free' | 'Pro' | 'Pro trial';

/** The short plan name for badges ("Free" / "Pro" / "Pro trial"). */
export function planLabel(status: LicenseStatus | null | undefined): PlanLabel | null {
  if (!status) return null;
  if (status.state === 'trial') return 'Pro trial';
  if (status.state === 'active') return 'Pro';
  return 'Free';
}

/** Who a license is for, in one line: the name, or the email when there is none. */
export function licenseeName(license: LicenseInfo): string {
  return license.name?.trim() || license.email;
}

/** `Ada Lovelace <ada@example.com>`, or the email alone. */
export function licenseeFull(license: LicenseInfo): string {
  const name = license.name?.trim();
  return name ? `${name} <${license.email}>` : license.email;
}

/** The license that unlocks Pro in this build (state `active`), for "Licensed to …". */
export function activeLicense(status: LicenseStatus | null | undefined): LicenseInfo | null {
  return status?.state === 'active' ? (status.license ?? null) : null;
}

export type LicenseTone = 'accent' | 'success' | 'warning' | 'danger' | 'neutral';

export interface LicenseSummary {
  /** "Akev Pro", "Akev Free", "Pro trial · 9 days left"… */
  title: string;
  /** What it means, one or two sentences. */
  detail: string;
  tone: LicenseTone;
}

/** Plan + state in words, for Settings → License and the upgrade dialog. */
export function describeLicense(status: LicenseStatus, now: number = Date.now()): LicenseSummary {
  const until = status.license ? formatLicenseDate(status.license.updatesUntil) : null;
  switch (status.state) {
    case 'active':
      return {
        title: 'Akev Pro',
        detail: until ? `Every Pro feature is unlocked, including updates released until ${until}. Thank you for supporting Akev!` : 'Every Pro feature is unlocked. Thank you for supporting Akev!',
        tone: 'success',
      };
    case 'trial': {
      const days = trialDaysLeft(status, now) ?? 0;
      return {
        title: `Pro trial · ${days === 0 ? 'ends today' : `${days} ${days === 1 ? 'day' : 'days'} left`}`,
        detail: `Every Pro feature is unlocked until ${status.trialEndsAt != null ? formatLicenseDate(status.trialEndsAt) : 'the trial ends'}. Nothing is deleted when it ends — Pro features just pause.`,
        tone: 'accent',
      };
    }
    case 'trial-ended':
      return {
        title: 'Akev Free',
        detail: 'Your Pro trial has ended. Your rules, schemas and settings are kept — Pro features are paused until you add a license.',
        tone: 'warning',
      };
    case 'updates-ended':
      return {
        title: 'Akev Free · updates ended',
        detail: `Your license covers versions released until ${until ?? 'its update date'}. Renew updates (${RENEWAL_PRICE}) or use an earlier version.`,
        tone: 'warning',
      };
    case 'invalid':
      return {
        title: 'License not valid',
        detail: status.error ? `${status.error}. Akev runs as Free until you add a valid key.` : "The stored license key couldn't be verified. Akev runs as Free until you add a valid key.",
        tone: 'danger',
      };
    case 'none':
    default:
      return {
        title: 'Akev Free',
        detail: status.trialAvailable ? `The open-source core, free forever. Try every Pro feature free for ${TRIAL_DAYS} days — no account needed.` : 'The open-source core, free forever.',
        tone: 'neutral',
      };
  }
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

/** `2027-11-12` (or unix ms) → `12 Nov 2027`. Unparseable input is returned as is. */
export function formatLicenseDate(date: string | number): string {
  if (typeof date === 'number') {
    const d = new Date(date);
    return `${d.getDate()} ${MONTHS[d.getMonth()]} ${d.getFullYear()}`;
  }
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(date);
  if (!m) return date;
  const month = MONTHS[Number(m[2]) - 1];
  return month ? `${Number(m[3])} ${month} ${m[1]}` : date;
}

/** A pasted key without the whitespace and line breaks e-mail clients add. */
export function normalizeLicenseKey(text: string): string {
  return text.replace(/\s+/g, '');
}

/** Quick client-side check before sending; the daemon does the real verification. */
export function validateLicenseKeyText(text: string): string | null {
  const key = normalizeLicenseKey(text);
  if (!key) return 'Paste your license key.';
  if (!key.startsWith(LICENSE_KEY_PREFIX)) return `That doesn't look like an Akev license key — keys start with ${LICENSE_KEY_PREFIX}`;
  return null;
}

/** The toast after `PUT /api/license` succeeded (a genuine key may still not cover this build). */
export function activationResult(status: LicenseStatus): { ok: boolean; title: string; description: string } {
  if (status.state === 'active') {
    const until = status.license ? formatLicenseDate(status.license.updatesUntil) : null;
    return { ok: true, title: 'Akev Pro activated', description: until ? `Thank you! Updates released until ${until} are included.` : 'Thank you! Every Pro feature is unlocked.' };
  }
  if (status.state === 'updates-ended') {
    return { ok: false, title: "License added — but this version isn't covered", description: describeLicense(status).detail };
  }
  return { ok: false, title: 'License not active', description: describeLicense(status).detail };
}

/**
 * A toast-worthy change between two statuses (from the stream), e.g. the trial expiring while
 * Akev is open. User-initiated changes are announced by their callers; this catches the rest.
 */
export function licenseTransition(prev: LicenseStatus | null | undefined, next: LicenseStatus): { title: string; description: string } | null {
  if (!prev || prev.state === next.state) return null;
  const was: LicenseState = prev.state;
  if (was === 'trial' && next.state === 'trial-ended') {
    return { title: 'Your Pro trial has ended', description: 'Nothing was deleted — Pro features are paused until you add a license.' };
  }
  if (was === 'active' && next.state === 'updates-ended') {
    return { title: "This version isn't covered by your license", description: `Akev runs as Free. Renew updates (${RENEWAL_PRICE}) or use an earlier version.` };
  }
  return null;
}
