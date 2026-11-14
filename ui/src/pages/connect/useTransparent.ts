import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../api/client';
import type { TransparentInfo, TransparentPlan, TransparentPlanRequest, TransparentSettings } from '../../api/types';
import { toast } from '../../components/ui';
import { useAppStore } from '../../state/app';
import { explainError } from './actions';
import { withTransparent } from './transparent';

/**
 * `GET /api/transparent`, re-fetched every `pollMs` while the window is visible (redirected
 * connections arriving is the "check" that the firewall rules work) and whenever the
 * transparent settings change.
 */
export function useTransparentInfo(pollMs = 2000): { info: TransparentInfo | null; error: string | null; refresh: () => void } {
  const [info, setInfo] = useState<TransparentInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const settingsKey = useAppStore((s) => JSON.stringify(s.settings?.proxy.transparent ?? null));
  const inFlight = useRef<AbortController | null>(null);

  const refresh = useCallback(() => {
    inFlight.current?.abort();
    const ctrl = new AbortController();
    inFlight.current = ctrl;
    api.transparent
      .status(ctrl.signal)
      .then((next) => {
        if (ctrl.signal.aborted) return;
        setInfo(next);
        setError(null);
      })
      .catch((e: unknown) => {
        if (!ctrl.signal.aborted) setError(explainError(e));
      });
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh, settingsKey]);

  useEffect(() => {
    if (!pollMs) return;
    const id = window.setInterval(() => {
      if (document.visibilityState !== 'visible' || useAppStore.getState().connection !== 'open') return;
      refresh();
    }, pollMs);
    return () => {
      window.clearInterval(id);
      inFlight.current?.abort();
    };
  }, [pollMs, refresh]);

  return { info, error, refresh };
}

/**
 * The commands for `request` (`POST /api/transparent/plan`), fetched 200 ms after it last
 * changed. `request = null` (invalid input) keeps the previous plan visible.
 */
export function useTransparentPlan(request: TransparentPlanRequest | null, port: number | null): { plan: TransparentPlan | null; error: string | null; loading: boolean } {
  const [plan, setPlan] = useState<TransparentPlan | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const key = request ? JSON.stringify(request) : null;

  useEffect(() => {
    if (key == null) return;
    const ctrl = new AbortController();
    setLoading(true);
    const timer = window.setTimeout(() => {
      api.transparent
        .plan(JSON.parse(key) as TransparentPlanRequest, ctrl.signal)
        .then((next) => {
          if (ctrl.signal.aborted) return;
          setPlan(next);
          setError(null);
        })
        .catch((e: unknown) => {
          if (ctrl.signal.aborted) return;
          setPlan(null);
          setError(explainError(e));
        })
        .finally(() => {
          if (!ctrl.signal.aborted) setLoading(false);
        });
    }, 200);
    return () => {
      ctrl.abort();
      window.clearTimeout(timer);
    };
    // `port`: the commands embed the listener port, so regenerate when it moves.
  }, [key, port]);

  return { plan, error, loading };
}

/** Save `settings.proxy.transparent` changes (toast on failure). Resolves to success. */
export async function saveTransparent(patch: Partial<TransparentSettings>, failure = "Couldn't update transparent capture"): Promise<boolean> {
  const { settings, saveSettings } = useAppStore.getState();
  if (!settings) return false;
  try {
    await saveSettings(withTransparent(settings, patch));
    return true;
  } catch (e) {
    toast.error(failure, { description: explainError(e) });
    return false;
  }
}
