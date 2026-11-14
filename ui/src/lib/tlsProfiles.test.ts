import { describe, expect, test } from 'bun:test';
import type { TlsProfiles } from '../api/types';
import {
  isTlsProfile,
  moveOverride,
  normalizeOverrideHost,
  parseAkamai,
  profileAvailable,
  removeOverride,
  setOverrideProfile,
  summarizeFingerprint,
  TLS_PROFILE_IDS,
  tlsProfileLabel,
  tlsProfileOptions,
  upsertOverride,
} from './tlsProfiles';

const catalog = (available: boolean): TlsProfiles => ({
  available,
  backend: available ? 'BoringSSL' : undefined,
  profiles: TLS_PROFILE_IDS.map((id) => ({
    id,
    label: tlsProfileLabel(id),
    description: '',
    imitates: id === 'chrome' ? 'Chrome 133+ (desktop)' : undefined,
    available: id === 'native' || available,
  })),
});

describe('profiles', () => {
  test('labels and ids', () => {
    expect(TLS_PROFILE_IDS).toEqual(['native', 'chrome', 'firefox', 'safari', 'edge', 'mirror-client']);
    expect(tlsProfileLabel('mirror-client')).toBe('Mirror client');
    expect(tlsProfileLabel('opera-gx')).toBe('opera-gx');
    expect(isTlsProfile('safari')).toBe(true);
    expect(isTlsProfile('Safari')).toBe(false);
  });

  test('availability follows the build', () => {
    expect(profileAvailable('native', catalog(false))).toBe(true);
    expect(profileAvailable('chrome', catalog(false))).toBe(false);
    expect(profileAvailable('mirror-client', catalog(false))).toBe(false);
    expect(profileAvailable('chrome', catalog(true))).toBe(true);
    // Unknown (catalogue not loaded yet): don't nag.
    expect(profileAvailable('chrome', null)).toBe(true);
  });

  test('options describe what is imitated and flag unavailable profiles', () => {
    const on = tlsProfileOptions(catalog(true));
    expect(on.map((o) => o.value)).toEqual([...TLS_PROFILE_IDS]);
    expect(on[1]!.description).toBe('Chrome 133+ (desktop)');
    expect(on.every((o) => !o.unavailable)).toBe(true);
    const off = tlsProfileOptions(catalog(false));
    expect(off[0]!.unavailable).toBe(false);
    expect(off[1]!.unavailable).toBe(true);
    expect(off[1]!.description).toContain('needs an impersonation build');
  });
});

describe('overrides', () => {
  test('upsert normalises and replaces existing hosts', () => {
    let list = upsertOverride([], '  API.Example.com. ', 'chrome');
    expect(list).toEqual([{ host: 'api.example.com', profile: 'chrome' }]);
    list = upsertOverride(list, '*.shop.test', 'safari');
    list = upsertOverride(list, 'api.example.com', 'firefox');
    expect(list).toEqual([
      { host: 'api.example.com', profile: 'firefox' },
      { host: '*.shop.test', profile: 'safari' },
    ]);
    expect(upsertOverride(list, '   ', 'edge')).toEqual(list);
    expect(normalizeOverrideHost('X.Y.')).toBe('x.y');
  });

  test('edit, remove and reorder', () => {
    const list = [
      { host: 'a', profile: 'chrome' as const },
      { host: 'b', profile: 'safari' as const },
      { host: 'c', profile: 'edge' as const },
    ];
    expect(setOverrideProfile(list, 1, 'firefox')[1]).toEqual({ host: 'b', profile: 'firefox' });
    expect(removeOverride(list, 0).map((o) => o.host)).toEqual(['b', 'c']);
    expect(moveOverride(list, 2, -1).map((o) => o.host)).toEqual(['a', 'c', 'b']);
    expect(moveOverride(list, 0, -1).map((o) => o.host)).toEqual(['a', 'b', 'c']);
    expect(moveOverride(list, 2, 1).map((o) => o.host)).toEqual(['a', 'b', 'c']);
  });
});

describe('fingerprint summary', () => {
  test('impersonated', () => {
    expect(summarizeFingerprint({ profile: 'chrome', ja4: 't13d1516h2_8daaf6152771_d8a2da3f94cd' })).toEqual({ label: 'Chrome', hint: 'impersonated', tone: 'accent' });
  });
  test('native', () => {
    expect(summarizeFingerprint({ profile: 'native' })).toEqual({ label: 'Native', hint: "Akev's own TLS stack", tone: 'neutral' });
  });
  test('mirrored', () => {
    expect(summarizeFingerprint({ profile: 'firefox', requested: 'mirror-client' })).toEqual({ label: 'Firefox', hint: 'mirrored from the client', tone: 'accent' });
  });
  test('fallback with a note', () => {
    const s = summarizeFingerprint({ profile: 'native', requested: 'chrome', note: 'browser profiles need an impersonation build' });
    expect(s).toEqual({ label: 'Native', hint: 'Chrome requested', tone: 'warning', note: 'browser profiles need an impersonation build' });
  });
  test('akamai parts', () => {
    expect(parseAkamai('1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p')).toEqual({ settings: '1:65536;2:0;4:6291456;6:262144', windowUpdate: '15663105', priority: '0', pseudoOrder: 'm,a,s,p' });
    expect(parseAkamai('nope')).toBeNull();
  });
});
