/**
 * Data hooks for BodyViewer: body views (LRU-cached per exchange/side/original, DESIGN.md §9,
 * stale-while-revalidate across `refreshKey` bumps) and raw bytes for Hex mode (fetched lazily).
 *
 * Both caches are bounded by size as well as count (a view can carry ~8 MiB of text plus a
 * pretty-printed copy), keep one entry per body (a newer `refreshKey` replaces the older one),
 * and are dropped for exchanges that are removed, cleared or resynced (ids restart when the
 * daemon does).
 */

import { useCallback, useEffect, useState } from 'react';
import { api, ApiError, errorMessage } from '../../../api/client';
import type { BodyView, ExchangeId, Side } from '../../../api/types';
import { utf8Encode } from '../../../lib/encoding';
import { LruCache } from '../../../lib/lru';
import { trafficStore } from '../../../state/traffic';

/** Approximate in-memory size of a body view (JS strings are UTF-16). */
export function bodyViewWeight(v: BodyView): number {
  const chars = (v.text?.length ?? 0) + (v.formatted?.length ?? 0);
  // `structured` (decoded payloads, parts, events) is roughly proportional to the body.
  const structured = v.structured != null ? Math.min(v.size, 64 << 20) * 2 : 0;
  return 2 * chars + structured + 512;
}

/** Budget for cached body views (~96 MiB) and raw bytes (~64 MiB). */
export const VIEW_CACHE_BYTES = 96 << 20;
export const BYTES_CACHE_BYTES = 64 << 20;

const views = new LruCache<string, BodyView>(50, { maxWeight: VIEW_CACHE_BYTES, weigh: bodyViewWeight });
const inflight = new Map<string, Promise<BodyView>>();
/** Bumped by {@link forgetBodies}: loads started before it must not repopulate the caches. */
let generation = 0;

/** `id:side:original` — one body; cache keys append the refresh key. */
function viewBase(id: ExchangeId, side: Side, original: boolean): string {
  return `${id}:${side}:${original ? 1 : 0}`;
}

function loadView(id: ExchangeId, side: Side, original: boolean, key: string): Promise<BodyView> {
  const hit = views.get(key);
  if (hit) return Promise.resolve(hit);
  let p = inflight.get(key);
  if (!p) {
    const base = `${viewBase(id, side, original)}:`;
    const gen = generation;
    const request: Promise<BodyView> = api.exchanges
      .body(id, side, { original })
      .then((v) => {
        if (gen === generation) {
          // A newer refresh key supersedes older views of the same body.
          views.deleteWhere((k) => k.startsWith(base) && k !== key);
          views.set(key, v);
        }
        return v;
      })
      .finally(() => {
        if (inflight.get(key) === request) inflight.delete(key);
      });
    p = request;
    inflight.set(key, p);
  }
  return p;
}

/**
 * Fetch an exchange body view through the shared LRU (same cache as {@link useBodyView}).
 * Pass the body's `refreshKey` (`bodyRefreshKey(summary, side)`) so a view cached while the
 * exchange was still in flight isn't served once it has finished.
 */
export function fetchBodyView(id: ExchangeId, side: Side, original = false, refreshKey?: string | number): Promise<BodyView> {
  return loadView(id, side, original, `${viewBase(id, side, original)}:${refreshKey ?? ''}`);
}

/** Drop cached views and bytes of some exchanges (or of all of them). */
export function forgetBodies(ids?: readonly ExchangeId[]): void {
  generation++;
  inflight.clear();
  if (!ids) {
    views.clear();
    bytesCache.clear();
    return;
  }
  const gone = new Set(ids.map(String));
  views.deleteWhere((k) => gone.has(k.slice(0, k.indexOf(':'))));
  bytesCache.deleteWhere((k) => k.startsWith('x:') && gone.has(k.slice(2, k.indexOf(':', 2))));
}

/** Current cache usage (for tests and diagnostics). */
export function bodyCacheStats(): { views: number; viewBytes: number; bytes: number; byteBytes: number } {
  return { views: views.size, viewBytes: views.totalWeight, bytes: bytesCache.size, byteBytes: bytesCache.totalWeight };
}

export interface BodyState {
  view: BodyView | null;
  loading: boolean;
  error: string | null;
  /** The 404 "not found" case (exchange deleted) vs other failures. */
  notFound: boolean;
  retry: () => void;
}

/**
 * Fetch (or take) the body view. `given !== undefined` short-circuits fetching. While a refresh
 * (`refreshKey` bump) is in flight the previous view of the same exchange/side stays visible.
 */
export function useBodyView(exchangeId: ExchangeId | undefined, side: Side | undefined, original: boolean, refreshKey: string | number | undefined, given: BodyView | null | undefined): BodyState {
  const base = exchangeId != null && side ? viewBase(exchangeId, side, original) : null;
  const key = base != null ? `${base}:${refreshKey ?? ''}` : null;
  const [state, setState] = useState<{ key: string | null; view: BodyView | null; error: string | null; notFound: boolean }>(() => ({
    key,
    view: key ? (views.peek(key) ?? null) : null,
    error: null,
    notFound: false,
  }));
  const [nonce, setNonce] = useState(0);

  useEffect(() => {
    if (given !== undefined || key == null || exchangeId == null || !side) return;
    const hit = views.get(key);
    if (hit) {
      setState({ key, view: hit, error: null, notFound: false });
      return;
    }
    let cancelled = false;
    setState((s) => ({ key, view: s.key?.startsWith(`${base}:`) ? s.view : null, error: null, notFound: false }));
    loadView(exchangeId, side, original, key).then(
      (v) => {
        if (!cancelled) setState({ key, view: v, error: null, notFound: false });
      },
      (e: unknown) => {
        if (cancelled) return;
        setState({ key, view: null, error: errorMessage(e), notFound: e instanceof ApiError && e.status === 404 });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [key, base, exchangeId, side, original, given, nonce]);

  const retry = useCallback(() => {
    if (key) views.delete(key);
    setNonce((n) => n + 1);
  }, [key]);

  if (given !== undefined) return { view: given, loading: false, error: null, notFound: false, retry };
  if (key == null) return { view: null, loading: false, error: null, notFound: false, retry };
  if (state.key === key) return { view: state.view, loading: state.view == null && state.error == null, error: state.error, notFound: state.notFound, retry };
  const cached = views.peek(key);
  if (cached) return { view: cached, loading: false, error: null, notFound: false, retry };
  const stale = state.key?.startsWith(`${base}:`) ? state.view : null;
  return { view: stale, loading: stale == null, error: null, notFound: false, retry };
}

// =====================================================================================
// Raw bytes
// =====================================================================================

const bytesCache = new LruCache<string, Uint8Array>(6, { maxWeight: BYTES_CACHE_BYTES, weigh: (b) => b.byteLength });

// Exchanges that are gone (or may have been replaced after a resync: ids restart with the
// daemon) must not be served from the caches.
trafficStore.onLifecycle((e) => forgetBodies(e.type === 'removed' ? e.ids : undefined));

export type BytesSource =
  | { type: 'exchange'; id: ExchangeId; side: Side; decoded: boolean }
  | { type: 'url'; url: string }
  | { type: 'text'; text: string };

function sourceKey(s: BytesSource): string {
  switch (s.type) {
    case 'exchange':
      return `x:${s.id}:${s.side}:${s.decoded ? 1 : 0}`;
    case 'url':
      return `u:${s.url}`;
    case 'text':
      return `t:${s.text.length}:${s.text.slice(0, 64)}`;
  }
}

async function fetchBytes(s: BytesSource, signal: AbortSignal): Promise<Uint8Array> {
  switch (s.type) {
    case 'exchange':
      return api.exchanges.rawBytes(s.id, s.side, { decoded: s.decoded }, signal);
    case 'url': {
      const res = await fetch(s.url, { signal });
      if (!res.ok) throw new Error(`HTTP ${res.status} fetching the raw body`);
      return new Uint8Array(await res.arrayBuffer());
    }
    case 'text':
      return utf8Encode(s.text);
  }
}

export interface BytesState {
  bytes: Uint8Array | null;
  loading: boolean;
  error: string | null;
  retry: () => void;
}

/** Lazily load raw bytes (only while `enabled`). Text sources are encoded synchronously-ish. */
export function useBytes(source: BytesSource | null, enabled: boolean, refreshKey?: string | number): BytesState {
  const key = source ? `${sourceKey(source)}:${refreshKey ?? ''}` : null;
  const [state, setState] = useState<{ key: string | null; bytes: Uint8Array | null; error: string | null }>({ key: null, bytes: null, error: null });
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    if (!enabled || !source || !key) return;
    const hit = bytesCache.get(key);
    if (hit) {
      setState({ key, bytes: hit, error: null });
      return;
    }
    const ctrl = new AbortController();
    const gen = generation;
    setState({ key, bytes: null, error: null });
    fetchBytes(source, ctrl.signal).then(
      (b) => {
        if (gen === generation) bytesCache.set(key, b);
        if (!ctrl.signal.aborted) setState({ key, bytes: b, error: null });
      },
      (e: unknown) => {
        if (!ctrl.signal.aborted) setState({ key, bytes: null, error: errorMessage(e) });
      },
    );
    return () => ctrl.abort();
    // `source` identity changes every render; `key` captures it.
  }, [key, enabled, nonce]);
  const retry = useCallback(() => {
    if (key) bytesCache.delete(key);
    setNonce((n) => n + 1);
  }, [key]);
  if (!key || state.key !== key) {
    const hit = key ? bytesCache.peek(key) : undefined;
    return { bytes: hit ?? null, loading: enabled && !hit, error: null, retry };
  }
  return { bytes: state.bytes, loading: state.bytes == null && state.error == null, error: state.error, retry };
}
