/**
 * Recognise what a client sent when it wasn't valid HTTP (client-error exchanges): the first
 * bytes usually give the protocol away — a TLS handshake on a plain-HTTP connection, an HTTP/2
 * prior-knowledge preface, SSH, SOCKS, the PROXY protocol or a malformed HTTP request.
 */

export type ProtocolId = 'tls' | 'h2c' | 'ssh' | 'socks4' | 'socks5' | 'proxy-protocol' | 'http' | 'text' | 'binary';

export interface ProtocolGuess {
  id: ProtocolId;
  /** Short name, e.g. `TLS handshake`. */
  label: string;
  /** What it means and what to do about it. */
  explanation: string;
}

const HTTP_METHOD = /^(GET|HEAD|POST|PUT|DELETE|CONNECT|OPTIONS|TRACE|PATCH|PROPFIND|PROPPATCH|MKCOL|COPY|MOVE|LOCK|UNLOCK|SEARCH|REPORT|PURGE) /;

function ascii(bytes: Uint8Array, n: number): string {
  let s = '';
  for (let i = 0; i < Math.min(n, bytes.length); i++) s += String.fromCharCode(bytes[i] ?? 0);
  return s;
}

function mostlyPrintable(bytes: Uint8Array, n = 64): boolean {
  const len = Math.min(n, bytes.length);
  if (len === 0) return false;
  let printable = 0;
  for (let i = 0; i < len; i++) {
    const b = bytes[i] ?? 0;
    if ((b >= 0x20 && b < 0x7f) || b === 0x0a || b === 0x0d || b === 0x09) printable++;
  }
  return printable / len >= 0.9;
}

/** Best guess for the protocol of `bytes` (`null` when empty). */
export function sniffProtocol(bytes: Uint8Array): ProtocolGuess | null {
  if (bytes.length === 0) return null;
  const b0 = bytes[0] ?? 0;
  const b1 = bytes[1] ?? 0;
  const head = ascii(bytes, 32);

  if (b0 === 0x16 && b1 === 0x03) {
    return {
      id: 'tls',
      label: 'TLS handshake',
      explanation: 'The client started a TLS handshake where Akev expected plain HTTP — usually an https:// URL sent to an http:// proxy setting, or a client that skipped CONNECT. Point it at the proxy as an HTTP proxy, or use https:// only in the target URL.',
    };
  }
  if (head.startsWith('PRI * HTTP/2.0')) {
    return {
      id: 'h2c',
      label: 'HTTP/2 prior knowledge',
      explanation: 'The client spoke cleartext HTTP/2 (h2c) without negotiating it. Enable HTTP/1.1 fallback in the client, or use TLS so HTTP/2 is negotiated via ALPN.',
    };
  }
  if (head.startsWith('SSH-')) {
    return { id: 'ssh', label: 'SSH', explanation: 'An SSH client connected to the proxy port. SSH can only pass through Akev as a CONNECT tunnel.' };
  }
  if (head.startsWith('PROXY TCP') || head.startsWith('PROXY UNKNOWN') || head.startsWith('\r\n\r\n\0\r\nQUIT\n')) {
    return { id: 'proxy-protocol', label: 'PROXY protocol', explanation: 'A load balancer sent a PROXY protocol header. Akev expects plain HTTP on this port — disable the PROXY protocol for it.' };
  }
  if (b0 === 0x05 && bytes.length >= 2 && b1 >= 1 && b1 <= 9) {
    return { id: 'socks5', label: 'SOCKS5 greeting', explanation: 'The client is configured to use a SOCKS proxy. Enable SOCKS in Settings → Proxy, or configure the client for an HTTP proxy.' };
  }
  if (b0 === 0x04 && (b1 === 0x01 || b1 === 0x02)) {
    return { id: 'socks4', label: 'SOCKS4 request', explanation: 'The client is configured to use a SOCKS4 proxy. Configure it for an HTTP proxy (or SOCKS5) instead.' };
  }
  if (HTTP_METHOD.test(head)) {
    return { id: 'http', label: 'Malformed HTTP', explanation: 'It looks like HTTP, but Akev couldn\'t parse it — often invalid characters in the request line or headers, or a broken chunked body.' };
  }
  if (mostlyPrintable(bytes)) {
    return { id: 'text', label: 'Text protocol', explanation: 'Readable text that isn\'t HTTP — probably another text protocol sent to the proxy port.' };
  }
  return { id: 'binary', label: 'Binary data', explanation: 'Unrecognised binary data — probably a non-HTTP protocol sent to the proxy port.' };
}

/**
 * The first bytes as one escaped line of text (`\r`, `\n`, `\t`, `\xNN` for anything else
 * non-printable), at most `max` source bytes, with `…` when cut.
 */
export function escapedPrefix(bytes: Uint8Array, max = 160): string {
  let out = '';
  const n = Math.min(max, bytes.length);
  for (let i = 0; i < n; i++) {
    const b = bytes[i] ?? 0;
    if (b === 0x0d) out += '\\r';
    else if (b === 0x0a) out += '\\n';
    else if (b === 0x09) out += '\\t';
    else if (b === 0x5c) out += '\\\\';
    else if (b >= 0x20 && b < 0x7f) out += String.fromCharCode(b);
    else out += `\\x${b.toString(16).padStart(2, '0')}`;
  }
  return bytes.length > max ? `${out}…` : out;
}
