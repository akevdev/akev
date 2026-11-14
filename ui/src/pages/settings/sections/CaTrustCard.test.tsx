import { beforeEach, describe, expect, test } from 'bun:test';
import { api } from '../../../api/client';
import type { TrustReport, TrustStore } from '../../../api/types';
import { useCaTrust } from '../../../state/caTrust';
import { render, screen, stub, userEvent, waitFor, within } from '../../../test/render';
import { CaTrustCard } from './CaTrustCard';

const store = (patch: Partial<TrustStore>): TrustStore => ({ id: 'user', label: 'Login keychain', description: 'Safari, Chrome and curl', trusted: false, canChange: true, needsAdmin: false, ...patch });
const report = (...stores: TrustStore[]): TrustReport => ({ trusted: stores[0]?.trusted ?? null, stores });

function row(name: string): HTMLElement {
  return screen.getByText(name).closest('[data-setting]') as HTMLElement;
}

beforeEach(() => {
  useCaTrust.setState({ report: null, busy: null });
});

describe('Settings → Certificates → Trust on this computer', () => {
  test('shows each store with its state', async () => {
    stub(api.ca, 'trust', async () => report(store({ trusted: true }), store({ id: 'nss', label: 'Firefox (NSS)', trusted: null, canChange: false, unavailableReason: 'Firefox is not installed' })));
    render(<CaTrustCard />);
    await screen.findByText('Login keychain');
    expect(within(row('Login keychain')).getByText('Trusted')).toBeTruthy();
    expect(within(row('Login keychain')).getByRole('button', { name: 'Remove' })).toBeTruthy();
    const firefox = row('Firefox (NSS)');
    expect(within(firefox).getByText('Unknown')).toBeTruthy();
    expect(within(firefox).getByText(/Firefox is not installed/)).toBeTruthy();
    // A store Akev can't change offers no button.
    expect(within(firefox).queryByRole('button')).toBeNull();
  });

  test('Trust asks first, then installs and shows the new state', async () => {
    stub(api.ca, 'trust', async () => report(store({ needsAdmin: true })));
    const installs = stub(api.ca, 'trustInstall', async () => report(store({ trusted: true, needsAdmin: true })));
    render(<CaTrustCard />);
    await userEvent.click(await screen.findByRole('button', { name: 'Trust' }));
    const dialog = await screen.findByRole('dialog');
    expect(within(dialog).getByText(/Your system will ask for your password/)).toBeTruthy();
    await userEvent.click(within(dialog).getByRole('button', { name: 'Trust' }));
    await waitFor(() => expect(installs).toEqual([['user']]));
    expect(await screen.findByText('Trusted')).toBeTruthy();
  });

  test('cancelling the confirmation changes nothing', async () => {
    stub(api.ca, 'trust', async () => report(store({})));
    const installs = stub(api.ca, 'trustInstall', async () => report(store({ trusted: true })));
    render(<CaTrustCard />);
    await userEvent.click(await screen.findByRole('button', { name: 'Trust' }));
    await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Cancel' }));
    await waitFor(() => expect(screen.queryByRole('dialog')).toBeNull());
    expect(installs).toHaveLength(0);
    expect(screen.getByText('Not trusted')).toBeTruthy();
  });

  test('Remove is destructive and confirmed', async () => {
    stub(api.ca, 'trust', async () => report(store({ trusted: true })));
    const removals = stub(api.ca, 'trustRemove', async () => report(store({ trusted: false })));
    render(<CaTrustCard />);
    await userEvent.click(await screen.findByRole('button', { name: 'Remove' }));
    await userEvent.click(within(await screen.findByRole('dialog')).getByRole('button', { name: 'Remove' }));
    await waitFor(() => expect(removals).toEqual([['user']]));
    expect(await screen.findByText('Not trusted')).toBeTruthy();
  });

  test('systems without a trust store point to the manual steps', async () => {
    stub(api.ca, 'trust', async () => report());
    render(<CaTrustCard />);
    expect(await screen.findByText('Not available on this system')).toBeTruthy();
  });
});
