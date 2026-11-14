/**
 * The one query-string parser: decoded params *and* their raw (wire) form, so views can show
 * readable values while copy/raw views and the Compose URL ↔ Params sync stay faithful to what
 * was captured or typed. Used by the detail Query tab, Compose and form bodies.
 */

export interface QueryParam {
  /** Decoded name (`+` → space, percent-decoding; malformed escapes are kept as-is). */
  name: string;
  /** Decoded value (`''` for a bare key). */
  value: string;
  /** Name as it appears on the wire. */
  rawName: string;
  /** Value as it appears on the wire (`null` for a bare key without `=`). */
  rawValue: string | null;
}

export interface UrlParts {
  /** Everything before `?` (scheme, authority, path). */
  base: string;
  /** Query string without `?`; `null` when there is no `?` at all. */
  query: string | null;
  /** Fragment including `#`, or `''`. */
  hash: string;
}

/** Split a URL or path (possibly with `{{vars}}`, possibly invalid) into base / query / fragment. */
export function splitUrl(url: string): UrlParts {
  const hashIdx = url.indexOf('#');
  const beforeHash = hashIdx >= 0 ? url.slice(0, hashIdx) : url;
  const hash = hashIdx >= 0 ? url.slice(hashIdx) : '';
  const q = beforeHash.indexOf('?');
  if (q < 0) return { base: beforeHash, query: null, hash };
  return { base: beforeHash.slice(0, q), query: beforeHash.slice(q + 1), hash };
}

/** The raw query string of a path or absolute URL, without `?` or `#fragment` (`''` when absent). */
export function rawQueryOf(pathOrUrl: string): string {
  return splitUrl(pathOrUrl).query ?? '';
}

/** Percent-decode a query component (`+` = space); malformed escapes are returned untouched. */
export function decodeQueryComponent(s: string): string {
  const plus = s.replace(/\+/g, ' ');
  try {
    return decodeURIComponent(plus);
  } catch {
    // Decode what we can, escape by escape.
    return plus.replace(/(%[0-9a-fA-F]{2})+/g, (m) => {
      try {
        return decodeURIComponent(m);
      } catch {
        return m;
      }
    });
  }
}

/**
 * Parse a raw query string (with or without a leading `?`) into ordered params. Duplicates are
 * kept; empty segments (`a=1&&b=2`) are skipped.
 */
export function parseQueryString(query: string | null | undefined): QueryParam[] {
  const q = query?.startsWith('?') ? query.slice(1) : query;
  if (!q) return [];
  const out: QueryParam[] = [];
  for (const part of q.split('&')) {
    if (!part) continue;
    const eq = part.indexOf('=');
    const rawName = eq < 0 ? part : part.slice(0, eq);
    const rawValue = eq < 0 ? null : part.slice(eq + 1);
    out.push({ name: decodeQueryComponent(rawName), value: rawValue == null ? '' : decodeQueryComponent(rawValue), rawName, rawValue });
  }
  return out;
}
