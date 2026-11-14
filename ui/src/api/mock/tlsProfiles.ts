/**
 * Mock upstream TLS fingerprint data: the `GET /api/tls-profiles` catalogue and the
 * `UpstreamInfo.fingerprint` of generated exchanges (as with `proxy.tlsProfile` =
 * `mirror-client`: browsers are mirrored, other clients get the native stack). Values are
 * the real ones the daemon reports. `?noImpersonate` simulates a build without the backend.
 */

import type { HttpVersion, TlsProfiles, UpstreamFingerprint } from '../types';

const CHROME_JA4 = 't13d1516h2_8daaf6152771_d8a2da3f94cd';
const FIREFOX_JA4 = 't13d1717h2_5b57614c22b0_3cbfd9057e0d';
const SAFARI_JA4 = 't13d2014h2_a09f3c656075_14788d8d241b';
const CHROME_H2 = '1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p';
const FIREFOX_H2 = '1:65536;2:0;4:131072;5:16384|12517377|0|m,p,a,s';
const SAFARI_H2 = '2:0;3:100;4:2097152;9:1|10420225|0|m,s,a,p';
const NATIVE_H2 = '2:0;4:2097152|1073676289|0|m,s,a,p';

export function mockTlsProfiles(available: boolean): TlsProfiles {
  return {
    available,
    ...(available ? { backend: 'BoringSSL' } : {}),
    profiles: [
      { id: 'native', label: 'Native', description: "Akev's own TLS stack (rustls). Always available.", available: true },
      {
        id: 'chrome',
        label: 'Chrome',
        description: "Google Chrome's ClientHello (randomised extension order, GREASE, X25519MLKEM768, ALPS, Brotli certificate compression, ECH GREASE) and HTTP/2 preface.",
        imitates: 'Chrome 133+ (desktop)',
        ja4: CHROME_JA4,
        http2: CHROME_H2,
        available,
      },
      {
        id: 'firefox',
        label: 'Firefox',
        description: "Mozilla Firefox's ClientHello (NSS extension order, X25519MLKEM768 + 3 key shares, delegated credentials, record size limit, ECH GREASE) and HTTP/2 preface.",
        imitates: 'Firefox 135+ (desktop)',
        ja4: FIREFOX_JA4,
        http2: FIREFOX_H2,
        available,
      },
      {
        id: 'safari',
        label: 'Safari',
        description: "Safari's ClientHello (GREASE, legacy 3DES suites, zlib certificate compression, padding) and HTTP/2 preface.",
        imitates: 'Safari 18 (macOS)',
        ja4: SAFARI_JA4,
        http2: SAFARI_H2,
        available,
      },
      {
        id: 'edge',
        label: 'Edge',
        description: 'Microsoft Edge: the same Chromium network stack, so the same fingerprint as Chrome.',
        imitates: 'Edge 133+ (desktop, Chromium)',
        ja4: CHROME_JA4,
        http2: CHROME_H2,
        available,
      },
      {
        id: 'mirror-client',
        label: 'Mirror client',
        description: "The intercepted client's own browser family (detected from its JA4 and User-Agent); native for clients that aren't a recognised browser.",
        available,
      },
    ],
  };
}

/** Whether the mock simulates an impersonation build (`?noImpersonate` turns it off). */
export function mockImpersonationAvailable(): boolean {
  const query = typeof window !== 'undefined' ? `${window.location.search}&${window.location.hash.split('?')[1] ?? ''}` : '';
  return !/[?&]noImpersonate(=|&|$)/.test(query);
}

/** The fingerprint a mirrored connection for `source` would report. Deterministic (no rng). */
export function mockUpstreamFingerprint(source: string, version: HttpVersion): UpstreamFingerprint {
  const h2 = version === 'HTTP/2';
  const browser = source === 'Chrome' ? 'chrome' : source === 'Firefox' ? 'firefox' : source === 'Safari' ? 'safari' : null;
  if (!browser || !mockImpersonationAvailable()) {
    return {
      profile: 'native',
      requested: 'mirror-client',
      note: browser ? 'browser profiles need an impersonation build of Akev (cargo feature `impersonate`); the native TLS stack was used' : "the client isn't a recognised browser; the native TLS stack was used",
      ja3: '1304886075fdf4f38d0c74f8d60f0fc5',
      ja3Text: '771,4866-4865-4867-49196-49195-52393-49200-49199-52392-255,23-45-11-13-35-10-51-0-16-43-5,29-23-24,0',
      ja4: h2 ? 't13d1011h2_61a7ad8aa9b6_3fcd1a44f3e3' : 't13d1011h1_61a7ad8aa9b6_3fcd1a44f3e3',
      ...(h2 ? { http2: NATIVE_H2 } : {}),
    };
  }
  const table = {
    chrome: { ja4: CHROME_JA4, http2: CHROME_H2, ja3: '4368383b29d30ec58958976bad0c498f', ja3Text: '771,4865-4866-4867-49195-49199-49196-49200-52393-52392-49171-49172-156-157-47-53,43-16-65037-18-51-23-35-45-13-65281-10-27-11-0-17613-5,4588-29-23-24,0' },
    firefox: { ja4: FIREFOX_JA4, http2: FIREFOX_H2, ja3: '6f7889b9fb1a62a9577e685c1fcfa919', ja3Text: '771,4865-4867-4866-49195-49199-52393-52392-49196-49200-49162-49161-49171-49172-156-157-47-53,0-23-65281-10-11-35-16-5-34-18-51-43-13-45-28-27-65037,4588-29-23-24-25-256-257,0' },
    safari: { ja4: SAFARI_JA4, http2: SAFARI_H2, ja3: '773906b0efdefa24a7f2b8eb6985bf37', ja3Text: '771,4865-4866-4867-49196-49195-52393-49200-49199-52392-49162-49161-49172-49171-157-156-53-47-49160-49170-10,0-23-65281-10-11-16-5-13-18-51-45-43-27-21,29-23-24-25,0' },
  }[browser];
  return {
    profile: browser,
    requested: 'mirror-client',
    ja3: table.ja3,
    ja3Text: table.ja3Text,
    ja4: h2 ? table.ja4 : `${table.ja4.slice(0, 8)}h1${table.ja4.slice(10)}`,
    ...(h2 ? { http2: table.http2 } : {}),
  };
}
