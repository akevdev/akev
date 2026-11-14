/**
 * Protobuf / gRPC helpers: gRPC call names from request paths, schema labels for the body
 * viewer, and the Settings → Protobuf schemas logic (upload preparation, mapping validation,
 * file summaries). Pure — no React.
 */

import type { ExchangeSummary, ProtoError, ProtoFileInfo, ProtoMapping, ProtoMappingSide, ProtoRegistry, ProtoUploadFile, SchemaInfo } from '../api/types';
import { bytesToBase64, utf8Decode } from './encoding';
import { pluralize } from './format';

// =====================================================================================
// gRPC calls
// =====================================================================================

export interface GrpcCall {
  /** Fully-qualified service (`shop.v1.Catalog`). */
  service: string;
  /** Service name without the package (`Catalog`). */
  shortService: string;
  /** Package (`shop.v1`, empty when none). */
  package: string;
  method: string;
}

const IDENT = /^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*$/;

/**
 * The service and method a gRPC request path names: `/shop.v1.Catalog/GetItem`. The last two
 * segments are used (gRPC-Web endpoints are often mounted under a prefix); a query is ignored.
 * Mirrors akev-content `schema::grpc_path_parts`.
 */
export function grpcCallFromPath(path: string | null | undefined): GrpcCall | null {
  if (!path) return null;
  const clean = path.split(/[?#]/, 1)[0] ?? '';
  const segments = clean.split('/');
  const method = segments[segments.length - 1] ?? '';
  const service = segments[segments.length - 2] ?? '';
  if (segments.length < 3 || !IDENT.test(service) || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(method)) return null;
  const dot = service.lastIndexOf('.');
  return { service, shortService: dot < 0 ? service : service.slice(dot + 1), package: dot < 0 ? '' : service.slice(0, dot), method };
}

/** Whether a Content-Type is gRPC or gRPC-Web. */
export function isGrpcContentType(contentType: string | null | undefined): boolean {
  return (contentType ?? '').trim().toLowerCase().startsWith('application/grpc');
}

// =====================================================================================
// Schema labels (body viewer)
// =====================================================================================

/** `shop.v1.Item` → `Item`. */
export function shortTypeName(fullName: string): string {
  const i = fullName.lastIndexOf('.');
  return i < 0 ? fullName : fullName.slice(i + 1);
}

/** One line on where a body's schema came from (tooltips). */
export function schemaSourceText(schema: SchemaInfo): string {
  switch (schema.source) {
    case 'grpc':
      return schema.method ? `gRPC method ${schema.method.service}/${schema.method.method}` : 'the gRPC method in the request path';
    case 'content-type':
      return 'the Content-Type header';
    case 'mapping':
      return schema.mapping ? `the mapping ${schema.mapping}` : 'a URL mapping';
  }
}

/** Short label for the unknown-fields notice: `21, related.99` (+ “and N more”). */
export function unknownFieldsText(paths: readonly string[] | undefined, max = 4): string | null {
  if (!paths?.length) return null;
  const shown = paths.slice(0, max).join(', ');
  return paths.length > max ? `${shown} and ${paths.length - max} more` : shown;
}

// =====================================================================================
// Uploads
// =====================================================================================

/** Extensions accepted by the file picker (descriptor sets come under many names). */
export const PROTO_ACCEPT: readonly string[] = ['.proto', '.desc', '.pb', '.binpb', '.protoset', '.dsc', '.bin'];

/** `.proto` files are text; everything else is uploaded as a descriptor set (bytes). */
export function isProtoSource(name: string): boolean {
  return name.toLowerCase().endsWith('.proto');
}

/**
 * A stored name for an uploaded file: forward slashes, no leading `./` or `/`, and only the
 * characters the daemon accepts (others become `_`). Dropped folders keep their relative path
 * (`api/shop/v1/catalog.proto`).
 */
export function normalizeProtoName(name: string): string {
  let n = name.trim().replace(/\\/g, '/');
  while (n.startsWith('./') || n.startsWith('/')) n = n.startsWith('./') ? n.slice(2) : n.slice(1);
  return n
    .split('/')
    .filter((s) => s && s !== '.' && s !== '..')
    .map((s) => s.replace(/[^A-Za-z0-9._+@-]/g, '_').replace(/^\.+/, '_'))
    .join('/');
}

/**
 * Paths of files dropped as one folder all start with that folder's name
 * (`protos/shop/v1/catalog.proto`); imports are relative to inside it, so drop it.
 */
export function stripCommonRoot(names: readonly string[]): string[] {
  const split = names.map((n) => n.split('/'));
  const first = split[0]?.[0];
  if (first == null || split.some((s) => s.length < 2 || s[0] !== first)) return [...names];
  return split.map((s) => s.slice(1).join('/'));
}

/** Turn picked/dropped files into the `POST /api/protos` payload (skipping unrelated files). */
export function toUploadFiles(files: readonly { name: string; data: Uint8Array }[]): { upload: ProtoUploadFile[]; skipped: string[] } {
  const upload: ProtoUploadFile[] = [];
  const skipped: string[] = [];
  for (const f of files) {
    const name = normalizeProtoName(f.name);
    const lower = name.toLowerCase();
    if (!name || !PROTO_ACCEPT.some((ext) => lower.endsWith(ext))) {
      skipped.push(f.name);
      continue;
    }
    upload.push(isProtoSource(name) ? { name, content: utf8Decode(f.data) } : { name, base64: bytesToBase64(f.data) });
  }
  return { upload, skipped };
}

// =====================================================================================
// Files
// =====================================================================================

export interface ProtoFileSummary {
  services: number;
  methods: number;
  messages: number;
  enums: number;
  /** `2 services · 5 methods · 12 messages`. */
  text: string;
}

export function summarizeProtoFile(file: ProtoFileInfo): ProtoFileSummary {
  const services = file.services.length;
  const methods = file.services.reduce((n, s) => n + s.methods.length, 0);
  const messages = file.messages.length;
  const enums = file.enums.length;
  const parts = [services && pluralize(services, 'service'), methods && pluralize(methods, 'method'), messages && pluralize(messages, 'message'), enums && pluralize(enums, 'enum')].filter(Boolean) as string[];
  return { services, methods, messages, enums, text: parts.length ? parts.join(' · ') : file.errors.length ? 'Not compiled' : 'No definitions' };
}

/** `catalog.proto:4:3` (location only when known). */
export function protoErrorLocation(e: ProtoError): string {
  return e.line != null ? `${e.file}:${e.line}${e.column != null ? `:${e.column}` : ''}` : e.file;
}

export interface RegistrySummary {
  files: number;
  failing: number;
  services: number;
  messages: number;
}

export function summarizeRegistry(registry: ProtoRegistry | null): RegistrySummary {
  const files = registry?.files ?? [];
  return {
    files: files.length,
    failing: files.filter((f) => f.errors.length > 0).length,
    services: files.reduce((n, f) => n + f.services.length, 0),
    messages: registry?.messageTypes.length ?? 0,
  };
}

/** Case-insensitive search over file names, packages, services, methods, messages and enums. */
export function fileMatches(file: ProtoFileInfo, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay = [file.name, file.package ?? '', ...file.messages, ...file.enums, ...file.services.flatMap((s) => [s.name, ...s.methods.map((m) => m.name)])];
  return hay.some((h) => h.toLowerCase().includes(q));
}

// =====================================================================================
// Mappings
// =====================================================================================

export const MAPPING_SIDES: readonly { value: ProtoMappingSide; label: string }[] = [
  { value: 'both', label: 'Both' },
  { value: 'request', label: 'Request' },
  { value: 'response', label: 'Response' },
];

/** A new, empty mapping row (the daemon assigns the id when it's empty). */
export function newMapping(messageType = ''): ProtoMapping {
  return { id: '', enabled: true, pattern: '', side: 'response', messageType };
}

/** Error for a URL pattern (null when fine). */
export function validateMappingPattern(pattern: string): string | null {
  const p = pattern.trim();
  if (!p) return 'Enter a URL pattern, e.g. api.example.com/v1/items/*';
  if (p.length > 2048) return 'The pattern is too long';
  if (/\s/.test(p)) return "Patterns can't contain spaces";
  return null;
}

/** Error for a message type name (null when fine). Mirrors the daemon's check. */
export function validateMessageTypeName(name: string): string | null {
  const n = name.trim().replace(/^\./, '');
  if (!n) return 'Choose a message type';
  if (n.length > 512 || !n.split('.').every((part) => /^[A-Za-z_][A-Za-z0-9_]*$/.test(part))) return 'Use a full message name like shop.v1.Item';
  return null;
}

/** Warning when a mapping names a type the imported schemas don't define. */
export function mappingTypeWarning(name: string, messageTypes: readonly string[]): string | null {
  const n = name.trim().replace(/^\./, '');
  if (!n || validateMessageTypeName(n)) return null;
  return messageTypes.includes(n) ? null : `${n} isn't defined by the imported schemas — the mapping is ignored until it is`;
}

/** The URL of a captured exchange (for matching mappings against traffic). */
export function summaryUrl(s: Pick<ExchangeSummary, 'scheme' | 'host' | 'port' | 'path'>): string {
  const scheme = s.scheme === 'wss' ? 'https' : s.scheme === 'ws' ? 'http' : s.scheme;
  const defaultPort = scheme === 'https' ? 443 : 80;
  const host = s.host.includes(':') ? `[${s.host}]` : s.host;
  return `${scheme}://${host}${s.port && s.port !== defaultPort ? `:${s.port}` : ''}${s.path}`;
}

/** How many of the given (most recent) exchanges a mapping pattern matches. */
export function countMatches(pattern: string, summaries: Iterable<Pick<ExchangeSummary, 'scheme' | 'host' | 'port' | 'path'>>): number {
  if (validateMappingPattern(pattern)) return 0;
  let n = 0;
  for (const s of summaries) if (mappingMatchesUrl(pattern, summaryUrl(s))) n++;
  return n;
}

/** Whether every row is valid (only then are mappings saved). */
export function mappingsValid(mappings: readonly ProtoMapping[]): boolean {
  return mappings.every((m) => !validateMappingPattern(m.pattern) && !validateMessageTypeName(m.messageType));
}

/** What gets saved: trimmed values. */
export function normalizeMappings(mappings: readonly ProtoMapping[]): ProtoMapping[] {
  return mappings.map((m) => ({ ...m, pattern: m.pattern.trim(), messageType: m.messageType.trim().replace(/^\./, '') }));
}

/**
 * Whether `pattern` matches a request URL — the daemon's rules (`*` = any characters, case
 * insensitive; `/path`, `host/path`, `scheme://host/path` or a bare host), used to preview
 * mappings against captured traffic.
 */
export function mappingMatchesUrl(pattern: string, url: string): boolean {
  let parsed: URL | null = null;
  try {
    parsed = new URL(url);
  } catch {
    parsed = null;
  }
  const p = pattern.trim();
  const path = parsed ? parsed.pathname : url;
  const query = parsed?.search ? parsed.search.slice(1) : null;
  const host = parsed?.hostname.toLowerCase() ?? '';
  const authority = parsed ? (parsed.port ? `${host}:${parsed.port}` : host) : '';
  const withQuery = (base: string) => (query != null ? [base, `${base}?${query}`] : [base]);
  let candidates: string[];
  if (p.startsWith('/')) candidates = withQuery(path);
  else if (p.includes('://')) candidates = [url.split(/[?#]/, 1)[0] ?? url, url];
  else if (!p.includes('/')) candidates = [authority, host];
  else candidates = withQuery(`${authority}${path}`);
  return candidates.some((c) => wildcardMatch(p, c));
}

/** `*` matches any run of characters (case-insensitive); everything else is literal. */
export function wildcardMatch(pattern: string, text: string): boolean {
  const p = pattern.toLowerCase();
  const t = text.toLowerCase();
  let pi = 0;
  let ti = 0;
  let star = -1;
  let mark = 0;
  while (ti < t.length) {
    if (pi < p.length && p[pi] === '*') {
      while (pi < p.length && p[pi] === '*') pi++;
      star = pi;
      mark = ti;
    } else if (pi < p.length && p[pi] === t[ti]) {
      pi++;
      ti++;
    } else if (star >= 0) {
      pi = star;
      ti = ++mark;
    } else return false;
  }
  while (pi < p.length && p[pi] === '*') pi++;
  return pi === p.length;
}
