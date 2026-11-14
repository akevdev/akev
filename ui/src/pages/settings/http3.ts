/** Settings → Proxy → HTTP/3: mode help and the QUIC listener status line. Pure (no React). */

import type { Http3ListenerStatus, Http3Settings, Http3UpstreamMode } from '../../api/types';

export const HTTP3_MODE_HELP: Record<Http3UpstreamMode, string> = {
  off: 'Akev talks HTTP/1.1 or HTTP/2 to servers. Compose requests can still ask for HTTP/3.',
  'alt-svc': 'Use HTTP/3 with servers that advertise it (Alt-Svc header), like browsers do. Falls back to TCP when QUIC fails.',
  prefer: "Try HTTP/3 first with every HTTPS server. Servers that don't answer over QUIC are remembered and reached over TCP.",
};

export interface ListenerLine {
  tone: 'on' | 'off' | 'error';
  text: string;
  /** The UDP address, when listening. */
  address?: string;
}

/**
 * What the "Accept HTTP/3" row says: where the QUIC listener is, why it isn't running, or that
 * it's off. `status` comes from `GET /api/status` (absent until it has loaded).
 */
export function listenerLine(settings: Http3Settings, status: Http3ListenerStatus | undefined, connected: boolean): ListenerLine {
  if (!settings.listen) return { tone: 'off', text: 'Off — clients reach Akev over TCP only.' };
  if (!connected || !status) return { tone: 'off', text: 'Not connected to Akev' };
  if (status.error) return { tone: 'error', text: status.error };
  if (status.address) return { tone: 'on', text: 'Listening on UDP', address: status.address };
  return { tone: 'off', text: 'Starting…' };
}

/** A warning for the QUIC port field (`0` = the proxy's port number). */
export function quicPortWarning(port: number): string | null {
  if (port > 0 && port < 1024) return 'Ports below 1024 usually need administrator rights.';
  return null;
}
