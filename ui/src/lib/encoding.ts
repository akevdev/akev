/**
 * Byte/text encoding helpers.
 *
 * The daemon speaks Rust byte offsets (UTF-8) in a few places — filter error spans and
 * autocomplete ranges (`/api/filter/*`, `filter-result.error`). JavaScript strings are UTF-16,
 * so always convert with {@link byteOffsetToIndex} / {@link indexToByteOffset} before slicing.
 */

const encoder = new TextEncoder();
const decoder = new TextDecoder('utf-8', { fatal: false });
const strictDecoder = new TextDecoder('utf-8', { fatal: true });

/** UTF-8 encode a string. */
export function utf8Encode(text: string): Uint8Array {
  return encoder.encode(text);
}

/** UTF-8 decode bytes (invalid sequences become U+FFFD). */
export function utf8Decode(bytes: Uint8Array): string {
  return decoder.decode(bytes);
}

/** UTF-8 decode, returning `null` if the bytes are not valid UTF-8. */
export function utf8DecodeStrict(bytes: Uint8Array): string | null {
  try {
    return strictDecoder.decode(bytes);
  } catch {
    return null;
  }
}

/** Number of UTF-8 bytes needed to encode `text` (no allocation). */
export function utf8Length(text: string): number {
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    if (c < 0x80) bytes += 1;
    else if (c < 0x800) bytes += 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        bytes += 4;
        i++;
      } else bytes += 3;
    } else bytes += 3;
  }
  return bytes;
}

/**
 * Convert a UTF-8 byte offset (as reported by the daemon) into a UTF-16 string index.
 * Offsets past the end clamp to `text.length`; offsets inside a multi-byte character snap to
 * the start of that character.
 */
export function byteOffsetToIndex(text: string, byteOffset: number): number {
  if (byteOffset <= 0) return 0;
  let bytes = 0;
  for (let i = 0; i < text.length; i++) {
    const c = text.charCodeAt(i);
    let width: number;
    let units = 1;
    if (c < 0x80) width = 1;
    else if (c < 0x800) width = 2;
    else if (c >= 0xd800 && c <= 0xdbff && i + 1 < text.length) {
      const next = text.charCodeAt(i + 1);
      if (next >= 0xdc00 && next <= 0xdfff) {
        width = 4;
        units = 2;
      } else width = 3;
    } else width = 3;
    if (bytes + width > byteOffset) return i;
    bytes += width;
    i += units - 1;
    if (bytes === byteOffset) return i + 1;
  }
  return text.length;
}

/** Convert a UTF-16 string index into a UTF-8 byte offset (for sending cursors to the daemon). */
export function indexToByteOffset(text: string, index: number): number {
  return utf8Length(text.slice(0, Math.max(0, Math.min(index, text.length))));
}

/** Decode standard base64 (as used by `EditableRequest.body`) into bytes. */
export function base64ToBytes(b64: string): Uint8Array {
  const clean = b64.replace(/[\s]/g, '');
  const native = (Uint8Array as unknown as { fromBase64?: (s: string) => Uint8Array }).fromBase64;
  if (typeof native === 'function') return native.call(Uint8Array, clean);
  const bin = atob(clean);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/** Encode bytes as standard base64. */
export function bytesToBase64(bytes: Uint8Array): string {
  const native = (bytes as unknown as { toBase64?: () => string }).toBase64;
  if (typeof native === 'function') return native.call(bytes);
  let bin = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    bin += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(bin);
}

/** Base64-encode a UTF-8 string. */
export function textToBase64(text: string): string {
  return bytesToBase64(utf8Encode(text));
}

/** Decode a base64 payload to text; `null` if it is not valid UTF-8. */
export function base64ToText(b64: string): string | null {
  return utf8DecodeStrict(base64ToBytes(b64));
}

/** Lowercase hex string of bytes, optionally separated (`"de ad be ef"`). */
export function bytesToHex(bytes: Uint8Array, separator = ''): string {
  let out = '';
  for (let i = 0; i < bytes.length; i++) {
    if (i > 0 && separator) out += separator;
    out += HEX[bytes[i] ?? 0];
  }
  return out;
}

const HEX: readonly string[] = Array.from({ length: 256 }, (_, i) => i.toString(16).padStart(2, '0'));

/** Two-character lowercase hex for a byte (lookup table, no allocation). */
export function byteHex(b: number): string {
  return HEX[b & 0xff] ?? '00';
}

/** Heuristic: does this byte array look like (mostly) printable text? */
export function looksLikeText(bytes: Uint8Array, sample = 2048): boolean {
  const n = Math.min(bytes.length, sample);
  if (n === 0) return true;
  let control = 0;
  for (let i = 0; i < n; i++) {
    const b = bytes[i] ?? 0;
    if (b === 0) return false;
    if (b < 0x09 || (b > 0x0d && b < 0x20)) control++;
  }
  return control / n < 0.05;
}
