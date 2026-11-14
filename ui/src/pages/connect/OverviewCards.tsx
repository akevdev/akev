import clsx from 'clsx';
import { ArrowRight, CircleAlert, Download, Globe, Laptop, ShieldCheck, ShieldAlert, Smartphone, TriangleAlert, Wifi } from 'lucide-react';
import { useEffect, type ReactNode } from 'react';
import { api } from '../../api/client';
import { navigate } from '../../app/router';
import { Badge, Button, CopyButton, QrCode, Spinner, Switch, Tooltip } from '../../components/ui';
import { formatDate } from '../../lib/format';
import { downloadUrl } from '../../lib/platform';
import { useAppStore } from '../../state/app';
import { applyCaInfo } from './actions';
import { proxyUrl } from './addresses';
import { useLanToggle, useProxyInfo } from './useProxyInfo';
import styles from './OverviewCards.module.css';

function Card({ icon: Icon, title, badge, children, className }: { icon: typeof Globe; title: string; badge?: ReactNode; children: ReactNode; className?: string }) {
  return (
    <section className={clsx(styles.card, className)} aria-label={title}>
      <header className={styles.cardHeader}>
        <span className={styles.cardIcon} aria-hidden>
          <Icon size={14} strokeWidth={1.9} />
        </span>
        <h2 className={styles.cardTitle}>{title}</h2>
        {badge && <span className={styles.cardBadge}>{badge}</span>}
      </header>
      {children}
    </section>
  );
}

/** One `label  value  [copy]` row. */
function AddressRow({ label, icon: Icon, value, copyLabel, muted, hint }: { label: string; icon: typeof Globe; value: string; copyLabel: string; muted?: boolean; hint?: string }) {
  return (
    <div className={clsx(styles.addressRow, muted && styles.mutedRow)}>
      <Icon size={14} strokeWidth={1.75} className={styles.rowIcon} aria-hidden />
      <span className={styles.rowLabel}>{label}</span>
      <Tooltip content={hint}>
        <span className={clsx(styles.address, 'mono selectable')} tabIndex={hint ? 0 : undefined}>
          {value}
        </span>
      </Tooltip>
      <CopyButton value={value} label={copyLabel} toastMessage={`${copyLabel.replace(/^Copy /, '').replace(/^./, (c) => c.toUpperCase())} copied`} disabled={muted} />
    </div>
  );
}

export function ProxyCard() {
  const info = useProxyInfo();
  const connection = useAppStore((s) => s.connection);
  const lan = useLanToggle();
  const online = connection === 'open' && info.local != null;
  const lanAddress = info.lan[0];

  return (
    <Card
      icon={Globe}
      title="Proxy"
      badge={
        online ? (
          <Badge variant="success" dot size="sm">
            Listening
          </Badge>
        ) : (
          <Badge variant="warning" dot size="sm">
            {connection === 'connecting' ? 'Connecting' : 'Offline'}
          </Badge>
        )
      }
    >
      <div className={styles.primaryValue}>
        <span className={clsx(styles.bigAddress, 'mono selectable')}>{info.local ?? '—'}</span>
        {info.local && <CopyButton value={info.local} label="Copy proxy address" toastMessage="Proxy address copied" size="md" />}
      </div>
      <p className={styles.caption}>Point any HTTP client at this address, or use a one-click option below.</p>

      <div className={styles.divider} />

      <Switch
        checked={lan.checked}
        onCheckedChange={lan.toggle}
        disabled={lan.disabled}
        label={
          <span className={styles.switchLabel}>
            Allow other devices
            {lan.pending && <Spinner size={12} tone="muted" />}
          </span>
        }
        description={
          lan.checked ? (
            // Same slot as the off-state hint, so turning LAN on doesn't push the page down.
            <span className={styles.lanWarning} role="note">
              <TriangleAlert size={12} strokeWidth={2} aria-hidden />
              <span>Anyone on this network can use the proxy and see its traffic. Turn off on public Wi-Fi.</span>
            </span>
          ) : (
            'Phones, tablets, VMs and other computers on your network.'
          )
        }
      />
      {lanAddress ? (
        <AddressRow
          label="Network"
          icon={Wifi}
          value={lanAddress}
          copyLabel="Copy network address"
          muted={!lan.checked}
          hint={lan.checked ? `Use ${proxyUrl(lanAddress)} on other devices` : 'Turn on “Allow other devices” to use this address'}
        />
      ) : (
        lan.checked && (
          <p className={styles.inlineNote}>
            <CircleAlert size={13} strokeWidth={1.9} aria-hidden /> No network address found — connect to Wi-Fi or Ethernet.
          </p>
        )
      )}
    </Card>
  );
}

/** `AA:BB:…:YY:ZZ` (first and last 4 bytes). */
export function shortFingerprint(fp: string): string {
  const parts = fp.split(':');
  return parts.length > 10 ? `${parts.slice(0, 4).join(':')} … ${parts.slice(-4).join(':')}` : fp;
}

export function CertificateCard() {
  const ca = useAppStore((s) => s.status?.ca ?? null);

  // `status.ca` comes from `hello`; refresh it so a regeneration elsewhere shows up.
  useEffect(() => {
    let alive = true;
    api.ca.get().then(
      (fresh) => alive && applyCaInfo(fresh),
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);

  const days = ca?.daysRemaining ?? null;
  const badge =
    days == null ? null : days < 0 ? (
      <Badge variant="danger" icon={ShieldAlert} size="sm">
        Expired
      </Badge>
    ) : days < 60 ? (
      <Badge variant="warning" icon={ShieldAlert} size="sm">
        Expires soon
      </Badge>
    ) : (
      <Badge variant="success" icon={ShieldCheck} size="sm">
        Valid
      </Badge>
    );

  return (
    <Card icon={ShieldCheck} title="Certificate authority" badge={badge}>
      {ca ? (
        <>
          <div className={styles.kv}>
            <span className={styles.k}>Name</span>
            <span className={clsx(styles.v, 'selectable')}>{/CN=([^,]+)/.exec(ca.subject)?.[1] ?? ca.subject}</span>
            <span className={styles.k}>SHA-256</span>
            <span className={styles.vRow}>
              <Tooltip content={<span className="mono">{ca.sha256Fingerprint}</span>} multiline>
                {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
                <span className={clsx(styles.v, styles.fingerprint, 'mono selectable')} tabIndex={0}>
                  {shortFingerprint(ca.sha256Fingerprint)}
                </span>
              </Tooltip>
              <CopyButton value={ca.sha256Fingerprint} label="Copy fingerprint" toastMessage="Fingerprint copied" />
            </span>
            <span className={styles.k}>Expires</span>
            <span className={clsx(styles.v, days != null && days < 60 && styles.vWarn, 'tnum')}>
              {formatDate(ca.notAfter, 'medium')}
              <span className={styles.vMuted}> · {days != null && days < 0 ? `${-days} days ago` : `in ${days} days`}</span>
            </span>
          </div>
          <p className={styles.caption}>Browsers and terminals launched from Akev trust it automatically. Other clients need it installed once.</p>
          <div className={styles.cardFooter}>
            <Button size="sm" icon={Download} onClick={() => void downloadUrl(api.ca.pemUrl(), 'akev-ca.pem')}>
              PEM
            </Button>
            <Tooltip content="DER-encoded .crt — for Windows and Android">
              <Button size="sm" icon={Download} onClick={() => void downloadUrl(api.ca.derUrl(), 'akev-ca.crt')}>
                DER
              </Button>
            </Tooltip>
            <span className={styles.spacer} />
            <Button size="sm" variant="ghost" iconRight={ArrowRight} onClick={() => navigate('/settings/certificates')}>
              Trust & regenerate
            </Button>
          </div>
        </>
      ) : (
        <p className={styles.caption}>Certificate details appear once Akev is connected.</p>
      )}
    </Card>
  );
}

export function DeviceSetupCard({ onOpenGuide }: { onOpenGuide?: (kind: 'ios' | 'android' | 'wireguard') => void }) {
  const info = useProxyInfo();
  const lan = useLanToggle();
  const ready = lan.checked && info.setupUrl != null && !lan.pending;
  const qrValue = info.setupUrl ?? 'http://akev.cert/';

  return (
    <Card icon={Smartphone} title="Phone & tablet setup">
      <div className={styles.deviceBody}>
        <div className={clsx(styles.qrWrap, !ready && styles.qrDisabled)} aria-hidden={!ready}>
          <QrCode value={qrValue} size={112} label={ready ? `QR code for ${qrValue}` : 'QR code unavailable'} className={styles.qr} />
          {!ready && (
            <span className={styles.qrOverlay}>
              {lan.pending ? <Spinner size={16} tone="muted" /> : <Laptop size={18} strokeWidth={1.6} aria-hidden />}
            </span>
          )}
        </div>
        <div className={styles.deviceText}>
          {ready ? (
            <>
              <p className={styles.deviceLead}>Scan with the phone's camera to open the setup page.</p>
              <span className={styles.vRow}>
                <span className={clsx(styles.setupUrl, 'mono selectable')}>{info.setupUrl}</span>
                <CopyButton value={info.setupUrl ?? ''} label="Copy setup page URL" toastMessage="Setup page URL copied" />
              </span>
              <p className={styles.caption}>
                It installs the certificate. Then set the device's Wi-Fi proxy to <span className="mono">{info.primaryLan}</span>.
              </p>
            </>
          ) : (
            <>
              <p className={styles.deviceLead}>{lan.checked && !info.setupUrl ? 'No network address found.' : 'Devices need to reach this computer over the network.'}</p>
              <p className={styles.caption}>
                {lan.checked && !info.setupUrl ? 'Connect this computer to Wi-Fi or Ethernet.' : 'Allow other devices to show the setup QR code.'}
              </p>
              {!lan.checked && (
                <Button size="sm" variant="primary" icon={Wifi} onClick={() => lan.toggle(true)} loading={lan.pending} disabled={lan.disabled} className={styles.allowButton}>
                  Allow other devices
                </Button>
              )}
            </>
          )}
          {onOpenGuide && (
            <div className={styles.guideLinks}>
              <button type="button" className={styles.link} onClick={() => onOpenGuide('ios')}>
                iPhone & iPad guide
              </button>
              <span className={styles.dot} aria-hidden>
                ·
              </span>
              <button type="button" className={styles.link} onClick={() => onOpenGuide('android')}>
                Android guide
              </button>
              <span className={styles.dot} aria-hidden>
                ·
              </span>
              <button type="button" className={styles.link} onClick={() => onOpenGuide('wireguard')} title="Scan one QR code with the WireGuard app — no proxy settings">
                WireGuard
              </button>
            </div>
          )}
        </div>
      </div>
    </Card>
  );
}
