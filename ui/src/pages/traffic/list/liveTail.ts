import type { ExchangeId } from '../../../api/types';
import { insertionIndex } from '../../../state/traffic';

/**
 * How many rows newer than `maxId` sit at the live edge of a display list — the end of an
 * ascending list, the start of a descending (newest-first) one. Binary search: O(log n).
 */
export function countNewer(list: readonly ExchangeId[], maxId: number, descending: boolean): number {
  if (!descending) return list.length - insertionIndex(list, maxId + 1);
  let lo = 0;
  let hi = list.length;
  while (lo < hi) {
    const mid = (lo + hi) >>> 1;
    if ((list[mid] as number) > maxId) lo = mid + 1;
    else hi = mid;
  }
  return lo;
}
