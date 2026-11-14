/**
 * DEV-ONLY: stress the traffic list with synthetic rows (never shipped — imported behind
 * `import.meta.env.DEV`). In the browser console:
 *
 *   __akevTraffic.bulk(100_000)   // prepend 100k synthetic historical exchanges
 *   __akevTraffic.reset()          // drop them again
 *   __akevTraffic.store            // the traffic store (inspect state from the console)
 *   __akevTraffic.asyncMock()      // deliver mock replies to client messages asynchronously
 *
 * `asyncMock()` works around a mock quirk: `MockSocket.send` answers `set-filter`
 * synchronously, i.e. inside `TrafficStore.sendFilter` before it records the new `seq`, so the
 * store drops the `filter-result` and the filter stays pending. A real WebSocket is always
 * asynchronous. (Reported upstream; this helper only exists for local testing.)
 *
 * Rows get negative ids so they sort before (and never collide with) live mock traffic.
 * They exist only in the UI store: opening one in the detail pane yields "not found".
 */

import type { ContentKind, ExchangeSummary } from '../../../api/types';
import { trafficStore } from '../../../state/traffic';

const HOSTS = ['api.github.com', 'cdn.jsdelivr.net', 'www.google.com', 'localhost', 'fonts.gstatic.com', 'telemetry.example.io', 'images.unsplash.com', 'auth.example.com'];
const PATHS = ['/repos/akev/akev/pulls', '/npm/react@19/umd/react.production.min.js', '/search?q=akev+proxy', '/api/v1/users/42', '/s/inter/v13/font.woff2', '/v1/batch', '/photo-1500?w=800&q=80', '/oauth/token'];
const KINDS: ContentKind[] = ['json', 'javascript', 'html', 'json', 'font', 'protobuf', 'image', 'json'];
const METHODS = ['GET', 'GET', 'GET', 'POST', 'GET', 'POST', 'GET', 'POST', 'PUT', 'DELETE'];
const STATUSES = [200, 200, 200, 201, 204, 304, 404, 500, 200, 302];
const SOURCES = ['Chrome', 'curl', 'node', 'python3', 'Firefox'];

function synth(n: number): ExchangeSummary[] {
  const out: ExchangeSummary[] = new Array(n);
  const start = Date.now() - n * 40;
  for (let i = 0; i < n; i++) {
    const h = (i * 2654435761) >>> 0;
    const k = h % HOSTS.length;
    out[i] = {
      id: -n + i,
      kind: 'http',
      state: 'complete',
      startedAt: start + i * 40,
      method: METHODS[h % METHODS.length] as string,
      scheme: k === 3 ? 'http' : 'https',
      host: HOSTS[k] as string,
      port: k === 3 ? 3000 : 443,
      path: PATHS[(h >>> 3) % PATHS.length] as string,
      httpVersion: k % 2 ? 'HTTP/1.1' : 'HTTP/2',
      status: STATUSES[(h >>> 5) % STATUSES.length],
      contentKind: KINDS[k] as ContentKind,
      requestSize: (h >>> 7) % 2048,
      responseSize: (h >>> 9) % 400_000,
      duration: ((h >>> 11) % 3000) + ((h >>> 13) % 10 === 0 ? 4000 : 0),
      source: SOURCES[(h >>> 15) % SOURCES.length],
      color: (h >>> 17) % 97 === 0 ? 'blue' : undefined,
      pinned: (h >>> 19) % 211 === 0 ? true : undefined,
    };
  }
  return out;
}

function current(): ExchangeSummary[] {
  const snap = trafficStore.getSnapshot();
  const out: ExchangeSummary[] = [];
  for (const id of snap.order) {
    const s = snap.summaries.get(id);
    if (s) out.push(s);
  }
  return out;
}

declare global {
  interface Window {
    __akevTraffic?: { bulk(n?: number): number; reset(): void; store: typeof trafficStore; asyncMock(): boolean };
  }
}

window.__akevTraffic = {
  store: trafficStore,
  bulk(n = 100_000) {
    const t0 = performance.now();
    const live = current().filter((s) => s.id >= 0);
    trafficStore.reset([...synth(n), ...live]);
    const ms = Math.round(performance.now() - t0);
    console.info(`[akev] loaded ${n.toLocaleString()} synthetic exchanges in ${ms} ms`);
    return ms;
  },
  reset() {
    trafficStore.reset(current().filter((s) => s.id >= 0));
  },
  asyncMock() {
    const server = (window as { __akevMock?: { server?: unknown } }).__akevMock?.server as
      | { handleClientMessage(socket: unknown, data: string): void; __asyncPatched?: boolean }
      | undefined;
    if (!server) return false;
    if (server.__asyncPatched) return true;
    const original = server.handleClientMessage.bind(server);
    server.handleClientMessage = (socket, data) => void setTimeout(() => original(socket, data), 0);
    server.__asyncPatched = true;
    // Re-send the current filter so a pending one resolves.
    const f = trafficStore.getSnapshot().filter;
    if (f.pending) {
      const { text, chip } = f;
      trafficStore.clearFilter();
      trafficStore.setFilterText(text);
      trafficStore.setTypeChip(chip);
      trafficStore.flushFilter();
    }
    return true;
  },
};

export {};
