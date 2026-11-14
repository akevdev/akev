import clsx from 'clsx';
import { ArrowRight, Check, Circle, Copy, FilterX, Radio, SearchX } from 'lucide-react';
import { useMemo, useState } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { InterceptorInfo } from '../../../api/types';
import { navigate } from '../../../app/router';
import { AppIcon, Button, Kbd, Spinner, toast } from '../../../components/ui';
import { formatNumber } from '../../../lib/format';
import { TYPE_CHIPS } from '../../../lib/http';
import { copyText } from '../../../lib/platform';
import { proxyAddress, useAppStore } from '../../../state/app';
import { trafficStore, useFilterState } from '../../../state/traffic';
import { toggleRecording } from '../actions/exchangeActions';
import styles from './EmptyStates.module.css';

/** The 3×2 grid on the empty state (DESIGN.md §6.3), in this order when available. */
const TOP_INTERCEPTORS: readonly (readonly string[])[] = [
  ['chrome', 'chromium', 'brave', 'edge'],
  ['firefox'],
  ['fresh-terminal', 'existing-terminal'],
  ['android-adb', 'android'],
  ['system-proxy'],
  ['manual-setup', 'manual'],
];

export function pickTop(all: readonly InterceptorInfo[]): InterceptorInfo[] {
  const byId = new Map(all.map((i) => [i.id, i]));
  const out: InterceptorInfo[] = [];
  for (const candidates of TOP_INTERCEPTORS) {
    const hit = candidates.map((id) => byId.get(id)).find((i) => i && i.supported);
    if (hit) out.push(hit);
  }
  // Fill up to six with other supported interceptors (keeps the grid complete).
  for (const i of all) {
    if (out.length >= 6) break;
    if (i.supported && !out.includes(i)) out.push(i);
  }
  return out.slice(0, 6);
}

/** One-click launch tile for an interceptor (`onActivated` runs after a successful launch). */
export function InterceptorTile({ info, onActivated }: { info: InterceptorInfo; onActivated?: () => void }) {
  const [busy, setBusy] = useState(false);
  const disabled = !info.available && !info.needsConfig;
  const action = info.active ? 'Active' : info.needsConfig ? 'Set up' : info.available ? 'Launch' : 'Unavailable';

  const run = async () => {
    if (info.needsConfig) {
      navigate(`/connect/${encodeURIComponent(info.id)}`);
      return;
    }
    setBusy(true);
    try {
      const res = await api.interceptors.activate(info.id);
      if (res.success) {
        toast.success(res.message ?? `${info.name} is now intercepted`, { description: 'New requests will appear here.' });
        onActivated?.();
      } else toast.error(res.message ?? `Couldn't launch ${info.name}`);
    } catch (e) {
      toast.error(`Couldn't launch ${info.name}`, { description: errorMessage(e), action: { label: 'Open Connect', onClick: () => navigate(`/connect/${encodeURIComponent(info.id)}`) } });
    } finally {
      setBusy(false);
    }
  };

  return (
    <button
      type="button"
      className={clsx(styles.tile, info.active && styles.tileActive)}
      onClick={run}
      disabled={disabled || busy}
      aria-label={`${action} ${info.name}`}
      title={disabled ? info.unavailableReason : undefined}
    >
      <AppIcon name={info.icon} size={30} tile />
      <span className={styles.tileText}>
        <span className={styles.tileName}>{info.name}</span>
        <span className={clsx(styles.tileAction, info.active && styles.tileActionActive)}>
          {busy ? (
            <>
              <Spinner size={10} /> Launching…
            </>
          ) : info.active ? (
            <>
              <Check size={11} strokeWidth={2.5} aria-hidden /> Active
            </>
          ) : disabled ? (
            (info.unavailableReason ?? 'Unavailable')
          ) : (
            <>
              {action}
              <ArrowRight size={11} strokeWidth={2.25} className={styles.tileArrow} aria-hidden />
            </>
          )}
        </span>
      </span>
    </button>
  );
}

/** The proxy address with a Copy button. */
export function ProxyAddressCard({ address }: { address: string | null }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    if (!address) return;
    if (await copyText(address)) {
      setCopied(true);
      toast.success('Proxy address copied', { description: address });
      setTimeout(() => setCopied(false), 1400);
    }
  };
  return (
    <div className={styles.addressCard}>
      <span className={styles.addressLabel}>Proxy address</span>
      <span className={clsx(styles.address, 'selectable')}>{address ?? '—'}</span>
      <Button size="sm" variant="secondary" icon={copied ? Check : Copy} onClick={copy} disabled={!address}>
        {copied ? 'Copied' : 'Copy'}
      </Button>
    </div>
  );
}

/** "Waiting for traffic": proxy address card + one-click interceptors (DESIGN.md §6.3). */
export function EmptyTraffic() {
  const status = useAppStore((s) => s.status);
  const interceptors = useAppStore((s) => s.interceptors);
  const recording = useAppStore((s) => s.recording);
  const address = proxyAddress(status);
  const top = useMemo(() => pickTop(interceptors), [interceptors]);

  return (
    <div className={styles.wrap}>
      <div className={styles.hero}>
        <div className={clsx(styles.beacon, !recording && styles.beaconPaused)} aria-hidden>
          <span className={styles.ring} />
          <span className={styles.ring} />
          <span className={styles.beaconIcon}>{recording ? <Radio size={20} strokeWidth={1.75} /> : <Circle size={18} strokeWidth={2} />}</span>
        </div>
        <h2 className={styles.title}>{recording ? 'Waiting for traffic' : 'Recording is paused'}</h2>
        <p className={styles.description}>
          {recording
            ? 'Point any HTTP client at the proxy, or launch a pre-configured app below. Requests appear here the moment they start.'
            : 'Traffic is still proxied and rules still apply, but nothing is being stored.'}
        </p>
        {!recording && (
          <Button variant="primary" icon={Circle} kbd="mod+shift+r" onClick={toggleRecording}>
            Resume recording
          </Button>
        )}
      </div>

      <ProxyAddressCard address={address} />

      {top.length > 0 && (
        <div className={styles.grid} role="list" aria-label="Intercept an app">
          {top.map((i) => (
            <div role="listitem" key={i.id} className={styles.gridItem}>
              <InterceptorTile info={i} />
            </div>
          ))}
        </div>
      )}

      <a className={styles.allLink} href="#/connect">
        All integrations <ArrowRight size={13} strokeWidth={2} aria-hidden />
      </a>
    </div>
  );
}

/** "No exchanges match <filter>" with a Clear filter action. */
export function NoMatches({ total }: { total: number }) {
  const filter = useFilterState();
  const chip = TYPE_CHIPS.find((c) => c.id === filter.chip);
  const hasText = filter.text.trim().length > 0;
  return (
    <div className={styles.noMatches} role="status">
      <span className={styles.noMatchesIcon} aria-hidden>
        <SearchX size={20} strokeWidth={1.75} />
      </span>
      <h2 className={styles.title}>No exchanges match</h2>
      <p className={styles.matchLine}>
        {hasText && <code className={styles.query}>{filter.text.trim()}</code>}
        {hasText && filter.chip !== 'all' && <span className={styles.and}>in</span>}
        {filter.chip !== 'all' && <span className={styles.chip}>{chip?.label}</span>}
      </p>
      <p className={styles.description}>
        {formatNumber(total)} captured exchange{total === 1 ? ' is' : 's are'} hidden by the filter.
      </p>
      <div className={styles.actions}>
        <Button icon={FilterX} onClick={() => trafficStore.clearFilter()}>
          Clear filter
        </Button>
      </div>
      <p className={styles.tip}>
        Tip: <Kbd combo="mod+f" size="sm" /> focuses the filter · <code>?</code> in the filter bar lists every field
      </p>
    </div>
  );
}
