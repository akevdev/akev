import { api, apiFetch, expect, openApp, test } from '../support/daemon';
import { testLicense } from '../support/env';

/**
 * Release-key verification: the daemon trusts only the public keys built into it, and that
 * can't be changed at run time. The CI job builds the e2e daemon with the test key's public
 * key (`AKEV_LICENSE_PUBLIC_KEY`) and sets `AKEV_E2E_TEST_LICENSE=1`; elsewhere the activation
 * test is skipped and the invalid-key and trial flows are covered.
 */
const TEST_KEY_TRUSTED = process.env.AKEV_E2E_TEST_LICENSE === '1';

test.afterEach(async () => {
  await api('/api/license', { method: 'DELETE' });
});

test('Free in an ephemeral session: no trial to start, keys checked before activation', async ({ page }) => {
  await openApp(page, 'settings/license');
  const plan = page.locator('[data-setting="license-plan"]');
  await expect(plan).toContainText('Free');
  // An ephemeral daemon can't record the once-per-computer trial, so it isn't offered.
  await expect(page.getByRole('button', { name: /day trial/ })).toHaveCount(0);
  await expect(page.getByRole('button', { name: /Buy Pro/ })).toBeVisible();

  const field = page.getByRole('textbox', { name: 'License key' });
  await field.fill('not a key');
  await page.getByRole('button', { name: /Activate/ }).click();
  await expect(page.getByText(/keys start with AKEV1\./)).toBeVisible();
});

test("the daemon rejects a key whose signature doesn't verify", async ({ page }) => {
  await openApp(page, 'settings/license');
  await page.getByRole('textbox', { name: 'License key' }).fill('AKEV1.eyJ2IjoxLCJpZCI6ImZha2UifQ.c2lnbmF0dXJl');
  await page.getByRole('button', { name: /Activate/ }).click();
  // Shown inline, and Akev stays Free.
  await expect(page.locator('[role="alert"], [class*="error"]').filter({ hasText: /license|key|signature/i }).first()).toBeVisible();
  await expect(page.locator('[data-setting="license-plan"]')).toContainText('Free');
  const status = await api<{ plan: string }>('/api/license');
  expect(status.plan).toBe('free');
});

test('the trial is refused by an ephemeral daemon', async () => {
  const res = await apiFetch('/api/license/trial', { method: 'POST' });
  expect(res.status).toBe(409);
  expect(((await res.json()) as { error?: { code?: string } }).error?.code).toBe('trial-unavailable');
});

test('a valid key unlocks Pro', async ({ page }) => {
  test.skip(!TEST_KEY_TRUSTED, 'needs a daemon built with AKEV_LICENSE_PUBLIC_KEY (CI); see the comment above');
  await openApp(page, 'settings/license');
  await page.getByRole('textbox', { name: 'License key' }).fill(testLicense().key);
  await page.getByRole('button', { name: /Activate/ }).click();
  await expect(page.locator('[data-setting="license-plan"]')).toContainText('Pro');
  await expect(page.getByRole('main').getByText('Licensed to', { exact: true })).toBeVisible();
  await expect(page.getByRole('main').getByText('Akev tests', { exact: true })).toBeVisible();
  // The status bar names the licensee too.
  await expect(page.getByRole('contentinfo')).toContainText('Akev tests');
});
