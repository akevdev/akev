/**
 * Visual regression: key screens against the mock backend (a `VITE_MOCK=1` build), light and dark. The mock's data is
 * seeded and `?still` stops new traffic; the clock is fixed so times, durations and anything in
 * flight render the same on every run. Baselines live in `__screenshots__/` (10 images):
 * update them with `bun run e2e -- --project visual --update-snapshots` and review the diff.
 */

import { expect, test, type Page } from '@playwright/test';

const FIXED_NOW = new Date('2026-11-11T10:00:00Z');

async function open(page: Page, route: string, theme: 'light' | 'dark', flags = ''): Promise<void> {
  await page.clock.setFixedTime(FIXED_NOW);
  await page.goto(`/?mock&still${flags}&theme=${theme}#/${route}`);
  await page.evaluate(() => document.fonts.ready);
  // Connected to the mock: the status bar shows the proxy.
  await expect(page.getByRole('contentinfo').getByText('127.0.0.1:8000')).toBeVisible();
}

/** Settle: no hover, no focus ring from the last click, lazy chunks loaded. */
async function settle(page: Page): Promise<void> {
  await page.mouse.move(0, 0);
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.waitForLoadState('networkidle');
  await page.waitForTimeout(300);
}

for (const theme of ['light', 'dark'] as const) {
  test.describe(`${theme} theme`, () => {
    test('traffic list and an exchange', async ({ page }) => {
      await open(page, 'traffic', theme);
      await page.locator('[role="row"][data-row-id]').filter({ hasText: 'api.github.com' }).last().click();
      await expect(page.getByRole('region', { name: 'Exchange details' }).first().getByRole('tab', { name: /^Headers/ }).first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`traffic-${theme}.png`);
    });

    test('first-run setup', async ({ page }) => {
      await open(page, 'traffic', theme, '&empty');
      await expect(page.getByRole('heading', { name: 'Set up Akev' })).toBeVisible();
      await expect(page.getByText('Not trusted yet')).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`first-run-${theme}.png`);
    });

    test('rules', async ({ page }) => {
      await open(page, 'rules', theme);
      await expect(page.getByRole('button', { name: 'New rule', exact: true }).first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`rules-${theme}.png`);
    });

    test('connect', async ({ page }) => {
      await open(page, 'connect', theme);
      await expect(page.getByRole('heading', { name: /Connect/ }).first()).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`connect-${theme}.png`);
    });

    test('settings: AI agents (MCP)', async ({ page }) => {
      await open(page, 'settings/mcp', theme);
      await expect(page.getByRole('heading', { name: 'Connect an AI agent' })).toBeVisible();
      await settle(page);
      await expect(page).toHaveScreenshot(`settings-mcp-${theme}.png`);
    });
  });
}
