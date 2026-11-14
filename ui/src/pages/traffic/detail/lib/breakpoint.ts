/**
 * Pure helpers for breakpoint editing (DESIGN.md §6.6): decoding the editable body, keeping
 * `Content-Length` in sync, status-message defaults, validation and building resume payloads.
 */

import type { EditableRequest, EditableResponse, Headers } from '../../../../api/types';
import { base64ToBytes, bytesToBase64, textToBase64, utf8DecodeStrict, utf8Length } from '../../../../lib/encoding';
import { statusText } from '../../../../lib/http';
import { formatJsonLossless } from '../../../../lib/json';

// =====================================================================================
// Body
// =====================================================================================

export type BodyMode = 'text' | 'base64';

export interface DecodedBody {
  /** UTF-8 text, or `null` when the bytes aren't valid UTF-8. */
  text: string | null;
  /** Byte length. */
  size: number;
  /** Original base64 (normalised). */
  base64: string;
}

/** Decode a base64 editable body (EditableRequest/Response `body`). */
export function decodeEditableBody(b64: string): DecodedBody {
  let bytes: Uint8Array;
  try {
    bytes = base64ToBytes(b64 || '');
  } catch {
    bytes = new Uint8Array();
  }
  return { text: utf8DecodeStrict(bytes), size: bytes.length, base64: bytesToBase64(bytes) };
}

export type EncodedBody = { ok: true; base64: string; size: number } | { ok: false; error: string };

/** Encode the editor's content back to base64 (text is UTF-8; base64 mode is validated). */
export function encodeEditableBody(mode: BodyMode, value: string): EncodedBody {
  if (mode === 'text') return { ok: true, base64: value ? textToBase64(value) : '', size: utf8Length(value) };
  const clean = value.replace(/\s+/g, '');
  if (clean === '') return { ok: true, base64: '', size: 0 };
  if (!/^[A-Za-z0-9+/]*={0,2}$/.test(clean) || clean.length % 4 === 1) {
    return { ok: false, error: 'Not valid base64' };
  }
  try {
    const bytes = base64ToBytes(clean);
    return { ok: true, base64: bytesToBase64(bytes), size: bytes.length };
  } catch {
    return { ok: false, error: 'Not valid base64' };
  }
}

/** Byte size of the editor content without encoding it (cheap for text). */
export function editableBodySize(mode: BodyMode, value: string): number | null {
  if (mode === 'text') return utf8Length(value);
  const r = encodeEditableBody('base64', value);
  return r.ok ? r.size : null;
}

// =====================================================================================
// Headers
// =====================================================================================

/** Minimal row shape shared with `KeyValueEditor` rows. */
export interface HeaderRowLike {
  name: string;
  value: string;
  enabled?: boolean;
}

/** RFC 9110 token characters. */
const HEADER_NAME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;

/** Whether `name` is a valid HTTP header field name. */
export function isValidHeaderName(name: string): boolean {
  return HEADER_NAME_RE.test(name);
}

/** Names of enabled rows that are not valid header names (for inline errors). */
export function invalidHeaderNames(rows: readonly HeaderRowLike[]): string[] {
  const out: string[] = [];
  for (const r of rows) {
    const n = r.name.trim();
    if (r.enabled === false || n === '') continue;
    if (!isValidHeaderName(n)) out.push(n);
  }
  return out;
}

/**
 * Keep `Content-Length` tracking the body: rows whose value equals the previous body size are
 * updated to the next size (a hand-edited value is left alone). Returns the same array when
 * nothing changed.
 */
export function syncContentLength<T extends HeaderRowLike>(rows: readonly T[], prevSize: number, nextSize: number): T[] {
  if (prevSize === nextSize) return rows as T[];
  let changed = false;
  const next = rows.map((r) => {
    if (r.name.trim().toLowerCase() === 'content-length' && r.value.trim() === String(prevSize)) {
      changed = true;
      return { ...r, value: String(nextSize) };
    }
    return r;
  });
  return changed ? next : (rows as T[]);
}

/** `host[:port]` of an absolute URL as a Host header would carry it (default ports omitted). */
export function urlAuthority(url: string): string | null {
  try {
    const u = new URL(url.trim());
    return u.host || null;
  } catch {
    return null;
  }
}

/**
 * Keep `Host` following the URL (like the daemon does on resume): rows whose value equals the
 * previous URL's authority are updated to the new one; a hand-edited Host is left alone, and an
 * unparseable URL changes nothing. Returns the same array when nothing changed.
 */
export function syncHostHeader<T extends HeaderRowLike>(rows: readonly T[], prevUrl: string, nextUrl: string): T[] {
  const prev = urlAuthority(prevUrl);
  const next = urlAuthority(nextUrl);
  if (!prev || !next || prev.toLowerCase() === next.toLowerCase()) return rows as T[];
  let changed = false;
  const out = rows.map((r) => {
    if (r.name.trim().toLowerCase() === 'host' && r.value.trim().toLowerCase() === prev.toLowerCase()) {
      changed = true;
      return { ...r, value: next };
    }
    return r;
  });
  return changed ? out : (rows as T[]);
}

/** Case-insensitive header lookup on rows (enabled only). */
export function rowHeader(rows: readonly HeaderRowLike[], name: string): string | undefined {
  const lower = name.toLowerCase();
  for (const r of rows) if (r.enabled !== false && r.name.trim().toLowerCase() === lower) return r.value;
  return undefined;
}

/** Headers equal (order, case and values). */
export function headersEqual(a: Headers, b: Headers): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) {
    const x = a[i] as [string, string];
    const y = b[i] as [string, string];
    if (x[0] !== y[0] || x[1] !== y[1]) return false;
  }
  return true;
}

// =====================================================================================
// Request / response
// =====================================================================================

/** `null` when valid, else a short error for the URL field. */
export function validateUrl(url: string): string | null {
  const u = url.trim();
  if (!u) return 'URL is required';
  try {
    const parsed = new URL(u);
    if (!/^(https?|wss?):$/.test(parsed.protocol)) return 'Use an http(s) or ws(s) URL';
    return null;
  } catch {
    return 'Not a valid absolute URL';
  }
}

/** `null` when valid, else a short error for the method field. */
export function validateMethod(method: string): string | null {
  const m = method.trim();
  if (!m) return 'Method is required';
  if (!HEADER_NAME_RE.test(m)) return 'Invalid method';
  return null;
}

/** `null` when valid, else a short error for the status field. */
export function validateStatus(status: number | null): string | null {
  if (status == null || !Number.isFinite(status)) return 'Status is required';
  if (!Number.isInteger(status) || status < 100 || status > 999) return '100–999';
  return null;
}

/**
 * Message to show after the status code changes: follows the canonical reason while the user
 * hasn't customised it, otherwise keeps their text.
 */
export function nextStatusMessage(prevStatus: number | null, prevMessage: string, nextStatus: number | null): string {
  const prevDefault = prevStatus == null ? '' : statusText(prevStatus);
  if (prevMessage.trim() === '' || prevMessage === prevDefault) return nextStatus == null ? '' : statusText(nextStatus);
  return prevMessage;
}

/** Whether a request draft differs from the paused original. */
export function requestChanged(original: EditableRequest, draft: EditableRequest): boolean {
  return (
    original.method !== draft.method ||
    original.url !== draft.url ||
    !headersEqual(original.headers, draft.headers) ||
    normaliseB64(original.body) !== normaliseB64(draft.body)
  );
}

/** Whether a response draft differs from the paused original. */
export function responseChanged(original: EditableResponse, draft: EditableResponse): boolean {
  return (
    original.status !== draft.status ||
    original.statusMessage !== draft.statusMessage ||
    !headersEqual(original.headers, draft.headers) ||
    normaliseB64(original.body) !== normaliseB64(draft.body)
  );
}

function normaliseB64(b64: string): string {
  return (b64 || '').replace(/\s+/g, '');
}

// =====================================================================================
// Editor language
// =====================================================================================

/** CodeMirror language for a Content-Type (`application/json; charset=utf-8` → `json`). */
export function languageForContentType(contentType: string | undefined): string {
  const ct = (contentType ?? '').split(';')[0]?.trim().toLowerCase() ?? '';
  if (!ct) return 'text';
  if (ct === 'application/json' || ct.endsWith('+json') || ct === 'application/graphql-response+json') return 'json';
  if (ct === 'application/x-ndjson' || ct === 'application/jsonl') return 'json';
  if (ct.includes('html')) return 'html';
  if (ct.endsWith('/xml') || ct.endsWith('+xml')) return 'xml';
  if (ct.includes('javascript') || ct.includes('ecmascript')) return 'javascript';
  if (ct === 'text/css') return 'css';
  if (ct.includes('yaml')) return 'yaml';
  if (ct === 'text/markdown') return 'markdown';
  return 'text';
}

/** Pretty-print JSON text (`null` if it doesn't parse) — losslessly, since the result is sent on. */
export function formatJsonText(text: string): string | null {
  try {
    return formatJsonLossless(text);
  } catch {
    return null;
  }
}

// =====================================================================================
// "Respond…" presets
// =====================================================================================

export interface RespondPreset {
  id: string;
  /** Chip label, e.g. `404`. */
  label: string;
  /** Tooltip / menu description. */
  description: string;
  status: number;
  headers: Headers;
  /** Body text (UTF-8). */
  body: string;
}

const json = (value: unknown): string => `${JSON.stringify(value, null, 2)}\n`;
const JSON_CT: [string, string] = ['content-type', 'application/json'];

/** Common answers for a paused request (Respond… dialog quick start). */
export const RESPOND_PRESETS: readonly RespondPreset[] = [
  { id: 'ok-json', label: '200 JSON', description: 'Success with a JSON body', status: 200, headers: [JSON_CT], body: json({ ok: true }) },
  { id: 'created', label: '201', description: 'Created', status: 201, headers: [JSON_CT, ['location', '/']], body: json({ id: 1 }) },
  { id: 'no-content', label: '204', description: 'No Content — success without a body', status: 204, headers: [], body: '' },
  { id: 'redirect', label: '302', description: 'Redirect (edit the Location header)', status: 302, headers: [['location', 'https://example.com/']], body: '' },
  { id: 'bad-request', label: '400', description: 'Bad Request', status: 400, headers: [JSON_CT], body: json({ error: 'bad_request', message: 'The request is invalid.' }) },
  { id: 'unauthorized', label: '401', description: 'Unauthorized', status: 401, headers: [JSON_CT, ['www-authenticate', 'Bearer']], body: json({ error: 'unauthorized' }) },
  { id: 'not-found', label: '404', description: 'Not Found', status: 404, headers: [JSON_CT], body: json({ error: 'not_found' }) },
  { id: 'rate-limited', label: '429', description: 'Too Many Requests (with Retry-After)', status: 429, headers: [JSON_CT, ['retry-after', '30']], body: json({ error: 'rate_limited', retryAfter: 30 }) },
  { id: 'server-error', label: '500', description: 'Internal Server Error', status: 500, headers: [JSON_CT], body: json({ error: 'internal_error' }) },
  { id: 'unavailable', label: '503', description: 'Service Unavailable (with Retry-After)', status: 503, headers: [JSON_CT, ['retry-after', '60']], body: json({ error: 'unavailable' }) },
];

/** The preset as an {@link EditableResponse} (canonical reason, Content-Length for the body). */
export function presetResponse(preset: RespondPreset): EditableResponse {
  const headers: Headers = [...preset.headers];
  if (preset.status !== 204 && preset.status !== 304) headers.push(['content-length', String(utf8Length(preset.body))]);
  return { status: preset.status, statusMessage: statusText(preset.status), headers, body: preset.body ? textToBase64(preset.body) : '' };
}
