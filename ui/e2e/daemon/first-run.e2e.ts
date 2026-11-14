import { api, expect, openApp, test, viaProxy } from '../support/daemon';

test('first launch: the setup waits for the first request and remembers it was done', async ({ page, upstream }) => {
  // A first launch: no UI preferences anywhere, no traffic.
  await api('/api/settings/ui', { method: 'PATCH', body: {} });
  await openApp(page, 'traffic', { keepFirstRun: true });

  await expect(page.getByRole('heading', { name: 'Set up Akev' })).toBeVisible();
  await expect(page.getByRole('heading', { name: 'Trust the Akev certificate on this computer' })).toBeVisible();
  await page.getByRole('button', { name: /Skip this step|Continue/ }).click();
  await expect(page.getByRole('heading', { name: 'Choose what to capture' })).toBeVisible();
  // The launch tiles come from the daemon's interceptors.
  await expect(page.getByRole('list', { name: 'Intercept an app' }).getByRole('button').first()).toBeVisible();
  await page.getByRole('button', { name: /Skip this step|Continue/ }).click();

  await expect(page.getByText('Waiting for your first request…')).toBeVisible();
  await viaProxy(`${upstream.url}/hello-akev`);
  await expect(page.getByRole('heading', { name: 'Your first request arrived' })).toBeVisible();
  await expect(page.getByText('/hello-akev')).toBeVisible();

  await page.getByRole('button', { name: 'Show my traffic' }).click();
  await expect(page).toHaveURL(/#\/traffic\/\d+$/);
  await expect(page.locator('[role="row"][data-row-id]').filter({ hasText: '/hello-akev' })).toHaveAttribute('aria-selected', 'true');

  // The daemon keeps the preference, so the desktop app and other windows skip it too.
  await expect.poll(async () => ((await api<{ ui: { pages?: Record<string, unknown> } }>('/api/settings')).ui.pages ?? {})['firstRun.done']).toBe(true);
  await page.reload();
  await expect(page.getByRole('heading', { name: 'Set up Akev' })).toHaveCount(0);
});

test('the setup guide can be reopened from the command palette', async ({ page, upstream: _ }) => {
  await openApp(page);
  await page.keyboard.press('Control+k');
  await page.getByPlaceholder(/Type a command/).fill('setup guide');
  await page.getByRole('option', { name: /Run the setup guide/ }).click();
  await expect(page.getByRole('heading', { name: 'Set up Akev' })).toBeVisible();
  await page.getByRole('button', { name: 'Skip setup' }).click();
  await expect(page.getByRole('heading', { name: 'Waiting for traffic' })).toBeVisible();
});
