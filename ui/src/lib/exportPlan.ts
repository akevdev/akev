/**
 * Exporting an explicit selection of exchanges through `GET /api/export/*?ids=…`.
 *
 * Every id travels in the query string, and the daemon rejects request URIs over 64 KiB
 * (`414 URI Too Long`, about 10k ids). Selections are therefore sent as compact `id:` range
 * filters when the plain list would be too long (`id:1-4000,4002-9000` — the export endpoints
 * accept `filter=`), and split into several requests when even that doesn't fit; HAR chunks are
 * merged back into one file.
 */

import type { ExchangeId } from '../api/types';
import { mayHaveInexactNumbers, parseJsonLossless, stringifyJson } from './json';

/** Longest export URL we produce — well under the daemon's 65 534-byte URI limit. */
export const MAX_EXPORT_URL = 30_000;

/** What selects the exchanges of one export request (`?ids=` or `?filter=`). */
export type SelectionQuery = { ids: ExchangeId[] } | { filter: string };

/** Distinct ids as ascending compact ranges: `[1, 2, 3, 5, 7, 8]` → `['1-3', '5', '7-8']`. */
export function idRanges(ids: Iterable<ExchangeId>): string[] {
  const sorted = [...new Set(ids)].filter((id) => Number.isSafeInteger(id) && id >= 0).sort((a, b) => a - b);
  const out: string[] = [];
  let i = 0;
  while (i < sorted.length) {
    const start = sorted[i] as number;
    let end = start;
    while (i + 1 < sorted.length && sorted[i + 1] === end + 1) end = sorted[++i] as number;
    out.push(start === end ? String(start) : `${start}-${end}`);
    i++;
  }
  return out;
}

/** Filter query selecting exactly the exchanges in `ranges` (`id:1-3,5,7-8`). */
export function idFilter(ranges: readonly string[]): string {
  return `id:${ranges.join(',')}`;
}

/**
 * The request(s) to export exactly `ids`, each with a URL (as built by `urlFor`) of at most
 * `maxUrl` characters: the id list itself when it fits, else the ids as `id:` ranges, else
 * consecutive range chunks. Returns `[]` for an empty selection.
 */
export function planSelectionExport(ids: readonly ExchangeId[], urlFor: (q: SelectionQuery) => string, maxUrl = MAX_EXPORT_URL): SelectionQuery[] {
  const ranges = idRanges(ids);
  if (ranges.length === 0) return [];
  const sorted = [...new Set(ids)].sort((a, b) => a - b);
  const plain: SelectionQuery = { ids: sorted };
  if (urlFor(plain).length <= maxUrl) return [plain];
  const whole: SelectionQuery = { filter: idFilter(ranges) };
  if (urlFor(whole).length <= maxUrl) return [whole];

  // Greedy chunks of ranges. A range costs its text plus an encoded comma (`%2C`); the first
  // one also the `id:` prefix — measured from a real URL, so this holds for any `urlFor`.
  const probe = urlFor({ filter: idFilter(['0']) }).length;
  const base = probe - 1 - 3; // minus the "0" and the comma it would have needed
  const out: SelectionQuery[] = [];
  let chunk: string[] = [];
  let len = base;
  const flush = () => {
    if (chunk.length) out.push(...splitToFit(chunk, urlFor, maxUrl));
    chunk = [];
    len = base;
  };
  for (const r of ranges) {
    const cost = r.length + 3;
    if (chunk.length && len + cost > maxUrl) flush();
    chunk.push(r);
    len += cost;
  }
  flush();
  return out;
}

/** Verify a chunk's real URL length, halving it until every part fits. */
function splitToFit(ranges: string[], urlFor: (q: SelectionQuery) => string, maxUrl: number): SelectionQuery[] {
  const q: SelectionQuery = { filter: idFilter(ranges) };
  if (ranges.length === 1 || urlFor(q).length <= maxUrl) return [q];
  const mid = Math.ceil(ranges.length / 2);
  return [...splitToFit(ranges.slice(0, mid), urlFor, maxUrl), ...splitToFit(ranges.slice(mid), urlFor, maxUrl)];
}

interface HarFile {
  log: { entries: unknown[]; pages?: { id?: unknown }[] } & Record<string, unknown>;
}

function isHar(v: unknown): v is HarFile {
  if (!v || typeof v !== 'object') return false;
  const log = (v as { log?: unknown }).log;
  return !!log && typeof log === 'object' && Array.isArray((log as { entries?: unknown }).entries);
}

/**
 * Incrementally merges HAR exports of consecutive chunks into one HAR document (entries in
 * chunk order, pages de-duplicated by id), so only one chunk's text is held at a time. Numbers
 * are kept exactly. `add` throws when a part isn't a HAR log.
 */
export class HarMerger {
  private merged: HarFile | null = null;
  private first: string | null = null;
  private parts = 0;
  private readonly pageIds = new Set<unknown>();

  add(text: string): void {
    this.parts++;
    const har: unknown = mayHaveInexactNumbers(text) ? parseJsonLossless(text) : JSON.parse(text);
    if (!isHar(har)) throw new Error('Akev returned something other than a HAR log');
    if (!this.merged) {
      this.merged = har;
      this.first = text;
      for (const p of har.log.pages ?? []) this.pageIds.add(p?.id);
      return;
    }
    this.first = null;
    const entries = this.merged.log.entries;
    for (const e of har.log.entries) entries.push(e);
    for (const p of har.log.pages ?? []) {
      if (this.pageIds.has(p?.id)) continue;
      this.pageIds.add(p?.id);
      (this.merged.log.pages ??= []).push(p);
    }
  }

  /** The merged HAR text (a single part is returned unchanged). */
  result(): string {
    if (this.parts === 1 && this.first != null) return this.first;
    return this.merged ? (stringifyJson(this.merged, 2) ?? '') : '';
  }
}

/** Merge HAR exports of consecutive chunks into one HAR document (see {@link HarMerger}). */
export function mergeHar(parts: readonly string[]): string {
  const m = new HarMerger();
  for (const p of parts) m.add(p);
  return m.result();
}
