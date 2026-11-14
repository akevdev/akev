import { Download, RefreshCcw, ShieldAlert, ShieldCheck, TriangleAlert } from 'lucide-react';
import { useEffect, useState } from 'react';
import { api } from '../../../api/client';
import type { CaInfo } from '../../../api/types';
import { AppIcon, Badge, Button, confirm, CopyButton, SegmentedControl, Skeleton, toast } from '../../../components/ui';
import { formatDate } from '../../../lib/format';
import { downloadUrl } from '../../../lib/platform';
import { useAppStore } from '../../../state/app';
import { applyCaInfo, explainError } from '../../connect/actions';
import { GuideView, TrustView } from '../../connect/GuideView';
import { commonName, TRUST_TARGETS, trustGuide, type TrustTargetId } from '../../connect/snippets';
import { useCaFiles, useProxyInfo } from '../../connect/useProxyInfo';
import { setProxy, useDraft, useEdit } from '../context';
import { PairTable } from '../PairTable';
import { SettingRow, SettingsCard } from '../SettingRow';
import { CaTrustCard } from './CaTrustCard';
import { StringListEditor } from '../StringListEditor';
import { normalizeHost, validateAbsolutePath, validateHostPattern } from '../validation';
import styles from './sections.module.css';

function CaCard() {
  const storeCa = useAppStore((s) => s.status?.ca ?? null);
  const files = useCaFiles();
  const [ca, setCa] = useState<CaInfo | null>(storeCa);
  const [regenerating, setRegenerating] = useState(false);

  useEffect(() => {
    let alive = true;
    api.ca.get().then(
      (fresh) => {
        if (!alive) return;
        setCa(fresh);
        applyCaInfo(fresh);
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);
  useEffect(() => {
    if (storeCa) setCa(storeCa);
  }, [storeCa]);

  const regenerate = async () => {
    const ok = await confirm({
      title: 'Regenerate the CA certificate?',
      message:
        'Akev creates a new certificate authority. Every device, browser and tool that trusts the current one stops trusting Akev until you install the new certificate. Browsers and terminals launched from Akev pick it up automatically.',
      destructive: true,
      confirmLabel: 'Regenerate',
    });
    if (!ok) return;
    setRegenerating(true);
    try {
      const fresh = await api.ca.regenerate();
      setCa(fresh);
      applyCaInfo(fresh);
      toast.success('New CA certificate created', {
        description: 'Install it on devices that trusted the old one.',
        action: { label: 'Download', onClick: () => void downloadUrl(api.ca.pemUrl(), 'akev-ca.pem') },
      });
    } catch (e) {
      toast.error("Couldn't regenerate the certificate", { description: explainError(e), action: { label: 'Retry', onClick: () => void regenerate() } });
    } finally {
      setRegenerating(false);
    }
  };

  const days = ca?.daysRemaining ?? null;
  const badge =
    days == null ? null : days < 0 ? (
      <Badge variant="danger" icon={ShieldAlert}>
        Expired {-days} days ago
      </Badge>
    ) : days <= 30 ? (
      <Badge variant="warning" icon={ShieldAlert}>
        Expires in {days} days
      </Badge>
    ) : (
      <Badge variant="success" icon={ShieldCheck}>
        Valid · {days} days left
      </Badge>
    );

  return (
    <SettingsCard id="ca" title="Certificate authority" description="Akev signs certificates for intercepted HTTPS sites with this CA. It was generated on this machine and never leaves it." actions={badge}>
      <div id="setting-ca" data-setting="ca">
        {ca ? (
          <div className={styles.caGrid}>
            <span className={styles.caKey}>Subject</span>
            <span className={`${styles.caValue} selectable`}>{ca.subject}</span>
            <span className={styles.caKey}>SHA-256</span>
            <span className={styles.caValue}>
              <span className={styles.fingerprint}>
                {ca.sha256Fingerprint.split(':').map((part, i, all) => (
                  <span key={i}>
                    {part}
                    {i < all.length - 1 && (
                      <>
                        :<wbr />
                      </>
                    )}
                  </span>
                ))}
              </span>
              <CopyButton value={ca.sha256Fingerprint} label="Copy SHA-256 fingerprint" toastMessage="Fingerprint copied" />
            </span>
            <span className={styles.caKey}>SPKI (base64)</span>
            <span className={styles.caValue}>
              <span className={styles.fingerprint}>{ca.spkiFingerprint}</span>
              <CopyButton value={ca.spkiFingerprint} label="Copy SPKI fingerprint" toastMessage="SPKI fingerprint copied" />
            </span>
            <span className={styles.caKey}>Android file name</span>
            <span className={styles.caValue}>
              <span className={styles.fingerprint}>{ca.subjectHashOld}.0</span>
              <CopyButton value={`${ca.subjectHashOld}.0`} label="Copy Android file name" />
            </span>
            <span className={styles.caKey}>Valid</span>
            <span className={`${styles.caValue} tnum`}>
              {formatDate(ca.notBefore, 'long')} <span className={styles.muted}>→</span> {formatDate(ca.notAfter, 'long')}
            </span>
            {files.caPath && (
              <>
                <span className={styles.caKey}>File</span>
                <span className={styles.caValue}>
                  <span className={styles.fingerprint}>{files.caPath}</span>
                  <CopyButton value={files.caPath} label="Copy certificate path" toastMessage="Certificate path copied" />
                </span>
              </>
            )}
          </div>
        ) : (
          <div className={styles.caGrid}>
            {Array.from({ length: 4 }, (_, i) => (
              <span key={i} className={styles.contents}>
                <Skeleton width={90} height={11} />
                <Skeleton width="70%" height={11} />
              </span>
            ))}
          </div>
        )}
      </div>
      <div className={styles.caFooter} id="setting-regenerate" data-setting="regenerate">
        <Button icon={Download} onClick={() => void downloadUrl(api.ca.pemUrl(), 'akev-ca.pem')}>
          Download PEM
        </Button>
        <Button icon={Download} onClick={() => void downloadUrl(api.ca.derUrl(), 'akev-ca.crt')}>
          Download DER (.crt)
        </Button>
        <span className={styles.spacer} />
        <Button variant="subtle-danger" icon={RefreshCcw} loading={regenerating} onClick={() => void regenerate()}>
          Regenerate…
        </Button>
      </div>
    </SettingsCard>
  );
}

function TrustCard() {
  const subject = useAppStore((s) => s.status?.ca.subject ?? null);
  const proxy = useProxyInfo();
  const files = useCaFiles();
  const [target, setTarget] = useState<TrustTargetId>(proxy.platform ?? 'macos');
  const thisMachine = target === proxy.platform;
  const guide = trustGuide(target, { caName: commonName(subject), providedCaPath: thisMachine ? files.caPath : null, setupUrl: proxy.setupUrl });
  return (
    <SettingsCard
      id="trust"
      title="Trust by hand"
      description={
        thisMachine && files.caPath
          ? "Commands for other devices, the system-wide store, or when you'd rather do it yourself. They use the certificate file above."
          : 'Commands for other devices. Download the certificate above first.'
      }
    >
      <SettingRow
        id="trust"
        title="Platform"
        layout="stacked"
        control={
          <SegmentedControl<TrustTargetId>
            value={target}
            onValueChange={setTarget}
            items={TRUST_TARGETS.map((t) => ({ value: t.id, label: <span className={styles.inlineGroup}><AppIcon name={t.icon} size={13} />{t.label}</span>, 'aria-label': t.label }))}
            aria-label="Platform"
          />
        }
      >
        {thisMachine && files.trust ? <TrustView trust={files.trust} /> : <GuideView guide={guide} />}
      </SettingRow>
    </SettingsCard>
  );
}

/** Settings → Certificates. */
export function CertificatesSection() {
  const draft = useDraft();
  const edit = useEdit();
  return (
    <>
      <CaCard />
      <CaTrustCard />
      <TrustCard />
      {draft && (
        <SettingsCard id="upstream-tls" title="Upstream servers" description="How Akev authenticates to, and verifies, the servers your clients talk to.">
          <SettingRow id="client-certs" title="Client certificates" description="Present a certificate to servers that require mutual TLS. PKCS#12 (.p12 / .pfx) or PEM files; the host supports globs." layout="stacked">
            <PairTable<'host' | 'path' | 'passphrase'>
              columns={[
                { key: 'host', label: 'Host', placeholder: '*.internal.example.com', validate: validateHostPattern, normalize: normalizeHost, mono: true },
                { key: 'path', label: 'Certificate file', placeholder: '/path/to/client.p12', validate: validateAbsolutePath, mono: true, width: 'minmax(0, 1.2fr)' },
                { key: 'passphrase', label: 'Passphrase', placeholder: 'Passphrase', secret: true, optional: true, width: 'minmax(0, 0.7fr)' },
              ]}
              rows={draft.proxy.clientCertificates}
              onChange={(rows) =>
                void edit(
                  setProxy(
                    'clientCertificates',
                    rows.map((r) => ({ host: r.host ?? '', path: r.path ?? '', ...(r.passphrase ? { passphrase: r.passphrase } : {}) })),
                  ),
                )
              }
              noun="certificate"
              uniqueKey="host"
              emptyText="No client certificates."
              aria-label="Client certificates"
            />
          </SettingRow>
          <SettingRow id="trusted-cas" title="Trusted CA certificates" description="Extra root certificates to trust for upstream servers, e.g. your company's internal CA. PEM files." layout="stacked">
            <StringListEditor
              items={draft.proxy.trustedCaPaths}
              onChange={(items) => void edit(setProxy('trustedCaPaths', items))}
              validate={validateAbsolutePath}
              splitPaste={false}
              placeholder="/etc/ssl/certs/corp-root.pem"
              noun="certificate"
              emptyText="Only the system's trusted roots are used."
              aria-label="Trusted CA certificate paths"
            />
          </SettingRow>
          <SettingRow id="ignore-cert-errors" title="Ignore certificate errors" description="Accept invalid, expired or self-signed certificates from these hosts." layout="stacked">
            {draft.proxy.ignoreUpstreamCertErrors.length > 0 && (
              <p className={`${styles.callout} ${styles.calloutWarning}`} role="note">
                <TriangleAlert size={13} strokeWidth={1.9} aria-hidden />
                Traffic to these hosts could be intercepted by someone else without you noticing. Only list hosts you control.
              </p>
            )}
            <StringListEditor
              items={draft.proxy.ignoreUpstreamCertErrors}
              onChange={(items) => void edit(setProxy('ignoreUpstreamCertErrors', items))}
              validate={validateHostPattern}
              normalize={normalizeHost}
              placeholder="localhost, *.dev.internal…"
              noun="host"
              emptyText="Certificates are always verified."
              aria-label="Hosts whose certificate errors are ignored"
            />
          </SettingRow>
        </SettingsCard>
      )}
    </>
  );
}
