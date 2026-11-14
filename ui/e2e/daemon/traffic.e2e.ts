import { readFileSync } from 'node:fs';
import { api, expect, openApp, test, viaProxy } from '../support/daemon';

/** Rows of the traffic list (the header row has no exchange id). */
const rows = (page: import('@playwright/test').Page) => page.locator('[role="row"][data-row-id]');

test('loads with the token and lists requests sent through the proxy', async ({ page, upstream }) => {
  await openApp(page);
  // The empty Traffic page waits for traffic.
  await expect(page.getByRole('heading', { name: 'Waiting for traffic' })).toBeVisible();

  const res = await viaProxy(`${upstream.url}/api/orders?page=2`);
  expect(res.status).toBe(200);
  expect(JSON.parse(res.body)).toMatchObject({ ok: true, path: '/api/orders?page=2' });
  await viaProxy(`${upstream.url}/api/users`, { method: 'POST', body: '{"name":"Ada"}', headers: { 'content-type': 'application/json' } });

  await expect(rows(page)).toHaveCount(2);
  const order = rows(page).filter({ hasText: '/api/orders' });
  await expect(order).toContainText('GET');
  await expect(order).toContainText('200');
  await expect(order).toContainText(upstream.host);
  await expect(rows(page).filter({ hasText: '/api/users' })).toContainText('POST');
  await expect(page.getByRole('contentinfo')).toContainText('2');
});

test('filters the list', async ({ page, upstream }) => {
  await openApp(page);
  for (const path of ['/api/a', '/api/b', '/missing/thing']) await viaProxy(`${upstream.url}${path}`);
  await expect(rows(page)).toHaveCount(3);

  const filter = page.getByRole('combobox', { name: 'Filter traffic' });
  await filter.fill('status:404');
  await filter.press('Enter');
  await expect(rows(page)).toHaveCount(1);
  await expect(rows(page).first()).toContainText('/missing/thing');
  await expect(page.getByText('1 match', { exact: true })).toBeVisible();

  // A filter that doesn't parse is flagged, with the daemon's explanation.
  await filter.fill('stauts:200');
  await filter.press('Enter');
  await expect(filter).toHaveAttribute('aria-invalid', 'true');

  await page.getByRole('button', { name: 'Clear filter' }).click();
  await expect(rows(page)).toHaveCount(3);
});

test("opens an exchange's details", async ({ page, upstream }) => {
  await openApp(page);
  await viaProxy(`${upstream.url}/api/detail?x=1`, { headers: { 'x-trace': 'e2e-42' } });
  await rows(page).filter({ hasText: '/api/detail' }).click();

  await expect(page).toHaveURL(/#\/traffic\/\d+$/);
  const detail = page.getByRole('region', { name: 'Exchange details' }).first();
  await expect(detail).toContainText('/api/detail');
  // Request headers as captured.
  await detail.getByRole('tab', { name: /^Headers/ }).first().click();
  await expect(detail.getByRole('gridcell', { name: 'e2e-42' })).toBeVisible();
  // The response: its headers, and the body.
  await detail.getByRole('tab', { name: /^Headers/ }).nth(1).click();
  await expect(detail.getByRole('gridcell', { name: 'akev-e2e' })).toBeVisible();
  await detail.getByRole('tab', { name: /^Body/ }).last().click();
  await expect(detail.getByText(/"served"/).first()).toBeVisible();
});

test('pauses at a breakpoint and resumes', async ({ page, upstream }) => {
  await api('/api/rules', {
    method: 'PUT',
    body: {
      version: 1,
      items: [{ type: 'rule', id: 'bp', enabled: true, matchers: [{ type: 'path', pattern: { op: 'equals', value: '/pause-me', caseSensitive: false } }], action: { type: 'breakpoint', request: true, response: false } }],
    },
  });
  await openApp(page);
  const pending = viaProxy(`${upstream.url}/pause-me`);

  const row = rows(page).filter({ hasText: '/pause-me' });
  await expect(row.getByLabel('Paused').first()).toBeVisible();
  // Held before it reached the server.
  expect(upstream.requests.some((r) => r.path === '/pause-me')).toBe(false);

  await row.click();
  // The paused banner's Resume (the editors below have one too).
  await page.getByRole('status').getByRole('button', { name: /^Resume/ }).click();
  const res = await pending;
  expect(res.status).toBe(200);
  expect(upstream.requests.filter((r) => r.path === '/pause-me')).toHaveLength(1);
  await expect(row.getByLabel('Paused')).toHaveCount(0);
  await expect(row).toContainText('200');
});

test('creates a mock rule from an exchange, and the proxy answers with it', async ({ page, upstream }) => {
  await openApp(page);
  await viaProxy(`${upstream.url}/mock-me`);
  const captured = rows(page).filter({ hasText: '/mock-me' });
  await captured.click({ button: 'right' });
  await page.getByRole('menuitem', { name: 'Create rule' }).hover();
  await page.getByRole('menuitem', { name: 'Mock this response' }).click();

  // The Rules page opens on the new rule, prefilled from the exchange.
  await expect(page).toHaveURL(/#\/rules\//);
  const status = page.locator('input[inputmode="numeric"]').first();
  await status.fill('418');
  await expect(page.getByText('Saved', { exact: true })).toBeVisible();

  const res = await viaProxy(`${upstream.url}/mock-me`);
  expect(res.status).toBe(418);
  // The captured body, replayed by Akev: the server wasn't asked again.
  expect(JSON.parse(res.body)).toMatchObject({ path: '/mock-me', served: 1 });
  expect(upstream.requests.filter((r) => r.path === '/mock-me')).toHaveLength(1);

  await page.getByRole('link', { name: 'Traffic', exact: true }).click();
  const mocked = rows(page).filter({ hasText: '/mock-me' }).last();
  await expect(mocked).toContainText('418');
  await expect(mocked.getByLabel('Mocked')).toBeVisible();
});

test('Compose sends a request', async ({ page, upstream }) => {
  await openApp(page, 'compose');
  const url = page.getByRole('combobox', { name: 'Request URL' });
  await url.click();
  await url.fill(`${upstream.url}/compose?from=e2e`);
  await page.getByRole('button', { name: 'Send request' }).click();

  await expect.poll(() => upstream.requests.some((r) => r.path === '/compose?from=e2e')).toBe(true);
  await expect(page.getByText('"served"').first()).toBeVisible();
  // Sent requests are recorded like any other traffic.
  await expect.poll(async () => (await api<{ path: string }[]>('/api/exchanges')).some((s) => s.path.startsWith('/compose'))).toBe(true);
});

test('exports traffic as HAR', async ({ page, upstream }) => {
  await openApp(page);
  await viaProxy(`${upstream.url}/har/one`, { headers: { authorization: 'Bearer e2e-secret-token-123' } });
  await viaProxy(`${upstream.url}/har/two`);
  await expect(rows(page)).toHaveCount(2);

  await page.getByRole('button', { name: 'Import / export' }).click();
  const downloading = page.waitForEvent('download');
  await page.getByRole('menuitem', { name: /Export all as HAR/ }).click();
  const download = await downloading;
  expect(download.suggestedFilename()).toMatch(/\.har$/);
  const text = readFileSync(await download.path(), 'utf8');
  const har = JSON.parse(text) as { log: { entries: { request: { url: string; headers: { name: string; value: string }[] }; response: { status: number } }[] } };
  expect(har.log.entries.map((e) => new URL(e.request.url).pathname).sort()).toEqual(['/har/one', '/har/two']);
  expect(har.log.entries.every((e) => e.response.status === 200)).toBe(true);
  // Secrets are redacted by default (Import / export → Redact secrets in HAR files).
  expect(text).not.toContain('e2e-secret-token-123');
  const auth = har.log.entries.flatMap((e) => e.request.headers).find((h) => h.name.toLowerCase() === 'authorization');
  expect(auth?.value).toBeTruthy();
});
