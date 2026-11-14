import { describe, expect, test } from 'bun:test';
import type { LicenseStatus, ServerMessage, Status, TlsProfiles } from '../types';
import { ALL_FEATURES, MockLicense, mockLicenseModeFromQuery, MOCK_RELEASE_DATE } from './license';
import { MockServer } from './server';

const DAY = 86_400_000;
const send = (server: MockServer, method: string, path: string, body?: unknown) =>
  server.fetch(path, { method, ...(body === undefined ? {} : { body: JSON.stringify(body), headers: { 'content-type': 'application/json' } }) });

describe('MockLicense', () => {
  test('URL flags pick the starting state', () => {
    expect(mockLicenseModeFromQuery('?mock')).toBe('free');
    expect(mockLicenseModeFromQuery('?mock&pro')).toBe('pro');
    expect(mockLicenseModeFromQuery('?mock&trial')).toBe('trial');
    expect(mockLicenseModeFromQuery('?mock&trialEnded')).toBe('trial-ended');
    expect(mockLicenseModeFromQuery('?mock&updatesEnded&')).toBe('updates-ended');
    expect(mockLicenseModeFromQuery('&invalidLicense')).toBe('invalid');
    expect(mockLicenseModeFromQuery('?mock&prompt=1')).toBe('free');
  });

  test("each mode reports the contract's shape", () => {
    const now = Date.UTC(2026, 10, 11);
    const s = (mode: ConstructorParameters<typeof MockLicense>[0]) => new MockLicense(mode, now).status(now);
    expect(s('free')).toEqual({ plan: 'free', state: 'none', features: [], trialAvailable: true, releaseDate: MOCK_RELEASE_DATE });
    expect(s('pro')).toMatchObject({ plan: 'pro', state: 'active', features: [...ALL_FEATURES], trialAvailable: true });
    expect(s('pro').license?.updatesUntil! >= MOCK_RELEASE_DATE).toBe(true);
    const trial = s('trial');
    expect(trial).toMatchObject({ plan: 'pro', state: 'trial', trialAvailable: false });
    expect(Math.ceil((trial.trialEndsAt! - now) / DAY)).toBe(9);
    expect(s('trial-ended')).toMatchObject({ plan: 'free', state: 'trial-ended', features: [], trialAvailable: false });
    const old = s('updates-ended');
    expect(old).toMatchObject({ plan: 'free', state: 'updates-ended', features: [] });
    expect(old.license!.updatesUntil < MOCK_RELEASE_DATE).toBe(true);
    expect(s('invalid')).toMatchObject({ plan: 'free', state: 'invalid', features: [] });
    expect(s('invalid').error).toBeTruthy();
  });

  test('keys, trial and removal', () => {
    const now = Date.UTC(2026, 10, 11);
    const l = new MockLicense('free', now);
    expect(() => l.activate('hello')).toThrow('AKEV1.');
    expect(() => l.activate('')).toThrow();
    expect(l.activate('AKEV1.payload.sig').state).toBe('active');
    expect(l.activate('AKEV1.old-key.sig').state).toBe('updates-ended');
    expect(l.remove().state).toBe('none');
    expect(l.startTrial(now)).toMatchObject({ state: 'trial', trialEndsAt: now + 14 * DAY });
    expect(() => l.startTrial(now)).toThrow('already been used');
    expect(l.status(now + 15 * DAY).state).toBe('trial-ended');
    expect(l.entitled('wireguard', now)).toBe(true);
    expect(l.entitled('wireguard', now + 15 * DAY)).toBe(false);
    expect(l.expireTrial(now)).toBe(true);
    expect(l.status(now).state).toBe('trial-ended');
  });
});

describe('mock /api/license and gated routes', () => {
  test('status and hello carry the license; changes are broadcast', async () => {
    const server = new MockServer();
    server.start();
    try {
      const socket = server.openSocket();
      const stream: ServerMessage[] = [];
      socket.onmessage = (ev) => stream.push(JSON.parse(String(ev.data)) as ServerMessage);
      await new Promise((r) => setTimeout(r, 60));
      const hello = stream.find((m) => m.type === 'hello');
      expect(hello?.type === 'hello' && hello.status.license?.state).toBe('none');
      expect(((await (await send(server, 'GET', '/api/status')).json()) as Status).license?.state).toBe('none');
      expect(((await (await send(server, 'GET', '/api/license')).json()) as LicenseStatus).trialAvailable).toBe(true);

      const bad = await send(server, 'PUT', '/api/license', { key: 'nope' });
      expect(bad.status).toBe(400);
      expect(((await bad.json()) as { error: { code: string } }).error.code).toBe('invalid-license');

      const trial = await send(server, 'POST', '/api/license/trial');
      expect(((await trial.json()) as LicenseStatus).state).toBe('trial');
      expect((await send(server, 'POST', '/api/license/trial')).status).toBe(409);
      const ok = await send(server, 'PUT', '/api/license', { key: 'AKEV1.abc.def' });
      expect(((await ok.json()) as LicenseStatus).state).toBe('active');
      const gone = await send(server, 'DELETE', '/api/license');
      expect(((await gone.json()) as LicenseStatus).state).toBe('trial');

      const broadcast = stream.filter((m): m is Extract<ServerMessage, { type: 'license' }> => m.type === 'license').map((m) => m.license.state);
      expect(broadcast).toEqual(['trial', 'active', 'trial']);

      // The trial ending is announced too.
      expect(server.expireTrial()).toBe(true);
      expect(stream.at(-1)).toMatchObject({ type: 'license', license: { state: 'trial-ended' } });
    } finally {
      server.stop();
    }
  });

  test('Free: gated routes answer 402 pro-required with the feature; Pro: they work', async () => {
    const expect402 = async (res: Response, feature: string) => {
      expect(res.status).toBe(402);
      expect(((await res.json()) as { error: { code: string; feature: string } }).error).toMatchObject({ code: 'pro-required', feature });
    };
    const free = new MockServer();
    free.start();
    try {
      await expect402(await send(free, 'PUT', '/api/wireguard', { enabled: true }), 'wireguard');
      await expect402(await send(free, 'POST', '/api/interceptors/jvm/activate', { options: { pid: 1 } }), 'jvm-attach');
      await expect402(await send(free, 'POST', '/api/protos', { files: [{ name: 'a.proto', content: 'syntax = "proto3";' }] }), 'proto-schemas');
      await expect402(await send(free, 'PUT', '/api/protos/mappings', []), 'proto-schemas');
      await expect402(await send(free, 'POST', '/api/transparent/plan', { mode: 'machine' }), 'transparent');
      const list = (await (await send(free, 'GET', '/api/exchanges?filter=is:llm&limit=1')).json()) as { id: number }[];
      await expect402(await send(free, 'GET', `/api/exchanges/${list[0]?.id}/llm`), 'llm-lens');
      const settings = (await (await send(free, 'GET', '/api/settings')).json()) as { proxy: Record<string, unknown> };
      const transparentOn = { ...settings, proxy: { ...settings.proxy, transparent: { enabled: true, port: 8001, mark: 0 } } };
      await expect402(await send(free, 'PUT', '/api/settings', transparentOn), 'transparent');
      expect(((await (await send(free, 'GET', '/api/tls-profiles')).json()) as TlsProfiles).licensed).toBe(false);
      // Free features and switching things off keep working.
      expect((await send(free, 'PUT', '/api/wireguard', { enabled: false })).status).toBe(200);
      expect((await send(free, 'GET', '/api/protos')).status).toBe(200);
      // Pro rule actions are saved, and reported as inactive.
      const rules = { version: 1, items: [{ type: 'rule', id: 'r1', enabled: true, matchers: [], action: { type: 'script', source: 'x' } }, { type: 'rule', id: 'r2', enabled: true, matchers: [], action: { type: 'tls-profile', profile: 'native' } }] };
      const put = (await (await send(free, 'PUT', '/api/rules', rules)).json()) as { errors: [string, string][] };
      expect(put.errors.map(([id]) => id)).toEqual(['r1']);
      expect(put.errors[0]?.[1]).toContain('Inactive');
    } finally {
      free.stop();
    }

    const pro = new MockServer({ license: 'pro' });
    pro.start();
    try {
      expect((await send(pro, 'PUT', '/api/wireguard', { enabled: true })).status).toBe(200);
      expect((await send(pro, 'PUT', '/api/protos/mappings', [])).status).toBe(200);
      expect(((await (await send(pro, 'GET', '/api/tls-profiles')).json()) as TlsProfiles).licensed).toBe(true);
    } finally {
      pro.stop();
    }
  });
});
