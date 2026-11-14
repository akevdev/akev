import { afterEach, describe, expect, test } from 'bun:test';
import { api, browserTransport } from '../../../api/client';
import { MockServer } from '../../../api/mock/server';
import type { MockLicenseMode } from '../../../api/mock/license';
import { useAppStore } from '../../../state/app';
import { licenseReducers, useLicenseStore } from '../../../state/license';
import { act, render, screen, testStatus, userEvent, waitFor, within } from '../../../test/render';
import { LicenseSection } from './LicenseSection';

let server: MockServer | null = null;

/** The mock daemon answers /api/license like the real one (activation, trial, removal). */
function daemon(license: MockLicenseMode): MockServer {
  server = new MockServer({ license });
  api.setTransport(server);
  useAppStore.setState({ status: testStatus(), hasConnected: true });
  licenseReducers.set(null);
  return server;
}

afterEach(() => {
  server?.stop(); // its trial timer would keep the test run alive
  server = null;
  api.setTransport(browserTransport);
  licenseReducers.set(null);
});

function planRow(): HTMLElement {
  return document.querySelector('[data-setting="license-plan"]') as HTMLElement;
}

describe('Settings → License', () => {
  test('Free: the plan, a trial, Buy Pro and the key field', async () => {
    daemon('free');
    render(<LicenseSection />);
    await screen.findByRole('heading', { name: 'Plan' });
    expect(within(planRow()).getByText('Free')).toBeTruthy();
    expect(screen.getByRole('button', { name: 'Start 14-day trial' })).toBeTruthy();
    expect(screen.getByRole('button', { name: /Buy Pro/ })).toBeTruthy();
    expect(screen.getByRole('textbox', { name: 'License key' })).toBeTruthy();
    // Every Pro feature is listed as locked.
    const features = screen.getByRole('list', { name: 'Pro features' });
    expect(within(features).getAllByText('(locked)').length).toBeGreaterThan(5);
  });

  test("a key that isn't one is refused inline, before asking the daemon", async () => {
    daemon('free');
    render(<LicenseSection />);
    await userEvent.type(await screen.findByRole('textbox', { name: 'License key' }), 'hello');
    await userEvent.click(screen.getByRole('button', { name: /Activate/ }));
    expect(await screen.findByText(/keys start with AKEV1\./)).toBeTruthy();
  });

  test("the daemon's reason when it rejects a key", async () => {
    daemon('free');
    render(<LicenseSection />);
    await userEvent.type(await screen.findByRole('textbox', { name: 'License key' }), 'AKEV1.x');
    await userEvent.click(screen.getByRole('button', { name: /Activate/ }));
    expect(await screen.findByText(/The license key is incomplete/)).toBeTruthy();
  });

  test('a valid key unlocks Pro and shows who it is licensed to', async () => {
    daemon('free');
    render(<LicenseSection />);
    await userEvent.type(await screen.findByRole('textbox', { name: 'License key' }), 'AKEV1.eyJpZCI6InRlc3QifQ.signature');
    await userEvent.click(screen.getByRole('button', { name: /Activate/ }));
    await waitFor(() => expect(screen.getByText('Licensed to')).toBeTruthy());
    expect(screen.queryByRole('button', { name: 'Start 14-day trial' })).toBeNull();
    expect(within(screen.getByRole('list', { name: 'Pro features' })).queryAllByText('(locked)')).toHaveLength(0);
    expect(screen.getByRole('button', { name: 'Remove…' })).toBeTruthy();
  });

  test('starting the trial', async () => {
    daemon('free');
    render(<LicenseSection />);
    await userEvent.click(await screen.findByRole('button', { name: 'Start 14-day trial' }));
    // Wait on the store, then read the DOM once (polling role queries while this section
    // re-renders is very slow on happy-dom).
    await waitFor(() => expect(useLicenseStore.getState().status?.state).toBe('trial'));
    await act(async () => {
      await new Promise((r) => setTimeout(r, 0));
    });
    expect(screen.queryByRole('button', { name: 'Start 14-day trial' })).toBeNull();
    expect(within(planRow()).getByText(/Pro trial · 14 days left/)).toBeTruthy();
  });

  test('removing the license is confirmed and goes back to Free', async () => {
    daemon('pro');
    render(<LicenseSection />);
    await userEvent.click(await screen.findByRole('button', { name: 'Remove…' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/Nothing is deleted/)).toBeTruthy();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Remove license' }));
    expect(await screen.findByRole('button', { name: 'Start 14-day trial' })).toBeTruthy();
    expect(screen.getByRole('textbox', { name: 'License key' })).toBeTruthy();
  });

  test('a key that no longer verifies says why', async () => {
    daemon('invalid');
    render(<LicenseSection />);
    await screen.findByRole('heading', { name: 'Plan' });
    expect(within(planRow()).getByRole('alert').textContent).toContain("signature doesn't match");
    // The key can be replaced right away.
    expect(screen.getByRole('button', { name: 'Enter a different key…' })).toBeTruthy();
  });
});
