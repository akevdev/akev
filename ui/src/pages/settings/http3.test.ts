import { describe, expect, test } from 'bun:test';
import { HTTP3_MODE_HELP, listenerLine, quicPortWarning } from './http3';
import { searchSettings, SETTINGS_INDEX } from './settingsIndex';

describe('Settings → Proxy → HTTP/3', () => {
  test('every upstream mode explains itself', () => {
    for (const mode of ['off', 'alt-svc', 'prefer'] as const) expect(HTTP3_MODE_HELP[mode].length).toBeGreaterThan(20);
  });

  test('listener status line', () => {
    const on = { upstream: 'off' as const, listen: true, listenPort: 0 };
    expect(listenerLine({ ...on, listen: false }, { enabled: false }, true)).toEqual({ tone: 'off', text: 'Off — clients reach Akev over TCP only.' });
    expect(listenerLine(on, undefined, true).text).toBe('Not connected to Akev');
    expect(listenerLine(on, { enabled: true, address: '127.0.0.1:8000', port: 8000 }, false).tone).toBe('off');
    expect(listenerLine(on, { enabled: true, address: '127.0.0.1:8000', port: 8000 }, true)).toEqual({ tone: 'on', text: 'Listening on UDP', address: '127.0.0.1:8000' });
    expect(listenerLine(on, { enabled: true, error: 'cannot listen on UDP 127.0.0.1:8000: address in use' }, true)).toEqual({ tone: 'error', text: 'cannot listen on UDP 127.0.0.1:8000: address in use' });
    // Just enabled, status not refreshed yet.
    expect(listenerLine(on, { enabled: false }, true).text).toBe('Starting…');
  });

  test('QUIC port warnings', () => {
    expect(quicPortWarning(0)).toBeNull();
    expect(quicPortWarning(8443)).toBeNull();
    expect(quicPortWarning(443)).toContain('administrator');
  });

  test('settings search finds the HTTP/3 rows', () => {
    for (const id of ['http3-upstream', 'http3-listen', 'http3-port']) expect(SETTINGS_INDEX.some((e) => e.id === id && e.section === 'proxy')).toBe(true);
    expect(searchSettings('quic').map((e) => e.id)).toContain('http3-listen');
    expect(searchSettings('alt-svc')[0]?.id).toBe('http3-upstream');
  });
});
