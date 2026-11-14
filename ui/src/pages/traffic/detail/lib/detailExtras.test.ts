import { describe, expect, it } from 'bun:test';
import { base64ToText } from '../../../../lib/encoding';
import { RESPOND_PRESETS, presetResponse, syncHostHeader, urlAuthority } from './breakpoint';
import { escapedPrefix, sniffProtocol } from './sniff';
import { fitTabs, sameIndices, stripWidth } from './tabsFit';

describe('tab overflow fitting', () => {
  const widths = [100, 80, 60, 90, 70];
  const opts = { gap: 2, padding: 8, more: 50 };

  it('measures a strip with padding and gaps', () => {
    expect(stripWidth(widths, [0, 1], 2, 8)).toBe(8 + 100 + 2 + 80);
    expect(stripWidth(widths, [], 2, 8)).toBe(8);
  });

  it('shows everything when all tabs fit (no room reserved for "More")', () => {
    const all = stripWidth(widths, [0, 1, 2, 3, 4], 2, 8);
    expect(fitTabs(widths, all, 0, opts)).toEqual([0, 1, 2, 3, 4]);
    expect(fitTabs(widths, 2000, 3, opts)).toEqual([0, 1, 2, 3, 4]);
  });

  it('keeps the leading tabs that fit next to the More button', () => {
    // Budget 320 - 50 = 270: 8 + 100 + 2 + 80 + 2 + 60 = 252 fits; adding 90 overflows.
    expect(fitTabs(widths, 320, 0, opts)).toEqual([0, 1, 2]);
    // Budget 250: the third tab (→ 252) no longer fits.
    expect(fitTabs(widths, 300, 0, opts)).toEqual([0, 1]);
  });

  it('swaps the active tab in when it would overflow, preserving order', () => {
    // [0,1,4] = 262 ≤ 270.
    expect(fitTabs(widths, 320, 4, opts)).toEqual([0, 1, 4]);
    // [0,1,3] = 282 > 270 → drop tab 1: [0,3] = 200.
    expect(fitTabs(widths, 320, 3, opts)).toEqual([0, 3]);
  });

  it('always shows the active tab, even when nothing else fits', () => {
    expect(fitTabs(widths, 40, 3, opts)).toEqual([3]);
    expect(fitTabs(widths, 40, -1, opts)).toEqual([0]);
    expect(fitTabs([], 100, -1, opts)).toEqual([]);
  });

  it('compares index lists', () => {
    expect(sameIndices([0, 1], [0, 1])).toBe(true);
    expect(sameIndices([0, 1], [0, 2])).toBe(false);
    expect(sameIndices(null, null)).toBe(true);
    expect(sameIndices(null, [0])).toBe(false);
  });
});

describe('Host header follows the URL', () => {
  const rows = [
    { name: 'Host', value: 'api.example.com', enabled: true },
    { name: 'Accept', value: '*/*', enabled: true },
  ];

  it('extracts the authority as a Host header carries it', () => {
    expect(urlAuthority('https://api.example.com/x')).toBe('api.example.com');
    expect(urlAuthority('https://api.example.com:443/x')).toBe('api.example.com');
    expect(urlAuthority('http://localhost:3000/')).toBe('localhost:3000');
    expect(urlAuthority('not a url')).toBeNull();
  });

  it('updates a Host that matched the old URL', () => {
    const next = syncHostHeader(rows, 'https://api.example.com/v1', 'https://staging.example.com:8443/v1');
    expect(next[0]?.value).toBe('staging.example.com:8443');
    expect(next[1]).toBe(rows[1] as (typeof rows)[number]);
  });

  it('leaves hand-edited hosts, same authorities and unparseable URLs alone', () => {
    const custom = [{ name: 'host', value: 'override.test', enabled: true }];
    expect(syncHostHeader(custom, 'https://api.example.com/', 'https://other.test/')).toBe(custom);
    expect(syncHostHeader(rows, 'https://api.example.com/a', 'https://API.example.com/b')).toBe(rows);
    expect(syncHostHeader(rows, 'https://api.example.com/', 'https://')).toBe(rows);
  });
});

describe('Respond presets', () => {
  it('have unique ids and valid statuses', () => {
    const ids = new Set(RESPOND_PRESETS.map((p) => p.id));
    expect(ids.size).toBe(RESPOND_PRESETS.length);
    for (const p of RESPOND_PRESETS) expect(p.status >= 100 && p.status <= 599).toBe(true);
  });

  it('build responses with canonical reasons, bodies and Content-Length', () => {
    const notFound = presetResponse(RESPOND_PRESETS.find((p) => p.id === 'not-found')!);
    expect(notFound.status).toBe(404);
    expect(notFound.statusMessage).toBe('Not Found');
    const text = base64ToText(notFound.body);
    expect(JSON.parse(text ?? '')).toEqual({ error: 'not_found' });
    expect(notFound.headers).toContainEqual(['content-length', String(new TextEncoder().encode(text ?? '').length)]);
  });

  it('send no body or Content-Length for 204', () => {
    const empty = presetResponse(RESPOND_PRESETS.find((p) => p.id === 'no-content')!);
    expect(empty.body).toBe('');
    expect(empty.headers.some(([k]) => k === 'content-length')).toBe(false);
  });
});

describe('protocol sniffing (client errors)', () => {
  const bytes = (...b: number[]) => new Uint8Array(b);
  const text = (s: string) => new TextEncoder().encode(s);

  it('recognises common non-HTTP protocols', () => {
    expect(sniffProtocol(bytes(0x16, 0x03, 0x01, 0x02, 0x00))?.id).toBe('tls');
    expect(sniffProtocol(text('PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n'))?.id).toBe('h2c');
    expect(sniffProtocol(text('SSH-2.0-OpenSSH_9.6\r\n'))?.id).toBe('ssh');
    expect(sniffProtocol(text('PROXY TCP4 10.0.0.1 10.0.0.2 5000 80\r\n'))?.id).toBe('proxy-protocol');
    expect(sniffProtocol(bytes(0x05, 0x01, 0x00))?.id).toBe('socks5');
    expect(sniffProtocol(bytes(0x04, 0x01, 0x00, 0x50))?.id).toBe('socks4');
  });

  it('tells malformed HTTP, other text and binary apart', () => {
    expect(sniffProtocol(text('GET /a b c HTTP/1.1\r\nHost: x\r\n\r\n'))?.id).toBe('http');
    expect(sniffProtocol(text('HELO mail.example.com\r\n'))?.id).toBe('text');
    expect(sniffProtocol(bytes(0x00, 0xff, 0x13, 0x37, 0x80, 0x81))?.id).toBe('binary');
    expect(sniffProtocol(bytes())).toBeNull();
  });

  it('escapes the first bytes for display', () => {
    expect(escapedPrefix(text('GET / HTTP/1.1\r\n'))).toBe('GET / HTTP/1.1\\r\\n');
    expect(escapedPrefix(bytes(0x16, 0x03, 0x5c))).toBe('\\x16\\x03\\\\');
    expect(escapedPrefix(text('abcdef'), 3)).toBe('abc…');
  });
});
