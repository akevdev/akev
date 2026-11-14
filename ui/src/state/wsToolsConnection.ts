/**
 * Stream glue for WebSocket tools: a toast (and an OS notification when the window is hidden)
 * when a WebSocket message pauses at a `ws-breakpoint` while the user looks elsewhere.
 */

import type { PausedMessage } from '../api/types';
import { getRoute, navigate } from '../app/router';
import { toast } from '../components/ui/toast';
import { notifyNative, windowInBackground } from '../lib/platform';
import { trafficStore } from './traffic';

export { wsToolsReducers } from './wsTools';

/** `client → chat.example.com/socket` style description of where a paused message goes. */
export function describePausedMessage(p: PausedMessage, summary?: { host: string; path: string }): string {
  const where = summary ? `${summary.host}${summary.path.split('?')[0] ?? ''}` : `WebSocket #${p.id}`;
  return p.direction === 'sent' ? `Client → ${where}` : `${where} → client`;
}

export function announcePausedMessage(p: PausedMessage): void {
  const route = getRoute();
  const viewing = route.page === 'traffic' && route.params.id === String(p.id);
  const what = describePausedMessage(p, trafficStore.getSnapshot().summaries.get(p.id));
  if (!viewing) {
    toast('WebSocket message paused', {
      description: what,
      duration: 8000,
      action: { label: 'Jump to', onClick: () => navigate(`/traffic/${p.id}`) },
    });
  }
  if (windowInBackground()) notifyNative('WebSocket message paused', what);
}
