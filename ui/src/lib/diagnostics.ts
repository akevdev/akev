/**
 * Settings → About → "Copy diagnostics" / "Report a problem": `GET /api/diagnostics` as plain
 * text, and a pre-filled GitHub issue (`.github/ISSUE_TEMPLATE/bug.yml`) the user reviews on
 * GitHub before submitting. The daemon already leaves out tokens and captured traffic.
 */

import type { Diagnostics } from '../api/types';

/** Keep the issue URL comfortably under GitHub's limits (~8 KB). */
export const MAX_ISSUE_URL = 7000;

function formatUptime(secs: number): string {
  const s = Math.max(0, Math.floor(secs));
  const d = Math.floor(s / 86_400);
  const h = Math.floor((s % 86_400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d} d ${h} h`;
  if (h > 0) return `${h} h ${m} min`;
  return `${m} min`;
}

function formatValue(v: unknown): string {
  if (v == null) return '—';
  if (typeof v === 'string') return v;
  if (typeof v === 'number' || typeof v === 'boolean') return String(v);
  return JSON.stringify(v);
}

/** Flatten a nested settings summary into `a.b: value` lines. */
function flatten(obj: Record<string, unknown>, prefix = '', out: string[] = []): string[] {
  for (const [k, v] of Object.entries(obj)) {
    const key = prefix ? `${prefix}.${k}` : k;
    if (v != null && typeof v === 'object' && !Array.isArray(v)) flatten(v as Record<string, unknown>, key, out);
    else out.push(`${key}: ${formatValue(v)}`);
  }
  return out;
}

/** How the bug form's "How you run Akev" dropdown names this install. */
export function runsAs(d: Pick<Diagnostics, 'desktop'>): string {
  return d.desktop ? 'Desktop app' : 'akev command line';
}

/** The diagnostics as readable plain text (what "Copy diagnostics" puts on the clipboard). */
export function formatDiagnostics(d: Diagnostics): string {
  const lines: string[] = [];
  lines.push(`Akev ${d.version}${d.releaseDate ? ` (${d.releaseDate})` : ''}`);
  lines.push(`OS: ${d.os} (${d.arch})`);
  lines.push(`Runs as: ${runsAs(d)} · platform ${d.platform}`);
  lines.push(`Uptime: ${formatUptime(d.uptimeSecs)}`);
  if (d.logFile) lines.push(`Log file: ${d.logFile}`);
  lines.push('');
  lines.push('Stats:');
  for (const line of flatten(d.stats as unknown as Record<string, unknown>)) lines.push(`  ${line}`);
  lines.push('');
  lines.push('Settings:');
  for (const line of flatten(d.settings ?? {})) lines.push(`  ${line}`);
  lines.push('');
  lines.push(`Crash reports: ${d.crashReports.length ? d.crashReports.join(', ') : 'none'}`);
  lines.push('');
  if (d.recentWarnings.length) {
    lines.push('Recent warnings:');
    for (const w of d.recentWarnings) lines.push(`  ${w.at} ${w.level} ${w.target}: ${w.message}`);
  } else {
    lines.push('Recent warnings: none');
  }
  return lines.join('\n');
}

/**
 * A new-issue URL for the bug form with version, OS, how Akev runs and the diagnostics text in
 * "Logs or diagnostics". The text is cut (newest warnings kept last) so the URL stays under
 * `maxLength`.
 */
export function issueUrl(d: Diagnostics, text: string = formatDiagnostics(d), maxLength = MAX_ISSUE_URL): string {
  const base = `${d.repository.replace(/\/+$/, '')}/issues/new`;
  const build = (logs: string) => {
    const params = new URLSearchParams({ template: 'bug.yml', version: d.version, app: runsAs(d), os: `${d.os} (${d.arch})`, logs });
    return `${base}?${params.toString()}`;
  };
  let url = build(text);
  if (url.length <= maxLength) return url;
  const note = '\n… (cut to fit; use "Copy diagnostics" for the rest)';
  let keep = text.length;
  // Shrink until it fits (URL-encoding makes the encoded length non-linear in characters).
  while (keep > 0) {
    keep = Math.floor(keep * 0.85);
    url = build(text.slice(0, keep) + note);
    if (url.length <= maxLength) return url;
  }
  return build('');
}
