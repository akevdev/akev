/**
 * A paused exchange the detail pane shows from its breakpoint snapshot because it isn't in
 * the traffic list: recording is paused, the capture settings ignore its host, or it was
 * cleared while waiting. Breakpoints still hold such requests, so they must stay reachable.
 *
 * Set by the Traffic page's route handling (`#/traffic/<id>` for an id that is paused but not
 * listed), cleared when the route moves elsewhere.
 */

import { create } from 'zustand';
import type { ExchangeId } from '../../api/types';

export const useUnrecordedPause = create<{ id: ExchangeId | null }>(() => ({ id: null }));

export function showUnrecordedPause(id: ExchangeId | null): void {
  if (useUnrecordedPause.getState().id !== id) useUnrecordedPause.setState({ id });
}
