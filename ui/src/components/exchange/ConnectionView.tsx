import clsx from 'clsx';
import { Check, ChevronRight, Fingerprint, Laptop, Link2, Lock, LockOpen, Server, ShieldAlert, ShieldCheck, ShieldQuestionMark, ShieldX, TriangleAlert, Wand2, XCircle } from 'lucide-react';
import { useMemo, useState, type ReactNode } from 'react';
import type { CertificateChain, CertificateSummary, ExchangeDetail } from '../../api/types';
import { formatDate } from '../../lib/format';
import { describeError } from '../../lib/http';
import { summarizeFingerprint } from '../../lib/tlsProfiles';
import { useAppStore } from '../../state/app';
import { AppIcon, Badge } from '../ui';
import { alpnLabel, certValidity, dnField, formatFingerprint, PROXY_MODE_LABELS, tlsVersionInfo, validityLabel } from './model/cert';
import { ANCHOR_SOURCE_SHORT, ANCHOR_SOURCES, chainModel, ROLE_LABELS, shortName, trustExplanation, type ChainNode } from './model/chain';
import { Chip, InfoGrid, type InfoRow } from './parts/Parts';
import styles from './ConnectionView.module.css';

export interface ConnectionViewProps {
  detail: ExchangeDetail;
  className?: string;
}

function Card({ icon: Icon, title, badge, children, className }: { icon: typeof Laptop; title: string; badge?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={clsx(styles.card, className)} aria-label={title}>
      <header className={styles.cardHead}>
        <Icon size={14} strokeWidth={2} aria-hidden />
        <span className={styles.cardTitle}>{title}</span>
        <span className={styles.spacer} />
        {badge}
      </header>
      {children}
    </section>
  );
}

function TlsBadge({ version }: { version?: string }) {
  const v = tlsVersionInfo(version);
  if (!v) return null;
  return (
    <Badge size="sm" variant={v.tone === 'success' ? 'success' : v.tone === 'warning' ? 'warning' : 'neutral'}>
      {v.label}
    </Badge>
  );
}

const SAN_LIMIT = 8;

const STATUS_LABELS = { valid: 'Valid', expiring: 'Expires soon', expired: 'Expired', 'not-yet-valid': 'Not yet valid', unknown: 'Unknown validity' } as const;

function validityVariant(status: keyof typeof STATUS_LABELS): 'success' | 'danger' | 'warning' {
  return status === 'valid' ? 'success' : status === 'expired' ? 'danger' : 'warning';
}

/** Validity bar + subject, issuer, SANs, serial and SHA-256 of one certificate. */
function CertificateDetails({ cert }: { cert: CertificateSummary }) {
  const [allSans, setAllSans] = useState(false);
  const v = certValidity(cert);
  const sans = allSans ? cert.san : cert.san.slice(0, SAN_LIMIT);
  return (
    <>
      <div className={styles.validity}>
        <div className={styles.validityTrack} role="meter" aria-label="Certificate lifetime used" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(v.elapsed * 100)}>
          <span className={clsx(styles.validityFill, styles[v.status])} style={{ width: `${Math.max(2, v.elapsed * 100)}%` }} />
        </div>
        <div className={clsx(styles.validityLabels, 'tnum')}>
          <span>{Number.isFinite(v.notBefore) ? formatDate(v.notBefore) : cert.notBefore}</span>
          <strong className={styles[v.status]}>{validityLabel(v)}</strong>
          <span>{Number.isFinite(v.notAfter) ? formatDate(v.notAfter) : cert.notAfter}</span>
        </div>
      </div>
      <div className={styles.certBody}>
        <InfoGrid
          rows={[
            { label: 'Subject', value: cert.subject, mono: true, copy: cert.subject },
            { label: 'Issuer', value: cert.issuer, mono: true, copy: cert.issuer },
            {
              label: `SANs (${cert.san.length})`,
              value: cert.san.length ? (
                <span className={styles.chips}>
                  {sans.map((s) => (
                    <Chip key={s}>{s}</Chip>
                  ))}
                  {cert.san.length > SAN_LIMIT && (
                    <button type="button" className={styles.moreButton} onClick={() => setAllSans((a) => !a)}>
                      {allSans ? 'Show fewer' : `+${cert.san.length - SAN_LIMIT} more`}
                    </button>
                  )}
                </span>
              ) : null,
              copy: cert.san.join('\n'),
              hidden: cert.san.length === 0,
            },
            { label: 'Serial', value: cert.serial, mono: true, copy: cert.serial, hidden: !cert.serial },
            { label: 'SHA-256', value: <span className={styles.fingerprint}>{formatFingerprint(cert.sha256Fingerprint)}</span>, copy: formatFingerprint(cert.sha256Fingerprint) },
          ]}
        />
      </div>
    </>
  );
}

/** The leaf certificate alone (exchanges recorded before Akev kept the whole chain). */
function CertificateCard({ cert }: { cert: CertificateSummary }) {
  const v = certValidity(cert);
  const cn = dnField(cert.subject, 'CN') ?? cert.subject;
  const issuerCn = dnField(cert.issuer, 'CN') ?? cert.issuer;
  const issuerOrg = dnField(cert.issuer, 'O');
  const Icon = v.status === 'valid' ? ShieldCheck : v.status === 'expired' ? ShieldX : ShieldAlert;
  return (
    <Card
      icon={Lock}
      title="Server certificate"
      className={styles.aCert}
      badge={
        <Badge size="sm" variant={validityVariant(v.status)}>
          {STATUS_LABELS[v.status]}
        </Badge>
      }
    >
      <div className={styles.certHero}>
        <span className={clsx(styles.certIcon, styles[v.status])} aria-hidden>
          <Icon size={18} strokeWidth={1.9} />
        </span>
        <div className={styles.certTitle}>
          <div className={styles.certName}>{cn}</div>
          <div className={styles.certIssuer}>
            Issued by {issuerCn}
            {issuerOrg && issuerOrg !== issuerCn ? ` · ${issuerOrg}` : ''}
          </div>
        </div>
      </div>
      <CertificateDetails cert={cert} />
    </Card>
  );
}

function ChainNodeRow({ node, open, onToggle }: { node: ChainNode; open: boolean; onToggle: () => void }) {
  const { cert, role } = node;
  const v = certValidity(cert);
  const Icon = role === 'root' ? ShieldCheck : role === 'leaf' ? Lock : Link2;
  return (
    <li className={clsx(styles.chainNode, role === 'unused' && styles.chainUnused)}>
      <button type="button" className={styles.chainHead} aria-expanded={open} onClick={onToggle}>
        <span className={clsx(styles.chainDot, styles[`role-${role}`])} aria-hidden>
          <Icon size={12} strokeWidth={2.2} />
        </span>
        <span className={styles.chainText}>
          <span className={styles.chainName}>{shortName(cert.subject)}</span>
          <span className={styles.chainMeta}>
            {role === 'unused' ? 'Not part of the verified path · ' : ''}
            Issued by {shortName(cert.issuer)}
          </span>
        </span>
        <span className={styles.chainBadges}>
          <Badge size="sm" variant={role === 'root' ? 'success' : 'neutral'}>
            {ROLE_LABELS[role]}
          </Badge>
          <Badge size="sm" variant={validityVariant(v.status)}>
            {STATUS_LABELS[v.status]}
          </Badge>
          <ChevronRight size={14} className={clsx(styles.chainChevron, open && styles.chainChevronOpen)} aria-hidden />
        </span>
      </button>
      {open && (
        <div className={styles.chainDetails}>
          <CertificateDetails cert={cert} />
        </div>
      )}
    </li>
  );
}

/** Every certificate the server presented, which ones Akev's verified path used, and the root it trusted. */
function CertificateChainCard({ chain }: { chain: CertificateChain }) {
  const model = useMemo(() => chainModel(chain), [chain]);
  const [open, setOpen] = useState<ReadonlySet<number>>(() => new Set([0]));
  const toggle = (i: number) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  const why = trustExplanation(model.trust);
  const TrustIcon = why.tone === 'success' ? ShieldCheck : why.tone === 'warning' ? ShieldAlert : ShieldQuestionMark;
  const anchor = model.externalAnchor;
  return (
    <Card
      icon={Lock}
      title="Certificate chain"
      className={styles.aCert}
      badge={
        <Badge size="sm" variant={why.tone === 'success' ? 'success' : why.tone === 'warning' ? 'warning' : 'neutral'}>
          {why.badge}
        </Badge>
      }
    >
      <div className={clsx(styles.trustNote, styles[`trust-${why.tone}`])}>
        <TrustIcon size={14} aria-hidden />
        <span>{why.text}</span>
      </div>
      <ol className={styles.chain} aria-label="Certificates in the order the server sent them">
        {model.nodes.map((node) => (
          <ChainNodeRow key={node.index} node={node} open={open.has(node.index)} onToggle={() => toggle(node.index)} />
        ))}
        {anchor && (
          <li className={clsx(styles.chainNode, styles.chainAnchor)}>
            <div className={styles.chainHead}>
              <span className={clsx(styles.chainDot, styles['role-root'])} aria-hidden>
                <ShieldCheck size={12} strokeWidth={2.2} />
              </span>
              <span className={styles.chainText}>
                <span className={styles.chainName}>{shortName(anchor.subject)}</span>
                <span className={styles.chainMeta}>Trusted root from {ANCHOR_SOURCES[anchor.source]} · not sent by the server</span>
              </span>
              <span className={styles.chainBadges}>
                <Badge size="sm" variant="success">
                  {ANCHOR_SOURCE_SHORT[anchor.source]}
                </Badge>
              </span>
            </div>
          </li>
        )}
      </ol>
    </Card>
  );
}

/**
 * Connection details (DESIGN.md §6.4): client socket/process/interceptor/mode, client TLS (SNI,
 * version, cipher, ALPN offered vs negotiated, JA3/JA4), upstream (address, proxy, reuse, TLS,
 * ALPN, the TLS fingerprint Akev presented: impersonation profile, JA3/JA4, HTTP/2) and the server's
 * certificate chain (every certificate it sent with subject, issuer, SANs, validity and SHA-256, the
 * verified path and the trusted root Akev's decision rested on).
 */
export function ConnectionView({ detail, className }: ConnectionViewProps) {
  const interceptors = useAppStore((s) => s.interceptors);
  const { client, upstream, request, response } = detail;
  const interceptorName = client.interceptor ? (interceptors.find((i) => i.id === client.interceptor)?.name ?? client.interceptor) : null;
  const interceptorIcon = client.interceptor ? (interceptors.find((i) => i.id === client.interceptor)?.icon ?? client.interceptor) : null;
  const tls = client.tls;
  const httpVersion = response && response.httpVersion !== request.httpVersion ? `${request.httpVersion} → ${response.httpVersion}` : request.httpVersion;

  const clientRows: InfoRow[] = [
    { label: 'Address', value: client.remoteAddr, mono: true, copy: client.remoteAddr },
    {
      label: 'Process',
      value: client.process ? (
        <span className={styles.app}>
          <AppIcon name={client.process.name} size={16} />
          {client.process.name}
        </span>
      ) : null,
      hint: client.process ? `PID ${client.process.pid}` : undefined,
      hidden: !client.process,
    },
    { label: 'Path', value: client.process?.path, mono: true, copy: client.process?.path, hidden: !client.process?.path },
    {
      label: 'Interceptor',
      value: interceptorName ? (
        <span className={styles.app}>
          <AppIcon name={interceptorIcon} size={16} />
          {interceptorName}
        </span>
      ) : null,
      hidden: !interceptorName,
    },
    { label: 'Proxy mode', value: PROXY_MODE_LABELS[client.via] ?? client.via },
    { label: 'Connection', value: <span className="tnum">#{client.connectionId}</span> },
    { label: 'HTTP version', value: httpVersion },
  ];

  const tlsRows: InfoRow[] = tls
    ? [
        { label: 'SNI', value: tls.sni, mono: true, copy: tls.sni, hidden: !tls.sni },
        { label: 'Version', value: <TlsBadge version={tls.version} />, hidden: !tls.version },
        { label: 'Cipher', value: tls.cipher, mono: true, copy: tls.cipher, hidden: !tls.cipher },
        {
          label: 'ALPN',
          value: (
            <span className={styles.chips}>
              {(tls.alpnOffered?.length ? tls.alpnOffered : tls.alpn ? [tls.alpn] : []).map((p) => (
                <Chip key={p} accent={p === tls.alpn} title={p === tls.alpn ? 'Negotiated' : 'Offered'}>
                  {p === tls.alpn && <Check size={10} strokeWidth={3} aria-hidden />}
                  {p}
                </Chip>
              ))}
            </span>
          ),
          hint: tls.alpn ? `negotiated ${alpnLabel(tls.alpn)}` : 'none negotiated',
          hidden: !tls.alpn && !tls.alpnOffered?.length,
        },
        { label: 'JA3', value: tls.ja3, mono: true, copy: tls.ja3, hidden: !tls.ja3 },
        { label: 'JA4', value: tls.ja4, mono: true, copy: tls.ja4, hidden: !tls.ja4 },
      ]
    : [];

  const fp = upstream?.fingerprint;
  const fpSummary = fp ? summarizeFingerprint(fp) : null;
  const upstreamRows: InfoRow[] = upstream
    ? [
        { label: 'Remote address', value: upstream.remoteAddr, mono: true, copy: upstream.remoteAddr, hidden: !upstream.remoteAddr },
        { label: 'Via proxy', value: upstream.viaProxy, mono: true, copy: upstream.viaProxy, hidden: !upstream.viaProxy },
        {
          label: 'Connection',
          value: (
            <Badge size="sm" variant={upstream.reused ? 'info' : 'neutral'}>
              {upstream.reused ? 'Reused' : 'New'}
            </Badge>
          ),
          hint: upstream.reused ? 'kept alive from an earlier exchange' : undefined,
        },
        { label: 'TLS version', value: <TlsBadge version={upstream.tlsVersion} />, hidden: !upstream.tlsVersion },
        { label: 'Cipher', value: upstream.cipher, mono: true, copy: upstream.cipher, hidden: !upstream.cipher },
        { label: 'ALPN', value: upstream.alpn ? <Chip accent>{upstream.alpn}</Chip> : null, hint: upstream.alpn ? alpnLabel(upstream.alpn) : undefined, hidden: !upstream.alpn },
        { label: 'HTTP version', value: response?.httpVersion, hidden: !response },
        // What the server saw of Akev's own TLS handshake (impersonation profile + JA3/JA4).
        {
          label: 'Fingerprint',
          value: fpSummary ? (
            <Badge size="sm" variant={fpSummary.tone === 'accent' ? 'accent' : fpSummary.tone === 'warning' ? 'warning' : 'neutral'}>
              <Fingerprint size={11} strokeWidth={2.25} aria-hidden />
              {fpSummary.label}
            </Badge>
          ) : null,
          hint: fpSummary?.hint,
          hidden: !fp,
        },
        { label: 'JA4 sent', value: fp?.ja4, mono: true, copy: fp?.ja4, hidden: !fp?.ja4 },
        // The copy button copies the full JA3 string (what fingerprint sites show next to the hash).
        { label: 'JA3 sent', value: fp?.ja3, mono: true, copy: fp?.ja3Text ?? fp?.ja3, hidden: !fp?.ja3 },
        { label: 'HTTP/2 sent', value: fp?.http2, mono: true, copy: fp?.http2, hint: fp?.http2 ? 'Akamai fingerprint' : undefined, hidden: !fp?.http2 },
      ]
    : [];

  const err = detail.error;
  return (
    <div className={clsx(styles.root, className)}>
      <div className={styles.grid}>
        <Card icon={Laptop} title="Client" className={styles.aClient}>
          <div className={styles.cardBody}>
            <InfoGrid rows={clientRows} />
          </div>
        </Card>
        <Card icon={tls ? Lock : LockOpen} title="Client TLS" className={styles.aTls} badge={tls ? <TlsBadge version={tls.version} /> : <Badge size="sm">Plain HTTP</Badge>}>
          {tls ? (
            <div className={styles.cardBody}>
              <InfoGrid rows={tlsRows} />
            </div>
          ) : (
            <div className={styles.note}>
              <LockOpen size={14} aria-hidden />
              <span>The client spoke plain {request.scheme === 'ws' ? 'WebSocket' : 'HTTP'} to Akev — nothing on this connection was encrypted.</span>
            </div>
          )}
        </Card>
        <Card icon={Server} title="Upstream" className={styles.aUpstream} badge={upstream ? <TlsBadge version={upstream.tlsVersion} /> : undefined}>
          {upstream ? (
            <>
              <div className={styles.cardBody}>
                <InfoGrid rows={upstreamRows} />
              </div>
              {fpSummary?.note && (
                <div className={styles.note}>
                  <TriangleAlert size={14} aria-hidden />
                  <span>{fpSummary.note.charAt(0).toUpperCase() + fpSummary.note.slice(1)}.</span>
                </div>
              )}
            </>
          ) : err ? (
            <div className={styles.note}>
              <XCircle size={14} aria-hidden />
              <span>
                {describeError(err.kind, request.host).title}. {err.message}
              </span>
            </div>
          ) : (
            <div className={styles.note}>
              <Wand2 size={14} aria-hidden />
              <span>No upstream connection — {detail.summary.mocked ? 'a rule answered this request directly.' : 'the response was produced by Akev.'}</span>
            </div>
          )}
        </Card>
        {upstream?.chain ? <CertificateChainCard chain={upstream.chain} /> : upstream?.certificate && <CertificateCard cert={upstream.certificate} />}
      </div>
    </div>
  );
}
