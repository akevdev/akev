/**
 * WireGuard mode panel logic (Connect page): connection state wording, per-device setup
 * steps and endpoint choices. Pure — rendered by `panels/WireguardPanel.tsx`.
 */

import type { WireguardInfo } from '../../api/types';
import { formatBytes, formatRelativeTime } from '../../lib/format';

export type WireguardDevice = 'ios' | 'android' | 'desktop';

export const WIREGUARD_DEVICES: readonly { value: WireguardDevice; label: string }[] = [
  { value: 'ios', label: 'iPhone / iPad' },
  { value: 'android', label: 'Android' },
  { value: 'desktop', label: 'macOS / Windows / Linux' },
];

/** The magic host serving the CA download page through the tunnel. */
export const SETUP_HOST = 'akev.cert';

/** Setup steps per device kind (plain strings; `**x**` marks UI labels to emphasise). */
export function wireguardSteps(device: WireguardDevice): string[] {
  switch (device) {
    case 'ios':
      return [
        'Install **WireGuard** from the App Store.',
        'In WireGuard tap **+** → **Create from QR code** and scan the code above. Name it “Akev” and allow adding the VPN configuration.',
        'Switch the Akev tunnel on. The status at the top of this panel turns green when the phone connects.',
        `For HTTPS, open **http://${SETUP_HOST}** in Safari and download the iOS profile, then Settings → **Profile Downloaded** → Install.`,
        'Settings → General → About → **Certificate Trust Settings** → enable full trust for the Akev CA.',
      ];
    case 'android':
      return [
        'Install **WireGuard** from Google Play (or F-Droid).',
        'In WireGuard tap **+** → **Scan from QR code** and scan the code above.',
        'Switch the tunnel on and accept the VPN connection request. The status at the top of this panel turns green when the phone connects.',
        `For HTTPS, open **http://${SETUP_HOST}** in Chrome and download the certificate, then Settings → Security → Encryption & credentials → **Install a certificate** → CA certificate.`,
        'Apps targeting Android 7+ only trust user-installed CAs when their network security config allows it (debug builds usually do); browsers always do.',
      ];
    case 'desktop':
      return [
        'Install the WireGuard app (wireguard.com/install) — or wireguard-tools on Linux.',
        'Download the configuration and import it: **Import tunnel(s) from file** (macOS / Windows), or `sudo wg-quick up ./akev.conf` (Linux).',
        "Activate the tunnel. All of that machine's traffic now goes through Akev.",
        `For HTTPS, open **http://${SETUP_HOST}** there and trust the Akev CA system-wide (Connect → System proxy shows the per-OS commands).`,
      ];
  }
}

export type PeerTone = 'off' | 'error' | 'waiting' | 'connected' | 'idle';

export interface PeerState {
  tone: PeerTone;
  title: string;
  detail?: string;
}

/** What the live status row says. */
export function peerState(info: Pick<WireguardInfo, 'enabled' | 'running' | 'error' | 'peer'>, now: number = Date.now()): PeerState {
  if (!info.enabled) return { tone: 'off', title: 'WireGuard mode is off' };
  if (!info.running) return { tone: 'error', title: "The WireGuard server isn't running", detail: info.error };
  const p = info.peer;
  const traffic = p.rxBytes + p.txBytes > 0 ? `${formatBytes(p.txBytes)} sent · ${formatBytes(p.rxBytes)} received` : undefined;
  if (p.connected) {
    const parts = [p.endpoint ? `from ${p.endpoint}` : null, p.lastHandshakeAt ? `handshake ${formatRelativeTime(p.lastHandshakeAt, now)}` : null, traffic ?? null].filter(Boolean);
    return { tone: 'connected', title: 'Device connected', detail: parts.join(' · ') || undefined };
  }
  if (p.lastSeenAt) {
    return { tone: 'idle', title: 'Device idle', detail: `Last seen ${formatRelativeTime(p.lastSeenAt, now)}. It reconnects automatically when it sends traffic.` };
  }
  return { tone: 'waiting', title: 'Waiting for a device…', detail: 'Scan the QR code with the WireGuard app and switch the tunnel on.' };
}

/** Endpoint host choices for the configuration (LAN addresses; the current one kept). */
export function endpointChoices(info: Pick<WireguardInfo, 'endpoints' | 'endpoint'>): string[] {
  const current = hostOf(info.endpoint);
  const out = [...info.endpoints];
  if (current && !out.includes(current)) out.push(current);
  return out;
}

/** Host part of `host:port` / `[v6]:port`. */
export function hostOf(endpoint: string): string {
  const v6 = /^\[([^\]]+)\]:\d+$/.exec(endpoint);
  if (v6) return v6[1] ?? '';
  const i = endpoint.lastIndexOf(':');
  return i > 0 ? endpoint.slice(0, i) : endpoint;
}

/** Split `**bold**` and `` `code` `` markers for rendering. */
export function richSegments(text: string): { text: string; kind: 'plain' | 'bold' | 'code' }[] {
  const out: { text: string; kind: 'plain' | 'bold' | 'code' }[] = [];
  const re = /\*\*([^*]+)\*\*|`([^`]+)`/g;
  let last = 0;
  for (let m = re.exec(text); m; m = re.exec(text)) {
    if (m.index > last) out.push({ text: text.slice(last, m.index), kind: 'plain' });
    if (m[1] != null) out.push({ text: m[1], kind: 'bold' });
    else out.push({ text: m[2] ?? '', kind: 'code' });
    last = m.index + m[0].length;
  }
  if (last < text.length) out.push({ text: text.slice(last), kind: 'plain' });
  return out;
}
