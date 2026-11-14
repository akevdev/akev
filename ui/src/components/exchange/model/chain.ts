/** The upstream certificate chain for ConnectionView: roles and the trust explanation. No React. */

import type { CertificateChain, CertificateSummary, ChainTrust, TrustAnchorInfo } from '../../../api/types';
import { dnField } from './cert';

/**
 * - `leaf`: the server's own certificate;
 * - `intermediate`: part of the verified path (or, unverified, any later certificate);
 * - `root`: the trusted root, when the server sent it (or a self-issued certificate, unverified);
 * - `unused`: sent by the server but not part of the verified path.
 */
export type ChainRole = 'leaf' | 'intermediate' | 'root' | 'unused';

export interface ChainNode {
  cert: CertificateSummary;
  /** Position in the order the server sent the certificates. */
  index: number;
  role: ChainRole;
}

export interface ChainModel {
  nodes: ChainNode[];
  /** The trusted root when the server didn't send it (shown after the presented certificates). */
  externalAnchor: TrustAnchorInfo | null;
  trust: ChainTrust;
}

const SELF_ISSUED = (c: CertificateSummary) => c.subject !== '' && c.subject === c.issuer;

export function chainModel(chain: CertificateChain): ChainModel {
  const { trust } = chain;
  const verified = trust.status === 'verified' && trust.path != null && trust.path.length > 0;
  const inPath = new Set(trust.path ?? []);
  const anchorIndex = trust.anchor?.index;
  const nodes = chain.certificates.map((cert, index): ChainNode => {
    let role: ChainRole;
    if (index === 0) role = 'leaf';
    else if (index === anchorIndex) role = 'root';
    else if (verified) role = inPath.has(index) ? 'intermediate' : 'unused';
    else role = SELF_ISSUED(cert) ? 'root' : 'intermediate';
    return { cert, index, role };
  });
  return { nodes, externalAnchor: trust.anchor && anchorIndex == null ? trust.anchor : null, trust };
}

export const ROLE_LABELS: Readonly<Record<ChainRole, string>> = {
  leaf: 'Server certificate',
  intermediate: 'Intermediate',
  root: 'Root',
  unused: 'Not used',
};

export const ANCHOR_SOURCES: Readonly<Record<TrustAnchorInfo['source'], string>> = {
  'built-in': "Mozilla's root store, built into Akev",
  system: "your operating system's trust store",
  custom: 'a CA certificate you trusted in Settings › Certificates',
};

export const ANCHOR_SOURCE_SHORT: Readonly<Record<TrustAnchorInfo['source'], string>> = {
  'built-in': 'Built-in roots',
  system: 'System trust store',
  custom: 'Your trusted CA',
};

/** Short display name of a DN: its CN, else the whole name. */
export function shortName(dn: string): string {
  return dnField(dn, 'CN') ?? (dn || 'Unnamed certificate');
}

export interface TrustExplanation {
  tone: 'success' | 'warning' | 'neutral';
  badge: string;
  text: string;
}

/** What Akev's trust decision rested on, in plain words. */
export function trustExplanation(trust: ChainTrust): TrustExplanation {
  if (trust.status === 'not-verified') {
    return {
      tone: 'warning',
      badge: 'Not verified',
      text: "Akev didn't verify this chain: certificate errors are ignored for this host (Settings › Certificates) or for this request.",
    };
  }
  if (trust.status === 'verified' && trust.anchor) {
    const root = shortName(trust.anchor.subject);
    const sent = trust.anchor.index != null ? ' The server sent this root too.' : '';
    return {
      tone: 'success',
      badge: 'Trusted',
      text: `Akev trusted this chain because it leads to ${root}, a root in ${ANCHOR_SOURCES[trust.anchor.source]}.${sent}`,
    };
  }
  return { tone: 'neutral', badge: 'Trusted', text: "Akev accepted this chain, but couldn't rebuild the path to name the root it trusted." };
}
