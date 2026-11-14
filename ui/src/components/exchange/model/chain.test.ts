import { describe, expect, test } from 'bun:test';
import type { CertificateChain, CertificateSummary } from '../../../api/types';
import { chainModel, shortName, trustExplanation } from './chain';

function cert(subject: string, issuer: string): CertificateSummary {
  return { subject, issuer, san: [], notBefore: '', notAfter: '', serial: '', sha256Fingerprint: '' };
}

const LEAF = cert('CN=api.example.com', 'CN=R11, O=Let\'s Encrypt');
const INTERMEDIATE = cert("CN=R11, O=Let's Encrypt", 'CN=ISRG Root X1, O=Internet Security Research Group');
const ROOT = cert('CN=ISRG Root X1, O=Internet Security Research Group', 'CN=ISRG Root X1, O=Internet Security Research Group');
const STRAY = cert('CN=Old Cross-sign', 'CN=DST Root CA X3');

describe('certificate chain model', () => {
  test('roles follow the verified path; the root the server sent is the anchor', () => {
    const chain: CertificateChain = {
      certificates: [LEAF, STRAY, INTERMEDIATE, ROOT],
      trust: { status: 'verified', path: [0, 2], anchor: { subject: ROOT.subject, index: 3, source: 'built-in' } },
    };
    const m = chainModel(chain);
    expect(m.nodes.map((n) => n.role)).toEqual(['leaf', 'unused', 'intermediate', 'root']);
    expect(m.externalAnchor).toBeNull();
    const why = trustExplanation(m.trust);
    expect(why.tone).toBe('success');
    expect(why.text).toContain('ISRG Root X1');
    expect(why.text).toContain("Mozilla's root store");
    expect(why.text).toContain('The server sent this root too.');
  });

  test("a root the server didn't send is shown after the presented certificates", () => {
    const m = chainModel({ certificates: [LEAF, INTERMEDIATE], trust: { status: 'verified', path: [0, 1], anchor: { subject: ROOT.subject, source: 'custom' } } });
    expect(m.nodes.map((n) => n.role)).toEqual(['leaf', 'intermediate']);
    expect(m.externalAnchor?.subject).toBe(ROOT.subject);
    expect(trustExplanation(m.trust).text).toContain('Settings › Certificates');
  });

  test('unverified chains are labelled by shape and explained', () => {
    const m = chainModel({ certificates: [LEAF, INTERMEDIATE, ROOT], trust: { status: 'not-verified' } });
    expect(m.nodes.map((n) => n.role)).toEqual(['leaf', 'intermediate', 'root']);
    expect(m.externalAnchor).toBeNull();
    const why = trustExplanation(m.trust);
    expect(why).toMatchObject({ tone: 'warning', badge: 'Not verified' });
    expect(trustExplanation({ status: 'unknown' }).tone).toBe('neutral');
  });

  test('short names', () => {
    expect(shortName(INTERMEDIATE.subject)).toBe('R11');
    expect(shortName('O=Acme')).toBe('O=Acme');
    expect(shortName('')).toBe('Unnamed certificate');
  });
});
