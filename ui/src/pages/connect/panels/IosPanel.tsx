import { CopyButton, QrCode } from '../../../components/ui';
import { metaStepNodes, Steps } from '../GuideView';
import { splitHostPort } from '../addresses';
import { KeyLine, LanRequired, PanelSection } from './kit';
import type { PanelProps } from './types';
import styles from './panels.module.css';

const DEFAULT_STEPS = [
  <>Connect the iPhone or iPad to the same Wi-Fi network as this computer.</>,
  <>
    Settings → Wi-Fi → ⓘ next to the network → <b>Configure Proxy</b> → Manual, with the server and port below.
  </>,
  <>Scan the QR code (or open the setup page in Safari) and tap “Download profile”.</>,
  <>
    Settings → General → <b>VPN & Device Management</b> → Akev CA → Install.
  </>,
  <>
    Settings → General → About → <b>Certificate Trust Settings</b> → enable full trust for Akev CA.
  </>,
];

/** iOS: setup-page QR + Wi-Fi proxy and profile trust steps. */
export function IosPanel({ meta, proxy }: PanelProps) {
  const lan = proxy.primaryLan;
  const fromLan = lan ? splitHostPort(lan) : null;
  // The daemon's host/port/setup URL are authoritative when present (it picks the best LAN address).
  const host = meta.host ?? fromLan?.host ?? null;
  const port = meta.port ?? fromLan?.port ?? proxy.port;
  const setupUrl = meta.setupUrl ?? proxy.setupUrl;
  const ready = proxy.lanEnabled && setupUrl != null && host != null;
  const steps = meta.steps.length ? metaStepNodes(meta.steps) : DEFAULT_STEPS;

  return (
    <>
      <LanRequired what="iPhones and iPads" />
      {ready && (
        <div className={styles.qrRow}>
          <QrCode value={setupUrl} size={132} className={styles.qr} label={`QR code for ${setupUrl}`} />
          <div className={styles.qrText}>
            <span className={styles.qrTitle}>Device setup page</span>
            <span className={styles.row}>
              <span className={styles.addr}>{setupUrl}</span>
              <CopyButton value={setupUrl} label="Copy setup page URL" toastMessage="Setup page URL copied" />
            </span>
            <KeyLine label="Server">
              <span className={styles.addr}>{host}</span>
              <CopyButton value={host} label="Copy server" toastMessage="Server copied" />
            </KeyLine>
            <KeyLine label="Port">
              <span className={`${styles.addr} tnum`}>{port}</span>
              {port != null && <CopyButton value={String(port)} label="Copy port" toastMessage="Port copied" />}
            </KeyLine>
            {meta.mobileconfigUrl && (
              <KeyLine label="Profile">
                <span className={styles.addr}>{meta.mobileconfigUrl}</span>
                <CopyButton value={meta.mobileconfigUrl} label="Copy profile URL" toastMessage="Profile URL copied" />
              </KeyLine>
            )}
          </div>
        </div>
      )}
      <PanelSection title="Steps">
        <Steps steps={steps} />
        {meta.instructions && <p className={styles.footnote}>{meta.instructions}</p>}
        {meta.notes.length > 0 ? (
          <ul className={styles.bullets}>
            {meta.notes.map((n) => (
              <li key={n}>{n}</li>
            ))}
          </ul>
        ) : (
          <p className={styles.footnote}>When you're done, set Configure Proxy back to Off — otherwise the device loses connectivity once Akev stops.</p>
        )}
      </PanelSection>
    </>
  );
}
