import { useCallback, useEffect, useRef, useState } from 'react';
import { api, errorMessage, isApiError } from '../../../../api/client';
import type { ExchangeDetail, ExchangeId, ExchangeSummary } from '../../../../api/types';
import { LruCache } from '../../../../lib/lru';
import { trafficStore, useExchangeSummary } from '../../../../state/traffic';

interface CacheEntry {
  /** Summary object the detail was fetched for (identity = freshness). */
  summary: ExchangeSummary | undefined;
  detail: ExchangeDetail;
}

/** Recently viewed details (DESIGN.md §9: LRU of 50) so flipping back is instant. */
const detailCache = new LruCache<ExchangeId, CacheEntry>(50);

/** Live updates refetch at most every 250 ms, and at least once per second while busy. */
const DEBOUNCE_MS = 250;
const MAX_WAIT_MS = 1000;

export interface ExchangeDetailState {
  /** Detail being shown — may belong to the *previous* selection while the next one loads. */
  detail: ExchangeDetail | null;
  /** Id `detail` belongs to. */
  detailId: ExchangeId | null;
  /** A fetch is in flight. */
  loading: boolean;
  /** `detail` belongs to a different exchange than the one requested. */
  stale: boolean;
  /** Last error for the requested id (cleared on success). */
  error: string | null;
  /** The requested exchange no longer exists on the daemon. */
  notFound: boolean;
  reload(): void;
}

interface State {
  detail: ExchangeDetail | null;
  detailId: ExchangeId | null;
  loading: boolean;
  error: string | null;
  errorId: ExchangeId | null;
  notFound: boolean;
}

/**
 * Fetch `GET /api/exchanges/:id` for the selected exchange.
 *
 * - Switching exchanges fetches immediately but keeps the previous detail on screen until the
 *   new one arrives (no skeleton flash for quick switches); cached details show instantly.
 * - Live updates (the summary object for this id changes) refetch, debounced.
 * - Responses for superseded requests are dropped (AbortController + sequence number).
 */
export function useExchangeDetail(id: ExchangeId | null): ExchangeDetailState {
  const summary = useExchangeSummary(id);
  const [state, setState] = useState<State>(() => {
    const cached = id == null ? undefined : detailCache.peek(id);
    return { detail: cached?.detail ?? null, detailId: cached ? id : null, loading: false, error: null, errorId: null, notFound: false };
  });

  const seq = useRef(0);
  const controller = useRef<AbortController | null>(null);
  const loadedFor = useRef<ExchangeSummary | undefined>(undefined);
  const prevId = useRef<ExchangeId | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const firstPendingAt = useRef(0);

  const clearTimer = () => {
    if (timer.current != null) clearTimeout(timer.current);
    timer.current = null;
  };

  const load = useCallback((target: ExchangeId) => {
    controller.current?.abort();
    const ctrl = new AbortController();
    controller.current = ctrl;
    const mySeq = ++seq.current;
    const forSummary = trafficStore.get(target);
    loadedFor.current = forSummary;
    setState((s) => (s.loading ? s : { ...s, loading: true }));
    api.exchanges.get(target, ctrl.signal).then(
      (detail) => {
        if (mySeq !== seq.current) return;
        detailCache.set(target, { summary: forSummary, detail });
        setState({ detail, detailId: target, loading: false, error: null, errorId: null, notFound: false });
      },
      (e: unknown) => {
        if (mySeq !== seq.current || (isApiError(e) && e.isAborted)) return;
        const notFound = isApiError(e) && e.isNotFound;
        if (notFound) detailCache.delete(target);
        setState((s) => ({ ...s, loading: false, error: errorMessage(e), errorId: target, notFound }));
      },
    );
  }, []);

  useEffect(() => {
    if (id == null) {
      prevId.current = null;
      clearTimer();
      return;
    }
    const switched = prevId.current !== id;
    prevId.current = id;
    if (switched) {
      clearTimer();
      const cached = detailCache.get(id);
      if (cached) {
        setState({ detail: cached.detail, detailId: id, loading: false, error: null, errorId: null, notFound: false });
        if (cached.summary === summary) {
          // Fresh: nothing changed since we fetched it.
          controller.current?.abort();
          seq.current++;
          loadedFor.current = summary;
          return;
        }
      }
      load(id);
      return;
    }
    if (!summary || summary === loadedFor.current) return;
    // Live update of the shown exchange: debounce, but never starve.
    const now = Date.now();
    if (timer.current == null) firstPendingAt.current = now;
    clearTimer();
    const wait = Math.max(0, Math.min(DEBOUNCE_MS, firstPendingAt.current + MAX_WAIT_MS - now));
    timer.current = setTimeout(() => {
      timer.current = null;
      load(id);
    }, wait);
  }, [id, summary, load]);

  useEffect(
    () => () => {
      clearTimer();
      controller.current?.abort();
      // A remount (StrictMode, HMR) must start over rather than trust aborted state.
      prevId.current = null;
      loadedFor.current = undefined;
    },
    [],
  );

  const reload = useCallback(() => {
    if (id != null) load(id);
  }, [id, load]);

  return {
    detail: state.detail,
    detailId: state.detailId,
    loading: state.loading,
    stale: id != null && state.detailId !== id,
    error: state.errorId === id ? state.error : null,
    notFound: state.errorId === id && state.notFound,
    reload,
  };
}

/** Drop a cached detail (e.g. after deleting the exchange). */
export function forgetExchangeDetail(id: ExchangeId): void {
  detailCache.delete(id);
}
