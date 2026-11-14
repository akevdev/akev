/**
 * Bundled MDN-style docs for well-known headers (the headers table, header-name autocomplete).
 * Its own module so the data only loads with the views that use it.
 */

import headerDocsJson from './headerDocs.json';

/** Documentation for a well-known header. */
export interface HeaderDoc {
  /** Canonical casing, e.g. `Content-Type`. */
  name: string;
  side: 'req' | 'res' | 'both';
  category: string;
  /** One-line description. */
  summary: string;
  /** MDN reference, when one exists. */
  url?: string;
}

const HEADER_DOCS = headerDocsJson as Readonly<Record<string, HeaderDoc>>;

/** Look up docs for a header name (case-insensitive). */
export function lookupHeaderDoc(name: string): HeaderDoc | undefined {
  return HEADER_DOCS[name.trim().toLowerCase()];
}

/** Canonical names of all documented headers (header-name autocomplete). */
export const KNOWN_HEADER_NAMES: readonly string[] = Object.values(HEADER_DOCS)
  .map((d) => d.name)
  .filter((n) => !n.startsWith(':'))
  .sort((a, b) => a.localeCompare(b));
