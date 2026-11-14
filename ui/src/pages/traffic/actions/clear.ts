import { toast } from '../../../components/ui';
import { askClear } from '../dialogs/TrafficDialogs';
import { clearPlan, clearTraffic } from './exchangeActions';

/**
 * Clear traffic (Ctrl/⌘ Shift X): undoable; asks first only when pinned exchanges would be
 * affected (DESIGN.md §1.6), offering to keep them.
 */
export async function requestClear(): Promise<void> {
  const plan = clearPlan();
  if (!plan) {
    toast('Nothing to clear');
    return;
  }
  let keepPinned = false;
  if (plan.pinned > 0) {
    const choice = await askClear(plan.ids.length, plan.pinned);
    if (choice === 'cancel') return;
    keepPinned = choice === 'keep-pinned';
  }
  clearTraffic(plan, keepPinned);
}
