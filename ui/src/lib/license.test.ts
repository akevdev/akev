import { describe, expect, test } from 'bun:test';
import type { Feature, LicenseStatus } from '../api/types';
import {
  actionFeature,
  actionNeedsPro,
  activationResult,
  describeLicense,
  FEATURES,
  formatLicenseDate,
  interceptorFeature,
  isEntitled,
  isFeature,
  activeLicense,
  licenseeFull,
  licenseeName,
  licenseTransition,
  normalizeLicenseKey,
  planLabel,
  showProBadge,
  trialDaysLeft,
  validateLicenseKeyText,
} from './license';

const ALL: Feature[] = ['scripting', 'tls-impersonation', 'wireguard', 'transparent', 'jvm-attach', 'proto-schemas', 'llm-lens', 'mcp-write'];
const NOW = Date.UTC(2026, 10, 11, 12, 0, 0);
const DAY = 86_400_000;
const LICENSE = { id: 'AKV-1', email: 'ada@example.com', name: 'Ada Lovelace', issued: '2026-11-05', updatesUntil: '2027-11-05' };

function status(patch: Partial<LicenseStatus>): LicenseStatus {
  return { plan: 'free', state: 'none', features: [], trialAvailable: true, releaseDate: '2026-11-02', ...patch };
}

const free = status({});
const pro = status({ plan: 'pro', state: 'active', features: ALL, license: LICENSE, trialAvailable: true });
const trial = status({ plan: 'pro', state: 'trial', features: ALL, trialEndsAt: NOW + 9 * DAY - 60_000, trialAvailable: false });

describe('entitlement', () => {
  test('features come from the status; no status gates nothing', () => {
    expect(isEntitled(null, 'wireguard')).toBe(true);
    expect(isEntitled(undefined, 'llm-lens')).toBe(true);
    expect(isEntitled(free, 'wireguard')).toBe(false);
    expect(isEntitled(pro, 'wireguard')).toBe(true);
    expect(isEntitled(status({ state: 'active', features: ['scripting'] }), 'wireguard')).toBe(false);
  });

  test('Pro badges show while locked and during the trial, not for licensed users', () => {
    expect(showProBadge(null, 'wireguard')).toBe(false);
    expect(showProBadge(free, 'wireguard')).toBe(true);
    expect(showProBadge(trial, 'wireguard')).toBe(true);
    expect(showProBadge(pro, 'wireguard')).toBe(false);
    expect(showProBadge(pro)).toBe(false);
    expect(showProBadge(status({ state: 'updates-ended', license: LICENSE }), 'scripting')).toBe(true);
  });

  test('which interceptors and rule actions are Pro', () => {
    expect(interceptorFeature('wireguard')).toBe('wireguard');
    expect(interceptorFeature('transparent-linux')).toBe('transparent');
    expect(interceptorFeature('jvm')).toBe('jvm-attach');
    expect(interceptorFeature('chrome')).toBeUndefined();
    expect(actionFeature('script')).toBe('scripting');
    expect(actionFeature('mock')).toBeUndefined();
    expect(actionNeedsPro({ type: 'tls-profile', profile: 'chrome' })).toBe('tls-impersonation');
    expect(actionNeedsPro({ type: 'tls-profile', profile: 'native' })).toBeUndefined();
    expect(actionNeedsPro({ type: 'script', source: '' })).toBe('scripting');
  });

  test('the feature catalogue covers every feature once', () => {
    expect(FEATURES.map((f) => f.id).sort()).toEqual([...ALL].sort());
    expect(isFeature('wireguard')).toBe(true);
    expect(isFeature('time-travel')).toBe(false);
    expect(isFeature(undefined)).toBe(false);
  });
});

describe('wording', () => {
  test('plan labels', () => {
    expect(planLabel(null)).toBeNull();
    expect(planLabel(free)).toBe('Free');
    expect(planLabel(pro)).toBe('Pro');
    expect(planLabel(trial)).toBe('Pro trial');
    expect(planLabel(status({ state: 'trial-ended', trialEndsAt: NOW - DAY, trialAvailable: false }))).toBe('Free');
    expect(planLabel(status({ state: 'updates-ended', license: LICENSE }))).toBe('Free');
  });

  test('trial days round up and stop at zero', () => {
    expect(trialDaysLeft(trial, NOW)).toBe(9);
    expect(trialDaysLeft({ ...trial, trialEndsAt: NOW + 3_600_000 }, NOW)).toBe(1);
    expect(trialDaysLeft({ ...trial, trialEndsAt: NOW - 5 }, NOW)).toBe(0);
    expect(trialDaysLeft(free, NOW)).toBeNull();
  });

  test('every state is described', () => {
    expect(describeLicense(pro, NOW)).toMatchObject({ title: 'Akev Pro', tone: 'success' });
    expect(describeLicense(pro, NOW).detail).toContain('5 Nov 2027');
    expect(describeLicense(trial, NOW)).toMatchObject({ title: 'Pro trial · 9 days left', tone: 'accent' });
    expect(describeLicense({ ...trial, trialEndsAt: NOW + 1000 }, NOW).title).toBe('Pro trial · 1 day left');
    expect(describeLicense(status({ state: 'trial-ended', trialAvailable: false }), NOW).detail).toContain('trial has ended');
    const ended = describeLicense(status({ state: 'updates-ended', license: { ...LICENSE, updatesUntil: '2025-06-14' } }), NOW);
    expect(ended.detail).toBe('Your license covers versions released until 14 Jun 2025. Renew updates ($25) or use an earlier version.');
    expect(ended.tone).toBe('warning');
    expect(describeLicense(status({ state: 'invalid', error: "The signature doesn't match" }), NOW)).toMatchObject({ tone: 'danger' });
    expect(describeLicense(status({ state: 'invalid', error: "The signature doesn't match" }), NOW).detail).toContain("The signature doesn't match");
    expect(describeLicense(free, NOW).detail).toContain('14 days');
    expect(describeLicense(status({ trialAvailable: false }), NOW).detail).not.toContain('14 days');
  });

  test('dates', () => {
    expect(formatLicenseDate('2027-11-05')).toBe('5 Nov 2027');
    expect(formatLicenseDate('2026-01-31')).toBe('31 Jan 2026');
    expect(formatLicenseDate('soon')).toBe('soon');
    expect(formatLicenseDate(new Date(2026, 10, 20, 10).getTime())).toBe('20 Nov 2026');
  });

  test('activation results: active, genuine but too old, other', () => {
    expect(activationResult(pro)).toMatchObject({ ok: true, title: 'Akev Pro activated' });
    const old = activationResult(status({ state: 'updates-ended', license: { ...LICENSE, updatesUntil: '2025-06-14' } }));
    expect(old.ok).toBe(false);
    expect(old.description).toContain('Renew updates');
    expect(activationResult(status({ state: 'invalid', error: 'Revoked' })).ok).toBe(false);
  });
});

describe('license keys', () => {
  test('whitespace from wrapped e-mails is removed', () => {
    expect(normalizeLicenseKey('  AKEV1.abc\n def \r\n.sig ')).toBe('AKEV1.abcdef.sig');
  });

  test('client-side check before sending', () => {
    expect(validateLicenseKeyText('')).toBe('Paste your license key.');
    expect(validateLicenseKeyText('   ')).toBe('Paste your license key.');
    expect(validateLicenseKeyText('hello')).toContain('keys start with AKEV1.');
    expect(validateLicenseKeyText('AKEV1.payload.sig')).toBeNull();
  });
});

describe('stream transitions', () => {
  test('the trial ending and an update cut-off are announced; other changes are not', () => {
    expect(licenseTransition(trial, status({ state: 'trial-ended', trialEndsAt: NOW, trialAvailable: false }))?.title).toBe('Your Pro trial has ended');
    expect(licenseTransition(pro, status({ state: 'updates-ended', license: LICENSE }))?.title).toContain("isn't covered");
    expect(licenseTransition(null, pro)).toBeNull();
    expect(licenseTransition(free, pro)).toBeNull();
    expect(licenseTransition(pro, pro)).toBeNull();
    expect(licenseTransition(pro, free)).toBeNull();
  });
});

describe('licensee', () => {
  test('the name, or the email without one', () => {
    expect(licenseeName(LICENSE)).toBe('Ada Lovelace');
    expect(licenseeFull(LICENSE)).toBe('Ada Lovelace <ada@example.com>');
    const noName = { ...LICENSE, name: undefined };
    expect(licenseeName(noName)).toBe('ada@example.com');
    expect(licenseeFull(noName)).toBe('ada@example.com');
    expect(licenseeName({ ...LICENSE, name: '  ' })).toBe('ada@example.com');
  });

  test('only an active license counts as the one unlocking Pro', () => {
    expect(activeLicense(pro)).toEqual(LICENSE);
    expect(activeLicense(status({ state: 'updates-ended', license: LICENSE }))).toBeNull();
    expect(activeLicense(trial)).toBeNull();
    expect(activeLicense(free)).toBeNull();
    expect(activeLicense(null)).toBeNull();
  });
});
