import { afterEach, describe, expect, test } from 'bun:test';
import { api, ApiClient, ApiError, browserTransport, type Transport, type WebSocketLike } from '../api/client';
import { MockServer } from '../api/mock/server';
import type { LicenseStatus, Status } from '../api/types';
import { isEntitled } from '../lib/license';
import { useAppStore } from './app';
import { closeUpgrade, getLicense, handleProRequired, installLicenseHandlers, isRedundantErrorToast, licenseReducers, openLicenseKeyEntry, openUpgrade, setUpgradeView, useLicenseStore } from './license';

const free: LicenseStatus = { plan: 'free', state: 'none', features: [], trialAvailable: true, releaseDate: '2026-11-02' };
const pro: LicenseStatus = { ...free, plan: 'pro', state: 'active', features: ['wireguard', 'scripting'] };

const proRequired = (method: string, feature: unknown = 'wireguard') => new ApiError(402, 'pro-required', 'WireGuard capture requires Akev Pro', method, '/api/wireguard', { feature });

afterEach(() => {
  closeUpgrade();
  licenseReducers.set(null);
});

describe('license store', () => {
  test('a status from the stream drives entitlement and Status.license', () => {
    useAppStore.setState({ status: { version: 'x' } as Status });
    expect(isEntitled(getLicense(), 'wireguard')).toBe(true); // no status: no gating
    licenseReducers.set(free);
    expect(isEntitled(getLicense(), 'wireguard')).toBe(false);
    expect(useAppStore.getState().status?.license).toBe(free);
    licenseReducers.set(pro);
    expect(isEntitled(getLicense(), 'wireguard')).toBe(true);
    expect(isEntitled(getLicense(), 'llm-lens')).toBe(false);
    expect(useAppStore.getState().status?.license).toBe(pro);
    useAppStore.setState({ status: null });
  });

  test('the upgrade dialog opens for a feature, at the key form, and closes', () => {
    openUpgrade('llm-lens');
    expect(useLicenseStore.getState().upgrade).toEqual({ open: true, feature: 'llm-lens', view: 'offer' });
    setUpgradeView('key');
    expect(useLicenseStore.getState().upgrade).toEqual({ open: true, feature: 'llm-lens', view: 'key' });
    closeUpgrade();
    expect(useLicenseStore.getState().upgrade.open).toBe(false);
    openLicenseKeyEntry();
    expect(useLicenseStore.getState().upgrade).toEqual({ open: true, feature: null, view: 'key' });
  });
});

describe('pro-required → upgrade dialog', () => {
  test('a user action answered with 402 opens the dialog for its feature', () => {
    expect(handleProRequired(proRequired('PUT'))).toBe(true);
    expect(useLicenseStore.getState().upgrade).toMatchObject({ open: true, feature: 'wireguard' });
  });

  test('background reads, other errors and unknown features', () => {
    expect(handleProRequired(proRequired('GET'))).toBe(false);
    expect(handleProRequired(new ApiError(400, 'invalid-request', 'nope', 'POST', '/api/x'))).toBe(false);
    expect(useLicenseStore.getState().upgrade.open).toBe(false);
    expect(handleProRequired(proRequired('POST', 'time-travel'))).toBe(true);
    expect(useLicenseStore.getState().upgrade).toMatchObject({ open: true, feature: null });
  });

  test('the error toast repeating it is redundant while the dialog explains it', () => {
    const now = 1_000_000;
    handleProRequired(proRequired('PUT'), now);
    expect(isRedundantErrorToast("Couldn't update WireGuard mode", { description: 'WireGuard capture requires Akev Pro' }, now + 10)).toBe(true);
    expect(isRedundantErrorToast("Couldn't update WireGuard mode", { description: 'Address already in use' }, now + 10)).toBe(false);
    expect(isRedundantErrorToast('x', { description: 'WireGuard capture requires Akev Pro' }, now + 10_000)).toBe(false);
    closeUpgrade();
    expect(isRedundantErrorToast('x', { description: 'WireGuard capture requires Akev Pro' }, now + 10)).toBe(false);
  });

  test("wired through the API client: the 402 body's feature reaches the dialog", async () => {
    const transport: Transport = {
      fetch: async () => new Response(JSON.stringify({ error: { code: 'pro-required', message: 'JVM attach requires Akev Pro', feature: 'jvm-attach' } }), { status: 402, headers: { 'content-type': 'application/json' } }),
      openSocket: () => ({}) as WebSocketLike,
    };
    const client = new ApiClient({ baseUrl: '', token: null }, transport);
    const uninstall = installLicenseHandlers(client);
    try {
      const err = await client.interceptors.activate('jvm', { pid: 1 }).catch((e: unknown) => e);
      expect(err).toBeInstanceOf(ApiError);
      expect((err as ApiError).status).toBe(402);
      expect((err as ApiError).isProRequired).toBe(true);
      expect((err as ApiError).feature).toBe('jvm-attach');
      expect(useLicenseStore.getState().upgrade).toMatchObject({ open: true, feature: 'jvm-attach' });
    } finally {
      uninstall();
    }
    // Uninstalled: no more dialogs.
    closeUpgrade();
    await client.interceptors.activate('jvm').catch(() => undefined);
    expect(useLicenseStore.getState().upgrade.open).toBe(false);
  });
});

describe('license actions against the mock daemon', () => {
  test('trial, bad key, good key, remove', async () => {
    const server = new MockServer();
    api.setTransport(server);
    try {
      expect((await useLicenseStore.getState().refresh()).state).toBe('none');
      const trial = await useLicenseStore.getState().startTrial();
      expect(trial).toMatchObject({ state: 'trial', plan: 'pro', trialAvailable: false });
      expect(getLicense()?.state).toBe('trial');
      const again = await useLicenseStore.getState().startTrial().catch((e: unknown) => e);
      expect(again).toMatchObject({ status: 409, code: 'trial-unavailable' });

      const bad = await useLicenseStore.getState().activate('not-a-key').catch((e: unknown) => e);
      expect(bad).toMatchObject({ status: 400, code: 'invalid-license' });
      expect((bad as ApiError).message).toContain('AKEV1.');
      expect(getLicense()?.state).toBe('trial');

      const active = await useLicenseStore.getState().activate(' AKEV1.payload\n.signature ');
      expect(active).toMatchObject({ state: 'active', plan: 'pro' });
      expect(active.license?.email).toBe('ada@example.com');
      expect(isEntitled(getLicense(), 'llm-lens')).toBe(true);

      const removed = await useLicenseStore.getState().remove();
      // Back to the (still running) trial.
      expect(removed.state).toBe('trial');
    } finally {
      api.setTransport(browserTransport);
      server.stop();
    }
  });
});
