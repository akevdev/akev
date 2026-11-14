/**
 * Web Worker running `diffText` for large inputs so TextDiff never blocks the main thread.
 * Protocol: `{ id, left, right, opts }` → `{ id, result }` | `{ id, error }`.
 */

import { diffText, type DiffOptions } from '../../../lib/diff';

interface Request {
  id: number;
  left: string;
  right: string;
  opts: DiffOptions;
}

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<Request>) => void) | null;
  postMessage(message: unknown): void;
};

scope.onmessage = (e) => {
  const { id, left, right, opts } = e.data;
  try {
    scope.postMessage({ id, result: diffText(left, right, opts) });
  } catch (err) {
    scope.postMessage({ id, error: err instanceof Error ? err.message : String(err) });
  }
};
