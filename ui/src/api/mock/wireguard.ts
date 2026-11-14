/**
 * WireGuard mode for the mock backend (`/api/wireguard*`): mirrors the daemon's payloads.
 * Once enabled, a phone "connects" after a few seconds so the panel's live status can be seen.
 */
import type { WireguardInfo, WireguardPatch, WireguardSettings } from '../types';

const SERVER_PUBLIC = 'kL3vQ0mE2Wq7yH8d1nBzR5tUcXo9fJ4gPaS6eVhNiM0=';
const CLIENT_PRIVATE = 'YJ0bW9jay1jbGllbnQta2V5LWZvci1ha2V2by11aSE=';
const CLIENT_PUBLIC = 'c1Fq8Lr3N0bV7xKeT2mWzP5hY9uGdA4sJoE6iRtCwB8=';
const LAN = ['192.168.1.23', '100.84.10.66'];
/** How long after enabling the demo phone completes its handshake. */
const CONNECT_AFTER_MS = 3500;

export function mockWireguardConfig(host: string, port: number, serverPublic = SERVER_PUBLIC, clientPrivate = CLIENT_PRIVATE): string {
  const endpoint = host.includes(':') ? `[${host}]:${port}` : `${host}:${port}`;
  return `[Interface]\nPrivateKey = ${clientPrivate}\nAddress = 10.0.0.2/32\nDNS = 10.0.0.53\n\n[Peer]\nPublicKey = ${serverPublic}\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = ${endpoint}\n`;
}

export class MockWireguard {
  private enabledAt: number | null = null;
  private serverPublic = SERVER_PUBLIC;
  private clientPrivate = CLIENT_PRIVATE;
  private keysCreatedAt = Date.now() - 6 * 86_400_000;

  constructor(private readonly settings: () => WireguardSettings) {
    if (settings().enabled) this.enabledAt = Date.now() - CONNECT_AFTER_MS;
  }

  /** Apply a settings change (start/stop the "server"); `connected` = the phone is already on. */
  applied(next: WireguardSettings, connected = false): void {
    if (next.enabled && this.enabledAt == null) this.enabledAt = Date.now() - (connected ? CONNECT_AFTER_MS + 60_000 : 0);
    if (!next.enabled) this.enabledAt = null;
  }

  patch(current: WireguardSettings, patch: WireguardPatch): WireguardSettings {
    if (patch.port !== undefined && (patch.port < 1 || patch.port > 65535)) throw new Error('port must be between 1 and 65535');
    const next: WireguardSettings = { ...current };
    if (patch.enabled !== undefined) next.enabled = patch.enabled;
    if (patch.port !== undefined) next.port = patch.port;
    if (patch.bindAddress !== undefined) next.bindAddress = patch.bindAddress?.trim() || undefined;
    return next;
  }

  regenerate(): void {
    const rot = (s: string) => s.slice(7, 43) + s.slice(0, 7) + '=';
    this.serverPublic = rot(this.serverPublic.replace(/=$/, ''));
    this.clientPrivate = rot(this.clientPrivate.replace(/=$/, ''));
    this.keysCreatedAt = Date.now();
    if (this.enabledAt != null) this.enabledAt = Date.now();
  }

  info(host?: string | null): WireguardInfo {
    const s = this.settings();
    const bind = s.bindAddress?.trim();
    const endpoints = bind && bind !== '0.0.0.0' && bind !== '::' ? [bind] : LAN;
    const h = host?.trim() || endpoints[0] || '127.0.0.1';
    const config = mockWireguardConfig(h, s.port, this.serverPublic, this.clientPrivate);
    const running = s.enabled && this.enabledAt != null;
    const now = Date.now();
    const connected = running && now - (this.enabledAt ?? now) >= CONNECT_AFTER_MS;
    const since = connected ? now - (this.enabledAt ?? now) - CONNECT_AFTER_MS : 0;
    const peer = {
      publicKey: CLIENT_PUBLIC,
      ...(connected ? { endpoint: '192.168.1.57:52117', lastHandshakeAt: (this.enabledAt ?? now) + CONNECT_AFTER_MS, lastSeenAt: now - 400 } : {}),
      rxBytes: connected ? Math.round(18_000 + since * 41) : 0,
      txBytes: connected ? Math.round(64_000 + since * 230) : 0,
      connected,
    };
    return {
      enabled: s.enabled,
      running,
      port: s.port,
      ...(bind ? { bindAddress: bind } : {}),
      ...(running ? { listenAddr: `${bind || '0.0.0.0'}:${s.port}` } : {}),
      endpoints,
      endpoint: h.includes(':') ? `[${h}]:${s.port}` : `${h}:${s.port}`,
      config,
      qrPayload: config,
      configFileName: 'akev.conf',
      serverPublicKey: this.serverPublic,
      clientAddress: '10.0.0.2/32',
      dnsAddress: '10.0.0.53',
      keysCreatedAt: this.keysCreatedAt,
      peer,
      ...(running
        ? {
            stats: {
              listenAddr: `${bind || '0.0.0.0'}:${s.port}`,
              peer,
              tcpActive: connected ? 7 : 0,
              tcpTotal: connected ? 12 + Math.floor(since / 1500) : 0,
              tcpRejected: 0,
              dnsQueries: connected ? 9 + Math.floor(since / 2500) : 0,
              udpFlows: connected ? 1 : 0,
              udpRelayed: connected ? 4 : 0,
              quicDropped: connected ? 3 : 0,
              droppedPackets: 0,
            },
          }
        : {}),
    };
  }
}
