/** Resource limits (akev-core settings): unit helpers and the "what the limits did" counters from `StatusStats`. */

import type { StatusStats } from '../../api/types';
import { formatBytes, pluralize } from '../../lib/format';

const MiB = 1024 * 1024;
const GiB = 1024 * MiB;

/** akev-core `MIN_EVENT_QUEUE_BYTES`. */
export const MIN_EVENT_QUEUE_BYTES = MiB;

export function bytesToGb(bytes: number): number {
  return Math.round((bytes / GiB) * 10) / 10;
}

export function gbToBytes(gb: number): number {
  return Math.round(gb * GiB);
}

/** What the limits did since start, as short sentences (only nonzero counters). */
export function limitCounters(stats: StatusStats | null | undefined): string[] {
  if (!stats) return [];
  const out: string[] = [];
  const bodies = (v: number) => pluralize(v, 'body', 'bodies');
  if (stats.evicted > 0) out.push(`${pluralize(stats.evicted, 'oldest exchange')} removed (max exchanges)`);
  if (stats.diskDroppedBodies > 0) out.push(`${bodies(stats.diskDroppedBodies)} deleted from disk (disk space limit)`);
  if (stats.droppedBodies > 0) out.push(`${bodies(stats.droppedBodies)} not recorded (capture overloaded)`);
  if (stats.passedThrough > 0) out.push(`${pluralize(stats.passedThrough, 'exchange')} passed through without body rules (rule memory used up)`);
  if (stats.autoReleased > 0) out.push(`${pluralize(stats.autoReleased, 'breakpoint')} released automatically or skipped`);
  return out;
}

/** "1.2 GB on disk" for the spill files, when there are any. */
export function spillOnDisk(stats: StatusStats | null | undefined): string | null {
  const bytes = stats?.spillDiskBytes ?? 0;
  return bytes > 0 ? `${formatBytes(bytes)} of spill files on disk` : null;
}
