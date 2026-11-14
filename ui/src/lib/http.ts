/**
 * HTTP domain helpers shared by every page: status texts and classes, methods,
 * content-kind labels/colours, error explanations, cookies, query strings and JWTs.
 * Pure functions only — no React, no I/O.
 */

import type { ContentKind, ErrorKind, ExchangeSummary, Headers, TagColor } from '../api/types';
import { base64ToText } from './encoding';

// =====================================================================================
// Status codes
// =====================================================================================

/** Canonical reason phrases (IANA registry + widely seen unofficial codes). */
export const STATUS_TEXTS: Readonly<Record<number, string>> = {
  100: 'Continue',
  101: 'Switching Protocols',
  102: 'Processing',
  103: 'Early Hints',
  200: 'OK',
  201: 'Created',
  202: 'Accepted',
  203: 'Non-Authoritative Information',
  204: 'No Content',
  205: 'Reset Content',
  206: 'Partial Content',
  207: 'Multi-Status',
  208: 'Already Reported',
  226: 'IM Used',
  300: 'Multiple Choices',
  301: 'Moved Permanently',
  302: 'Found',
  303: 'See Other',
  304: 'Not Modified',
  305: 'Use Proxy',
  307: 'Temporary Redirect',
  308: 'Permanent Redirect',
  400: 'Bad Request',
  401: 'Unauthorized',
  402: 'Payment Required',
  403: 'Forbidden',
  404: 'Not Found',
  405: 'Method Not Allowed',
  406: 'Not Acceptable',
  407: 'Proxy Authentication Required',
  408: 'Request Timeout',
  409: 'Conflict',
  410: 'Gone',
  411: 'Length Required',
  412: 'Precondition Failed',
  413: 'Content Too Large',
  414: 'URI Too Long',
  415: 'Unsupported Media Type',
  416: 'Range Not Satisfiable',
  417: 'Expectation Failed',
  418: "I'm a teapot",
  421: 'Misdirected Request',
  422: 'Unprocessable Content',
  423: 'Locked',
  424: 'Failed Dependency',
  425: 'Too Early',
  426: 'Upgrade Required',
  428: 'Precondition Required',
  429: 'Too Many Requests',
  431: 'Request Header Fields Too Large',
  444: 'No Response',
  451: 'Unavailable For Legal Reasons',
  499: 'Client Closed Request',
  500: 'Internal Server Error',
  501: 'Not Implemented',
  502: 'Bad Gateway',
  503: 'Service Unavailable',
  504: 'Gateway Timeout',
  505: 'HTTP Version Not Supported',
  506: 'Variant Also Negotiates',
  507: 'Insufficient Storage',
  508: 'Loop Detected',
  510: 'Not Extended',
  511: 'Network Authentication Required',
  520: 'Web Server Returned an Unknown Error',
  521: 'Web Server Is Down',
  522: 'Connection Timed Out',
  523: 'Origin Is Unreachable',
  524: 'A Timeout Occurred',
  525: 'SSL Handshake Failed',
  526: 'Invalid SSL Certificate',
};

/** Reason phrase for a status code (`""` if unknown). */
export function statusText(status: number | null | undefined): string {
  if (status == null) return '';
  return STATUS_TEXTS[status] ?? '';
}

export type StatusClass = '1xx' | '2xx' | '3xx' | '4xx' | '5xx' | 'err';

/** Status class used for colouring (`err` for missing/out-of-range codes). */
export function statusClass(status: number | null | undefined): StatusClass {
  if (status == null || status < 100 || status >= 600) return 'err';
  if (status < 200) return '1xx';
  if (status < 300) return '2xx';
  if (status < 400) return '3xx';
  if (status < 500) return '4xx';
  return '5xx';
}

/** CSS variable holding the colour of a status class, e.g. `var(--s-4xx)`. */
export function statusColorVar(status: number | null | undefined): string {
  return `var(--s-${statusClass(status)})`;
}

// =====================================================================================
// Methods
// =====================================================================================

/** Standard methods in menu order (Compose method picker, rule matchers). */
export const HTTP_METHODS = ['GET', 'POST', 'PUT', 'PATCH', 'DELETE', 'HEAD', 'OPTIONS', 'CONNECT', 'TRACE'] as const;

const METHOD_VARS: Readonly<Record<string, string>> = {
  GET: 'var(--m-get)',
  POST: 'var(--m-post)',
  PUT: 'var(--m-put)',
  PATCH: 'var(--m-patch)',
  DELETE: 'var(--m-delete)',
  HEAD: 'var(--m-head)',
  OPTIONS: 'var(--m-options)',
  CONNECT: 'var(--m-connect)',
};

/** CSS colour variable for a method (case-insensitive; unknown → `--m-other`). */
export function methodColorVar(method: string): string {
  return METHOD_VARS[method.toUpperCase()] ?? 'var(--m-other)';
}

/** Methods whose requests conventionally carry a body. */
export function methodHasBody(method: string): boolean {
  const m = method.toUpperCase();
  return m !== 'GET' && m !== 'HEAD' && m !== 'OPTIONS' && m !== 'TRACE' && m !== 'CONNECT';
}

// =====================================================================================
// Headers
// =====================================================================================

/** First value of a header (case-insensitive), or `undefined`. */
export function getHeader(headers: Headers, name: string): string | undefined {
  const lower = name.toLowerCase();
  for (const [k, v] of headers) if (k.toLowerCase() === lower) return v;
  return undefined;
}

/** All values of a header (case-insensitive), in wire order. */
export function getHeaderAll(headers: Headers, name: string): string[] {
  const lower = name.toLowerCase();
  const out: string[] = [];
  for (const [k, v] of headers) if (k.toLowerCase() === lower) out.push(v);
  return out;
}

/** Returns a copy with pseudo-headers (`:authority`) first and the rest sorted by name. */
export function sortHeaders(headers: Headers): Headers {
  return [...headers].sort(([a], [b]) => {
    const pa = a.startsWith(':');
    const pb = b.startsWith(':');
    if (pa !== pb) return pa ? -1 : 1;
    return a.toLowerCase().localeCompare(b.toLowerCase());
  });
}

// =====================================================================================
// URL, query & cookies
// =====================================================================================

/** `host[:port]`, omitting the scheme's default port. IPv6 hosts are bracketed. */
export function formatAuthority(scheme: string, host: string, port: number): string {
  const h = host.includes(':') ? `[${host}]` : host;
  const isDefault = ((scheme === 'http' || scheme === 'ws') && port === 80) || ((scheme === 'https' || scheme === 'wss') && port === 443);
  return isDefault || !port ? h : `${h}:${port}`;
}

/** Full URL of a summary row (`https://api.example.com/v1/users?x=1`). */
export function summaryUrl(s: Pick<ExchangeSummary, 'scheme' | 'host' | 'port' | 'path'>): string {
  return `${s.scheme}://${formatAuthority(s.scheme, s.host, s.port)}${s.path}`;
}

/** A parsed `Cookie` request header pair. */
export interface RequestCookie {
  name: string;
  value: string;
}

/** Parse a `Cookie:` header value (`a=1; b=2`). */
export function parseCookieHeader(value: string): RequestCookie[] {
  const out: RequestCookie[] = [];
  for (const part of value.split(';')) {
    const trimmed = part.trim();
    if (!trimmed) continue;
    const eq = trimmed.indexOf('=');
    out.push(eq < 0 ? { name: '', value: trimmed } : { name: trimmed.slice(0, eq).trim(), value: trimmed.slice(eq + 1).trim() });
  }
  return out;
}

/** A parsed `Set-Cookie` response header. */
export interface SetCookie {
  name: string;
  value: string;
  domain?: string;
  path?: string;
  expires?: string;
  maxAge?: number;
  secure: boolean;
  httpOnly: boolean;
  sameSite?: string;
  partitioned: boolean;
  /** Attributes we don't model explicitly (`Priority=High`...). */
  other: [string, string][];
}

/** Parse one `Set-Cookie` header value. */
export function parseSetCookie(value: string): SetCookie {
  const [pair = '', ...attrs] = value.split(';');
  const eq = pair.indexOf('=');
  const cookie: SetCookie = {
    name: eq < 0 ? '' : pair.slice(0, eq).trim(),
    value: eq < 0 ? pair.trim() : pair.slice(eq + 1).trim(),
    secure: false,
    httpOnly: false,
    partitioned: false,
    other: [],
  };
  for (const raw of attrs) {
    const a = raw.trim();
    if (!a) continue;
    const i = a.indexOf('=');
    const key = (i < 0 ? a : a.slice(0, i)).trim();
    const val = i < 0 ? '' : a.slice(i + 1).trim();
    switch (key.toLowerCase()) {
      case 'domain':
        cookie.domain = val;
        break;
      case 'path':
        cookie.path = val;
        break;
      case 'expires':
        cookie.expires = val;
        break;
      case 'max-age': {
        const n = Number(val);
        if (Number.isFinite(n)) cookie.maxAge = n;
        break;
      }
      case 'secure':
        cookie.secure = true;
        break;
      case 'httponly':
        cookie.httpOnly = true;
        break;
      case 'samesite':
        cookie.sameSite = val;
        break;
      case 'partitioned':
        cookie.partitioned = true;
        break;
      default:
        cookie.other.push([key, val]);
    }
  }
  return cookie;
}

// =====================================================================================
// JWT
// =====================================================================================

export interface DecodedJwt {
  header: Record<string, unknown>;
  payload: Record<string, unknown>;
  signature: string;
  /** Expiry (unix ms) from `exp`, if present. */
  expiresAt?: number;
  /** Issued-at (unix ms) from `iat`, if present. */
  issuedAt?: number;
  expired: boolean;
}

function base64UrlToText(part: string): string | null {
  const b64 = part.replace(/-/g, '+').replace(/_/g, '/');
  const padded = b64 + '='.repeat((4 - (b64.length % 4)) % 4);
  try {
    return base64ToText(padded);
  } catch {
    return null;
  }
}

/**
 * Decode (without verifying!) a JWT. Accepts a bare token or a `Bearer <token>` header value.
 * Returns `null` if the input isn't a well-formed JWS compact token.
 */
export function decodeJwt(input: string, now: number = Date.now()): DecodedJwt | null {
  const token = input.trim().replace(/^bearer\s+/i, '');
  const parts = token.split('.');
  if (parts.length !== 3 || !parts[0] || !parts[1]) return null;
  const headerText = base64UrlToText(parts[0]);
  const payloadText = base64UrlToText(parts[1]);
  if (headerText == null || payloadText == null) return null;
  try {
    const header: unknown = JSON.parse(headerText);
    const payload: unknown = JSON.parse(payloadText);
    if (!header || typeof header !== 'object' || !payload || typeof payload !== 'object') return null;
    const p = payload as Record<string, unknown>;
    const expiresAt = typeof p.exp === 'number' ? p.exp * 1000 : undefined;
    const issuedAt = typeof p.iat === 'number' ? p.iat * 1000 : undefined;
    return {
      header: header as Record<string, unknown>,
      payload: p,
      signature: parts[2] ?? '',
      expiresAt,
      issuedAt,
      expired: expiresAt != null && expiresAt < now,
    };
  } catch {
    return null;
  }
}

// =====================================================================================
// Content kinds
// =====================================================================================

interface KindInfo {
  /** Short label for the Type column (`json`, `img`). */
  label: string;
  /** CSS colour variable. */
  color: string;
  /** Default CodeMirror language for text viewing. */
  language?: string;
  textual: boolean;
}

const KIND_INFO: Readonly<Record<ContentKind, KindInfo>> = {
  json: { label: 'json', color: 'var(--k-json)', language: 'json', textual: true },
  'json-lines': { label: 'jsonl', color: 'var(--k-json)', language: 'json', textual: true },
  xml: { label: 'xml', color: 'var(--k-xml)', language: 'xml', textual: true },
  html: { label: 'html', color: 'var(--k-html)', language: 'html', textual: true },
  javascript: { label: 'js', color: 'var(--k-js)', language: 'javascript', textual: true },
  css: { label: 'css', color: 'var(--k-css)', language: 'css', textual: true },
  markdown: { label: 'md', color: 'var(--k-text)', language: 'markdown', textual: true },
  yaml: { label: 'yaml', color: 'var(--k-text)', language: 'yaml', textual: true },
  csv: { label: 'csv', color: 'var(--k-text)', textual: true },
  text: { label: 'text', color: 'var(--k-text)', textual: true },
  'form-url-encoded': { label: 'form', color: 'var(--k-form)', textual: true },
  multipart: { label: 'multipart', color: 'var(--k-form)', textual: false },
  image: { label: 'img', color: 'var(--k-image)', textual: false },
  audio: { label: 'audio', color: 'var(--k-media)', textual: false },
  video: { label: 'video', color: 'var(--k-media)', textual: false },
  font: { label: 'font', color: 'var(--k-font)', textual: false },
  pdf: { label: 'pdf', color: 'var(--k-binary)', textual: false },
  wasm: { label: 'wasm', color: 'var(--k-binary)', textual: false },
  protobuf: { label: 'proto', color: 'var(--k-grpc)', textual: false },
  grpc: { label: 'grpc', color: 'var(--k-grpc)', textual: false },
  'grpc-web': { label: 'grpc-web', color: 'var(--k-grpc)', textual: false },
  'msg-pack': { label: 'msgpack', color: 'var(--k-binary)', textual: false },
  cbor: { label: 'cbor', color: 'var(--k-binary)', textual: false },
  'event-stream': { label: 'sse', color: 'var(--k-ws)', textual: true },
  'graph-ql': { label: 'graphql', color: 'var(--k-json)', language: 'json', textual: true },
  binary: { label: 'bin', color: 'var(--k-binary)', textual: false },
  unknown: { label: '—', color: 'var(--k-binary)', textual: false },
};

export function contentKindLabel(kind: ContentKind): string {
  return KIND_INFO[kind]?.label ?? kind;
}

export function contentKindColorVar(kind: ContentKind): string {
  return KIND_INFO[kind]?.color ?? 'var(--k-binary)';
}

/** Whether bodies of this kind are viewable as text. */
export function isTextualKind(kind: ContentKind): boolean {
  return KIND_INFO[kind]?.textual ?? false;
}

/** Default code-editor language for a kind (`undefined` → plain text). */
export function languageForKind(kind: ContentKind): string | undefined {
  return KIND_INFO[kind]?.language;
}

/** Traffic-list type chips (DESIGN.md §6.2), each translated into a server-side filter clause. */
export type TypeChip = 'all' | 'fetch' | 'doc' | 'js' | 'css' | 'img' | 'media' | 'font' | 'ws' | 'llm' | 'other';

export interface TypeChipInfo {
  id: TypeChip;
  label: string;
  /** Filter-language clause ANDed with the user's query; `''` for `all`. */
  clause: string;
}

const FETCH_KINDS = ['json', 'json-lines', 'xml', 'protobuf', 'grpc', 'grpc-web', 'form-url-encoded', 'msg-pack', 'cbor', 'graph-ql'];
const OTHER_EXCLUDED = [...FETCH_KINDS, 'html', 'javascript', 'css', 'image', 'audio', 'video', 'font'];

export const TYPE_CHIPS: readonly TypeChipInfo[] = [
  { id: 'all', label: 'All', clause: '' },
  { id: 'fetch', label: 'Fetch/XHR', clause: `type:${FETCH_KINDS.join(',')}` },
  { id: 'doc', label: 'Doc', clause: 'type:html' },
  { id: 'js', label: 'JS', clause: 'type:javascript' },
  { id: 'css', label: 'CSS', clause: 'type:css' },
  { id: 'img', label: 'Img', clause: 'type:image' },
  { id: 'media', label: 'Media', clause: 'type:audio,video' },
  { id: 'font', label: 'Font', clause: 'type:font' },
  { id: 'ws', label: 'WS', clause: '(is:ws OR is:sse)' },
  { id: 'llm', label: 'LLM', clause: 'is:llm' },
  { id: 'other', label: 'Other', clause: `-type:${OTHER_EXCLUDED.join(',')} -is:ws -is:sse` },
];

/** Filter clause for a type chip (`''` for `all`). */
export function typeChipClause(chip: TypeChip): string {
  return TYPE_CHIPS.find((c) => c.id === chip)?.clause ?? '';
}

// =====================================================================================
// Errors
// =====================================================================================

export interface ErrorInfo {
  /** Very short label for the Status column (`Refused`, `TLS`, `Timeout`). */
  short: string;
  /** Plain-language title. `{host}` is substituted by {@link describeError}. */
  title: string;
  /** What probably happened. */
  explanation: string;
  /** Suggested fix, if any. */
  fix?: string;
}

const ERROR_INFO: Readonly<Record<ErrorKind, ErrorInfo>> = {
  'client-aborted': {
    short: 'Aborted',
    title: 'The client closed the connection',
    explanation: 'The client gave up before the exchange completed — a navigation, a cancelled fetch or a client-side timeout.',
  },
  'dns-failed': {
    short: 'DNS',
    title: "Couldn't resolve {host}",
    explanation: 'The DNS lookup for the upstream host failed. The name may not exist, or the DNS server is unreachable.',
    fix: 'Check the hostname, or add a DNS override in Settings → Proxy.',
  },
  'connection-refused': {
    short: 'Refused',
    title: 'Connection refused by {host}',
    explanation: 'Nothing is accepting connections on that host and port.',
    fix: 'Make sure the server is running and listening on the expected port.',
  },
  'connection-reset': {
    short: 'Reset',
    title: 'Connection reset by {host}',
    explanation: 'The upstream server (or something in between) abruptly closed the connection mid-exchange.',
  },
  'host-unreachable': {
    short: 'Unreachable',
    title: "Can't reach {host}",
    explanation: 'The network or host is unreachable from this machine.',
    fix: 'Check your network connection, VPN and upstream proxy settings.',
  },
  timeout: {
    short: 'Timeout',
    title: '{host} took too long to respond',
    explanation: 'Akev timed out connecting to or waiting for the upstream server.',
    fix: 'Increase the timeouts in Settings → Proxy if this server is legitimately slow.',
  },
  'upstream-cert-invalid': {
    short: 'Cert',
    title: "{host}'s certificate isn't valid",
    explanation: 'The upstream TLS certificate is untrusted, expired or issued for a different hostname.',
    fix: 'Add this host to "Ignore certificate errors" in Settings → Certificates, or trust its CA.',
  },
  'upstream-tls-failed': {
    short: 'TLS',
    title: 'TLS handshake with {host} failed',
    explanation: "Akev and the server couldn't agree on a TLS version or cipher, or the server sent a TLS alert.",
  },
  'upstream-protocol-error': {
    short: 'Protocol',
    title: '{host} sent an invalid HTTP response',
    explanation: "The upstream response couldn't be parsed as HTTP.",
  },
  'client-protocol-error': {
    short: 'Bad request',
    title: 'The client sent a malformed request',
    explanation: "The request couldn't be parsed as HTTP.",
  },
  'client-tls-rejected': {
    short: 'TLS',
    title: 'The client rejected the Akev certificate',
    explanation: "The client doesn't trust the Akev CA, or it pins certificates for {host}.",
    fix: 'Trust the Akev CA certificate (Connect page), or add the host to TLS passthrough.',
  },
  'rule-closed': {
    short: 'Closed',
    title: 'Closed by a rule',
    explanation: 'A rule deliberately closed the connection.',
  },
  'rule-reset': {
    short: 'Reset',
    title: 'Reset by a rule',
    explanation: 'A rule deliberately reset the connection.',
  },
  'rule-timeout': {
    short: 'Timeout',
    title: 'Timed out by a rule',
    explanation: 'A rule accepted the request and never responded.',
  },
  'breakpoint-aborted': {
    short: 'Aborted',
    title: 'Aborted at a breakpoint',
    explanation: 'The exchange was closed or reset from a breakpoint.',
  },
  'upstream-proxy-failed': {
    short: 'Proxy',
    title: 'The upstream proxy failed',
    explanation: "Akev couldn't connect through the configured upstream proxy.",
    fix: 'Check Settings → Proxy → Upstream proxy.',
  },
  internal: {
    short: 'Error',
    title: 'Internal error',
    explanation: 'Something went wrong inside Akev while handling this exchange.',
  },
};

/** Static info for an error kind. */
export function errorInfo(kind: ErrorKind): ErrorInfo {
  return ERROR_INFO[kind] ?? ERROR_INFO.internal;
}

/** Error info with `{host}` substituted (for banners). */
export function describeError(kind: ErrorKind, host: string): ErrorInfo {
  const info = errorInfo(kind);
  const sub = (s: string) => s.replaceAll('{host}', host || 'the server');
  return { short: info.short, title: sub(info.title), explanation: sub(info.explanation), fix: info.fix && sub(info.fix) };
}

// =====================================================================================
// Tags
// =====================================================================================

/** CSS colour variable for a user tag (`red` → `var(--tag-red)`); unknown → grey. */
export function tagColorVar(color: string | null | undefined): string {
  switch (color as TagColor) {
    case 'red':
    case 'orange':
    case 'yellow':
    case 'green':
    case 'blue':
    case 'purple':
    case 'grey':
      return `var(--tag-${color})`;
    default:
      return 'var(--tag-grey)';
  }
}

// =====================================================================================
// Raw message rendering
// =====================================================================================

/**
 * Render an HTTP/1.1-style message (request/status line, headers in wire order, blank line,
 * body) for the Raw tab and "copy as raw". `body` of `null` renders `<N bytes binary>`.
 */
export function renderRawMessage(
  startLine: string,
  headers: Headers,
  body: string | null,
  binarySize = 0,
): string {
  let out = `${startLine}\r\n`;
  for (const [k, v] of headers) out += `${k}: ${v}\r\n`;
  out += '\r\n';
  if (body != null) out += body;
  else if (binarySize > 0) out += `<${binarySize} bytes binary>`;
  return out;
}
