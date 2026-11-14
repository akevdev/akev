import { Info } from 'lucide-react';
import type { Http3Settings, Http3UpstreamMode, Settings } from '../../../api/types';
import { NumberInput, SegmentedControl, Switch } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { refreshStatus } from '../../connect/actions';
import { setProxy, useEdit } from '../context';
import { HTTP3_MODE_HELP, listenerLine, quicPortWarning } from '../http3';
import { SettingRow, SettingsCard } from '../SettingRow';
import styles from './sections.module.css';

/** Settings → Proxy → HTTP/3: upstream mode, the QUIC listener and its port. */
export function Http3Card({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const h3 = draft.proxy.http3;
  const status = useAppStore((s) => s.status?.proxy ?? null);
  const connected = useAppStore((s) => s.connection) === 'open';
  const proxyPort = status?.port ?? draft.proxy.port;
  const line = listenerLine(h3, status?.http3, connected);

  const update = async (patch: Partial<Http3Settings>, debounce = false) => {
    await edit(setProxy('http3', { ...h3, ...patch }), { debounce });
    // The listener starts / moves / stops with the settings: show where it is now.
    if ('listen' in patch || 'listenPort' in patch) await refreshStatus();
  };

  return (
    <SettingsCard id="http3" title="HTTP/3" description="HTTP over QUIC (UDP). Akev can speak it to servers and accept it from clients — recorded as HTTP/3 like any other exchange.">
      <SettingRow
        id="http3-upstream"
        title="HTTP/3 to servers"
        description={HTTP3_MODE_HELP[h3.upstream]}
        control={
          <SegmentedControl<Http3UpstreamMode>
            value={h3.upstream}
            onValueChange={(upstream) => void update({ upstream })}
            items={[
              { value: 'off', label: 'Off' },
              { value: 'alt-svc', label: 'Alt-Svc', tooltip: 'When the server advertises HTTP/3' },
              { value: 'prefer', label: 'Prefer', tooltip: 'Every HTTPS server first' },
            ]}
            aria-label="HTTP/3 to servers"
          />
        }
      />
      <SettingRow
        id="http3-listen"
        title="Accept HTTP/3 from clients"
        description={
          <span className={styles.listening}>
            {h3.listen && <span className={line.tone === 'on' ? styles.listeningDot : `${styles.listeningDot} ${styles.listeningDotOff}`} aria-hidden />}
            {line.tone === 'error' ? 'Not listening' : line.text}
            {line.address && <span className={styles.addr}>{line.address}</span>}
          </span>
        }
        error={line.tone === 'error' ? line.text : undefined}
        control={<Switch checked={h3.listen} onCheckedChange={(listen) => void update({ listen })} aria-label="Accept HTTP/3 from clients" />}
      >
        {h3.listen && (
          <p className={`${styles.callout} ${styles.calloutInfo}`} role="note">
            <Info size={13} strokeWidth={1.9} aria-hidden />
            <span>
              For clients sent to Akev directly — DNS or hosts entries pointing at this machine, or <code>curl --http3 --connect-to ::127.0.0.1:{h3.listenPort || proxyPort}</code>. Browsers using Akev as their proxy keep using HTTP/1.1 or HTTP/2 to it.
            </span>
          </p>
        )}
      </SettingRow>
      {h3.listen && (
        <SettingRow
          id="http3-port"
          title="QUIC port"
          description={`UDP port of the HTTP/3 listener. Empty = the proxy's port number (${proxyPort}).`}
          warning={quicPortWarning(h3.listenPort) ?? undefined}
          control={
            <NumberInput
              value={h3.listenPort > 0 ? h3.listenPort : null}
              onValueChange={(v) => void update({ listenPort: v ?? 0 }, true)}
              allowEmpty
              min={1}
              max={65535}
              placeholder={String(proxyPort)}
              aria-label="QUIC port"
              className={styles.numberInput}
              inputClassName="tnum"
            />
          }
        />
      )}
    </SettingsCard>
  );
}
