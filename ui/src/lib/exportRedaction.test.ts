import { afterEach, describe, expect, test } from 'bun:test';
import { ApiClient } from '../api/client';
import { redactExports, setRedactExports } from './exportRedaction';

describe('export redaction preference', () => {
  const g = globalThis as { window?: unknown };
  const hadWindow = 'window' in g;
  // The test preload provides a DOM `window`: put it back after the fakes.
  const realWindow = g.window;
  afterEach(() => {
    if (!hadWindow) delete g.window;
    else g.window = realWindow;
  });

  test('HAR exports redact by default, .akev downloads do not', () => {
    expect(redactExports('har')).toBe(true);
    expect(redactExports('session')).toBe(false);
  });

  test('the choice is remembered', () => {
    const items = new Map<string, string>();
    g.window = {
      localStorage: {
        getItem: (k: string) => items.get(k) ?? null,
        setItem: (k: string, v: string) => void items.set(k, v),
        removeItem: (k: string) => void items.delete(k),
      },
    };
    setRedactExports('har', false);
    setRedactExports('session', true);
    expect(redactExports('har')).toBe(false);
    expect(redactExports('session')).toBe(true);
  });

  test('export URLs carry redact=1 only when asked', () => {
    const client = new ApiClient({ baseUrl: 'http://127.0.0.1:8100', token: 't' });
    expect(client.transfer.harUrl({ redact: true })).toContain('redact=1');
    expect(client.transfer.harUrl({ filter: 'host:x', redact: false })).not.toContain('redact');
    expect(client.transfer.sessionUrl({ redact: true })).toContain('redact=1');
  });
});
