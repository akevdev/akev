/**
 * Fixed addresses and secrets of the end-to-end setup (`playwright.config.ts` starts the
 * servers with them). Nothing here is a real secret: the daemon is ephemeral and loopback-only.
 */

import { readFileSync } from 'node:fs';
import { join } from 'node:path';

/** The daemon's API / UI port. */
export const API_PORT = 45491;
/** The Vite dev server for the visual tests (mock backend). */
export const MOCK_UI_PORT = 5191;
/** The daemon's session token (24+ characters, as a network-safe token would need). */
export const TOKEN = 'akev-e2e-local-token-7f3a9c41d2';

export const API_URL = `http://127.0.0.1:${API_PORT}`;
export const MOCK_UI_URL = `http://127.0.0.1:${MOCK_UI_PORT}`;

export const REPO_ROOT = join(import.meta.dirname, '..', '..', '..');

/**
 * The test Pro license from the Rust suites (`crates/akev-api/tests/common/pro.rs`). A daemon
 * only accepts it when built with `AKEV_LICENSE_PUBLIC_KEY` set to its public key (the CI job
 * does, and sets `AKEV_E2E_TEST_LICENSE=1`); release builds never do.
 */
export function testLicense(): { key: string; publicKey: string } {
  const source = readFileSync(join(REPO_ROOT, 'crates/akev-api/tests/common/pro.rs'), 'utf8');
  const key = /TEST_KEY: &str = "([^"]+)"/.exec(source)?.[1];
  const publicKey = /TEST_PUBLIC_KEY: &str = "([^"]+)"/.exec(source)?.[1];
  if (!key || !publicKey) throw new Error('test license not found in crates/akev-api/tests/common/pro.rs');
  return { key, publicKey };
}
