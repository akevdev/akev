/**
 * Upstream TLS fingerprint profiles (`proxy.tlsProfile`, the `tls-profile` rule action and
 * `UpstreamInfo.fingerprint`): labels, select options, override-list editing and the
 * "what did the server see" summary for the Connection tab. Pure — no React.
 */

import type { TlsProfile, TlsProfileInfo, TlsProfileOverride, TlsProfiles, UpstreamFingerprint } from '../api/types';

export const TLS_PROFILE_IDS: readonly TlsProfile[] = ['native', 'chrome', 'firefox', 'safari', 'edge', 'mirror-client'];

export const TLS_PROFILE_LABELS: Readonly<Record<TlsProfile, string>> = {
  native: 'Native',
  chrome: 'Chrome',
  firefox: 'Firefox',
  safari: 'Safari',
  edge: 'Edge',
  'mirror-client': 'Mirror client',
};

/** One-line hints for selects (the daemon's catalogue has the long descriptions). */
export const TLS_PROFILE_HINTS: Readonly<Record<TlsProfile, string>> = {
  native: "Akev's own TLS stack (rustls)",
  chrome: 'Desktop Chrome: ClientHello + HTTP/2',
  firefox: 'Desktop Firefox: ClientHello + HTTP/2',
  safari: 'Safari on macOS: ClientHello + HTTP/2',
  edge: 'Edge (same Chromium fingerprint as Chrome)',
  'mirror-client': "The intercepted browser's own family",
};

export function isTlsProfile(v: string): v is TlsProfile {
  return (TLS_PROFILE_IDS as readonly string[]).includes(v);
}

/** Label of a profile id (unknown ids from newer daemons are shown verbatim). */
export function tlsProfileLabel(id: string): string {
  return isTlsProfile(id) ? TLS_PROFILE_LABELS[id] : id;
}

/** Whether a profile needs the impersonation backend. */
export function needsImpersonation(id: TlsProfile): boolean {
  return id !== 'native';
}

/** Whether `id` works with this daemon (`catalog` = `GET /api/tls-profiles`; unknown = assume yes). */
export function profileAvailable(id: TlsProfile, catalog: TlsProfiles | null | undefined): boolean {
  if (!needsImpersonation(id) || !catalog) return true;
  return catalog.profiles.find((p) => p.id === id)?.available ?? catalog.available;
}

export interface TlsProfileOption {
  value: TlsProfile;
  label: string;
  description: string;
  /** Selectable but falls back to native in this build. */
  unavailable: boolean;
}

/** Select options, annotated with availability. */
export function tlsProfileOptions(catalog: TlsProfiles | null | undefined): TlsProfileOption[] {
  return TLS_PROFILE_IDS.map((id) => {
    const info: TlsProfileInfo | undefined = catalog?.profiles.find((p) => p.id === id);
    const unavailable = !profileAvailable(id, catalog);
    const base = info?.imitates ?? TLS_PROFILE_HINTS[id];
    return { value: id, label: TLS_PROFILE_LABELS[id], description: unavailable ? `${base} · needs an impersonation build` : base, unavailable };
  });
}

// =====================================================================================
// Per-host overrides
// =====================================================================================

/** Normalise an override host as typed (trim, lowercase, drop a trailing dot). */
export function normalizeOverrideHost(host: string): string {
  const h = host.trim().toLowerCase();
  return h.endsWith('.') ? h.slice(0, -1) : h;
}

/** Add (or replace the profile of) an override; returns the new list. */
export function upsertOverride(list: readonly TlsProfileOverride[], host: string, profile: TlsProfile): TlsProfileOverride[] {
  const h = normalizeOverrideHost(host);
  if (!h) return [...list];
  const i = list.findIndex((o) => normalizeOverrideHost(o.host) === h);
  if (i >= 0) return list.map((o, j) => (j === i ? { host: h, profile } : o));
  return [...list, { host: h, profile }];
}

export function setOverrideProfile(list: readonly TlsProfileOverride[], index: number, profile: TlsProfile): TlsProfileOverride[] {
  return list.map((o, i) => (i === index ? { ...o, profile } : o));
}

export function removeOverride(list: readonly TlsProfileOverride[], index: number): TlsProfileOverride[] {
  return list.filter((_, i) => i !== index);
}

/** Move an override up/down (order matters: the first match wins). */
export function moveOverride(list: readonly TlsProfileOverride[], index: number, delta: -1 | 1): TlsProfileOverride[] {
  const to = index + delta;
  if (to < 0 || to >= list.length) return [...list];
  const next = [...list];
  const [item] = next.splice(index, 1);
  next.splice(to, 0, item!);
  return next;
}

// =====================================================================================
// Connection tab
// =====================================================================================

export type FingerprintTone = 'accent' | 'neutral' | 'warning';

export interface FingerprintSummary {
  /** e.g. `Chrome`, `Native`. */
  label: string;
  /** e.g. `mirrored from the client`, `Chrome requested`. */
  hint?: string;
  tone: FingerprintTone;
  /** Explanation when the requested profile wasn't used. */
  note?: string;
}

/** How to present `UpstreamInfo.fingerprint`. */
export function summarizeFingerprint(fp: UpstreamFingerprint): FingerprintSummary {
  const label = tlsProfileLabel(fp.profile);
  const requested = fp.requested;
  if (fp.note) {
    return { label, hint: requested ? `${tlsProfileLabel(requested)} requested` : undefined, tone: 'warning', note: fp.note };
  }
  if (requested === 'mirror-client') return { label, hint: 'mirrored from the client', tone: fp.profile === 'native' ? 'neutral' : 'accent' };
  if (requested && requested !== fp.profile) return { label, hint: `${tlsProfileLabel(requested)} requested`, tone: 'neutral' };
  return { label, hint: fp.profile === 'native' ? "Akev's own TLS stack" : 'impersonated', tone: fp.profile === 'native' ? 'neutral' : 'accent' };
}

/** Split an Akamai HTTP/2 fingerprint into its parts for display. */
export function parseAkamai(fp: string): { settings: string; windowUpdate: string; priority: string; pseudoOrder: string } | null {
  const parts = fp.split('|');
  if (parts.length !== 4) return null;
  const [settings, windowUpdate, priority, pseudoOrder] = parts as [string, string, string, string];
  return { settings, windowUpdate, priority, pseudoOrder };
}
