import { describe, expect, test } from 'bun:test';
import { MockTransparent } from '../../api/mock/transparent';
import { TRANSPARENT_PLANS } from '../../api/mock/transparentFixtures';
import type { InterceptorInfo, Settings, TransparentStatus } from '../../api/types';
import { iconKey, panelKindFor, primaryActionFor, statusOf, stopsFromPanel } from './catalog';
import {
  DEFAULT_TRANSPARENT_OPTIONS,
  formatMark,
  listenerSummary,
  normalizeOptions,
  parseDestinations,
  parseMark,
  parsePortList,
  planRequestOf,
  recentRows,
  validateMarkText,
  withTransparent,
} from './transparent';

const NOW = 1_800_000_000_000;

function status(p: Partial<TransparentStatus> = {}): TransparentStatus {
  return { enabled: true, supported: true, listening: true, port: 8001, requestedPort: 8001, addresses: ['127.0.0.1:8001', '[::1]:8001'], mark: 0, connections: 0, redirected: 0, direct: 0, loops: 0, recent: [], ...p };
}

describe('transparent settings', () => {
  test('patching keeps the rest of the settings', () => {
    const s = { proxy: { port: 8000, transparent: { enabled: false, port: 8001, mark: 0 } } } as unknown as Settings;
    const next = withTransparent(s, { enabled: true });
    expect(next.proxy.transparent).toEqual({ enabled: true, port: 8001, mark: 0 });
    expect(next.proxy.port).toBe(8000);
    expect(withTransparent(next, { mark: 42 }).proxy.transparent).toEqual({ enabled: true, port: 8001, mark: 42 });
  });

  test('firewall marks parse as decimal or hex and print as hex', () => {
    expect(parseMark('')).toBe(0);
    expect(parseMark('0')).toBe(0);
    expect(parseMark('42')).toBe(42);
    expect(parseMark('0x1A4B')).toBe(0x1a4b);
    expect(parseMark(' 0xffffffff ')).toBe(0xffffffff);
    expect(parseMark('0x100000000')).toBeNull();
    expect(parseMark('4294967296')).toBeNull();
    expect(parseMark('-1')).toBeNull();
    expect(parseMark('0xg')).toBeNull();
    expect(parseMark('1.5')).toBeNull();
    expect(formatMark(0)).toBe('0');
    expect(formatMark(0x1a4b)).toBe('0x1a4b');
    expect(validateMarkText('0x2a')).toBeNull();
    expect(validateMarkText('mark')).toContain('number');
  });
});

describe('panel options', () => {
  test('port lists and destinations are validated before asking the daemon', () => {
    expect(parsePortList('8080, 8443 8080')).toEqual({ ports: [8080, 8443], error: null });
    expect(parsePortList('').error).toBe('Enter at least one port');
    expect(parsePortList('80, 70000').error).toContain('70000');
    expect(parsePortList('http').error).toContain('http');
    expect(parseDestinations('10.0.0.0/8, 192.168.1.10 fd00::/8')).toEqual({ items: ['10.0.0.0/8', '192.168.1.10', 'fd00::/8'], error: null });
    expect(parseDestinations('').items).toEqual([]);
    expect(parseDestinations('example.com').error).toContain('example.com');
    expect(parseDestinations('10.0.0.0/33').error).toContain('10.0.0.0/33');
    expect(parseDestinations('::1/129').error).toContain('::1/129');
  });

  test('stored options are sanitised', () => {
    expect(normalizeOptions(null)).toEqual(DEFAULT_TRANSPARENT_OPTIONS);
    expect(normalizeOptions({ mode: 'machine', ports: 'bogus', sudo: 'yes', command: 'firefox' })).toEqual({ ...DEFAULT_TRANSPARENT_OPTIONS, mode: 'machine', command: 'firefox' });
  });

  test('options map to plan requests per mode', () => {
    expect(planRequestOf(DEFAULT_TRANSPARENT_OPTIONS)).toEqual({ request: { mode: 'app', ports: 'web', backend: 'nft', sudo: true, launcher: 'systemd', trustCa: true }, error: null });
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, command: '  firefox  ', ports: 'custom', customPorts: '3000', excludeDestinations: '10.0.0.0/8' }).request).toEqual({
      mode: 'app',
      ports: 'custom',
      customPorts: [3000],
      excludeDestinations: ['10.0.0.0/8'],
      backend: 'nft',
      sudo: true,
      launcher: 'systemd',
      trustCa: true,
      command: 'firefox',
    });
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, mode: 'user' })).toEqual({ request: null, error: 'Enter the user name or id to capture' });
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, mode: 'user', user: ' tester ' }).request).toEqual({ mode: 'user', ports: 'web', backend: 'nft', sudo: true, user: 'tester' });
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, mode: 'machine', exclude: 'mark', backend: 'iptables', sudo: false }).request).toEqual({ mode: 'machine', ports: 'web', backend: 'iptables', sudo: false, exclude: 'mark' });
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, ports: 'custom', customPorts: 'x' }).error).toContain('x');
    expect(planRequestOf({ ...DEFAULT_TRANSPARENT_OPTIONS, excludeDestinations: 'nope' }).error).toContain('nope');
  });
});

describe('listener status line', () => {
  test('covers off, failure, waiting, active and stale', () => {
    expect(listenerSummary(null).tone).toBe('off');
    expect(listenerSummary(status({ enabled: false, listening: false })).title).toBe('Transparent listener is off');
    const failed = listenerSummary(status({ listening: false, port: undefined, error: "couldn't listen on port 8001: address in use" }));
    expect(failed).toEqual({ tone: 'error', title: "Transparent listener isn't running", detail: "couldn't listen on port 8001: address in use" });
    expect(listenerSummary(status(), NOW)).toEqual({ tone: 'waiting', title: 'Waiting for redirected connections on port 8001', detail: 'Load the rules below, then run the check command.' });
    expect(listenerSummary(status({ port: 8002 }), NOW).title).toBe('Waiting for redirected connections on port 8002 (8001 was busy)');
    const active = listenerSummary(status({ redirected: 3, lastRedirectAt: NOW - 12_000 }), NOW);
    expect(active).toEqual({ tone: 'active', title: 'Redirection is working — last connection 12s ago', detail: '3 redirected connections on port 8001.' });
    const stale = listenerSummary(status({ redirected: 1, lastRedirectAt: NOW - 3_600_000 }), NOW);
    expect(stale.tone).toBe('waiting');
    expect(stale.title).toBe('No redirected connections lately (last 1h ago)');
    expect(stale.detail).toBe('1 redirected connection on port 8001.');
  });

  test('recent connections show the original destination and the process', () => {
    const rows = recentRows(
      status({
        recent: [
          { at: NOW - 3000, client: '192.168.1.5:5000', originalDestination: '93.184.216.34:443', process: 'curl' },
          { at: NOW - 60_000, client: '127.0.0.1:5001' },
        ],
      }),
      NOW,
    );
    expect(rows.map(({ destination, who, when, redirected }) => ({ destination, who, when, redirected }))).toEqual([
      { destination: '93.184.216.34:443', who: 'curl', when: 'just now', redirected: true },
      { destination: 'direct to the port', who: '127.0.0.1:5001', when: '1m ago', redirected: false },
    ]);
    expect(recentRows(null)).toEqual([]);
  });
});

describe('catalogue integration', () => {
  const info: InterceptorInfo = { id: 'transparent-linux', name: 'Transparent capture (Linux)', description: '', category: 'system', tags: [], icon: 'linux', supported: true, available: true, active: false, needsConfig: true };

  test('opens its own panel; stopping happens there (root commands)', () => {
    expect(panelKindFor(info)).toBe('transparent');
    expect(primaryActionFor(info)).toEqual({ kind: 'setup', label: 'Set up' });
    expect(primaryActionFor({ ...info, active: true })).toEqual({ kind: 'manage', label: 'Manage' });
    expect(statusOf({ ...info, active: true }).label).toBe('Active');
    expect(stopsFromPanel(info)).toBe(true);
    expect(stopsFromPanel({ id: 'chrome' })).toBe(false);
    expect(iconKey('linux')).toBe('linux');
  });
});

describe('mock backend', () => {
  const on = { enabled: true, port: 8001, mark: 0 };

  test('listener follows the settings; redirects are simulated after a plan request', () => {
    const m = new MockTransparent(false, NOW);
    expect(m.info({ enabled: false, port: 8001, mark: 0 }, NOW).listener).toMatchObject({ listening: false, addresses: [] });
    expect(m.info(on, NOW)).toMatchObject({ supported: true, active: false, listener: { listening: true, port: 8001, redirected: 0 } });
    expect(m.plan({}, on, NOW).status).toBeUndefined();
    expect(m.info(on, NOW + 1000).active).toBe(false);
    const later = m.info(on, NOW + 30_000);
    expect(later.active).toBe(true);
    expect(later.listener.redirected).toBeGreaterThan(0);
    expect(later.listener.recent[0]?.originalDestination).toBe('140.82.112.6:443');
    expect(m.interceptor(on, NOW + 30_000)).toMatchObject({ id: 'transparent-linux', active: true, needsConfig: true });
    expect(new MockTransparent(true, NOW).info(on, NOW).active).toBe(true);
  });

  test('plans come from captured fixtures, with the live port and options applied', () => {
    const m = new MockTransparent(false, NOW);
    const plan = m.plan({ mode: 'app', command: 'firefox' }, { ...on, port: 9001 }, NOW).json as (typeof TRANSPARENT_PLANS)[string];
    expect(plan.rules).toContain('redirect to :9001');
    expect(plan.launch?.command).toEndWith('-- env SSL_CERT_FILE=/home/dev/.local/share/akev/ca-bundle.pem REQUESTS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem CURL_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem GIT_SSL_CAINFO=/home/dev/.local/share/akev/ca-bundle.pem AWS_CA_BUNDLE=/home/dev/.local/share/akev/ca-bundle.pem DENO_CERT=/home/dev/.local/share/akev/ca-bundle.pem NODE_EXTRA_CA_CERTS=/home/dev/.local/share/akev/ca.pem firefox');
    const ipt = m.plan({ mode: 'machine', backend: 'iptables', sudo: false }, on, NOW).json as (typeof TRANSPARENT_PLANS)[string];
    expect(ipt.rules).toContain('-m owner --uid-owner 1000 -j RETURN');
    expect(ipt.setup.every((s) => !s.command.includes('sudo '))).toBe(true);
    expect(m.plan({ mode: 'user' }, on, NOW).status).toBe(400);
    expect(m.plan({ mode: 'machine', exclude: 'mark' }, on, NOW).status).toBe(400);
    expect(m.plan({ mode: 'user', user: 'tester' }, on, NOW).json).toMatchObject({ summary: 'Every program run by tester (uid 65534)' });
    const off = m.plan({}, { ...on, enabled: false }, NOW).json as (typeof TRANSPARENT_PLANS)[string];
    expect(off.warnings[0]).toContain('isn\'t running');
  });

  test('fixtures cover every mode and backend and never leak local paths', () => {
    for (const mode of ['app', 'user', 'machine']) for (const backend of ['nft', 'iptables']) expect(TRANSPARENT_PLANS[`${mode}-${backend}`]?.port).toBe(8001);
    const text = JSON.stringify(TRANSPARENT_PLANS);
    expect(text).not.toMatch(/\/tmp\/|scratchpad|\/home\/(?!dev\/)/);
  });
});
