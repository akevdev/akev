import { describe, expect, test } from 'bun:test';
import { mockSettings } from '../../api/mock/data';
import { setSessions } from './context';
import { SETTINGS_INDEX } from './settingsIndex';
import { mergeSettings } from './transfer';
import { storedListenerTarget } from './validation';

describe('session settings', () => {
  test('setSessions edits one field and keeps the rest', () => {
    const cur = mockSettings();
    const next = setSessions('autosaveIntervalSecs', 15)(cur);
    expect(next.sessions).toEqual({ ...cur.sessions, autosaveIntervalSecs: 15 });
    expect(next.proxy).toBe(cur.proxy);
  });

  test('configuration import merges sessions key by key, validated', () => {
    const cur = mockSettings();
    const merged = mergeSettings(cur, { sessions: { autosave: false, autosaveIntervalSecs: 0, restoreOnStartup: 'yes', autosaveMaxBytes: 5 } });
    expect(merged.sessions).toEqual({ ...cur.sessions, autosave: false, autosaveMaxBytes: 5 });
    // Imports without sessions keep the current ones (never drop autosave by accident).
    expect(mergeSettings(cur, { capture: { maxExchanges: 5 } }).sessions).toEqual(cur.sessions);
  });

  test('every Data-section session row is searchable', () => {
    for (const id of ['autosave', 'autosave-interval', 'restore-on-startup', 'autosave-size', 'saved-sessions']) {
      expect(SETTINGS_INDEX.find((e) => e.id === id)?.section).toBe('data');
    }
  });
});

describe('moving the proxy back to the stored listener', () => {
  const at = (port: number, bindAddress: string) => ({ port, bindAddress });

  test('nothing to do when the proxy listens where the settings say', () => {
    expect(storedListenerTarget(at(8000, '127.0.0.1'), at(8000, '127.0.0.1'))).toBeNull();
    expect(storedListenerTarget(at(8000, 'localhost'), at(8000, '127.0.0.1'))).toBeNull();
    // Port 0 = any port: never "overridden".
    expect(storedListenerTarget(at(0, '127.0.0.1'), at(54321, '127.0.0.1'))).toBeNull();
  });

  test('busy-port fallback or --proxy-port: the stored port on the live address', () => {
    expect(storedListenerTarget(at(8000, '127.0.0.1'), at(8001, '127.0.0.1'))).toEqual({ port: 8000, bindAddress: '127.0.0.1', what: 'port' });
    expect(storedListenerTarget(at(8000, '0.0.0.0'), at(8001, '0.0.0.0'))).toEqual({ port: 8000, bindAddress: '0.0.0.0', what: 'port' });
  });

  test('--proxy-bind override, and both', () => {
    expect(storedListenerTarget(at(8000, 'localhost'), at(8000, '0.0.0.0'))).toEqual({ port: 8000, bindAddress: '127.0.0.1', what: 'bind' });
    expect(storedListenerTarget(at(8000, '[::1]'), at(9000, '0.0.0.0'))).toEqual({ port: 8000, bindAddress: '::1', what: 'both' });
  });
});
