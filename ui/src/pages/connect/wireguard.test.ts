import { describe, expect, test } from 'bun:test';
import { MockServer } from '../../api/mock/server';
import { mockWireguardConfig } from '../../api/mock/wireguard';
import type { InterceptorInfo, WireguardInfo, WireguardPeer } from '../../api/types';
import { appIconFor } from '../../components/ui/AppIcon';
import { PROXY_MODE_LABELS } from '../../components/exchange/model/cert';
import { filterCatalog, panelKindFor, primaryActionFor } from './catalog';
import { endpointChoices, hostOf, peerState, richSegments, wireguardSteps, WIREGUARD_DEVICES } from './wireguard';

const NOW = 1_700_000_000_000;

function peer(p: Partial<WireguardPeer> = {}): WireguardPeer {
  return { publicKey: 'k', rxBytes: 0, txBytes: 0, connected: false, ...p };
}

function info(p: Partial<WireguardInfo> = {}): WireguardInfo {
  return {
    enabled: true,
    running: true,
    port: 51820,
    endpoints: ['192.168.1.23'],
    endpoint: '192.168.1.23:51820',
    config: '',
    qrPayload: '',
    configFileName: 'akev.conf',
    serverPublicKey: 's',
    clientAddress: '10.0.0.2/32',
    dnsAddress: '10.0.0.53',
    keysCreatedAt: 0,
    peer: peer(),
    ...p,
  };
}

describe('WireGuard panel logic', () => {
  test('peer state wording', () => {
    expect(peerState(info({ enabled: false, running: false }), NOW).tone).toBe('off');
    const failed = peerState(info({ running: false, error: "couldn't start the WireGuard server on UDP 0.0.0.0:51820: Address in use" }), NOW);
    expect(failed.tone).toBe('error');
    expect(failed.detail).toContain('Address in use');
    expect(peerState(info(), NOW)).toEqual({ tone: 'waiting', title: 'Waiting for a device…', detail: expect.stringContaining('Scan the QR code') });
    const connected = peerState(info({ peer: peer({ connected: true, endpoint: '192.168.1.57:5000', lastHandshakeAt: NOW - 30_000, rxBytes: 2048, txBytes: 1_048_576 }) }), NOW);
    expect(connected.tone).toBe('connected');
    expect(connected.detail).toContain('from 192.168.1.57:5000');
    expect(connected.detail).toContain('handshake 30s ago');
    expect(connected.detail).toContain('sent');
    const idle = peerState(info({ peer: peer({ lastSeenAt: NOW - 600_000 }) }), NOW);
    expect(idle.tone).toBe('idle');
    expect(idle.detail).toContain('10m ago');
  });

  test('setup steps for every device kind', () => {
    for (const d of WIREGUARD_DEVICES) {
      const steps = wireguardSteps(d.value);
      expect(steps.length).toBeGreaterThanOrEqual(4);
      expect(steps.join(' ')).toContain('akev.cert');
    }
    expect(wireguardSteps('ios').join(' ')).toContain('Certificate Trust Settings');
    expect(wireguardSteps('android').join(' ')).toContain('Scan from QR code');
    expect(wireguardSteps('desktop').join(' ')).toContain('wg-quick');
  });

  test('endpoint helpers', () => {
    expect(hostOf('192.168.1.23:51820')).toBe('192.168.1.23');
    expect(hostOf('[fd00::1]:51820')).toBe('fd00::1');
    expect(hostOf('pc.lan:1')).toBe('pc.lan');
    expect(endpointChoices(info({ endpoints: ['10.1.1.1', '192.168.1.23'], endpoint: '192.168.1.23:51820' }))).toEqual(['10.1.1.1', '192.168.1.23']);
    expect(endpointChoices(info({ endpoints: ['10.1.1.1'], endpoint: 'custom.host:51820' }))).toEqual(['10.1.1.1', 'custom.host']);
  });

  test('rich step text', () => {
    expect(richSegments('Tap **+** then run `wg-quick up` now')).toEqual([
      { text: 'Tap ', kind: 'plain' },
      { text: '+', kind: 'bold' },
      { text: ' then run ', kind: 'plain' },
      { text: 'wg-quick up', kind: 'code' },
      { text: ' now', kind: 'plain' },
    ]);
    expect(richSegments('plain')).toEqual([{ text: 'plain', kind: 'plain' }]);
  });

  test('catalogue: own panel, found by phone searches, icon and proxy mode label', () => {
    const wg: InterceptorInfo = {
      id: 'wireguard',
      name: 'WireGuard (phones & any device)',
      description: 'Scan a QR code with the WireGuard app',
      category: 'mobile',
      tags: ['wireguard', 'vpn'],
      icon: 'wireguard',
      supported: true,
      available: true,
      active: false,
      needsConfig: true,
    };
    expect(panelKindFor(wg)).toBe('wireguard');
    expect(primaryActionFor(wg)).toEqual({ kind: 'setup', label: 'Set up' });
    expect(primaryActionFor({ ...wg, active: true })).toEqual({ kind: 'manage', label: 'Manage' });
    for (const q of ['iphone', 'vpn', 'wireguard', 'qr code']) expect(filterCatalog([wg], 'all', q).map((i) => i.id)).toEqual(['wireguard']);
    expect(appIconFor('wireguard')).toBe('wireguard');
    expect(PROXY_MODE_LABELS.wireguard).toBe('WireGuard tunnel');
  });
});

describe('mock WireGuard backend', () => {
  const call = async (server: MockServer, method: string, path: string, body?: unknown) => {
    const res = await server.fetch(path, { method, ...(body !== undefined ? { body: JSON.stringify(body), headers: { 'content-type': 'application/json' } } : {}) });
    return { status: res.status, json: res.status === 204 ? null : await res.json() };
  };

  test('enable, configuration, interceptor state, regenerate', async () => {
    const server = new MockServer({ license: 'pro' });
    server.start();
    try {
      let r = await call(server, 'GET', '/api/wireguard');
      expect(r.json.enabled).toBe(false);
      expect(r.json.config).toBe(r.json.qrPayload);
      expect(r.json.config).toContain('AllowedIPs = 0.0.0.0/0, ::/0');
      r = await call(server, 'PUT', '/api/wireguard', { enabled: true, port: 51999 });
      expect(r.json.running).toBe(true);
      expect(r.json.endpoint).toBe('192.168.1.23:51999');
      expect(r.json.stats.listenAddr).toBe('0.0.0.0:51999');
      const settings = (await call(server, 'GET', '/api/settings')).json;
      expect(settings.proxy.wireguard).toEqual({ enabled: true, port: 51999 });
      const interceptors = (await call(server, 'GET', '/api/interceptors')).json as InterceptorInfo[];
      expect(interceptors.find((i) => i.id === 'wireguard')?.active).toBe(true);
      r = await call(server, 'GET', '/api/wireguard?host=100.84.10.66');
      expect(r.json.config).toContain('Endpoint = 100.84.10.66:51999');
      const before = r.json.serverPublicKey;
      r = await call(server, 'POST', '/api/wireguard/regenerate');
      expect(r.json.serverPublicKey).not.toBe(before);
      expect((await call(server, 'PUT', '/api/wireguard', { port: 0 })).status).toBe(400);
      await call(server, 'POST', '/api/interceptors/wireguard/deactivate', {});
      expect((await call(server, 'GET', '/api/wireguard')).json.running).toBe(false);
    } finally {
      server.stop();
    }
  });

  test('configuration text matches the daemon format', () => {
    expect(mockWireguardConfig('fd00::1', 51820, 'S', 'C')).toBe('[Interface]\nPrivateKey = C\nAddress = 10.0.0.2/32\nDNS = 10.0.0.53\n\n[Peer]\nPublicKey = S\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = [fd00::1]:51820\n');
  });
});
