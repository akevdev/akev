/**
 * Mock Akev Pro licensing (`/api/license`, `Status.license`, 402 `pro-required`).
 *
 * URL flags pick the starting state: default = Free with the trial available, `?pro` = an
 * active license, `?trial` = a trial with 9 days left, `?trialEnded`, `?updatesEnded` = a
 * genuine key that doesn't cover this build, `?invalidLicense`. Any key starting with `AKEV1.`
 * activates (`AKEV1.old…` = a key whose updates ended before this build); anything else is
 * rejected with 400 `invalid-license`, like the daemon.
 */

import type { Feature, LicenseInfo, LicenseStatus } from '../types';

export type MockLicenseMode = 'free' | 'pro' | 'trial' | 'trial-ended' | 'updates-ended' | 'invalid';

export const ALL_FEATURES: readonly Feature[] = ['scripting', 'tls-impersonation', 'wireguard', 'transparent', 'jvm-attach', 'proto-schemas', 'llm-lens', 'mcp-write'];

/** This "build's" release date. */
export const MOCK_RELEASE_DATE = '2026-11-02';

const DAY = 86_400_000;
const TRIAL_DAYS = 14;

const FEATURE_NAMES: Record<Feature, string> = {
  scripting: 'Script rules',
  'tls-impersonation': 'TLS fingerprint impersonation',
  wireguard: 'WireGuard capture',
  transparent: 'Transparent capture',
  'jvm-attach': 'JVM attach',
  'proto-schemas': 'Protobuf schemas',
  'llm-lens': 'The LLM conversation view',
  'mcp-write': 'MCP write tools',
};

/** An error the mock server turns into an HTTP error response. */
export class MockLicenseError extends Error {
  constructor(
    readonly status: number,
    readonly code: string,
    message: string,
    readonly details: Record<string, unknown> = {},
  ) {
    super(message);
  }
}

function addYears(date: string, years: number): string {
  const [y, rest] = [Number(date.slice(0, 4)), date.slice(4)];
  return `${y + years}${rest}`;
}

/** The license a mock key carries (`AKEV1.old…` keys stopped getting updates before this build). */
function infoFor(key: string): LicenseInfo {
  const old = key.toLowerCase().startsWith('akev1.old');
  const issued = old ? '2024-06-14' : '2026-11-05';
  return { id: old ? 'AKV-3HF7-QL2D' : 'AKV-7Q2M-9XKD', email: 'ada@example.com', name: 'Ada Lovelace', issued, updatesUntil: addYears(issued, 1) };
}

/** Which mode the page's URL asks for (`?pro`, `?trial`, `?trialEnded`, `?updatesEnded`, `?invalidLicense`). */
export function mockLicenseModeFromQuery(query: string): MockLicenseMode {
  const has = (flag: string) => new RegExp(`[?&]${flag}(=|&|$)`).test(query);
  if (has('pro')) return 'pro';
  if (has('trial')) return 'trial';
  if (has('trialEnded')) return 'trial-ended';
  if (has('updatesEnded')) return 'updates-ended';
  if (has('invalidLicense')) return 'invalid';
  return 'free';
}

export class MockLicense {
  private key: string | null = null;
  private trialEndsAt: number | null = null;
  private keyError: string | null = null;

  constructor(mode: MockLicenseMode = 'free', now: number = Date.now()) {
    switch (mode) {
      case 'pro':
        this.key = 'AKEV1.mock';
        break;
      case 'trial':
        // 9 days left (minus a minute, so "9 days" doesn't round up to 10 at load).
        this.trialEndsAt = now + 9 * DAY - 60_000;
        break;
      case 'trial-ended':
        this.trialEndsAt = now - 2 * DAY;
        break;
      case 'updates-ended':
        this.key = 'AKEV1.old-mock';
        break;
      case 'invalid':
        this.key = 'AKEV1.tampered';
        this.keyError = "The license signature doesn't match — the key was changed after it was issued";
        break;
      case 'free':
        break;
    }
  }

  status(now: number = Date.now()): LicenseStatus {
    const base = { releaseDate: MOCK_RELEASE_DATE, trialAvailable: this.trialEndsAt == null };
    if (this.key) {
      const license = infoFor(this.key);
      if (this.keyError) return { ...base, plan: 'free', state: 'invalid', features: [], license, error: this.keyError };
      if (license.updatesUntil < MOCK_RELEASE_DATE) return { ...base, plan: 'free', state: 'updates-ended', features: [], license };
      return { ...base, plan: 'pro', state: 'active', features: [...ALL_FEATURES], license };
    }
    if (this.trialEndsAt != null) {
      const active = now < this.trialEndsAt;
      return { ...base, plan: active ? 'pro' : 'free', state: active ? 'trial' : 'trial-ended', features: active ? [...ALL_FEATURES] : [], trialEndsAt: this.trialEndsAt };
    }
    return { ...base, plan: 'free', state: 'none', features: [] };
  }

  entitled(feature: Feature, now: number = Date.now()): boolean {
    return this.status(now).features.includes(feature);
  }

  /** Throws 402 `pro-required` unless `feature` is licensed. */
  require(feature: Feature, now: number = Date.now()): void {
    if (this.entitled(feature, now)) return;
    throw new MockLicenseError(402, 'pro-required', `${FEATURE_NAMES[feature]} requires Akev Pro`, { feature });
  }

  /** `PUT /api/license`. */
  activate(rawKey: unknown): LicenseStatus {
    const key = typeof rawKey === 'string' ? rawKey.replace(/\s+/g, '') : '';
    if (!key) throw new MockLicenseError(400, 'invalid-license', 'No license key given');
    if (!key.startsWith('AKEV1.')) throw new MockLicenseError(400, 'invalid-license', "This isn't an Akev license key: keys start with AKEV1.");
    if (key.length < 10) throw new MockLicenseError(400, 'invalid-license', 'The license key is incomplete — copy the whole key from your e-mail');
    this.key = key;
    this.keyError = null;
    return this.status();
  }

  /** `DELETE /api/license`. */
  remove(): LicenseStatus {
    this.key = null;
    this.keyError = null;
    return this.status();
  }

  /** `POST /api/license/trial`. */
  startTrial(now: number = Date.now()): LicenseStatus {
    if (this.trialEndsAt != null) throw new MockLicenseError(409, 'trial-unavailable', 'The Pro trial has already been used on this computer');
    this.trialEndsAt = now + TRIAL_DAYS * DAY;
    return this.status(now);
  }

  /** Dev helper: end a running trial now. Returns whether there was one. */
  expireTrial(now: number = Date.now()): boolean {
    if (this.trialEndsAt == null || this.trialEndsAt <= now) return false;
    this.trialEndsAt = now - 1;
    return true;
  }

  /** Unix ms when the running trial ends (for a timer that announces it), else null. */
  trialEnd(now: number = Date.now()): number | null {
    return !this.key && this.trialEndsAt != null && this.trialEndsAt > now ? this.trialEndsAt : null;
  }
}
