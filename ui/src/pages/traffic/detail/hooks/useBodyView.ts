import type { BodyView, ExchangeId, Side } from '../../../../api/types';
import { useBodyView as useSharedBodyView } from '../../../../components/exchange/body/data';

export interface BodyViewState {
  view: BodyView | null;
  loading: boolean;
  error: string | null;
}

/**
 * Fetch a decoded body (`GET /api/exchanges/:id/body/:side`). A changed `refreshKey` (e.g. the
 * exchange finished streaming) refetches while keeping the previous view of the *same* body.
 *
 * Shares BodyViewer's size-bounded cache (`components/exchange/body/data`), so the Body and Raw
 * tabs hold one copy of a body instead of two.
 */
export function useBodyView(id: ExchangeId | null, side: Side, original = false, refreshKey?: string | number, enabled = true): BodyViewState {
  const { view, loading, error } = useSharedBodyView(enabled && id != null ? id : undefined, side, original, refreshKey, undefined);
  return { view, loading, error };
}
