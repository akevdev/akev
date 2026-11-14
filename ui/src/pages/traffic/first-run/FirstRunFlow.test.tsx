import { afterEach, beforeEach, describe, expect, test } from 'bun:test';
import { api } from '../../../api/client';
import { mockInterceptors, mockSettings } from '../../../api/mock/data';
import type { Settings, TrustReport } from '../../../api/types';
import { getRoute } from '../../../app/router';
import { useAppStore } from '../../../state/app';
import { useCaTrust } from '../../../state/caTrust';
import { trafficStore } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { render, screen, stub, summary, testStatus, userEvent, waitFor, within } from '../../../test/render';
import { FIRST_RUN_DONE_PREF, useFirstRun } from './firstRun';
import FirstRunFlow, { curlCommand } from './FirstRunFlow';

const untrusted: TrustReport = {
  trusted: false,
  stores: [{ id: 'nss', label: 'Browsers (NSS)', description: 'Chrome and Firefox on Linux', trusted: false, canChange: true, needsAdmin: false }],
};
const trusted: TrustReport = { trusted: true, stores: [{ ...untrusted.stores[0]!, trusted: true }] };

function settings(autosave: boolean): Settings {
  const s = mockSettings();
  return { ...s, sessions: { ...s.sessions!, autosave } };
}

beforeEach(() => {
  trafficStore.reset([]);
  trafficStore.flushNow();
  useFirstRun.setState({ status: 'open', step: 0, baseline: 0 });
  useAppStore.setState({
    status: testStatus(),
    recording: true,
    settings: settings(false),
    interceptors: mockInterceptors('linux').map((i) => ({ ...i, active: false })),
  });
  useCaTrust.setState({ report: null, busy: null });
});

afterEach(() => {
  useUiStore.getState().setPagePref(FIRST_RUN_DONE_PREF, undefined);
  window.location.hash = '';
});

describe('first-run setup', () => {
  test('step 1 says so when the CA is already trusted', async () => {
    stub(api.ca, 'trust', async () => trusted);
    render(<FirstRunFlow />);
    expect(await screen.findByText('Already trusted on this computer')).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Trust' })).toBeNull();
    // A finished step offers "Continue" instead of "Skip this step".
    expect(screen.getByRole('button', { name: 'Continue' })).toBeTruthy();
  });

  test('step 1 trusts the CA after confirming, then moves on', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    const installs = stub(api.ca, 'trustInstall', async () => trusted);
    render(<FirstRunFlow />);
    await userEvent.click(await screen.findByRole('button', { name: 'Trust' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/Trust the Akev certificate in “Browsers \(NSS\)”/)).toBeTruthy();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Trust' }));
    await waitFor(() => expect(useFirstRun.getState().step).toBe(1));
    expect(installs).toEqual([['nss']]);
    expect(await screen.findByText('Choose what to capture', { selector: 'h3' })).toBeTruthy();
  });

  test('step 2 launches an interceptor and waits for traffic', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    const activations = stub(api.interceptors, 'activate', async () => ({ success: true, message: 'Chrome is now intercepted' }));
    useFirstRun.setState({ step: 1 });
    render(<FirstRunFlow />);
    const grid = screen.getByRole('list', { name: 'Intercept an app' });
    await userEvent.click(within(grid).getByRole('button', { name: 'Launch Chrome' }));
    await waitFor(() => expect(useFirstRun.getState().step).toBe(2));
    expect(activations[0]?.[0]).toBe('chrome');
    expect(await screen.findByText('Waiting for your first request…')).toBeTruthy();
    expect(screen.getByText(curlCommand('127.0.0.1:8000'))).toBeTruthy();
  });

  test('step 3 shows the first request when it arrives', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    useFirstRun.setState({ step: 2 });
    render(<FirstRunFlow />);
    expect(screen.getByText('Waiting for your first request…')).toBeTruthy();
    trafficStore.applyExchanges([summary(1, { method: 'POST', host: 'api.example.com', path: '/v1/login', status: 201 })], []);
    expect(await screen.findByText('Your first request arrived')).toBeTruthy();
    expect(screen.getByText('/v1/login')).toBeTruthy();
    // The third step is ticked in the stepper.
    expect(screen.getByRole('button', { name: /See your first request \(done\)/ })).toBeTruthy();
    await userEvent.click(screen.getByRole('button', { name: 'Show my traffic' }));
    expect(useFirstRun.getState().status).toBe('closed');
    expect(useUiStore.getState().pages[FIRST_RUN_DONE_PREF]).toBe(true);
    await waitFor(() => expect(getRoute().params.id).toBe('1'));
  });

  test('offers autosave', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    const saved = stub(useAppStore.getState(), 'saveSettings', async (s: Settings) => {
      useAppStore.setState({ settings: s });
      return s;
    });
    useFirstRun.setState({ step: 2 });
    render(<FirstRunFlow />);
    await userEvent.click(screen.getByRole('switch', { name: /Keep traffic when Akev restarts/ }));
    await waitFor(() => expect(saved.length).toBe(1));
    expect((saved[0]?.[0] as Settings).sessions?.autosave).toBe(true);
  });

  test('"Skip setup" closes it for good', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    render(<FirstRunFlow />);
    await userEvent.click(screen.getByRole('button', { name: 'Skip setup' }));
    expect(useFirstRun.getState().status).toBe('closed');
    expect(useUiStore.getState().pages[FIRST_RUN_DONE_PREF]).toBe(true);
  });

  test('every step can be skipped', async () => {
    stub(api.ca, 'trust', async () => untrusted);
    render(<FirstRunFlow />);
    await userEvent.click(screen.getByRole('button', { name: 'Skip this step' }));
    expect(useFirstRun.getState().step).toBe(1);
    await userEvent.click(screen.getByRole('button', { name: 'Skip this step' }));
    expect(useFirstRun.getState().step).toBe(2);
    await userEvent.click(screen.getByRole('button', { name: 'Close setup' }));
    expect(useFirstRun.getState().status).toBe('closed');
  });
});
