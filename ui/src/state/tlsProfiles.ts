/**
 * The daemon's upstream TLS fingerprint catalogue (`GET /api/tls-profiles`): which profiles
 * exist and whether this build can present browser fingerprints. Fetched once per page load
 * (it's a property of the daemon build) and refreshed when a component mounts.
 */

import { useEffect, useState } from 'react';
import { api } from '../api/client';
import type { TlsProfiles } from '../api/types';

let cached: TlsProfiles | null = null;
let inflight: Promise<TlsProfiles | null> | null = null;
const listeners = new Set<(c: TlsProfiles | null) => void>();

/** Fetch the catalogue (deduplicated). A failed request keeps the last one (`null` if none). */
export function loadTlsProfiles(): Promise<TlsProfiles | null> {
  inflight ??= api.settings
    .tlsProfiles()
    .then((c) => {
      cached = c;
      return c;
    })
    .catch(() => cached)
    .finally(() => {
      inflight = null;
      for (const l of listeners) l(cached);
    });
  return inflight;
}

/** The catalogue, `null` until loaded. */
export function useTlsProfiles(): TlsProfiles | null {
  const [catalog, setCatalog] = useState<TlsProfiles | null>(cached);
  useEffect(() => {
    listeners.add(setCatalog);
    void loadTlsProfiles();
    return () => {
      listeners.delete(setCatalog);
    };
  }, []);
  return catalog;
}
