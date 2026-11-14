import { useCallback, useEffect, useRef, useState } from 'react';
import { ApiError, api, errorMessage } from '../../../../api/client';
import type { ExchangeId, ExchangeSummary, LlmExchange } from '../../../../api/types';

/** While a response streams, refetch at most this often (each fetch re-parses the prompt). */
const LIVE_THROTTLE_MS = 450;

export interface LlmExchangeState {
  data: LlmExchange | null;
  /** The shown data belongs to an older version of this exchange (a refetch is running). */
  stale: boolean;
  loading: boolean;
  error: string | null;
  /** The daemon doesn't recognise the exchange as an LLM call. */
  notLlm: boolean;
  /** The daemon answered `pro-required` (the license changed under us). */
  locked: boolean;
  reload: () => void;
}

/** What changes the conversation: response arrival, streamed events, completion. */
export function llmVersion(s: ExchangeSummary): string {
  return `${s.state}:${s.status ?? ''}:${s.messageCount ?? 0}:${s.responseSize}:${s.requestSize}`;
}

/**
 * `GET /api/exchanges/:id/llm`, refetched as the exchange progresses (throttled while the
 * response streams, so the conversation fills in live without hammering the daemon).
 */
export function useLlmExchange(id: ExchangeId, version: string, live: boolean): LlmExchangeState {
  const [result, setResult] = useState<{ id: ExchangeId; version: string; data: LlmExchange | null; error: string | null; notLlm: boolean; locked?: boolean } | null>(null);
  const [nonce, setNonce] = useState(0);
  const seq = useRef(0);
  const lastFetch = useRef(0);
  const ctrl = useRef<AbortController | null>(null);

  useEffect(() => {
    const run = () => {
      const mine = ++seq.current;
      ctrl.current?.abort();
      const c = new AbortController();
      ctrl.current = c;
      lastFetch.current = Date.now();
      api.exchanges.llm(id, c.signal).then(
        (data) => {
          if (mine === seq.current) setResult({ id, version, data, error: null, notLlm: false });
        },
        (e: unknown) => {
          if (c.signal.aborted || mine !== seq.current) return;
          const notLlm = e instanceof ApiError && e.status === 404 && e.code === 'not-llm';
          const locked = e instanceof ApiError && e.isProRequired;
          setResult({ id, version, data: null, error: notLlm || locked ? null : errorMessage(e), notLlm, locked });
        },
      );
    };
    const wait = live ? Math.max(0, LIVE_THROTTLE_MS - (Date.now() - lastFetch.current)) : 0;
    const timer = setTimeout(run, wait);
    return () => clearTimeout(timer);
  }, [id, version, live, nonce]);

  // Abort the request in flight when switching exchanges or unmounting.
  useEffect(() => () => ctrl.current?.abort(), [id]);

  const reload = useCallback(() => setNonce((n) => n + 1), []);
  const current = result?.id === id ? result : null;
  return {
    data: current?.data ?? null,
    stale: current != null && current.version !== version,
    loading: current == null || current.version !== version,
    error: current?.error ?? null,
    notLlm: current?.notLlm ?? false,
    locked: current?.locked ?? false,
    reload,
  };
}
