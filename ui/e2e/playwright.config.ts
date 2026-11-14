/**
 * End-to-end tests (`bun run e2e`).
 *
 * - `daemon/`: the real UI served by a real, ephemeral Akev (debug build, `cargo run`), with
 *   traffic sent through its proxy to a local upstream server.
 * - `visual/`: screenshots of key screens against the mock backend (a `VITE_MOCK=1` build),
 *   light and dark, compared with the committed baselines in `visual/__screenshots__/`.
 *   Update them with `bun run e2e -- --update-snapshots` and review the PNG diff.
 *
 * Both servers are started (or reused locally when already running); `AKEV_E2E_PROJECT=daemon`
 * or `=visual` runs one project and starts only its server (CI runs the visual tests in the
 * Playwright container the baselines were rendered in). The daemon's per-user config directory
 * is a temporary one so it never sees or touches the real Pro trial record.
 */

import { mkdtempSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { defineConfig, devices, type PlaywrightTestConfig } from '@playwright/test';
import { API_PORT, API_URL, MOCK_UI_PORT, MOCK_UI_URL, REPO_ROOT, TOKEN } from './support/env';

const CI = !!process.env.CI;
const configHome = mkdtempSync(join(tmpdir(), 'akev-e2e-config-'));
const only = process.env.AKEV_E2E_PROJECT;
const wanted = (project: 'daemon' | 'visual') => !only || only === project;

type WebServer = NonNullable<PlaywrightTestConfig['webServer']> extends infer W ? (W extends readonly (infer S)[] ? S : W) : never;

// A developer's settings for their own Akev (their shell or the repository's .env) must not reach
// the test daemon. Playwright merges `env` over process.env, and undefined drops a variable.
const notInherited = ['AKEV_DATA_DIR', 'AKEV_TOKEN', 'AKEV_LICENSE', 'AKEV_URL', 'AKEV_UPDATE_MANIFEST_URL'];
const daemonEnv = {
  ...Object.fromEntries(notInherited.map((name) => [name, undefined])),
  XDG_CONFIG_HOME: configHome,
  RUST_LOG: 'warn',
} as Record<string, string>;

const daemonServer: WebServer = {
  // The debug build; `cargo run` builds it first when needed (CI builds it in an earlier step).
  command: `cargo run -q -p akev-cli -- start --no-open --ephemeral --api-port ${API_PORT} --proxy-port 0 --token ${TOKEN}`,
  cwd: REPO_ROOT,
  url: `${API_URL}/`,
  timeout: 15 * 60_000,
  reuseExistingServer: !CI,
  env: daemonEnv,
  stdout: 'ignore',
  stderr: 'pipe',
};

// A production build that always runs on the mock backend (`VITE_MOCK=1`): no on-demand
// compiling while a test waits, unlike the dev server.
const mockUiServer: WebServer = {
  command: `npx vite build --outDir dist-e2e --emptyOutDir --logLevel warn && npx vite preview --outDir dist-e2e --port ${MOCK_UI_PORT} --strictPort --host 127.0.0.1`,
  cwd: join(import.meta.dirname, '..'),
  url: `${MOCK_UI_URL}/`,
  env: { VITE_MOCK: '1' },
  timeout: 180_000,
  reuseExistingServer: !CI,
  stdout: 'ignore',
  stderr: 'pipe',
};

export default defineConfig({
  testDir: '.',
  testMatch: '**/*.e2e.ts',
  // One daemon, shared state (traffic, rules): run the tests one at a time.
  fullyParallel: false,
  workers: 1,
  retries: CI ? 1 : 0,
  timeout: 30_000,
  expect: {
    timeout: 7_000,
    toHaveScreenshot: { maxDiffPixelRatio: 0.015, threshold: 0.2, animations: 'disabled', caret: 'hide', scale: 'css' },
  },
  forbidOnly: CI,
  reporter: CI ? [['github'], ['list']] : [['list']],
  outputDir: '../test-results/e2e',
  snapshotPathTemplate: '{testDir}/visual/__screenshots__/{arg}{ext}',
  use: {
    ...devices['Desktop Chrome'],
    viewport: { width: 1280, height: 800 },
    locale: 'en-US',
    timezoneId: 'UTC',
    trace: 'retain-on-failure',
  },
  projects: [
    { name: 'daemon', testMatch: 'daemon/**/*.e2e.ts', use: { baseURL: API_URL } },
    { name: 'visual', testMatch: 'visual/**/*.e2e.ts', use: { baseURL: MOCK_UI_URL } },
  ].filter((p) => wanted(p.name as 'daemon' | 'visual')),
  webServer: [...(wanted('daemon') ? [daemonServer] : []), ...(wanted('visual') ? [mockUiServer] : [])],
});
