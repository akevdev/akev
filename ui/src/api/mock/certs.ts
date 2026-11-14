/** Mock upstream certificates: a leaf plus the chain the server presented and how it was trusted. */

import type { CertificateChain, CertificateSummary } from '../types';
import type { Rng } from './rng';

interface Issuer {
  intermediate: string;
  root: string;
  source: 'built-in' | 'system';
}

const ISSUERS: readonly Issuer[] = [
  { intermediate: "CN=R11, O=Let's Encrypt, C=US", root: 'CN=ISRG Root X1, O=Internet Security Research Group, C=US', source: 'built-in' },
  { intermediate: 'CN=DigiCert Global G2 TLS RSA SHA256 2020 CA1, O=DigiCert Inc, C=US', root: 'CN=DigiCert Global Root G2, OU=www.digicert.com, O=DigiCert Inc, C=US', source: 'built-in' },
  { intermediate: 'CN=WE1, O=Google Trust Services, C=US', root: 'CN=GTS Root R4, O=Google Trust Services LLC, C=US', source: 'system' },
];

const DAY = 86_400_000;

function fingerprint(r: Rng): string {
  return Array.from({ length: 32 }, () => r.int(0, 255).toString(16).padStart(2, '0').toUpperCase()).join(':');
}

function serial(r: Rng): string {
  return r.int(1e8, 9e8).toString(16).toUpperCase();
}

/**
 * The leaf for `host` and its chain. `expired` makes an expired, unverified leaf (like
 * expired.badssl.com with certificate errors ignored).
 */
export function mockCertificates(r: Rng, host: string, start: number, expired: boolean): { certificate: CertificateSummary; chain: CertificateChain } {
  const issuer = r.pick(ISSUERS);
  const domain = host.split('.').slice(-2).join('.');
  const certificate: CertificateSummary = {
    subject: `CN=${host}`,
    issuer: expired ? 'CN=BadSSL Expired' : issuer.intermediate,
    san: [host, `*.${domain}`],
    notBefore: new Date(start - 40 * DAY).toISOString(),
    notAfter: expired ? new Date(start - 900 * DAY).toISOString() : new Date(start + 50 * DAY).toISOString(),
    serial: serial(r),
    sha256Fingerprint: fingerprint(r),
  };
  if (expired) {
    return { certificate, chain: { certificates: [certificate], trust: { status: 'not-verified' } } };
  }
  const intermediate: CertificateSummary = {
    subject: issuer.intermediate,
    issuer: issuer.root,
    san: [],
    notBefore: new Date(start - 400 * DAY).toISOString(),
    notAfter: new Date(start + 700 * DAY).toISOString(),
    serial: serial(r),
    sha256Fingerprint: fingerprint(r),
  };
  // Some servers send their root too.
  const sendsRoot = r.chance(0.2);
  const root: CertificateSummary = {
    subject: issuer.root,
    issuer: issuer.root,
    san: [],
    notBefore: new Date(start - 3000 * DAY).toISOString(),
    notAfter: new Date(start + 4000 * DAY).toISOString(),
    serial: serial(r),
    sha256Fingerprint: fingerprint(r),
  };
  const certificates = sendsRoot ? [certificate, intermediate, root] : [certificate, intermediate];
  return {
    certificate,
    chain: {
      certificates,
      trust: { status: 'verified', path: [0, 1], anchor: { subject: issuer.root, index: sendsRoot ? 2 : undefined, source: issuer.source } },
    },
  };
}
