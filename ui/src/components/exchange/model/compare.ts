/**
 * Text normalisation for ExchangeCompare: every section becomes plain text that diffs well
 * (URL split per query parameter, headers as sorted `name: value` lines, bodies pretty-printed)
 * plus a structured timing comparison. No React.
 */

import type { BodyView, ExchangeDetail, Headers } from '../../../api/types';
import { formatBytes, formatDuration } from '../../../lib/format';
import { contentKindLabel, statusText } from '../../../lib/http';
import { formatJsonLossless, stringifyJson } from '../../../lib/json';
import { buildWaterfall, type PhaseId } from './timing';

export type CompareSectionId = 'request-line' | 'status' | 'request-headers' | 'response-headers' | 'request-body' | 'response-body' | 'timing';

export const COMPARE_SECTION_LABELS: Readonly<Record<CompareSectionId, string>> = {
  'request-line': 'Request line',
  status: 'Status',
  'request-headers': 'Request headers',
  'response-headers': 'Response headers',
  'request-body': 'Request body',
  'response-body': 'Response body',
  timing: 'Timing',
};

/**
 * Request line text: `METHOD scheme://authority/path HTTP/x`, then one `?name=value` /
 * `&name=value` line per query parameter so parameter changes diff line by line.
 */
export function requestLineText(detail: ExchangeDetail): string {
  const { method, url, httpVersion } = detail.request;
  const q = url.indexOf('?');
  const hash = url.indexOf('#');
  const base = q < 0 ? (hash < 0 ? url : url.slice(0, hash)) : url.slice(0, q);
  const query = q < 0 ? '' : url.slice(q + 1, hash > q ? hash : undefined);
  const lines = [`${method} ${base} ${httpVersion}`];
  query
    .split('&')
    .filter(Boolean)
    .forEach((p, i) => lines.push(`${i === 0 ? '?' : '&'}${p}`));
  return lines.join('\n');
}

/** Status text: `HTTP/2 200 OK`, an error line, or a pending marker. */
export function statusLineText(detail: ExchangeDetail): string {
  if (detail.response) {
    const r = detail.response;
    return `${r.httpVersion} ${r.status} ${r.statusMessage || statusText(r.status)}`.trim();
  }
  if (detail.error) return `Failed: ${detail.error.kind}${detail.error.code ? ` (${detail.error.code})` : ''}\n${detail.error.message}`;
  if (detail.tunnel) return `Tunnel: ${detail.tunnel.reason}`;
  return '(no response yet)';
}

/**
 * Headers normalised for diffing: lowercase names, sorted by name (pseudo-headers first, stable
 * for duplicates), one `name: value` line each. Removes noise from casing and wire order.
 */
export function headersText(headers: Headers | undefined): string {
  if (!headers?.length) return '';
  const rows = headers.map(([k, v], i) => ({ k: k.toLowerCase(), v, i }));
  rows.sort((a, b) => {
    const pa = a.k.startsWith(':');
    const pb = b.k.startsWith(':');
    if (pa !== pb) return pa ? -1 : 1;
    return a.k < b.k ? -1 : a.k > b.k ? 1 : a.i - b.i;
  });
  return rows.map((r) => `${r.k}: ${r.v}`).join('\n');
}

/** Body text for diffing: pretty text → text → structured JSON → a binary placeholder. */
export function bodyText(view: BodyView | null | undefined): { text: string; binary: boolean; language: string } {
  if (!view || (view.size === 0 && !view.text)) return { text: '', binary: false, language: 'text' };
  if (view.formatted != null) return { text: view.formatted, binary: false, language: languageOf(view) };
  if (view.text != null) {
    if (view.kind === 'json' || view.kind === 'graph-ql') {
      try {
        return { text: formatJsonLossless(view.text), binary: false, language: 'json' };
      } catch {
        // fall through to raw text
      }
    }
    return { text: view.text, binary: false, language: languageOf(view) };
  }
  if (view.structured != null && ['protobuf', 'grpc', 'grpc-web', 'msg-pack', 'cbor'].includes(view.kind)) {
    return { text: stringifyJson(view.structured, 2) ?? 'null', binary: false, language: 'json' };
  }
  return { text: `<${formatBytes(view.size)} ${contentKindLabel(view.kind)} body>`, binary: true, language: 'text' };
}

function languageOf(view: BodyView): string {
  switch (view.kind) {
    case 'json':
    case 'json-lines':
    case 'graph-ql':
      return 'json';
    case 'html':
      return 'html';
    case 'xml':
      return 'xml';
    case 'javascript':
      return 'javascript';
    case 'css':
      return 'css';
    case 'yaml':
      return 'yaml';
    case 'markdown':
      return 'markdown';
    default:
      return 'text';
  }
}

export interface TimingRow {
  id: PhaseId | 'total';
  label: string;
  a: number | null;
  b: number | null;
  /** b − a (ms), when both are known. */
  delta: number | null;
}

const TIMING_ORDER: [TimingRow['id'], string][] = [
  ['total', 'Total'],
  ['dns', 'DNS lookup'],
  ['connect', 'Connect'],
  ['tls', 'TLS handshake'],
  ['sent', 'Request sent'],
  ['wait', 'Waiting (TTFB)'],
  ['download', 'Content download'],
];

/** Phase-by-phase timing comparison (rows where neither side has a value are omitted). */
export function compareTiming(a: ExchangeDetail, b: ExchangeDetail): TimingRow[] {
  const wa = buildWaterfall(a.timing, { state: a.summary.state });
  const wb = buildWaterfall(b.timing, { state: b.summary.state });
  const pick = (w: ReturnType<typeof buildWaterfall>, id: TimingRow['id']): number | null => {
    if (id === 'total') return w.total;
    const p = w.phases.find((x) => x.id === id);
    return p ? p.duration : null;
  };
  const rows: TimingRow[] = [];
  for (const [id, label] of TIMING_ORDER) {
    const va = pick(wa, id);
    const vb = pick(wb, id);
    if (va == null && vb == null) continue;
    rows.push({ id, label, a: va, b: vb, delta: va != null && vb != null ? vb - va : null });
  }
  return rows;
}

/** `+120 ms (+85%)` / `−3 ms (−2%)` / `same`. */
export function formatDelta(a: number | null, b: number | null): string {
  if (a == null || b == null) return '—';
  const d = b - a;
  if (Math.abs(d) < 0.5) return 'same';
  const pct = a > 0 ? Math.round((d / a) * 100) : null;
  return `${d > 0 ? '+' : '−'}${formatDuration(Math.abs(d))}${pct != null && Number.isFinite(pct) ? ` (${pct > 0 ? '+' : '−'}${Math.abs(pct)}%)` : ''}`;
}
