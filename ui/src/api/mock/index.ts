/**
 * In-browser mock backend entry point (`?mock` or `VITE_MOCK=1`).
 *
 * `installMock(api)` swaps the API client's transport for a {@link MockServer} that
 * implements every REST endpoint and the live stream with realistic generated traffic, so
 * pages can be built, reviewed and screenshotted without the daemon. The module is loaded
 * with a dynamic `import()` from `main.tsx`, so none of it ships in the normal startup path.
 *
 * Dev helpers are exposed on `window.__akevMock` (see {@link MockControls}), e.g. in the
 * browser console: `__akevMock.drop(5000)` to simulate the daemon going away for 5 s.
 */

import type { ApiClient } from '../client';
import { MockServer } from './server';

export { MockServer } from './server';

/** Console helpers for exercising edge cases by hand. */
export interface MockControls {
  /** The running mock server. */
  readonly server: MockServer;
  /** Drop all stream connections; refuse reconnects for `refuseForMs` (reconnect overlay). */
  drop(refuseForMs?: number): void;
  /** Stop generating traffic and close sockets. */
  stop(): void;
  /** Start generating traffic (`?empty` waits for an interceptor launch). */
  startTraffic(): void;
  /** End a running Pro trial now (start one with `?trial`); the UI gets the `license` message. */
  expireTrial(): boolean;
}

declare global {
  interface Window {
    __akevMock?: MockControls;
  }
}

/**
 * Install the mock backend into `client` and start generating traffic. Idempotent: a second
 * call returns the already-running server.
 */
export function installMock(client: ApiClient): MockServer {
  const existing = typeof window !== 'undefined' ? window.__akevMock?.server : undefined;
  if (existing) {
    client.setTransport(existing);
    return existing;
  }
  const server = new MockServer();
  // Same-origin, token-less: the mock transport ignores auth entirely.
  client.configure({ baseUrl: '' });
  client.setTransport(server);
  server.start();
  if (typeof window !== 'undefined') {
    window.__akevMock = {
      server,
      drop: (refuseForMs = 0) => server.dropConnections(refuseForMs),
      stop: () => server.stop(),
      startTraffic: () => server.startTraffic(),
      expireTrial: () => server.expireTrial(),
    };
    console.info(
      '%cAkev%c mock backend active — try __akevMock.drop(5000) to test reconnecting.',
      'background:#7c5cff;color:#fff;border-radius:3px;padding:1px 5px;font-weight:600',
      'color:inherit',
    );
  }
  return server;
}
