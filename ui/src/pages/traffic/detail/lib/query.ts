/**
 * The Query tab's view of a request's query string (parsing lives in `lib/query.ts`).
 */

import { parseQueryString, rawQueryOf, type QueryParam } from '../../../../lib/query';

export interface QueryInfo {
  /** Raw query string without the leading `?` and without any `#fragment`. */
  raw: string;
  /** Params in wire order; `index` keys duplicates apart. */
  params: (QueryParam & { index: number })[];
  /** Names that occur more than once (names are case-sensitive). */
  duplicates: ReadonlySet<string>;
}

/** Query info for a request path (`/a?b=c`) or absolute URL. */
export function extractQuery(pathOrUrl: string): QueryInfo {
  const raw = rawQueryOf(pathOrUrl);
  const params = parseQueryString(raw).map((p, index) => ({ ...p, index }));
  const seen = new Set<string>();
  const duplicates = new Set<string>();
  for (const p of params) {
    if (seen.has(p.name)) duplicates.add(p.name);
    seen.add(p.name);
  }
  return { raw, params, duplicates };
}

/** Decoded params as `name=value` lines (for "Copy all"). */
export function queryParamsToText(params: readonly QueryParam[]): string {
  return params.map((p) => (p.rawValue == null ? p.name : `${p.name}=${p.value}`)).join('\n');
}
