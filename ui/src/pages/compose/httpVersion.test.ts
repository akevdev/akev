import { describe, expect, test } from 'bun:test';
import { DEFAULT_OPTIONS, HTTP_VERSION_CHOICES, sanitizeOptions } from './model';
import { HTTP_VERSION_HINTS } from './OptionsPopover';
import { toSendOptions } from './store';

describe('Compose → HTTP version', () => {
  test('defaults to auto and keeps valid choices', () => {
    expect(DEFAULT_OPTIONS.httpVersion).toBe('auto');
    expect(sanitizeOptions({}).httpVersion).toBe('auto');
    for (const v of HTTP_VERSION_CHOICES) expect(sanitizeOptions({ httpVersion: v }).httpVersion).toBe(v);
    expect(sanitizeOptions({ httpVersion: 'h9' }).httpVersion).toBe('auto');
    expect(sanitizeOptions({ httpVersion: 'HTTP/3' }).httpVersion).toBe('auto');
  });

  test('send options carry httpVersion', () => {
    expect(toSendOptions({ ...DEFAULT_OPTIONS })).toEqual({ applyRules: true, followRedirects: false, insecure: false });
    expect(toSendOptions({ ...DEFAULT_OPTIONS, httpVersion: 'http3' })).toMatchObject({ httpVersion: 'http3' });
    expect(toSendOptions({ ...DEFAULT_OPTIONS, httpVersion: 'http1' })).toEqual({ applyRules: true, followRedirects: false, insecure: false, httpVersion: 'http1' });
    expect(toSendOptions({ ...DEFAULT_OPTIONS, httpVersion: 'http2', timeoutMs: 5000 })).toMatchObject({ httpVersion: 'http2', timeoutMs: 5000 });
  });

  test('every choice explains itself', () => {
    for (const v of HTTP_VERSION_CHOICES) expect(HTTP_VERSION_HINTS[v].length).toBeGreaterThan(10);
  });
});
