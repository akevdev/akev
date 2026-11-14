/**
 * WebSocket / SSE message helpers for the Messages tab: merging live batches, filtering,
 * previews, close-frame decoding and payload formatting (JSON pretty-print, hex dumps).
 */

import type { Direction, StreamMessageWithPayload } from '../../../../api/types';
import { base64ToBytes, byteHex, utf8DecodeStrict } from '../../../../lib/encoding';
import { formatJsonLossless } from '../../../../lib/json';

export type DirectionFilter = 'all' | Direction;

/**
 * Merge a batch into a seq-ordered list, de-duplicating by `seq`. Appending newer messages (the
 * common case) is O(batch); out-of-order batches fall back to a sort.
 */
export function mergeMessages(existing: readonly StreamMessageWithPayload[], incoming: readonly StreamMessageWithPayload[]): StreamMessageWithPayload[] {
  if (incoming.length === 0) return existing as StreamMessageWithPayload[];
  const lastSeq = existing.length ? (existing[existing.length - 1] as StreamMessageWithPayload).seq : -Infinity;
  let ordered = true;
  let prev = lastSeq;
  for (const m of incoming) {
    if (m.seq <= prev) {
      ordered = false;
      break;
    }
    prev = m.seq;
  }
  if (ordered) return [...existing, ...incoming];
  // An older page ("Load older messages"): everything goes in front.
  const firstSeq = existing.length ? (existing[0] as StreamMessageWithPayload).seq : Infinity;
  let older = true;
  prev = -Infinity;
  for (const m of incoming) {
    if (m.seq <= prev || m.seq >= firstSeq) {
      older = false;
      break;
    }
    prev = m.seq;
  }
  if (older) return [...incoming, ...existing];
  const bySeq = new Map<number, StreamMessageWithPayload>();
  for (const m of existing) bySeq.set(m.seq, m);
  for (const m of incoming) bySeq.set(m.seq, m);
  return [...bySeq.values()].sort((a, b) => a.seq - b.seq);
}

/** Payload bytes of a message (`null` for text payloads). */
export function payloadBytes(m: Pick<StreamMessageWithPayload, 'base64'>): Uint8Array | null {
  if (m.base64 == null) return null;
  try {
    return base64ToBytes(m.base64);
  } catch {
    return null;
  }
}

/** Space-separated hex of the first `max` bytes, with an ellipsis when longer. */
export function hexPreview(bytes: Uint8Array, max = 24): string {
  const n = Math.min(bytes.length, max);
  let out = '';
  for (let i = 0; i < n; i++) out += (i ? ' ' : '') + byteHex(bytes[i] ?? 0);
  return bytes.length > max ? `${out} …` : out;
}

/** Classic hex dump: offset, 16 hex bytes, ASCII column. */
export function hexDump(bytes: Uint8Array, perRow = 16): string {
  const lines: string[] = [];
  const digits = Math.max(4, bytes.length.toString(16).length);
  for (let off = 0; off < bytes.length; off += perRow) {
    let hex = '';
    let ascii = '';
    for (let i = 0; i < perRow; i++) {
      const b = bytes[off + i];
      if (b === undefined) {
        hex += '   ';
        continue;
      }
      hex += `${byteHex(b)} `;
      ascii += b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '.';
    }
    lines.push(`${off.toString(16).padStart(digits, '0')}  ${hex} ${ascii}`);
  }
  return lines.join('\n');
}

// =====================================================================================
// Close frames
// =====================================================================================

export interface CloseInfo {
  code: number | null;
  reason: string;
}

/** Decode a close frame: 2-byte big-endian code + UTF-8 reason (or `"1000 reason"` text). */
export function parseCloseFrame(m: Pick<StreamMessageWithPayload, 'text' | 'base64'>): CloseInfo {
  const bytes = payloadBytes(m);
  if (bytes && bytes.length >= 2) {
    const code = ((bytes[0] ?? 0) << 8) | (bytes[1] ?? 0);
    return { code, reason: utf8DecodeStrict(bytes.subarray(2)) ?? '' };
  }
  const text = (m.text ?? '').trim();
  const match = /^(\d{4})\b\s*[:\-–]?\s*(.*)$/s.exec(text);
  if (match) return { code: Number(match[1]), reason: match[2] ?? '' };
  return { code: null, reason: text };
}

const CLOSE_CODES: Readonly<Record<number, string>> = {
  1000: 'Normal closure',
  1001: 'Going away',
  1002: 'Protocol error',
  1003: 'Unsupported data',
  1005: 'No status received',
  1006: 'Abnormal closure',
  1007: 'Invalid payload data',
  1008: 'Policy violation',
  1009: 'Message too big',
  1010: 'Mandatory extension',
  1011: 'Internal error',
  1012: 'Service restart',
  1013: 'Try again later',
  1014: 'Bad gateway',
  1015: 'TLS handshake failure',
};

/** RFC 6455 meaning of a close code (`Application-defined` for 4000–4999). */
export function closeCodeMeaning(code: number | null): string {
  if (code == null) return 'No close code';
  if (CLOSE_CODES[code]) return CLOSE_CODES[code] as string;
  if (code >= 3000 && code < 4000) return 'Registered (library/framework)';
  if (code >= 4000 && code < 5000) return 'Application-defined';
  return 'Unknown';
}

// =====================================================================================
// Previews & filtering
// =====================================================================================

/** One-line preview for a collapsed row (whitespace collapsed, at most `max` chars). */
export function messagePreview(m: StreamMessageWithPayload, max = 240): string {
  if (m.kind === 'close') {
    const c = parseCloseFrame(m);
    return c.code == null ? c.reason || 'Connection closed' : `${c.code} ${c.reason || closeCodeMeaning(c.code)}`;
  }
  if (m.text != null) {
    const t = m.text.length > max * 2 ? m.text.slice(0, max * 2) : m.text;
    const collapsed = t.replace(/\s+/g, ' ').trim();
    if (!collapsed) return m.kind === 'ping' || m.kind === 'pong' ? '' : '(empty)';
    return collapsed.length > max ? `${collapsed.slice(0, max)}…` : collapsed;
  }
  const bytes = payloadBytes(m);
  if (!bytes || bytes.length === 0) return m.kind === 'ping' || m.kind === 'pong' ? '' : '(empty)';
  return hexPreview(bytes);
}

/** Case-insensitive search over payload text, SSE event name/id and kind. */
export function matchesMessage(m: StreamMessageWithPayload, needle: string, direction: DirectionFilter): boolean {
  if (direction !== 'all' && m.direction !== direction) return false;
  if (!needle) return true;
  const n = needle.toLowerCase();
  if (m.text?.toLowerCase().includes(n)) return true;
  if (m.event?.toLowerCase().includes(n)) return true;
  if (m.eventId?.toLowerCase().includes(n)) return true;
  if (m.kind === n) return true;
  return false;
}

/** Filtered view (returns the same array when no filter is active). */
export function filterMessages(messages: readonly StreamMessageWithPayload[], query: string, direction: DirectionFilter): readonly StreamMessageWithPayload[] {
  const needle = query.trim();
  if (!needle && direction === 'all') return messages;
  return messages.filter((m) => matchesMessage(m, needle, direction));
}

export interface DirectionCounts {
  all: number;
  sent: number;
  received: number;
}

export function countDirections(messages: readonly StreamMessageWithPayload[]): DirectionCounts {
  let sent = 0;
  for (const m of messages) if (m.direction === 'sent') sent++;
  return { all: messages.length, sent, received: messages.length - sent };
}

// =====================================================================================
// Expanded payload
// =====================================================================================

export interface FormattedPayload {
  language: 'json' | 'text' | 'hex';
  text: string;
  /** Framing prefix stripped before JSON parsing (e.g. socket.io's `42`). */
  prefix?: string;
}

/** Pretty JSON (with socket.io-style numeric prefixes), plain text, or a hex dump. */
export function formatPayload(m: StreamMessageWithPayload): FormattedPayload {
  if (m.text != null) {
    const t = m.text.trim();
    const pretty = (s: string) => {
      try {
        return formatJsonLossless(s);
      } catch {
        return null;
      }
    };
    if (t.startsWith('{') || t.startsWith('[')) {
      const p = pretty(t);
      if (p != null) return { language: 'json', text: p };
    }
    const prefixed = /^(\d{1,3})([[{].*)$/s.exec(t);
    if (prefixed) {
      const p = pretty(prefixed[2] ?? '');
      if (p != null) return { language: 'json', text: p, prefix: prefixed[1] };
    }
    return { language: 'text', text: m.text };
  }
  const bytes = payloadBytes(m);
  if (!bytes || bytes.length === 0) return { language: 'text', text: '' };
  return { language: 'hex', text: hexDump(bytes) };
}

/** Offset in ms of a message relative to `baseMicros` (the exchange start). */
export function offsetMs(atMicros: number, baseMicros: number): number {
  return (atMicros - baseMicros) / 1000;
}
