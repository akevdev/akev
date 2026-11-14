/**
 * Component-test helpers (Testing Library on happy-dom, preloaded by `bunfig.toml`).
 *
 * ```tsx
 * import { render, screen, userEvent, stub } from '../../test/render';
 * stub(api.interceptors, 'activate', async () => ({ success: true }));
 * render(<InterceptorTile info={chrome} />);
 * await userEvent.click(screen.getByRole('button', { name: 'Launch Chrome' }));
 * ```
 *
 * `render` wraps the tree in the providers the app shell normally adds (tooltips, confirm
 * dialogs) and every test is cleaned up afterwards, stubs included (`cleanup.ts`, preloaded).
 */

import { render as rtlRender, type RenderOptions, type RenderResult } from '@testing-library/react';
import type { ReactElement, ReactNode } from 'react';
import { mockCaInfo } from '../api/mock/data';
import type { ExchangeSummary, Status } from '../api/types';
import { ConfirmHost } from '../components/ui/confirm';
import { TooltipProvider } from '../components/ui/Tooltip';
import { afterCurrentTest } from './cleanup';

export * from '@testing-library/react';
export { default as userEvent } from '@testing-library/user-event';

function Providers({ children }: { children: ReactNode }) {
  return (
    <TooltipProvider>
      {children}
      <ConfirmHost />
    </TooltipProvider>
  );
}

/** Render inside the app's providers (tooltips, the confirm dialog host). */
export function render(ui: ReactElement, options?: Omit<RenderOptions, 'wrapper'>): RenderResult {
  return rtlRender(ui, { wrapper: Providers, ...options });
}

/** Replace `obj[key]` for the current test (restored after it). Returns the call log. */
export function stub<T extends object, K extends keyof T>(obj: T, key: K, impl: T[K]): unknown[][] {
  const original = obj[key];
  const calls: unknown[][] = [];
  const fn = impl as unknown as (...args: unknown[]) => unknown;
  (obj as Record<K, unknown>)[key] = (...args: unknown[]) => {
    calls.push(args);
    return fn(...args);
  };
  afterCurrentTest(() => {
    obj[key] = original;
  });
  return calls;
}

/** Run `fn` after the current test (e.g. reset a store). */
export function afterThisTest(fn: () => void): void {
  afterCurrentTest(fn);
}

/** A daemon status for `useAppStore` (proxy on 127.0.0.1:8000, API on :45456). */
export function testStatus(patch: Partial<Status> = {}): Status {
  return {
    version: '0.1.0',
    proxy: { port: 8000, bindAddress: '127.0.0.1', addresses: ['127.0.0.1:8000'], http3: { enabled: false } },
    apiUrl: 'http://127.0.0.1:45456',
    dataDir: '/home/me/.local/share/akev',
    ca: mockCaInfo(1_700_000_000_000),
    startedAt: 1_700_000_000_000,
    stats: {} as Status['stats'],
    platform: 'linux',
    desktop: false,
    license: { plan: 'free', state: 'none', features: [], trialAvailable: true, releaseDate: '2026-11-02' },
    ...patch,
  };
}

/** A complete HTTP exchange summary for tests. */
export function summary(id: number, patch: Partial<ExchangeSummary> = {}): ExchangeSummary {
  return {
    id,
    kind: 'http',
    state: 'complete',
    startedAt: 1_700_000_000_000 + id,
    method: 'GET',
    scheme: 'https',
    host: 'api.example.com',
    port: 443,
    path: `/v1/items/${id}`,
    httpVersion: 'HTTP/2',
    status: 200,
    contentKind: 'json',
    requestSize: 0,
    responseSize: 1234,
    ...patch,
  };
}
