import clsx from 'clsx';
import { ArrowUpCircle, BadgeCheck, CirclePause, FlaskConical, HardDrive, ShieldAlert, ShieldCheck, WifiOff } from 'lucide-react';
import { type ReactNode, useEffect } from 'react';
import { errorMessage } from '../api/client';
import { AppIcon, Tooltip, toast } from '../components/ui';
import { formatBytes, formatNumber } from '../lib/format';
import { activeLicense, formatLicenseDate, licenseeFull, licenseeName } from '../lib/license';
import { copyText } from '../lib/platform';
import { limitCounters } from '../pages/settings/limits';
import { proxyAddress, useActiveInterceptors, useAppStore } from '../state/app';
import { reconnectNow } from '../state/connection';
import { useCaTrust, watchCaTrust } from '../state/caTrust';
import { useLicense } from '../state/license';
import { useUpdate, watchUpdates } from '../state/update';
import { useTraffic, useTrafficCounts } from '../state/traffic';
import { useWsToolsStore } from '../state/wsTools';
import { navigate } from './router';
import styles from './StatusBar.module.css';

function Segment({ tip, onClick, children, className, label }: { tip: ReactNode; onClick?: () => void; children: ReactNode; className?: string; label?: string }) {
  const content = onClick ? (
    <button type="button" className={clsx(styles.segment, styles.clickable, className)} onClick={onClick} aria-label={label}>
      {children}
    </button>
  ) : (
    // biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip
    <span className={clsx(styles.segment, className)} role="group" tabIndex={0} aria-label={label}>
      {children}
    </span>
  );
  return (
    <Tooltip content={tip} side="top">
      {content}
    </Tooltip>
  );
}

/** Bottom status bar: proxy, recording, counts, breakpoints, memory, CA, interceptors, licensee, connection. */
export function StatusBar() {
  const status = useAppStore((s) => s.status);
  const stats = useAppStore((s) => s.stats);
  const loadingOlder = useTraffic((s) => s.loadingOlder);
  const recording = useAppStore((s) => s.recording);
  const setRecording = useAppStore((s) => s.setRecording);
  const paused = useAppStore((s) => s.paused);
  // WebSocket messages held at ws-breakpoints count too.
  const pausedMessages = useWsToolsStore((s) => s.paused);
  const pausedTotal = paused.length + pausedMessages.length;
  const firstPausedId = paused[0]?.id ?? pausedMessages[0]?.id;
  const connection = useAppStore((s) => s.connection);
  const mock = useAppStore((s) => s.mock);
  const active = useActiveInterceptors();
  const counts = useTrafficCounts();
  const address = proxyAddress(status);
  const ca = status?.ca;
  const caExpired = ca != null && (ca.expiry === 'expired' || ca.daysRemaining < 0);
  const caExpiring = ca != null && !caExpired && (ca.expiry === 'expiring' || ca.daysRemaining < 60);
  const license = activeLicense(useLicense());
  const live = connection === 'open';
  const caTrust = useCaTrust((s) => s.report);
  useEffect(() => (live ? watchCaTrust() : undefined), [live]);
  // Update checks start once connected (the daemon decides whether automatic checks are on).
  useEffect(() => (live ? watchUpdates() : undefined), [live]);
  const update = useUpdate((s) => s.info);
  const caUntrusted = caTrust?.trusted === false;
  const caWarn = caExpired || caExpiring || caUntrusted;

  const copyAddress = async () => {
    if (!address) return;
    if (await copyText(address)) toast.success('Proxy address copied', { description: address });
  };

  const toggleRecording = () => {
    setRecording().catch((e: unknown) => toast.error("Couldn't change recording state", { description: errorMessage(e) }));
  };

  return (
    <footer className={styles.bar} role="contentinfo" aria-label="Status">
      <div className={styles.group}>
        <Segment
          tip={live ? `Proxy listening on ${status?.proxy.addresses.join(', ') || address} — click to copy` : 'Proxy status unknown (not connected)'}
          onClick={copyAddress}
          label="Copy proxy address"
        >
          <span className={clsx(styles.dot, live ? styles.ok : styles.bad)} aria-hidden />
          <span>Proxy</span>
          <span className={clsx(styles.strong, 'tnum')}>{address ?? '—'}</span>
        </Segment>
        <Sep />
        <Segment tip={recording ? 'Recording traffic — click to pause (traffic is still proxied)' : 'Recording paused — click to resume'} onClick={toggleRecording} label={recording ? 'Pause recording' : 'Resume recording'}>
          {recording ? (
            <>
              <span className={clsx(styles.dot, styles.rec)} aria-hidden />
              <span>Recording</span>
            </>
          ) : (
            <>
              <span className={styles.pausedIcon} aria-hidden>
                <span />
                <span />
              </span>
              <span>Paused</span>
            </>
          )}
        </Segment>
        <Sep />
        <Segment tip={`${formatNumber(counts.visible)} shown of ${formatNumber(counts.total)} captured${loadingOlder > 0 ? ` · loading ${formatNumber(loadingOlder)} older` : ''}`} onClick={() => navigate('/traffic')}>
          <span className="tnum">
            <span className={styles.strong}>{formatNumber(counts.visible)}</span>
            {counts.visible !== counts.total && <span className={styles.dim}> / {formatNumber(counts.total)}</span>}
          </span>
          <span className={styles.dim}>{counts.total === 1 ? 'exchange' : 'exchanges'}</span>
        </Segment>
        {pausedTotal > 0 && (
          <>
            <Sep />
            <Segment tip="Paused at breakpoints — click to jump to the first" onClick={() => firstPausedId != null && navigate(`/traffic/${firstPausedId}`)} className={styles.warning}>
              <CirclePause size={12} strokeWidth={2} aria-hidden />
              <span className="tnum">{pausedTotal} paused</span>
            </Segment>
          </>
        )}
        {stats && (
          <>
            <Sep />
            <Segment tip={[`Bodies in memory: ${formatBytes(stats.memoryBodyBytes)}`, `spilled to disk: ${formatBytes(stats.spilledBodyBytes)}`, `${formatNumber(stats.activeConnections)} open connections`, ...limitCounters(stats)].join(' · ')}>
              <HardDrive size={12} strokeWidth={1.75} aria-hidden />
              <span className="tnum">{formatBytes(stats.memoryBodyBytes)}</span>
              {stats.spilledBodyBytes > 0 && <span className={clsx(styles.dim, 'tnum')}>+{formatBytes(stats.spilledBodyBytes)}</span>}
            </Segment>
          </>
        )}
        {ca && (
          <>
            <Sep />
            <Segment
              tip={
                caExpired
                  ? 'The Akev CA has expired, so HTTPS interception fails. Regenerate it in Settings → Certificates and trust the new one on your devices.'
                  : caExpiring
                    ? `The Akev CA expires in ${ca.daysRemaining} days. Regenerate it in Settings → Certificates and trust the new one on your devices.`
                    : caUntrusted
                      ? "This computer doesn't trust the Akev CA yet: apps Akev didn't launch will reject its certificates. Trust it in Settings → Certificates."
                      : caTrust?.trusted
                        ? `Trusted by this computer · valid for ${ca.daysRemaining} more days`
                        : `CA certificate valid for ${ca.daysRemaining} more days`
              }
              onClick={() => navigate('/settings/certificates')}
              className={caWarn ? styles.warning : undefined}
            >
              {caWarn ? <ShieldAlert size={12} strokeWidth={1.75} aria-hidden /> : <ShieldCheck size={12} strokeWidth={1.75} aria-hidden />}
              <span>{caExpired ? 'CA expired' : caExpiring ? `CA expires in ${ca.daysRemaining}d` : caUntrusted ? 'CA not trusted' : 'CA'}</span>
            </Segment>
          </>
        )}
      </div>
      <div className={styles.group}>
        {update?.available && update.latest && (
          <Segment tip={`Akev ${update.latest} is available — see Settings → About`} onClick={() => navigate('/settings/about')}>
            <ArrowUpCircle size={12} strokeWidth={1.75} aria-hidden />
            <span>Update available</span>
          </Segment>
        )}
        {mock && (
          <Segment tip="Running against the in-browser mock backend (?mock). No real traffic is captured.">
            <FlaskConical size={12} strokeWidth={1.75} aria-hidden />
            <span>Mock data</span>
          </Segment>
        )}
        {active.length > 0 && (
          <>
            <Segment tip={`Active: ${active.map((a) => a.name).join(', ')}`} onClick={() => navigate('/connect')}>
              <span className={styles.icons}>
                {active.slice(0, 3).map((a) => (
                  <AppIcon key={a.id} name={a.icon} size={12} />
                ))}
              </span>
              <span className={styles.truncate}>{active.map((a) => a.name).join(', ')}</span>
            </Segment>
          </>
        )}
        {license && (
          <Segment
            tip={`Akev Pro, licensed to ${licenseeFull(license)} · updates until ${formatLicenseDate(license.updatesUntil)}`}
            onClick={() => navigate('/settings/license')}
            label={`Akev Pro, licensed to ${licenseeName(license)}`}
          >
            <BadgeCheck size={12} strokeWidth={1.75} aria-hidden />
            <span className={styles.truncate}>
              <span className={styles.dim}>Licensed to </span>
              {licenseeName(license)}
            </span>
          </Segment>
        )}
        {connection !== 'open' && (
          <Segment tip={connection === 'unauthorized' ? 'Akev rejected this session token' : 'Lost connection to Akev. Click to retry now.'} onClick={reconnectNow}>
            <span className={styles.pill}>
              <WifiOff size={11} strokeWidth={2} aria-hidden />
              {connection === 'unauthorized' ? 'Unauthorized' : connection === 'connecting' ? 'Connecting…' : 'Reconnecting…'}
            </span>
          </Segment>
        )}
      </div>
    </footer>
  );
}

function Sep() {
  return <span className={styles.sep} aria-hidden />;
}
