/**
 * Exception-safe Web Storage access. Storage can throw (Safari private mode, disabled cookies,
 * quota exceeded, sandboxed iframes); every caller would otherwise need try/catch.
 */

type Area = 'local' | 'session';

function area(kind: Area): Storage | null {
  try {
    return kind === 'local' ? window.localStorage : window.sessionStorage;
  } catch {
    return null;
  }
}

export function storageGet(key: string, kind: Area = 'local'): string | null {
  try {
    return area(kind)?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

export function storageSet(key: string, value: string, kind: Area = 'local'): boolean {
  try {
    const s = area(kind);
    if (!s) return false;
    s.setItem(key, value);
    return true;
  } catch {
    return false;
  }
}

export function storageRemove(key: string, kind: Area = 'local'): void {
  try {
    area(kind)?.removeItem(key);
  } catch {
    // Storage unavailable: nothing to remove.
  }
}

/** Read and JSON-parse a value; returns `undefined` when missing or malformed. */
export function storageGetJson<T = unknown>(key: string, kind: Area = 'local'): T | undefined {
  const raw = storageGet(key, kind);
  if (raw == null) return undefined;
  try {
    return JSON.parse(raw) as T;
  } catch {
    return undefined;
  }
}

export function storageSetJson(key: string, value: unknown, kind: Area = 'local'): boolean {
  try {
    return storageSet(key, JSON.stringify(value), kind);
  } catch {
    return false;
  }
}
