/**
 * Raw HTTP message rendering for the Raw tab (DESIGN.md §6.4): the full HTTP/1.1-style message
 * text — request line / status line, headers in wire order, a blank line, then the body text or
 * `<N bytes binary>`.
 *
 * HTTP/2 and HTTP/3 pseudo-headers (`:method`, `:path`, …) are folded into the start line;
 * `:authority` becomes a `Host` header when the message has none, so the text reads like the
 * message a developer would type.
 */

import type { Headers, RequestHead, ResponseHead } from '../../../../api/types';
import { formatBytes } from '../../../../lib/format';
import { statusText } from '../../../../lib/http';

/** What we know about a body when rendering it as text. */
export interface RawBody {
  /** Decoded text, or `null` when binary / not decodable. */
  text: string | null;
  /** Decoded size in bytes. */
  size: number;
  /** The text was cut for transport (only a prefix is shown). */
  truncated?: boolean;
  /** The proxy only captured a prefix of the body. */
  captureTruncated?: boolean;
}

/** Request line, e.g. `GET /v1/users?page=2 HTTP/1.1` (CONNECT uses the authority form). */
export function requestStartLine(head: Pick<RequestHead, 'method' | 'path' | 'host' | 'port' | 'httpVersion' | 'headers'>): string {
  const method = head.method.toUpperCase();
  let target: string;
  if (method === 'CONNECT') {
    const host = head.host.includes(':') ? `[${head.host}]` : head.host;
    target = `${host}:${head.port}`;
  } else {
    target = head.path || pseudo(head.headers, ':path') || '/';
  }
  return `${method} ${target} ${head.httpVersion}`;
}

/** Status line, e.g. `HTTP/1.1 404 Not Found` (canonical reason when none was sent). */
export function responseStartLine(head: Pick<ResponseHead, 'status' | 'statusMessage' | 'httpVersion'>): string {
  const reason = head.statusMessage || statusText(head.status);
  return `${head.httpVersion} ${head.status}${reason ? ` ${reason}` : ''}`;
}

function pseudo(headers: Headers, name: string): string | undefined {
  for (const [k, v] of headers) if (k === name) return v;
  return undefined;
}

/**
 * Headers as they should appear in the raw text: pseudo-headers removed, `:authority` turned
 * into a leading `Host` header when no `Host` header exists. Wire order is preserved.
 */
export function rawHeaders(headers: Headers): Headers {
  const out: Headers = [];
  let authority: string | undefined;
  let hasHost = false;
  for (const [k, v] of headers) {
    if (k.startsWith(':')) {
      if (k === ':authority') authority = v;
      continue;
    }
    if (k.toLowerCase() === 'host') hasHost = true;
    out.push([k, v]);
  }
  if (authority && !hasHost) out.unshift(['Host', authority]);
  return out;
}

/**
 * Render a full message. `body` of `null` means "not loaded / no body section". `eol` is `\n`
 * for display (CodeMirror) and `\r\n` for copying exact wire text.
 */
export function buildRawMessage(startLine: string, headers: Headers, body: RawBody | null, eol = '\n'): string {
  const lines = [startLine];
  for (const [k, v] of headers) lines.push(`${k}: ${v}`);
  let out = lines.join(eol) + eol + eol;
  if (!body || body.size === 0) return out;
  if (body.text != null) {
    out += body.text;
    if (body.truncated || body.captureTruncated) {
      const why = body.captureTruncated ? 'capture truncated' : 'truncated for display';
      out += `${eol}${eol}<… ${why} — ${formatBytes(body.size)} total>`;
    }
  } else {
    out += `<${body.size} bytes binary>`;
  }
  return out;
}

/** Raw request text from its head and (optional) body. */
export function buildRawRequest(head: RequestHead, body: RawBody | null, eol = '\n'): string {
  return buildRawMessage(requestStartLine(head), rawHeaders(head.headers), body, eol);
}

/** Raw response text from its head and (optional) body. */
export function buildRawResponse(head: ResponseHead, body: RawBody | null, eol = '\n'): string {
  return buildRawMessage(responseStartLine(head), rawHeaders(head.headers), body, eol);
}
