/**
 * Codegen targets for "Copy as…" (`GET /api/codegen/targets`), fetched once per session.
 * Menus read the cached list synchronously; components subscribe with the hook.
 */

import { useEffect, useState } from 'react';
import { api } from '../../../api/client';
import type { TargetInfo } from '../../../api/types';

/** Fallback when the daemon can't be asked (keeps "Copy as cURL" working). */
const FALLBACK_TARGETS: TargetInfo[] = [{ id: 'curl', name: 'cURL', language: 'shell', group: 'Shell' }];

let targetsPromise: Promise<TargetInfo[]> | null = null;
let targetsCache: TargetInfo[] | null = null;

/** Fetch the targets (once); resolves to the cURL-only fallback when the request fails. */
export function loadCodegenTargets(): Promise<TargetInfo[]> {
  if (targetsCache) return Promise.resolve(targetsCache);
  targetsPromise ??= api.codegen.targets().then(
    (t) => {
      targetsCache = t.length ? t : FALLBACK_TARGETS;
      return targetsCache;
    },
    () => {
      targetsPromise = null;
      return FALLBACK_TARGETS;
    },
  );
  return targetsPromise;
}

/** The cached targets (empty until {@link loadCodegenTargets} has resolved). */
export function codegenTargets(): readonly TargetInfo[] {
  return targetsCache ?? [];
}

/** The targets, or `null` while loading. */
export function useCodegenTargets(): TargetInfo[] | null {
  const [targets, setTargets] = useState<TargetInfo[] | null>(targetsCache);
  useEffect(() => {
    if (targetsCache) return;
    let alive = true;
    void loadCodegenTargets().then((t) => alive && setTargets(t));
    return () => {
      alive = false;
    };
  }, []);
  return targets;
}
