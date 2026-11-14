/**
 * WebSocket composer & paused-message editing: turning what the user typed (text, JSON, hex or
 * base64) into an API payload, and paused messages into editable text — without ever changing
 * an untouched message (forwarding it unchanged keeps its original framing).
 */

import type { MessageResume, PausedMessage, WsMessageBody } from '../../../../api/types';
import { base64ToBytes, bytesToBase64, bytesToHex, utf8DecodeStrict, utf8Encode, utf8Length } from '../../../../lib/encoding';

export type ComposerMode = 'text' | 'json' | 'hex' | 'base64';

export type Encoded = { ok: true; body: WsMessageBody & ({ kind: 'text'; text: string } | { kind: 'binary'; base64: string }); size: number } | { ok: false; error: string };

/** Bytes from hex: whitespace, `:`/`-`/`,` separators and `0x` prefixes are ignored. */
export function parseHex(input: string): Uint8Array | string {
  const clean = input.replace(/0x/gi, '').replace(/[\s:,-]/g, '');
  if (/[^0-9a-f]/i.test(clean)) return 'Hex may only contain 0-9 and a-f';
  if (clean.length % 2 !== 0) return 'Hex needs an even number of digits (two per byte)';
  const out = new Uint8Array(clean.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(clean.slice(i * 2, i * 2 + 2), 16);
  return out;
}

/** Hex for editing: space-separated bytes, 16 per line. */
export function formatHex(bytes: Uint8Array): string {
  const lines: string[] = [];
  for (let i = 0; i < bytes.length; i += 16) lines.push(bytesToHex(bytes.subarray(i, i + 16), ' '));
  return lines.join('\n');
}

/** Bytes from base64 (whitespace ignored), or an error. */
export function parseBase64(input: string): Uint8Array | string {
  const clean = input.replace(/\s/g, '');
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(clean) || clean.length % 4 === 1) return 'Not valid base64';
  try {
    return base64ToBytes(clean);
  } catch {
    return 'Not valid base64';
  }
}

/** Why `input` isn't JSON (null when it is). */
export function jsonError(input: string): string | null {
  if (!input.trim()) return 'Enter a JSON value';
  try {
    JSON.parse(input);
    return null;
  } catch (e) {
    return `Invalid JSON: ${e instanceof Error ? e.message.replace(/^JSON\.parse: /, '') : String(e)}`;
  }
}

/** The API payload for what was typed in `mode` (JSON is sent exactly as typed). */
export function encodeComposed(mode: ComposerMode, input: string): Encoded {
  switch (mode) {
    case 'text':
      return { ok: true, body: { kind: 'text', text: input }, size: utf8Length(input) };
    case 'json': {
      const error = jsonError(input);
      return error ? { ok: false, error } : { ok: true, body: { kind: 'text', text: input }, size: utf8Length(input) };
    }
    case 'hex': {
      const bytes = parseHex(input);
      return typeof bytes === 'string' ? { ok: false, error: bytes } : { ok: true, body: { kind: 'binary', base64: bytesToBase64(bytes) }, size: bytes.length };
    }
    case 'base64': {
      const bytes = parseBase64(input);
      return typeof bytes === 'string' ? { ok: false, error: bytes } : { ok: true, body: { kind: 'binary', base64: bytesToBase64(bytes) }, size: bytes.length };
    }
  }
}

/** Text is JSON when it is an object or array that parses. */
export function looksLikeJson(text: string): boolean {
  const t = text.trim();
  if (!(t.startsWith('{') || t.startsWith('['))) return false;
  return jsonError(t) == null;
}

/** How a paused message is shown for editing: text as typed (JSON highlighted), binary as hex. */
export function editablePaused(p: Pick<PausedMessage, 'kind' | 'payload'>): { mode: ComposerMode; input: string } {
  const bytes = base64ToBytes(p.payload);
  if (p.kind === 'text') {
    const text = utf8DecodeStrict(bytes) ?? '';
    return { mode: looksLikeJson(text) ? 'json' : 'text', input: text };
  }
  return { mode: 'hex', input: formatHex(bytes) };
}

/** Same bytes? */
function sameBytes(a: Uint8Array, b: Uint8Array): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

/**
 * The resume payload for forwarding a paused message after editing it in `mode`. Unchanged
 * content (same kind and bytes) forwards the original untouched; invalid input is an error.
 */
export function forwardResume(p: Pick<PausedMessage, 'kind' | 'payload'>, mode: ComposerMode, input: string): { ok: true; resume: MessageResume; changed: boolean } | { ok: false; error: string } {
  // JSON validity doesn't matter at a breakpoint: the user may want to send broken JSON.
  const encoded = encodeComposed(mode === 'json' ? 'text' : mode, input);
  if (!encoded.ok) return encoded;
  const body = encoded.body;
  const bytes = body.kind === 'text' ? utf8Encode(body.text) : base64ToBytes(body.base64);
  if (body.kind === p.kind && sameBytes(bytes, base64ToBytes(p.payload))) return { ok: true, resume: { action: 'forward' }, changed: false };
  return { ok: true, resume: { action: 'forward', ...body }, changed: true };
}

/** One-line preview of composed input for history rows. */
export function composedPreview(mode: ComposerMode, input: string, max = 120): string {
  const collapsed = input.replace(/\s+/g, ' ').trim();
  if (!collapsed) return mode === 'hex' || mode === 'base64' ? '(no bytes)' : '(empty)';
  return collapsed.length > max ? `${collapsed.slice(0, max)}…` : collapsed;
}
