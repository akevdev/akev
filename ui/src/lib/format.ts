/**
 * Human-friendly formatting of bytes, durations, times and numbers.
 *
 * Numbers and dates follow the system locale (`1,234.5` / `1.234,5`); units and the compact
 * time formats (`14:03:22`, `1m 05s`) stay fixed. All functions are pure, allocation-light and
 * safe for hot paths (list rows call them on every render). `Intl` formatters are created once
 * and cached.
 */

const BYTE_UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

const numberFmt = new Intl.NumberFormat();
const compactFmt = new Intl.NumberFormat(undefined, { notation: 'compact', maximumFractionDigits: 1 });
/** Up to 0 / 1 / 2 fraction digits (trailing zeros dropped), no grouping. */
const decimalFmts = [0, 1, 2].map((digits) => new Intl.NumberFormat(undefined, { maximumFractionDigits: digits, useGrouping: false }));
const offsetFmt = new Intl.NumberFormat(undefined, { minimumFractionDigits: 3, maximumFractionDigits: 3, useGrouping: false });

/** `12.4` / `12,4` with at most `digits` fraction digits. */
function decimal(n: number, digits: 0 | 1 | 2): string {
  return (decimalFmts[digits] as Intl.NumberFormat).format(n);
}

function pad2(n: number): string {
  return n < 10 ? `0${n}` : String(n);
}

function pad3(n: number): string {
  return n < 10 ? `00${n}` : n < 100 ? `0${n}` : String(n);
}

/**
 * Format a byte count using binary multiples with familiar labels: `0 B`, `512 B`, `12.4 KB`,
 * `3.1 MB`. One decimal below 100 units, none above (`148 KB`).
 * Negative / non-finite inputs render as `—`.
 */
export function formatBytes(bytes: number | null | undefined): string {
  if (bytes == null || !Number.isFinite(bytes) || bytes < 0) return '—';
  if (bytes < 1024) return `${Math.round(bytes)} B`;
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  // Avoid "1024 KB": rounding may push the value to the next unit.
  if (value >= 999.95 && unit < BYTE_UNITS.length - 1) {
    value /= 1024;
    unit++;
  }
  const text = value >= 100 ? decimal(Math.round(value), 0) : decimal(value, 1);
  return `${text} ${BYTE_UNITS[unit]}`;
}

/**
 * Format a duration in milliseconds: `0.4 ms`, `143 ms`, `1.2 s`, `12.5 s`, `1m 05s`, `1h 02m`.
 * `null`/`undefined` renders as `—`.
 */
export function formatDuration(ms: number | null | undefined): string {
  if (ms == null || !Number.isFinite(ms)) return '—';
  const v = Math.max(0, ms);
  if (v < 0.95) return v === 0 ? '0 ms' : `${decimal(v, 1)} ms`;
  // Thresholds are compared after rounding, so 999.6 ms reads "1 s" (not "1000 ms") and
  // 59.97 s reads "1m 00s" (not "60 s").
  if (v < 999.5) return `${Math.round(v)} ms`;
  if (v < 9995) return `${decimal(v / 1000, 2)} s`;
  if (v < 59_950) return `${decimal(v / 1000, 1)} s`;
  const totalSeconds = Math.round(v / 1000);
  if (totalSeconds < 3600) {
    const m = Math.floor(totalSeconds / 60);
    return `${m}m ${pad2(totalSeconds % 60)}s`;
  }
  const h = Math.floor(totalSeconds / 3600);
  return `${h}h ${pad2(Math.floor((totalSeconds % 3600) / 60))}m`;
}

/** Severity bucket of a duration, used to colour the Time column (warning >1s, danger >5s). */
export function durationSeverity(ms: number | null | undefined): 'normal' | 'slow' | 'very-slow' {
  if (ms == null) return 'normal';
  if (ms > 5000) return 'very-slow';
  if (ms > 1000) return 'slow';
  return 'normal';
}

/** `14:03:22.418` (local time) from unix milliseconds. */
export function formatTime(unixMs: number, withMillis = true): string {
  const d = new Date(unixMs);
  const base = `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`;
  return withMillis ? `${base}.${pad3(d.getMilliseconds())}` : base;
}

const dateFmts = {
  medium: new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'short', day: 'numeric' }),
  long: new Intl.DateTimeFormat(undefined, { year: 'numeric', month: 'long', day: 'numeric' }),
};

/**
 * A local calendar date from unix milliseconds or an RFC 3339 string: `iso` (default) is the
 * sortable `2026-11-08` used next to precise times; `medium` (`Nov 8, 2026`) and `long`
 * (`November 8, 2026`) follow the system locale. Unparseable strings are returned as-is.
 */
export function formatDate(when: number | string, style: 'iso' | 'medium' | 'long' = 'iso'): string {
  const d = new Date(when);
  if (Number.isNaN(d.getTime())) return typeof when === 'string' ? when : '—';
  if (style !== 'iso') return dateFmts[style].format(d);
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** `2026-11-08 14:03:22` (local) from unix milliseconds. */
export function formatDateTime(unixMs: number, withMillis = false): string {
  return `${formatDate(unixMs)} ${formatTime(unixMs, withMillis)}`;
}

/** Converts the daemon's `UnixMicros` timestamps to unix milliseconds. */
export function microsToMillis(us: number): number {
  return us / 1000;
}

/**
 * Relative time: `just now`, `12s ago`, `5m ago`, `3h ago`, `yesterday`, `4d ago`, then a date.
 * Future timestamps (clock skew) render as `just now` / `in 5m`.
 */
export function formatRelativeTime(unixMs: number, now: number = Date.now()): string {
  const diff = now - unixMs;
  const abs = Math.abs(diff);
  const future = diff < 0;
  if (abs < 5_000) return 'just now';
  const fmt = (v: number, unit: string) => (future ? `in ${v}${unit}` : `${v}${unit} ago`);
  if (abs < 60_000) return fmt(Math.floor(abs / 1000), 's');
  if (abs < 3_600_000) return fmt(Math.floor(abs / 60_000), 'm');
  if (abs < 86_400_000) return fmt(Math.floor(abs / 3_600_000), 'h');
  if (!future && abs < 2 * 86_400_000) return 'yesterday';
  if (abs < 30 * 86_400_000) return fmt(Math.floor(abs / 86_400_000), 'd');
  return formatDate(unixMs);
}

/** Relative offset within a stream, e.g. `+1.204s` / `+12ms`. */
export function formatOffset(ms: number): string {
  if (!Number.isFinite(ms)) return '—';
  const sign = ms < 0 ? '−' : '+';
  const v = Math.abs(ms);
  if (v < 1000) return `${sign}${Math.round(v)}ms`;
  return `${sign}${offsetFmt.format(v / 1000)}s`;
}

/** `1,234,567` (system locale). */
export function formatNumber(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return numberFmt.format(n);
}

/** `1.2K`, `34M`. */
export function formatCompactNumber(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return '—';
  return compactFmt.format(n);
}

/** `42%` from a 0..1 ratio (one decimal below 10%). */
export function formatPercent(ratio: number): string {
  if (!Number.isFinite(ratio)) return '—';
  const p = ratio * 100;
  return `${p > 0 && p < 10 ? decimal(p, 1) : Math.round(p)}%`;
}

/** `1 exchange` / `3 exchanges`; pass `plural` for irregular nouns. */
export function pluralize(count: number, singular: string, plural = `${singular}s`): string {
  return `${formatNumber(count)} ${count === 1 ? singular : plural}`;
}

/** Live "waiting" timer text for pending rows: `0.8 s`, `12 s`, `1m 05s`. */
export function formatElapsed(sinceUnixMs: number, now: number = Date.now()): string {
  return formatDuration(Math.max(0, now - sinceUnixMs));
}

/** Days until a date (RFC3339 string or unix ms); negative when in the past. */
export function daysUntil(when: string | number, now: number = Date.now()): number {
  const t = typeof when === 'number' ? when : Date.parse(when);
  if (!Number.isFinite(t)) return Number.NaN;
  return Math.floor((t - now) / 86_400_000);
}
