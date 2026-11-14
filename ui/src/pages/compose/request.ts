/**
 * Draft ⇄ wire request:
 * - {@link autoHeaders}: headers Akev derives from the URL, body mode and auth (shown dimmed in
 *   the Headers tab; Content-Type/Authorization are sent, Host/Content-Length are informational —
 *   the daemon frames the request).
 * - {@link serializeBody}: body mode → bytes (JSON/Text/Form/Multipart/Binary).
 * - {@link buildRequest}: draft + variables → `EditableRequest` for `POST /api/send` / codegen.
 * - {@link draftFromEditable}: a captured/imported `EditableRequest` → a structured draft.
 */

import type { EditableRequest, Headers } from '../../api/types';
import { makeRow, type KeyValueRow } from '../../components/ui/KeyValueEditor';
import { base64ToBytes, bytesToBase64, textToBase64, utf8Decode, utf8DecodeStrict, utf8Encode, looksLikeText } from '../../lib/encoding';
import { formatJsonLossless } from '../../lib/json';
import { emptyDraft, newBoundary, TEXT_TYPES, type AuthState, type BodyState, type FileRef, type MultipartRow, type RequestDraft } from './model';
import { rowsFromUrl } from './urlParams';
import { substitute, type VarMap } from './variables';

// =====================================================================================
// Auto headers
// =====================================================================================

export interface AutoHeader {
  /** Lower-case key. */
  key: string;
  name: string;
  value: string;
  /** Explains where it comes from. */
  note: string;
  /** The user can switch it off (`draft.disabledAuto`). */
  toggleable: boolean;
  /** Included in the request Akev sends (vs. informational, set by the daemon). */
  sent: boolean;
  /** Switched off by the user. */
  disabled: boolean;
  /** An enabled user header with the same name replaces it. */
  overridden: boolean;
}

/** Content-Type implied by the body mode (null = none). */
export function bodyContentType(body: BodyState): string | null {
  switch (body.mode) {
    case 'json':
      return 'application/json';
    case 'text':
      return body.textType || 'text/plain';
    case 'form':
      return 'application/x-www-form-urlencoded';
    case 'multipart':
      return `multipart/form-data; boundary=${body.boundary}`;
    case 'binary':
      return body.binary ? body.binary.type || 'application/octet-stream' : null;
    default:
      return null;
  }
}

/** `Authorization` value implied by the auth settings (null = none). */
export function authHeaderValue(auth: AuthState, subst: (s: string) => string = (s) => s): string | null {
  if (auth.type === 'basic') {
    if (!auth.username && !auth.password) return null;
    return `Basic ${textToBase64(`${subst(auth.username)}:${subst(auth.password)}`)}`;
  }
  if (auth.type === 'bearer') {
    const token = subst(auth.token).trim();
    return token ? `Bearer ${token}` : null;
  }
  return null;
}

/** Host (with non-default port) of a URL, or null if it can't be parsed. */
export function urlHost(url: string): string | null {
  try {
    return new URL(normalizeUrl(url)).host || null;
  } catch {
    return null;
  }
}

/**
 * Auto headers for a draft. `bodySize` is the serialized body length (bytes) when known.
 * `subst` resolves variables (Authorization is built from resolved credentials).
 */
export function autoHeaders(draft: RequestDraft, opts: { bodySize?: number | null; resolvedUrl?: string; subst?: (s: string) => string } = {}): AutoHeader[] {
  const userNames = new Set(draft.headers.filter((h) => h.enabled && h.name.trim()).map((h) => h.name.trim().toLowerCase()));
  const disabled = new Set(draft.disabledAuto);
  const out: AutoHeader[] = [];
  const push = (key: string, name: string, value: string, note: string, toggleable: boolean, sent: boolean) =>
    out.push({ key, name, value, note, toggleable, sent, disabled: toggleable && disabled.has(key), overridden: userNames.has(key) });

  const host = urlHost(opts.resolvedUrl ?? draft.url);
  if (host) push('host', 'Host', host, 'From the URL', false, false);
  const auth = authHeaderValue(draft.auth, opts.subst);
  if (auth) push('authorization', 'Authorization', auth, draft.auth.type === 'basic' ? 'From Auth · Basic' : 'From Auth · Bearer token', false, true);
  const ct = bodyContentType(draft.body);
  if (ct) push('content-type', 'Content-Type', ct, 'From the body type', true, true);
  if (draft.body.mode !== 'none' && opts.bodySize != null) push('content-length', 'Content-Length', String(opts.bodySize), 'Calculated when sent', false, false);
  return out;
}

// =====================================================================================
// Body serialisation
// =====================================================================================

/** `application/x-www-form-urlencoded` component encoding (space → `+`). */
export function encodeFormComponent(s: string): string {
  return encodeURIComponent(s).replace(/%20/g, '+');
}

function concatBytes(parts: readonly Uint8Array[]): Uint8Array {
  let len = 0;
  for (const p of parts) len += p.length;
  const out = new Uint8Array(len);
  let off = 0;
  for (const p of parts) {
    out.set(p, off);
    off += p.length;
  }
  return out;
}

function quoteParam(s: string): string {
  return s.replace(/\\/g, '\\\\').replace(/"/g, '\\"').replace(/\r?\n/g, ' ');
}

/** Build a multipart/form-data body from enabled rows. */
export function buildMultipart(rows: readonly MultipartRow[], boundary: string, subst: (s: string) => string = (s) => s): Uint8Array {
  const parts: Uint8Array[] = [];
  for (const r of rows) {
    if (!r.enabled || (!r.name && !r.value && !r.file)) continue;
    const name = quoteParam(subst(r.name));
    if (r.kind === 'file') {
      const f = r.file;
      const filename = quoteParam(f?.name ?? 'file');
      parts.push(utf8Encode(`--${boundary}\r\nContent-Disposition: form-data; name="${name}"; filename="${filename}"\r\nContent-Type: ${f?.type || 'application/octet-stream'}\r\n\r\n`));
      if (f?.data) parts.push(base64ToBytes(f.data));
      parts.push(utf8Encode('\r\n'));
    } else {
      parts.push(utf8Encode(`--${boundary}\r\nContent-Disposition: form-data; name="${name}"\r\n\r\n${subst(r.value)}\r\n`));
    }
  }
  parts.push(utf8Encode(`--${boundary}--\r\n`));
  return concatBytes(parts);
}

export interface SerializedBody {
  /** Base64 for `EditableRequest.body`. */
  base64: string;
  /** Byte length. */
  size: number;
}

/** Serialize the active body mode (variables substituted in textual content). */
export function serializeBody(body: BodyState, subst: (s: string) => string = (s) => s): SerializedBody {
  switch (body.mode) {
    case 'json':
    case 'text': {
      const bytes = utf8Encode(subst(body.mode === 'json' ? body.json : body.text));
      return { base64: bytesToBase64(bytes), size: bytes.length };
    }
    case 'form': {
      const text = body.form
        .filter((r) => r.enabled && (r.name || r.value))
        .map((r) => `${encodeFormComponent(subst(r.name))}=${encodeFormComponent(subst(r.value))}`)
        .join('&');
      const bytes = utf8Encode(text);
      return { base64: bytesToBase64(bytes), size: bytes.length };
    }
    case 'multipart': {
      const bytes = buildMultipart(body.multipart, body.boundary, subst);
      return { base64: bytesToBase64(bytes), size: bytes.length };
    }
    case 'binary':
      return body.binary ? { base64: body.binary.data, size: body.binary.data ? body.binary.size : 0 } : { base64: '', size: 0 };
    default:
      return { base64: '', size: 0 };
  }
}

// =====================================================================================
// URL normalisation
// =====================================================================================

const LOCAL_HOST = /^(?:localhost|127(?:\.\d{1,3}){3}|0\.0\.0\.0|\[::1?\]|[\w-]+\.(?:localhost|local|test|internal)|(?:\d{1,3}\.){3}\d{1,3})(?::\d+)?(?:[/?#]|$)/i;

/** Add a scheme when missing: `http://` for local/IP hosts, `https://` otherwise. */
export function normalizeUrl(url: string): string {
  const u = url.trim();
  if (!u) return u;
  if (/^[a-z][a-z0-9+.-]*:\/\//i.test(u)) return u;
  if (u.startsWith('//')) return `https:${u}`;
  return `${LOCAL_HOST.test(u) ? 'http' : 'https'}://${u}`;
}

// =====================================================================================
// Draft → EditableRequest
// =====================================================================================

export interface BuiltRequest {
  request: EditableRequest;
  /** Resolved, normalised URL. */
  url: string;
  /** Unresolved `{{variables}}` across URL, headers, auth and body. */
  unresolved: string[];
  /** Serialized body size in bytes. */
  bodySize: number;
}

/** Build the wire request. Disabled/empty rows are skipped; user headers win over auto ones. */
export function buildRequest(draft: RequestDraft, vars: VarMap, generate?: (name: string) => string | undefined): BuiltRequest {
  const unresolved: string[] = [];
  const subst = (s: string): string => {
    const r = substitute(s, vars, generate);
    for (const n of r.unresolved) if (!unresolved.includes(n)) unresolved.push(n);
    return r.text;
  };
  const url = normalizeUrl(subst(draft.url));
  const body = serializeBody(draft.body, subst);
  const headers: Headers = [];
  for (const a of autoHeaders(draft, { subst, resolvedUrl: url })) {
    if (a.sent && !a.disabled && !a.overridden) headers.push([a.name, a.value]);
  }
  for (const h of draft.headers) {
    if (!h.enabled || !h.name.trim()) continue;
    headers.push([subst(h.name.trim()), subst(h.value)]);
  }
  return {
    request: { method: (draft.method.trim() || 'GET').toUpperCase(), url, headers, body: body.base64 },
    url,
    unresolved,
    bodySize: body.size,
  };
}

// =====================================================================================
// EditableRequest → draft
// =====================================================================================

function headerParam(value: string, param: string): string | null {
  const re = new RegExp(`(?:^|;)\\s*${param}\\s*=\\s*(?:"((?:[^"\\\\]|\\\\.)*)"|([^;]*))`, 'i');
  const m = re.exec(value);
  if (!m) return null;
  return (m[1] ?? m[2] ?? '').trim();
}

function bytesToLatin1(bytes: Uint8Array): string {
  let s = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) s += String.fromCharCode(...bytes.subarray(i, i + chunk));
  return s;
}

function latin1ToBytes(s: string): Uint8Array {
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i) & 0xff;
  return out;
}

/** Parse a multipart/form-data body into rows (null if it doesn't parse). */
export function parseMultipart(bytes: Uint8Array, boundary: string): MultipartRow[] | null {
  const text = bytesToLatin1(bytes);
  const delim = `--${boundary}`;
  const chunks = text.split(delim);
  if (chunks.length < 3) return null;
  const rows: MultipartRow[] = [];
  for (let i = 1; i < chunks.length; i++) {
    let chunk = chunks[i] as string;
    if (chunk.startsWith('--')) break;
    chunk = chunk.replace(/^\r?\n/, '').replace(/\r?\n$/, '');
    const sep = chunk.search(/\r?\n\r?\n/);
    if (sep < 0) return null;
    const head = chunk.slice(0, sep);
    const content = chunk.slice(sep).replace(/^\r?\n\r?\n/, '');
    const headers = head.split(/\r?\n/).map((l) => [l.slice(0, l.indexOf(':')).trim().toLowerCase(), l.slice(l.indexOf(':') + 1).trim()] as const);
    const disposition = headers.find(([k]) => k === 'content-disposition')?.[1] ?? '';
    const name = utf8Decode(latin1ToBytes(headerParam(disposition, 'name') ?? ''));
    const filename = headerParam(disposition, 'filename');
    const type = headers.find(([k]) => k === 'content-type')?.[1];
    const data = latin1ToBytes(content);
    if (filename != null) {
      rows.push({ ...makeRow(name, ''), kind: 'file', file: { name: utf8Decode(latin1ToBytes(filename)) || 'file', type: type || 'application/octet-stream', size: data.length, data: bytesToBase64(data) } });
    } else {
      rows.push({ ...makeRow(name, utf8Decode(data)), kind: 'text', file: null });
    }
  }
  return rows;
}

function parseFormRows(text: string): KeyValueRow[] {
  return text
    .split('&')
    .filter(Boolean)
    .map((seg) => {
      const eq = seg.indexOf('=');
      const dec = (s: string) => {
        try {
          return decodeURIComponent(s.replace(/\+/g, ' '));
        } catch {
          return s;
        }
      };
      return makeRow(dec(eq < 0 ? seg : seg.slice(0, eq)), dec(eq < 0 ? '' : seg.slice(eq + 1)));
    });
}

function isJsonType(ct: string): boolean {
  return ct === 'application/json' || ct.endsWith('+json') || ct === 'text/json';
}

function textTypeFor(ct: string): string | null {
  if (!ct) return null;
  if (TEXT_TYPES.some((t) => t.value === ct)) return ct;
  if (ct.startsWith('text/') || ct.endsWith('+xml') || ct === 'application/x-javascript' || ct === 'text/javascript') return ct;
  return null;
}

/** File name for a captured binary body (`/upload/avatar.png` → `avatar.png`). */
function fileNameFromUrl(url: string, type: string): string {
  try {
    const last = new URL(url).pathname.split('/').filter(Boolean).pop();
    if (last && /\.[a-z0-9]{1,6}$/i.test(last)) return decodeURIComponent(last);
  } catch {
    // ignore
  }
  const ext = type.split('/')[1]?.split(/[;+]/)[0];
  return `body${ext && /^[a-z0-9.-]{1,12}$/i.test(ext) && ext !== 'octet-stream' ? `.${ext}` : '.bin'}`;
}

/**
 * Structure a captured/imported request into a draft: query → Params, body → the matching body
 * mode (Content-Type kept as a user header only when it carries more than the mode implies),
 * `Authorization: Basic/Bearer` → Auth, HTTP/2 pseudo-headers and framing headers dropped.
 */
export function draftFromEditable(req: EditableRequest): RequestDraft {
  const draft = emptyDraft();
  draft.method = (req.method || 'GET').toUpperCase();
  draft.url = req.url;
  draft.params = rowsFromUrl(req.url);

  const bytes = req.body ? base64ToBytes(req.body) : new Uint8Array();
  const headers = req.headers.filter(([name]) => !name.startsWith(':'));
  const lower = (n: string) => n.toLowerCase();
  const ctFull = headers.find(([n]) => lower(n) === 'content-type')?.[1] ?? '';
  const ct = ctFull.split(';')[0]?.trim().toLowerCase() ?? '';
  let keepContentType = false;

  if (bytes.length > 0) {
    const text = utf8DecodeStrict(bytes);
    if (isJsonType(ct) && text != null) {
      draft.body.mode = 'json';
      draft.body.json = text;
      keepContentType = ct !== 'application/json' || ctFull.includes(';');
    } else if (ct === 'application/x-www-form-urlencoded' && text != null) {
      draft.body.mode = 'form';
      draft.body.form = parseFormRows(text);
      keepContentType = ctFull.includes(';');
    } else if (ct === 'multipart/form-data') {
      const boundary = headerParam(ctFull, 'boundary');
      const rows = boundary ? parseMultipart(bytes, boundary) : null;
      if (rows) {
        draft.body.mode = 'multipart';
        draft.body.multipart = rows;
        draft.body.boundary = newBoundary();
      } else {
        draft.body.mode = 'binary';
        draft.body.binary = { name: 'body.bin', type: ctFull, size: bytes.length, data: bytesToBase64(bytes) };
      }
    } else if (text != null && (textTypeFor(ct) || (!ct && looksLikeText(bytes)))) {
      // JSON without a content type is still JSON.
      const trimmed = text.trim();
      if (!ct && (trimmed.startsWith('{') || trimmed.startsWith('['))) {
        try {
          JSON.parse(trimmed);
          draft.body.mode = 'json';
          draft.body.json = text;
        } catch {
          draft.body.mode = 'text';
          draft.body.text = text;
        }
      } else {
        draft.body.mode = 'text';
        draft.body.text = text;
        draft.body.textType = textTypeFor(ct) ?? 'text/plain';
      }
      keepContentType = !!ct && (ctFull.includes(';') || !TEXT_TYPES.some((t) => t.value === ct));
      if (!ct) draft.disabledAuto = ['content-type'];
    } else {
      draft.body.mode = 'binary';
      const type = ct || 'application/octet-stream';
      draft.body.binary = { name: fileNameFromUrl(req.url, type), type, size: bytes.length, data: bytesToBase64(bytes) };
      keepContentType = ctFull.includes(';');
      if (!ct) draft.disabledAuto = ['content-type'];
    }
  } else if (ct) {
    keepContentType = true;
  }

  // Authorization → Auth tab (only one, recognised scheme).
  const auths = headers.filter(([n]) => lower(n) === 'authorization');
  let authMoved = false;
  if (auths.length === 1) {
    const value = (auths[0] as [string, string])[1].trim();
    const basic = /^basic\s+(\S+)$/i.exec(value);
    const bearer = /^bearer\s+(.+)$/i.exec(value);
    if (basic) {
      let decoded: string | null = null;
      try {
        decoded = utf8DecodeStrict(base64ToBytes(basic[1] as string));
      } catch {
        decoded = null;
      }
      if (decoded != null) {
        const i = decoded.indexOf(':');
        draft.auth = { type: 'basic', username: i < 0 ? decoded : decoded.slice(0, i), password: i < 0 ? '' : decoded.slice(i + 1), token: '' };
        authMoved = true;
      }
    } else if (bearer) {
      draft.auth = { type: 'bearer', username: '', password: '', token: (bearer[1] as string).trim() };
      authMoved = true;
    }
  }

  const host = urlHost(req.url);
  draft.headers = headers
    .filter(([name, value]) => {
      const n = lower(name);
      if (n === 'content-length' || n === 'transfer-encoding') return false;
      if (n === 'host') return !host || value.toLowerCase() !== host.toLowerCase();
      if (n === 'content-type') return keepContentType;
      if (n === 'authorization') return !authMoved;
      return true;
    })
    .map(([n, v]) => makeRow(n, v));
  return draft;
}

/** Base64 of a file picked by the user. */
export function fileRef(name: string, data: Uint8Array, type?: string): FileRef {
  return { name, type: type || guessMime(name), size: data.length, data: bytesToBase64(data) };
}

const MIME_BY_EXT: Readonly<Record<string, string>> = {
  json: 'application/json',
  txt: 'text/plain',
  csv: 'text/csv',
  xml: 'application/xml',
  html: 'text/html',
  htm: 'text/html',
  js: 'application/javascript',
  css: 'text/css',
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  avif: 'image/avif',
  svg: 'image/svg+xml',
  ico: 'image/x-icon',
  pdf: 'application/pdf',
  zip: 'application/zip',
  gz: 'application/gzip',
  wasm: 'application/wasm',
  mp3: 'audio/mpeg',
  mp4: 'video/mp4',
  webm: 'video/webm',
  yaml: 'application/yaml',
  yml: 'application/yaml',
  proto: 'text/plain',
  bin: 'application/octet-stream',
};

/** MIME type from a file name (fallback `application/octet-stream`). */
export function guessMime(name: string): string {
  const ext = name.split('.').pop()?.toLowerCase() ?? '';
  return MIME_BY_EXT[ext] ?? 'application/octet-stream';
}

// =====================================================================================
// JSON validation (variables allowed)
// =====================================================================================

export type JsonCheck = { ok: true } | { ok: false; message: string; line: number; column: number };

/**
 * Validate JSON where `{{vars}}` may appear outside strings (`"id": {{id}}`): each such variable
 * is replaced by `null` padded to the same length, so error positions stay accurate.
 */
export function checkJson(text: string): JsonCheck {
  if (!text.trim()) return { ok: true };
  let masked = '';
  let inString = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i] as string;
    if (inString) {
      masked += ch;
      if (ch === '\\') {
        masked += text[i + 1] ?? '';
        i++;
      } else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      masked += ch;
      continue;
    }
    if (ch === '{' && text[i + 1] === '{') {
      const end = text.indexOf('}}', i + 2);
      if (end > 0 && !text.slice(i + 2, end).includes('{')) {
        const len = end + 2 - i;
        masked += 'null'.padEnd(len, ' ');
        i = end + 1;
        continue;
      }
    }
    masked += ch;
  }
  try {
    JSON.parse(masked);
    return { ok: true };
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    const posMatch = /position (\d+)/i.exec(message);
    const lcMatch = /line (\d+) column (\d+)/i.exec(message);
    let line = 1;
    let column = 1;
    if (lcMatch) {
      line = Number(lcMatch[1]);
      column = Number(lcMatch[2]);
    } else if (posMatch) {
      const pos = Math.min(Number(posMatch[1]), text.length);
      const before = text.slice(0, pos);
      line = before.split('\n').length;
      column = pos - before.lastIndexOf('\n');
    } else {
      const lines = text.split('\n');
      line = lines.length;
      column = (lines[lines.length - 1]?.length ?? 0) + 1;
    }
    const clean = message
      .replace(/^JSON\.parse:\s*/i, '')
      .replace(/\s*in JSON at position \d+(?: \(line \d+ column \d+\))?/i, '')
      .replace(/\s*at line \d+ column \d+ of the JSON data/i, '')
      .replace(/^Unexpected end of JSON input$/i, 'Unexpected end of input');
    return { ok: false, message: clean, line, column };
  }
}

/** Pretty-print JSON (2 spaces), keeping `{{vars}}` outside strings intact. Null if invalid. */
export function formatJson(text: string): string | null {
  if (!text.trim()) return text;
  const tokens: string[] = [];
  // Replace bare variables by unique string placeholders, format, then restore.
  let masked = '';
  let inString = false;
  for (let i = 0; i < text.length; i++) {
    const ch = text[i] as string;
    if (inString) {
      masked += ch;
      if (ch === '\\') {
        masked += text[i + 1] ?? '';
        i++;
      } else if (ch === '"') inString = false;
      continue;
    }
    if (ch === '"') {
      inString = true;
      masked += ch;
      continue;
    }
    if (ch === '{' && text[i + 1] === '{') {
      const end = text.indexOf('}}', i + 2);
      if (end > 0 && !text.slice(i + 2, end).includes('{')) {
        tokens.push(text.slice(i, end + 2));
        masked += `"__akev_var_${tokens.length - 1}__"`;
        i = end + 1;
        continue;
      }
    }
    masked += ch;
  }
  try {
    // Lossless: formatting must never change the numbers (64-bit ids) the user is sending.
    const pretty = formatJsonLossless(masked);
    return pretty.replace(/"__akev_var_(\d+)__"/g, (_, n: string) => tokens[Number(n)] ?? '');
  } catch {
    return null;
  }
}
