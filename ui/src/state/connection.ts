/**
 * Wires the live stream to the stores. Called once from `main.tsx`.
 *
 *   StreamClient ──hello────────▶ trafficStore.reset, appStore (status/rules/settings/paused), ui prefs
 *                ──hello-more───▶ trafficStore.prependOlder (older summaries of a paged hello)
 *                ──exchanges────▶ trafficStore.applyExchanges (rAF-batched notify)
 *                ──filter-result▶ trafficStore.applyFilterResult
 *                ──paused───────▶ appStore + "Request paused" toast (+ OS notification when not focused)
 *                ──license──────▶ licence store (+ a toast when the trial ends while open)
 *                ──…            ▶ see handlers below
 */

import { api, isApiError } from '../api/client';
import { StreamClient } from '../api/stream';
import type { PausedExchange } from '../api/types';
import { getRoute, navigate } from '../app/router';
import { announceRestoredSession } from '../app/sessions/actions';
import { toast } from '../components/ui/toast';
import { ACCOUNT_URL, licenseTransition, PRICING_URL } from '../lib/license';
import { notifyNative, openExternal, windowInBackground } from '../lib/platform';
import { appReducers, useAppStore } from './app';
import { getLicense, installLicenseHandlers, licenseReducers } from './license';
import { trafficStore } from './traffic';
import { announceNewDevice } from './newDevices';
import { enableRemotePrefsSync, useUiStore } from './ui';
import { announcePausedMessage, wsToolsReducers } from './wsToolsConnection';

let stream: StreamClient | null = null;

/** Skip the backoff and reconnect now (Retry buttons). */
export function reconnectNow(): void {
  if (useAppStore.getState().connection === 'unauthorized') {
    // A fresh token may have been provided via a new link; restart from scratch.
    stream?.close();
    stream = null;
    useAppStore.setState({ connection: 'connecting', failedAttempts: 0 });
    startConnection();
    return;
  }
  stream?.reconnectNow();
}

/** Fetch state that `hello` doesn't carry (interceptors, rule stats, recording). */
async function refreshExtras(): Promise<void> {
  const [interceptors, stats, capture] = await Promise.allSettled([api.interceptors.list(), api.rules.stats(), api.capture.get()]);
  if (interceptors.status === 'fulfilled') useAppStore.setState({ interceptors: interceptors.value });
  if (stats.status === 'fulfilled') appReducers.replaceRuleStats(stats.value);
  if (capture.status === 'fulfilled') useAppStore.setState({ recording: capture.value.recording });
}

function describePaused(p: PausedExchange): string {
  let path = p.request.url;
  try {
    const u = new URL(p.request.url);
    path = `${u.host}${u.pathname}`;
  } catch {
    // keep raw URL
  }
  return `${p.request.method} ${path}`;
}

function announcePaused(p: PausedExchange): void {
  const route = getRoute();
  const viewing = route.page === 'traffic' && route.params.id === String(p.id);
  const what = describePaused(p);
  if (!viewing) {
    toast(p.stage === 'request' ? 'Request paused' : 'Response paused', {
      description: what,
      duration: 8000,
      action: { label: 'Jump to', onClick: () => navigate(`/traffic/${p.id}`) },
    });
  }
  // Not only when hidden: the window may be open behind the app the user is working in.
  if (windowInBackground()) notifyNative(p.stage === 'request' ? 'Request paused' : 'Response paused', what);
}

/** Create the stream client, subscribe the stores and connect. Idempotent. */
export function startConnection(): StreamClient {
  if (stream) return stream;
  const s = new StreamClient({ openSocket: () => api.openStreamSocket() });
  stream = s;
  installLicenseHandlers(api);

  trafficStore.setFilterSink((query) => s.setFilter(query));

  s.on('hello', (m) => {
    trafficStore.reset(m.summaries, m.remaining);
    appReducers.hello(m.status, m.paused, m.rules, m.settings);
    licenseReducers.set(m.status.license);
    wsToolsReducers.hello(m.pausedMessages);
    enableRemotePrefsSync(true);
    useUiStore.getState().applyRemote(m.settings.ui);
    void refreshExtras();
    announceRestoredSession(m.status.restoredSession, m.status.startedAt);
  });
  s.on('hello-more', (m) => trafficStore.prependOlder(m.summaries, m.remaining));
  s.on('exchanges', (m) => trafficStore.applyExchanges(m.added, m.updated, m.filtered));
  s.on('removed', (m) => trafficStore.remove(m.ids));
  s.on('cleared', () => trafficStore.clear());
  s.on('messages', (m) => trafficStore.notifyMessages(m.id, m.messages));
  s.on('filter-result', (m) => trafficStore.applyFilterResult(m.seq, m.ids, m.error));
  s.on('paused', (m) => {
    appReducers.paused(m.exchange);
    announcePaused(m.exchange);
  });
  s.on('resumed', (m) => appReducers.resumed(m.id));
  s.on('message-paused', (m) => {
    wsToolsReducers.paused(m.message);
    announcePausedMessage(m.message);
  });
  s.on('message-resumed', (m) => wsToolsReducers.resumed(m.pauseId));
  s.on('removed', (m) => wsToolsReducers.forget(m.ids));
  s.on('cleared', () => wsToolsReducers.forget('all'));
  s.on('rules', (m) => useAppStore.setState({ rules: m.rules }));
  s.on('rule-stats', (m) => appReducers.ruleStats(m.stats));
  s.on('settings', (m) => {
    useAppStore.setState({ settings: m.settings });
    useUiStore.getState().applyRemote(m.settings.ui);
  });
  s.on('interceptors', (m) => useAppStore.setState({ interceptors: m.interceptors }));
  s.on('stats', (m) => appReducers.stats(m.stats));
  s.on('capture', (m) => useAppStore.setState({ recording: m.recording }));
  s.on('session-restored', (m) => {
    const status = useAppStore.getState().status;
    if (status) useAppStore.setState({ status: { ...status, restoredSession: m.session } });
    announceRestoredSession(m.session, status?.startedAt ?? 0);
  });
  s.on('license', (m) => {
    const change = licenseTransition(getLicense(), m.license);
    licenseReducers.set(m.license);
    if (change) {
      // A licence whose updates ended is renewed from the account page; anything else is a purchase.
      const renew = m.license.state === 'updates-ended';
      toast(change.title, {
        description: change.description,
        duration: 12_000,
        action: renew ? { label: 'Renew updates', onClick: () => openExternal(ACCOUNT_URL) } : { label: 'Buy Pro', onClick: () => openExternal(PRICING_URL) },
      });
    }
  });
  s.on('client-connected', (m) => announceNewDevice(m.address));
  s.on('notice', (m) => {
    if (m.level === 'error') toast.error(m.message, { duration: 10_000 });
    else if (m.level === 'warning') toast.warning(m.message, { duration: 8000 });
    else toast.info(m.message);
  });

  s.onState((state) => {
    if (useAppStore.getState().connection === 'unauthorized' && state !== 'open') return;
    useAppStore.setState({ connection: state, retryAt: s.retryAt });
    if (state !== 'open') enableRemotePrefsSync(false);
  });

  s.onFailure((f) => {
    useAppStore.setState({ failedAttempts: f.attempt, retryAt: Date.now() + f.retryIn });
    // A WebSocket can't report HTTP 401; probe REST to tell "daemon down" from "bad token".
    if (f.attempt === 2 || f.attempt % 6 === 0) {
      api.status().then(
        () => undefined,
        (e: unknown) => {
          if (isApiError(e) && e.isUnauthorized) {
            useAppStore.setState({ connection: 'unauthorized', retryAt: null });
            s.close();
          }
        },
      );
    }
  });

  s.connect();
  return s;
}
