/** Which view the Traffic page shows: the list or the timeline (a UI pref; the timeline is lazy). */

import { usePagePref, useUiStore } from '../../../state/ui';

export type TrafficViewMode = 'list' | 'timeline';

const VIEW_PREF = 'traffic.view';

export function useTrafficViewMode(): TrafficViewMode {
  const [view] = usePagePref<TrafficViewMode>(VIEW_PREF, 'list');
  return view === 'timeline' ? 'timeline' : 'list';
}

export function setTrafficView(view: TrafficViewMode): void {
  useUiStore.getState().setPagePref(VIEW_PREF, view);
}

export function toggleTrafficView(): void {
  setTrafficView(useUiStore.getState().pages[VIEW_PREF] === 'timeline' ? 'list' : 'timeline');
}
