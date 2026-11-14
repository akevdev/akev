/**
 * Web Worker that pretty-prints large JSON / JSON Lines bodies so the body viewer never blocks
 * the main thread. Protocol: `{ id, text, lines }` → `{ id, text }` | `{ id, error }`.
 */

import { formatJson, formatJsonLines } from '../model/body';

interface Request {
  id: number;
  text: string;
  /** JSON Lines (one document per line) instead of one JSON document. */
  lines: boolean;
}

const scope = self as unknown as {
  onmessage: ((e: MessageEvent<Request>) => void) | null;
  postMessage(message: unknown): void;
};

scope.onmessage = (e) => {
  const { id, text, lines } = e.data;
  try {
    if (lines) {
      scope.postMessage({ id, text: formatJsonLines(text) });
      return;
    }
    const f = formatJson(text);
    // Only the text crosses back: the parsed value (lossless numbers) isn't needed here.
    scope.postMessage(f.ok ? { id, text: f.text } : { id, error: f.error });
  } catch (err) {
    scope.postMessage({ id, error: err instanceof Error ? err.message : String(err) });
  }
};
