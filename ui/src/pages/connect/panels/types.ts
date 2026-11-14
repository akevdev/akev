import { useEffect, useRef, useState } from 'react';
import type { InterceptorInfo } from '../../../api/types';
import { toast } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { explainError } from '../actions';
import type { InterceptorMeta } from '../metadata';
import type { ProxyInfo } from '../useProxyInfo';

export interface PanelProps {
  info: InterceptorInfo;
  meta: InterceptorMeta;
  proxy: ProxyInfo;
  /** Open another interceptor's panel (e.g. Manual → Android). */
  onOpenOther: (id: string) => void;
}

/**
 * Re-fetch the interceptor list (device/container lists live in metadata). A manual refresh
 * reports failures; with `autoMs` the list is also re-fetched quietly on that interval while
 * the panel is mounted and the window is visible (the daemon only pushes `interceptors` when
 * activation state changes, not when a device is plugged in).
 */
export function useRefreshInterceptors(autoMs?: number): { refreshing: boolean; refresh: () => void } {
  const [refreshing, setRefreshing] = useState(false);
  const inFlight = useRef(false);

  const refresh = () => {
    if (inFlight.current) return;
    inFlight.current = true;
    setRefreshing(true);
    useAppStore
      .getState()
      .refreshInterceptors()
      .catch((e: unknown) => toast.error("Couldn't refresh", { description: explainError(e) }))
      .finally(() => {
        inFlight.current = false;
        setRefreshing(false);
      });
  };

  useEffect(() => {
    if (!autoMs) return;
    const id = window.setInterval(() => {
      if (inFlight.current || document.visibilityState !== 'visible' || useAppStore.getState().connection !== 'open') return;
      inFlight.current = true;
      useAppStore
        .getState()
        .refreshInterceptors()
        .catch(() => undefined)
        .finally(() => {
          inFlight.current = false;
        });
    }, autoMs);
    return () => window.clearInterval(id);
  }, [autoMs]);

  return { refreshing, refresh };
}
