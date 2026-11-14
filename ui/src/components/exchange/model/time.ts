/** Compact relative times for expiry-style values (cookies, JWTs, certificates). No React. */

const UNITS: [number, string][] = [
  [365 * 86_400_000, 'y'],
  [30 * 86_400_000, 'mo'],
  [86_400_000, 'd'],
  [3_600_000, 'h'],
  [60_000, 'm'],
  [1000, 's'],
];

/** `in 3d`, `2h ago`, `in 74y`, `just now` (no calendar fallback, unlike `formatRelativeTime`). */
export function relTime(at: number, now: number = Date.now()): string {
  if (!Number.isFinite(at)) return '—';
  const diff = at - now;
  const abs = Math.abs(diff);
  if (abs < 5000) return 'just now';
  for (const [size, unit] of UNITS) {
    if (abs >= size) {
      const n = Math.floor(abs / size);
      return diff > 0 ? `in ${n}${unit}` : `${n}${unit} ago`;
    }
  }
  return 'just now';
}
