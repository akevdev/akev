/**
 * Pure body-viewing logic for BodyViewer: which view modes a body supports, what "Auto"
 * resolves to, info text, download filenames, client-side formatting fallbacks and the
 * structured payload accessors. No React.
 */

import type { BodyView, ContentKind, ExchangeSummary, GrpcMessage, Headers, ImageInfo, MultipartPart, Side, SseEvent } from '../../../api/types';
import { formatBytes, formatNumber } from '../../../lib/format';
import { parseJsonLossless, stringifyJson } from '../../../lib/json';
import { getHeader, languageForKind } from '../../../lib/http';
import { parseQueryString } from '../../../lib/query';

export type BodyMode = 'auto' | 'pretty' | 'raw' | 'tree' | 'preview' | 'decoded' | 'parts' | 'events' | 'form' | 'hex';
export type ConcreteMode = Exclude<BodyMode, 'auto'>;
export type PreviewKind = 'html' | 'markdown' | 'image' | 'svg' | 'pdf' | 'audio' | 'video' | 'font' | 'csv';

export const MODE_LABELS: Readonly<Record<BodyMode, string>> = {
  auto: 'Auto',
  pretty: 'Pretty',
  raw: 'Raw',
  tree: 'Tree',
  preview: 'Preview',
  decoded: 'Decoded',
  parts: 'Parts',
  events: 'Events',
  form: 'Form',
  hex: 'Hex',
};

export const MODE_TOOLTIPS: Readonly<Record<BodyMode, string>> = {
  auto: 'Best view for this content',
  pretty: 'Formatted & highlighted',
  raw: 'Text exactly as received',
  tree: 'Collapsible JSON tree',
  preview: 'Rendered preview',
  decoded: 'Decoded binary payload',
  parts: 'Multipart parts',
  events: 'Server-sent events',
  form: 'Form fields',
  hex: 'Raw bytes',
};

/** Bodies above this many characters open unformatted (Raw, no highlighting) by default. */
export const LARGE_TEXT = 2 * 1024 * 1024;
/** Above this, Tree mode asks before parsing. */
export const HUGE_JSON = 8 * 1024 * 1024;

export interface BodyModel {
  /** Effective content kind (the daemon's, refined from the Content-Type when unknown). */
  kind: ContentKind;
  preview: PreviewKind | null;
  /** Concrete modes offered, in display order (Auto is always offered in addition). */
  modes: ConcreteMode[];
  /** What Auto resolves to. */
  auto: ConcreteMode;
  /** Text above {@link LARGE_TEXT}: no formatting/highlighting unless asked. */
  large: boolean;
  /** Decoded text (may be truncated for transport). */
  text: string | null;
  /** Content-encoding decoding failed. */
  decodeFailed: boolean;
  /** CodeMirror language for text modes. */
  language: string;
  /** Key under which the chosen mode is remembered (per content kind). */
  prefKey: string;
}

/** Essence of a Content-Type value (`application/json`). */
export function essence(contentType: string | null | undefined): string {
  return (contentType ?? '').split(';')[0]?.trim().toLowerCase() ?? '';
}

/** Best-effort ContentKind from a Content-Type value (mirrors akev-content's detection). */
export function kindFromContentType(contentType: string | null | undefined): ContentKind {
  const ct = essence(contentType);
  if (!ct) return 'unknown';
  const [type = '', sub = ''] = ct.split('/');
  const base = sub.split('+');
  const suffix = base.length > 1 ? base[base.length - 1] : '';
  if (ct === 'application/grpc-web' || ct.startsWith('application/grpc-web')) return 'grpc-web';
  if (ct.startsWith('application/grpc')) return 'grpc';
  if (/^application\/(x-)?(protobuf|vnd\.google\.protobuf|x-google-protobuf|proto|x-protobuffer)$/.test(ct)) return 'protobuf';
  if (/(^|\/)(x-)?(ndjson|jsonl|jsonlines|json-lines|json-seq|x-jsonlines)$/.test(ct)) return 'json-lines';
  if (ct === 'application/graphql-response+json' || ct === 'application/graphql') return 'graph-ql';
  if (sub === 'json' || suffix === 'json' || ct === 'text/json') return 'json';
  if (ct === 'text/event-stream') return 'event-stream';
  if (ct === 'application/x-www-form-urlencoded') return 'form-url-encoded';
  if (type === 'multipart') return 'multipart';
  if (ct === 'text/html' || ct === 'application/xhtml+xml') return 'html';
  if (type === 'image') return 'image';
  if (sub === 'xml' || suffix === 'xml') return 'xml';
  if (/javascript|ecmascript/.test(sub)) return 'javascript';
  if (ct === 'text/css') return 'css';
  if (ct === 'text/markdown' || ct === 'text/x-markdown') return 'markdown';
  if (/yaml/.test(sub)) return 'yaml';
  if (ct === 'text/csv') return 'csv';
  if (type === 'audio') return 'audio';
  if (type === 'video') return 'video';
  if (type === 'font' || /font-(woff2?|ttf|otf)|x-font/.test(sub)) return 'font';
  if (ct === 'application/pdf') return 'pdf';
  if (ct === 'application/wasm') return 'wasm';
  if (/msgpack/.test(sub)) return 'msg-pack';
  if (sub === 'cbor') return 'cbor';
  if (type === 'text') return 'text';
  if (ct === 'application/octet-stream') return 'binary';
  return 'unknown';
}

function previewFor(kind: ContentKind, contentType: string): PreviewKind | null {
  switch (kind) {
    case 'html':
      return 'html';
    case 'markdown':
      return 'markdown';
    case 'image':
      return contentType === 'image/svg+xml' ? 'svg' : 'image';
    case 'pdf':
      return 'pdf';
    case 'audio':
      return 'audio';
    case 'video':
      return 'video';
    case 'font':
      return 'font';
    case 'csv':
      return 'csv';
    default:
      return null;
  }
}

/**
 * Work out the view modes for a body. `canFetchBytes` = raw bytes are reachable (exchange raw
 * endpoint or a `rawUrl`); without it Hex falls back to the UTF-8 bytes of the text.
 */
export function analyzeBody(view: BodyView, contentType: string | null | undefined, canFetchBytes: boolean): BodyModel {
  const ct = essence(contentType);
  const fromCt = kindFromContentType(ct);
  let kind: ContentKind = view.kind;
  if ((kind === 'unknown' || kind === 'binary' || kind === 'text') && fromCt !== 'unknown' && fromCt !== kind) {
    // Only refine towards something more specific; never downgrade a sniffed kind.
    if (!(kind === 'text' && fromCt === 'binary')) kind = fromCt;
  }
  const isSvg = kind === 'image' && (ct === 'image/svg+xml' || (view.structured as ImageInfo | null)?.format === 'svg');
  const text = view.text;
  const hasText = text != null;
  const large = (text?.length ?? 0) > LARGE_TEXT;
  const decodeFailed = view.decodeError != null;
  const hexOk = canFetchBytes || hasText;
  const prefKey = isSvg ? 'svg' : kind;
  const language = isSvg ? 'xml' : (languageForKind(kind) ?? 'text');

  const modes: ConcreteMode[] = [];
  let auto: ConcreteMode = 'raw';
  const add = (...ms: ConcreteMode[]) => {
    for (const m of ms) if (!modes.includes(m)) modes.push(m);
  };

  if (decodeFailed) {
    if (hasText) add('raw');
    if (hexOk) add('hex');
    return { kind, preview: null, modes, auto: hexOk ? 'hex' : 'raw', large, text, decodeFailed, language, prefKey: `${prefKey}:failed` };
  }

  const preview: PreviewKind | null = isSvg ? 'svg' : previewFor(kind, ct);
  const structured = view.structured;

  switch (kind) {
    case 'json':
    case 'graph-ql':
      add('pretty', 'raw', 'tree');
      auto = large ? 'raw' : 'pretty';
      break;
    case 'json-lines':
      add('pretty', 'raw', 'tree');
      auto = large ? 'raw' : 'pretty';
      break;
    case 'xml':
    case 'html':
    case 'javascript':
    case 'css':
      if (view.formatted != null) add('pretty');
      add('raw');
      if (kind === 'html' && hasText) add('preview');
      auto = view.formatted != null && !large ? 'pretty' : 'raw';
      break;
    case 'markdown':
      add('preview', 'raw');
      auto = large ? 'raw' : 'preview';
      break;
    case 'csv':
      add('preview', 'raw');
      auto = large ? 'raw' : 'preview';
      break;
    case 'form-url-encoded':
      add('form', 'raw');
      auto = 'form';
      break;
    case 'multipart':
      add('parts');
      if (hasText) add('raw');
      auto = 'parts';
      break;
    case 'event-stream':
      add('events', 'raw');
      auto = 'events';
      break;
    case 'image':
      add('preview');
      if (isSvg && hasText) add('raw');
      auto = 'preview';
      break;
    case 'audio':
    case 'video':
    case 'pdf':
    case 'font':
      add('preview');
      auto = 'preview';
      break;
    case 'protobuf':
    case 'grpc':
    case 'grpc-web':
    case 'msg-pack':
    case 'cbor': {
      const decodable = structured != null || view.schema?.typed != null;
      if (decodable) add('decoded');
      if (hasText) add('raw');
      auto = decodable ? 'decoded' : hexOk ? 'hex' : 'raw';
      break;
    }
    default:
      if (view.formatted != null) add('pretty');
      if (hasText) add('raw');
      auto = view.formatted != null && !large ? 'pretty' : hasText ? 'raw' : 'hex';
  }
  // A protobuf schema matched (mapping / Content-Type) but didn't decode the body: the Decoded
  // view explains why.
  if (view.schema && !modes.includes('decoded')) add('decoded');
  if (hexOk) add('hex');
  // Text-less previews (media without a reachable URL) can't render.
  if (!canFetchBytes && preview && preview !== 'html' && preview !== 'markdown' && preview !== 'csv' && !(preview === 'svg' && hasText)) {
    const i = modes.indexOf('preview');
    if (i >= 0) modes.splice(i, 1);
    if (auto === 'preview') auto = modes[0] ?? 'raw';
  }
  if (!modes.includes(auto)) auto = modes[0] ?? 'raw';
  return { kind, preview, modes, auto, large, text, decodeFailed, language, prefKey };
}

/** Resolve a requested mode against the model (unknown/unavailable → Auto's choice). */
export function resolveMode(model: BodyModel, requested: BodyMode | undefined | null): ConcreteMode {
  if (!requested || requested === 'auto') return model.auto;
  return model.modes.includes(requested) ? requested : model.auto;
}

// =====================================================================================
// Info text
// =====================================================================================

const ENCODING_NAMES: Readonly<Record<string, string>> = { br: 'brotli', zstd: 'zstd', gzip: 'gzip', 'x-gzip': 'gzip', deflate: 'deflate', 'x-deflate': 'deflate' };

/** Toolbar info, e.g. `gzip · 3.1 KB → 12.8 KB`, plus a longer tooltip. */
export function bodyInfo(view: Pick<BodyView, 'size' | 'wireSize' | 'encodings'>): { text: string; title: string } {
  const encodings = view.encodings.filter((e) => e && e.toLowerCase() !== 'identity');
  if (encodings.length && view.wireSize !== view.size) {
    const names = encodings.map((e) => ENCODING_NAMES[e.toLowerCase()] ?? e);
    const ratio = view.size > 0 ? 1 - view.wireSize / view.size : 0;
    const pct = Math.round(Math.abs(ratio) * 100);
    return {
      text: `${encodings.join(' + ')} · ${formatBytes(view.wireSize)} → ${formatBytes(view.size)}`,
      title: `Compressed with ${names.join(', then ')}: ${formatBytes(view.wireSize)} on the wire, ${formatBytes(view.size)} decoded (${pct}% ${ratio >= 0 ? 'smaller' : 'larger'})`,
    };
  }
  return { text: formatBytes(view.size), title: `${formatNumber(view.size)} bytes` };
}

// =====================================================================================
// Download filenames
// =====================================================================================

const EXTENSIONS: Readonly<Record<string, string>> = {
  'application/json': 'json',
  'text/html': 'html',
  'application/xhtml+xml': 'xhtml',
  'application/xml': 'xml',
  'text/xml': 'xml',
  'application/rss+xml': 'xml',
  'application/atom+xml': 'xml',
  'application/javascript': 'js',
  'text/javascript': 'js',
  'text/css': 'css',
  'text/markdown': 'md',
  'text/csv': 'csv',
  'text/plain': 'txt',
  'text/event-stream': 'txt',
  'application/x-www-form-urlencoded': 'txt',
  'application/yaml': 'yaml',
  'text/yaml': 'yaml',
  'image/png': 'png',
  'image/jpeg': 'jpg',
  'image/gif': 'gif',
  'image/webp': 'webp',
  'image/avif': 'avif',
  'image/svg+xml': 'svg',
  'image/x-icon': 'ico',
  'image/vnd.microsoft.icon': 'ico',
  'image/bmp': 'bmp',
  'audio/mpeg': 'mp3',
  'audio/ogg': 'ogg',
  'audio/wav': 'wav',
  'audio/webm': 'weba',
  'video/mp4': 'mp4',
  'video/webm': 'webm',
  'video/ogg': 'ogv',
  'application/pdf': 'pdf',
  'application/wasm': 'wasm',
  'font/woff2': 'woff2',
  'font/woff': 'woff',
  'font/ttf': 'ttf',
  'font/otf': 'otf',
  'application/zip': 'zip',
  'application/gzip': 'gz',
  'application/x-ndjson': 'ndjson',
  'application/msgpack': 'msgpack',
  'application/cbor': 'cbor',
};

const KIND_EXTENSIONS: Partial<Record<ContentKind, string>> = {
  json: 'json',
  'json-lines': 'ndjson',
  'graph-ql': 'json',
  xml: 'xml',
  html: 'html',
  javascript: 'js',
  css: 'css',
  markdown: 'md',
  yaml: 'yaml',
  csv: 'csv',
  text: 'txt',
  'form-url-encoded': 'txt',
  multipart: 'txt',
  'event-stream': 'txt',
  pdf: 'pdf',
  wasm: 'wasm',
  protobuf: 'bin',
  grpc: 'bin',
  'grpc-web': 'bin',
  'msg-pack': 'msgpack',
  cbor: 'cbor',
};

/** File extension (without dot) for a content type / kind. */
export function extensionFor(contentType: string | null | undefined, kind: ContentKind): string {
  const ct = essence(contentType);
  if (EXTENSIONS[ct]) return EXTENSIONS[ct] as string;
  if (ct.endsWith('+json')) return 'json';
  if (ct.endsWith('+xml')) return 'xml';
  return KIND_EXTENSIONS[kind] ?? 'bin';
}

/** Characters that are invalid in filenames on common platforms. */
function sanitizeFilename(name: string): string {
  return name
    .replace(/[\u0000-\u001f<>:"/\\|?*]+/g, '_')
    .replace(/^\.+/, '')
    .slice(0, 180)
    .trim();
}

/**
 * Filename from a `Content-Disposition` value: RFC 5987 `filename*=` wins, then quoted or
 * unquoted `filename=` (HTK only handled ` filename="…"`). Paths are reduced to the basename.
 */
export function contentDispositionFilename(value: string | null | undefined): string | null {
  if (!value) return null;
  const star = /filename\*\s*=\s*([^']*)'[^']*'([^;]+)/i.exec(value);
  let name: string | null = null;
  if (star?.[2]) {
    try {
      name = decodeURIComponent(star[2].trim().replace(/^"|"$/g, ''));
    } catch {
      name = star[2].trim();
    }
  }
  if (!name) {
    const quoted = /filename\s*=\s*"((?:[^"\\]|\\.)*)"/i.exec(value);
    if (quoted?.[1] != null) name = quoted[1].replace(/\\(.)/g, '$1');
    else {
      const bare = /filename\s*=\s*([^;\s]+)/i.exec(value);
      if (bare?.[1]) name = bare[1];
    }
  }
  if (!name) return null;
  const base = name.split(/[/\\]/).pop() ?? '';
  const clean = sanitizeFilename(base);
  return clean || null;
}

/** Download filename for a body: Content-Disposition → URL basename → `<segment|side>.<ext>`. */
export function downloadFilename(opts: { path?: string; contentType?: string | null; kind: ContentKind; side?: 'request' | 'response'; headers?: Headers }): string {
  const fromHeader = contentDispositionFilename(opts.headers ? getHeader(opts.headers, 'content-disposition') : undefined);
  if (fromHeader) return fromHeader;
  const ext = extensionFor(opts.contentType, opts.kind);
  const pathOnly = (opts.path ?? '').split(/[?#]/)[0] ?? '';
  const segments = pathOnly.split('/').filter(Boolean);
  let last = segments[segments.length - 1] ?? '';
  try {
    last = decodeURIComponent(last);
  } catch {
    // keep raw
  }
  last = sanitizeFilename(last);
  if (last.includes('.') && !last.endsWith('.')) return last;
  if (last) return `${last}.${ext}`;
  return `${opts.side ?? 'response'}-body.${ext}`;
}

// =====================================================================================
// Body freshness
// =====================================================================================

/**
 * `refreshKey` for an exchange body view: changes whenever the body on that side may have
 * changed, and stays put once it is final (so finished bodies are fetched once and cached).
 *
 * - Request: still being received (`requesting`) or held at a request breakpoint (it may be
 *   edited before it is sent) → pending; afterwards final at its size.
 * - Response: final once the exchange completed or aborted; until then one key per state.
 */
export function bodyRefreshKey(summary: Pick<ExchangeSummary, 'state' | 'paused' | 'requestSize' | 'responseSize'>, side: Side): string {
  if (side === 'request') {
    return summary.state === 'requesting' || summary.paused === 'request' ? `pending:${summary.state}` : `done:${summary.requestSize}`;
  }
  return summary.state === 'complete' || summary.state === 'aborted' ? `done:${summary.responseSize}` : summary.state;
}

// =====================================================================================
// Formatting fallbacks
// =====================================================================================

export type JsonFormat = { ok: true; text: string; value: unknown } | { ok: false; error: string };

/**
 * Pretty-print JSON (2-space indent) without changing any value: 64-bit integers, long
 * decimals and duplicate keys are written exactly as received (see `lib/json`).
 */
export function formatJson(text: string): JsonFormat {
  try {
    const value: unknown = parseJsonLossless(text);
    return { ok: true, text: stringifyJson(value, 2) ?? 'null', value };
  } catch (e) {
    return { ok: false, error: e instanceof Error ? e.message : String(e) };
  }
}

/**
 * Parse newline-delimited JSON (also RFC 7464 RS-prefixed records) losslessly; bad lines
 * become `{ $invalid }`.
 */
export function parseJsonLines(text: string): unknown[] {
  const out: unknown[] = [];
  for (const raw of text.split(/\r?\n|\u001e/)) {
    const line = raw.trim();
    if (!line) continue;
    try {
      out.push(parseJsonLossless(line));
    } catch {
      out.push({ $invalid: line });
    }
  }
  return out;
}

/** Pretty JSON-lines: each record formatted, separated by blank lines. */
export function formatJsonLines(text: string): string {
  return parseJsonLines(text)
    .map((v) => stringifyJson(v, 2) ?? 'null')
    .join('\n\n');
}

/** Minimal RFC 4180 CSV parser (quoted fields, escaped quotes, CRLF). Stops after `maxRows`. */
export function parseCsv(text: string, maxRows = 2000): { rows: string[][]; truncated: boolean } {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = '';
  let quoted = false;
  let i = 0;
  const n = text.length;
  const delimiter = detectDelimiter(text);
  while (i < n) {
    const ch = text[i] as string;
    if (quoted) {
      if (ch === '"') {
        if (text[i + 1] === '"') {
          field += '"';
          i += 2;
          continue;
        }
        quoted = false;
        i++;
        continue;
      }
      field += ch;
      i++;
      continue;
    }
    if (ch === '"' && field === '') {
      quoted = true;
      i++;
    } else if (ch === delimiter) {
      row.push(field);
      field = '';
      i++;
    } else if (ch === '\n' || ch === '\r') {
      row.push(field);
      rows.push(row);
      row = [];
      field = '';
      if (ch === '\r' && text[i + 1] === '\n') i++;
      i++;
      if (rows.length >= maxRows) return { rows, truncated: i < n };
    } else {
      field += ch;
      i++;
    }
  }
  if (field !== '' || row.length) {
    row.push(field);
    rows.push(row);
  }
  return { rows, truncated: false };
}

function detectDelimiter(text: string): string {
  const nl = text.indexOf('\n');
  const firstLine = text.slice(0, nl < 0 ? 1000 : Math.min(nl, 1000));
  const counts = [',', ';', '\t', '|'].map((d) => [d, firstLine.split(d).length - 1] as const);
  counts.sort((a, b) => b[1] - a[1]);
  return counts[0] && counts[0][1] > 0 ? counts[0][0] : ',';
}

// =====================================================================================
// Structured payload accessors (tolerant of missing/odd daemon data)
// =====================================================================================

function isRecord(v: unknown): v is Record<string, unknown> {
  return v != null && typeof v === 'object' && !Array.isArray(v);
}

/** Form fields: `structured` pairs, else parsed from the text. */
export function formFields(view: BodyView): [string, string][] {
  const s = view.structured;
  if (Array.isArray(s) && s.every((p) => Array.isArray(p) && p.length === 2)) return s.map((p) => [String(p[0]), String(p[1])]);
  return view.text != null ? parseQueryString(view.text.trim()).map((p) => [p.name, p.value]) : [];
}

/** SSE events: `structured`, else parsed from the text (WHATWG rules). */
export function sseEvents(view: BodyView): SseEvent[] {
  const s = view.structured;
  if (Array.isArray(s) && s.every((e) => isRecord(e) && 'data' in e)) return s as SseEvent[];
  return view.text != null ? parseSse(view.text) : [];
}

/** WHATWG event-stream parsing: CR/LF/CRLF lines, comments, one leading space stripped. */
export function parseSse(text: string): SseEvent[] {
  const events: SseEvent[] = [];
  let data: string[] = [];
  let event: string | null = null;
  let id: string | null = null;
  let retry: number | null = null;
  let dirty = false;
  const dispatch = () => {
    if (dirty) events.push({ event, id, retry, data: data.join('\n') });
    data = [];
    event = null;
    retry = null;
    dirty = false;
  };
  for (const line of text.replace(/^﻿/, '').split(/\r\n|\r|\n/)) {
    if (line === '') {
      dispatch();
      continue;
    }
    if (line.startsWith(':')) continue;
    const i = line.indexOf(':');
    const field = i < 0 ? line : line.slice(0, i);
    let value = i < 0 ? '' : line.slice(i + 1);
    if (value.startsWith(' ')) value = value.slice(1);
    dirty = true;
    if (field === 'data') data.push(value);
    else if (field === 'event') event = value;
    else if (field === 'id') id = value;
    else if (field === 'retry' && /^\d+$/.test(value)) retry = Number(value);
  }
  dispatch();
  return events;
}

/** gRPC messages. The daemon sends `{messages: GrpcMessage[], error?}` (akev-content
 *  `view_body`); also tolerates a bare list or a single decoded object. */
export function grpcMessages(view: BodyView): GrpcMessage[] {
  let s = view.structured;
  if (isRecord(s) && Array.isArray(s.messages)) s = s.messages;
  if (Array.isArray(s)) {
    return s.map((m) =>
      isRecord(m) && ('decoded' in m || 'trailers' in m)
        ? {
            compressed: Boolean(m.compressed),
            decoded: m.decoded ?? null,
            trailers: (m.trailers as Headers | null) ?? null,
            // Typed decode with an imported schema (`BodyView.schema`).
            ...('typed' in m ? { typed: m.typed } : {}),
            ...(typeof m.typedError === 'string' ? { typedError: m.typedError } : {}),
            ...(Array.isArray(m.unknownFields) ? { unknownFields: m.unknownFields as string[] } : {}),
          }
        : { compressed: false, decoded: m, trailers: null },
    );
  }
  if (s != null) return [{ compressed: false, decoded: s, trailers: null }];
  return [];
}

/** Multipart parts (structured only — the daemon parses them). */
export function grpcParseError(view: BodyView): string | null {
  const s = view.structured;
  return isRecord(s) && typeof s.error === 'string' ? s.error : null;
}

/** Multipart parts (structured only — the daemon parses them). */
export function multipartParts(view: BodyView): MultipartPart[] {
  const s = view.structured;
  return Array.isArray(s) ? (s.filter((p) => isRecord(p) && Array.isArray(p.headers)) as MultipartPart[]) : [];
}

export function imageInfo(view: BodyView): ImageInfo | null {
  const s = view.structured;
  return isRecord(s) && typeof s.format === 'string' ? (s as unknown as ImageInfo) : null;
}

// =====================================================================================
// gRPC status
// =====================================================================================

export const GRPC_STATUS: readonly string[] = [
  'OK',
  'CANCELLED',
  'UNKNOWN',
  'INVALID_ARGUMENT',
  'DEADLINE_EXCEEDED',
  'NOT_FOUND',
  'ALREADY_EXISTS',
  'PERMISSION_DENIED',
  'RESOURCE_EXHAUSTED',
  'FAILED_PRECONDITION',
  'ABORTED',
  'OUT_OF_RANGE',
  'UNIMPLEMENTED',
  'INTERNAL',
  'UNAVAILABLE',
  'DATA_LOSS',
  'UNAUTHENTICATED',
];

export interface GrpcStatus {
  code: number;
  name: string;
  message: string | null;
  ok: boolean;
}

/** `grpc-status` / `grpc-message` from trailers (or trailers-only headers). */
export function grpcStatus(...sources: (Headers | null | undefined)[]): GrpcStatus | null {
  for (const h of sources) {
    if (!h) continue;
    const raw = getHeader(h, 'grpc-status');
    if (raw == null) continue;
    const code = Number(raw.trim());
    const msg = getHeader(h, 'grpc-message');
    let message: string | null = null;
    if (msg != null) {
      try {
        message = decodeURIComponent(msg);
      } catch {
        message = msg;
      }
    }
    return { code, name: GRPC_STATUS[code] ?? `CODE ${raw.trim()}`, message, ok: code === 0 };
  }
  return null;
}

// =====================================================================================
// HTML preview hardening
// =====================================================================================

/**
 * CSP for sandboxed HTML previews: no scripts, no network (inline styles, data:
 * images/fonts/media only), no form submissions, no base URL tricks.
 */
export const PREVIEW_CSP = "default-src 'none'; style-src 'unsafe-inline'; img-src data:; font-src data:; media-src data:; form-action 'none'; base-uri 'none'";

/**
 * Tags renamed to inert unknown elements in previewed HTML (defence in depth: none can do
 * anything useful offline). Each is a request source that CSP doesn't cover in every engine or
 * version: `<meta http-equiv=refresh>` navigates the frame (only newer engines refuse it in
 * sandboxed frames), `<link rel=preconnect|dns-prefetch|prefetch>` resource hints, `<base>`
 * retargeting URLs, and nested frames/objects carrying documents of their own.
 */
const INERT_TAGS = /<(\/?)(meta|link|base|iframe|object|embed|portal|fencedframe)(?=[\t\n\f\r />])/gi;

/** What {@link hardenHtmlPreview} puts in front of the document. */
export const PREVIEW_PREAMBLE =
  `<meta http-equiv="Content-Security-Policy" content="${PREVIEW_CSP}">` +
  '<meta http-equiv="x-dns-prefetch-control" content="off">' +
  '<meta name="referrer" content="no-referrer">' +
  // Links open "in a new window", which the sandbox (no allow-popups) refuses: clicks stay inert.
  '<base target="_blank">' +
  '<style>:root{color-scheme:light;background:Canvas;color:CanvasText}</style>';

/**
 * Make captured HTML safe to render in a `sandbox=""` srcdoc iframe without any network access.
 *
 * The CSP `<meta>` is placed before everything else, so the parser applies it (in an implicit
 * `<head>`) before it sees a single byte of the captured markup — nothing in the document can
 * come first, hide it in a comment/attribute or push it into `<body>`. (An iframe srcdoc
 * document is never in quirks mode, so a doctype after it changes nothing.) CSP can only be
 * tightened, never relaxed, by later policies. Tags that can trigger requests CSP doesn't
 * govern everywhere are renamed to inert elements first; tag names can't be obfuscated with
 * character references, so this textual rename is exact.
 */
export function hardenHtmlPreview(html: string): string {
  const body = (html.charCodeAt(0) === 0xfeff ? html.slice(1) : html).replace(INERT_TAGS, '<$1x-inert-$2');
  return PREVIEW_PREAMBLE + body;
}
