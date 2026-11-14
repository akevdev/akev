import clsx from 'clsx';
import { CircleCheck, CircleDashed, Download, KeyRound, Smartphone } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';
import { api } from '../../../api/client';
import type { WireguardInfo } from '../../../api/types';
import { ProCallout } from '../../../components/pro';
import { Button, confirm, CopyButton, NumberInput, QrCode, SegmentedControl, Select, Spinner, Switch, toast } from '../../../components/ui';
import { saveData } from '../../../lib/platform';
import { useAppStore } from '../../../state/app';
import { useFeature } from '../../../state/license';
import { explainError } from '../actions';
import { Steps } from '../GuideView';
import { endpointChoices, hostOf, peerState, richSegments, wireguardSteps, WIREGUARD_DEVICES, type WireguardDevice } from '../wireguard';
import { Callout, KeyLine, PanelEmpty, PanelSection } from './kit';
import type { PanelProps } from './types';
import panels from './panels.module.css';
import styles from './WireguardPanel.module.css';

/** How often the live status is refreshed while the panel is open. */
const POLL_MS = 2000;

function Rich({ text }: { text: string }) {
  return (
    <>
      {richSegments(text).map((s, i) =>
        s.kind === 'bold' ? (
          <b key={i}>{s.text}</b>
        ) : s.kind === 'code' ? (
          <code key={i} className={panels.inlineCode}>
            {s.text}
          </code>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </>
  );
}

/** WireGuard mode: on/off, QR code + configuration, live device status, per-device steps. */
export function WireguardPanel(_props: PanelProps) {
  const [wg, setWg] = useState<WireguardInfo | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [host, setHost] = useState<string | undefined>(undefined);
  const [device, setDevice] = useState<WireguardDevice>('ios');
  const [busy, setBusy] = useState<'toggle' | 'port' | 'regenerate' | null>(null);
  const [port, setPort] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const licensed = useFeature('wireguard');
  const hostRef = useRef(host);
  hostRef.current = host;

  const load = useCallback(async (signal?: AbortSignal) => {
    try {
      const info = await api.wireguard.get(hostRef.current, signal);
      setWg(info);
      setLoadError(null);
      setNow(Date.now());
    } catch (e) {
      if (!signal?.aborted) setLoadError(explainError(e));
    }
  }, []);

  useEffect(() => {
    const ctrl = new AbortController();
    void load(ctrl.signal);
    const id = window.setInterval(() => {
      if (document.visibilityState === 'visible') void load(ctrl.signal);
    }, POLL_MS);
    return () => {
      ctrl.abort();
      window.clearInterval(id);
    };
  }, [load, host]);

  const apply = async (kind: 'toggle' | 'port' | 'regenerate', run: () => Promise<WireguardInfo>, done?: (info: WireguardInfo) => void) => {
    setBusy(kind);
    try {
      const info = await run();
      setWg(info);
      done?.(info);
      void useAppStore
        .getState()
        .refreshInterceptors()
        .catch(() => undefined);
      if (info.enabled && !info.running && info.error) toast.error("WireGuard didn't start", { description: info.error });
    } catch (e) {
      toast.error("Couldn't update WireGuard mode", { description: explainError(e) });
    } finally {
      setBusy(null);
    }
  };

  const toggle = (on: boolean) => void apply('toggle', () => api.wireguard.update({ enabled: on }));

  const savePort = () => {
    if (port == null || !wg || port === wg.port) return;
    void apply('port', () => api.wireguard.update({ port }), () => setPort(null));
  };

  const regenerate = async () => {
    const ok = await confirm({
      title: 'Generate new WireGuard keys?',
      message: 'Devices using the current configuration stop working until they scan the new QR code.',
      confirmLabel: 'Generate new keys',
      destructive: true,
    });
    if (ok) void apply('regenerate', () => api.wireguard.regenerate(), () => toast.success('New keys generated', { description: 'Scan the new QR code on your devices.' }));
  };

  const download = async () => {
    if (!wg) return;
    try {
      if (await saveData(wg.config, wg.configFileName, 'text/plain')) toast.success('Configuration saved', { description: wg.configFileName });
    } catch (e) {
      toast.error("Couldn't save the configuration", { description: explainError(e) });
    }
  };

  if (!wg) {
    return loadError ? (
      <Callout tone="danger" title="Couldn't load WireGuard mode" action={<Button size="sm" onClick={() => void load()}>Retry</Button>}>
        {loadError}
      </Callout>
    ) : (
      <div className={styles.loading}>
        <Spinner size={16} tone="muted" />
      </div>
    );
  }

  const state = peerState(wg, now);
  const choices = endpointChoices(wg);
  const currentHost = hostOf(wg.endpoint);
  const portDirty = port != null && port !== wg.port;

  // Without Pro: nothing to turn on (turning an existing server off always works).
  const locked = !licensed && !wg.enabled;

  return (
    <>
      {locked ? (
        <ProCallout feature="wireguard" variant="panel" title="Intercept a whole phone with one QR code">
          WireGuard capture is part of Akev Pro. Turn on the server, scan the code with the official WireGuard app and every app's traffic shows up in Traffic — no proxy settings.
        </ProCallout>
      ) : (
        <div className={clsx(panels.toggleCard, wg.running && panels.toggleCardOn)}>
          <Switch
            checked={wg.enabled}
            onCheckedChange={toggle}
            disabled={busy != null}
            label={
              <span className={panels.row}>
                WireGuard server
                {busy === 'toggle' && <Spinner size={12} tone="muted" />}
              </span>
            }
            description={
              wg.running
                ? `Listening on UDP ${wg.listenAddr ?? wg.port}. Only devices with this configuration can connect.`
                : 'Your phone sends all of its traffic through Akev — no proxy settings, works in every app.'
            }
          />
        </div>
      )}
      {!licensed && wg.enabled && <ProCallout feature="wireguard">WireGuard capture is paused until Pro is active. Turn the server off, or unlock Pro to keep intercepting your devices.</ProCallout>}

      {wg.enabled && !wg.running && wg.error && (
        <Callout tone="danger" title="The WireGuard server isn't running">
          {wg.error}. Choose another port below, or stop the program using it.
        </Callout>
      )}
      {loadError && <Callout tone="warning">{loadError}</Callout>}

      {wg.running ? (
        <>
          <div className={clsx(styles.status, styles[state.tone])} role="status" aria-live="polite">
            {state.tone === 'connected' ? <CircleCheck size={16} strokeWidth={2} aria-hidden /> : state.tone === 'idle' ? <CircleDashed size={16} strokeWidth={2} aria-hidden /> : <Spinner size={14} tone="muted" />}
            <span className={styles.statusText}>
              <span className={styles.statusTitle}>{state.title}</span>
              {state.detail && <span className={styles.statusDetail}>{state.detail}</span>}
            </span>
            {wg.stats && wg.stats.tcpTotal > 0 && (
              <span className={clsx(styles.statusCount, 'tnum')} title="TCP connections handed to the proxy (open / total)">
                {wg.stats.tcpActive} / {wg.stats.tcpTotal} conn.
              </span>
            )}
          </div>

          <div className={styles.qrCard}>
            <QrCode value={wg.qrPayload} size={196} level="M" className={panels.qr} label="QR code with the WireGuard configuration" />
            <div className={styles.qrSide}>
              <span className={panels.qrTitle}>Scan with the WireGuard app</span>
              <KeyLine label="Endpoint">
                {choices.length > 1 ? (
                  <Select<string>
                    value={currentHost}
                    onValueChange={(h) => setHost(h)}
                    options={choices.map((h) => ({ value: h, label: `${h}:${wg.port}` }))}
                    size="sm"
                    mono
                    aria-label="Address the device connects to"
                  />
                ) : (
                  <>
                    <span className={panels.addr}>{wg.endpoint}</span>
                    <CopyButton value={wg.endpoint} label="Copy endpoint" toastMessage="Endpoint copied" />
                  </>
                )}
              </KeyLine>
              <KeyLine label="Tunnel">
                <span className={panels.addr}>
                  {wg.clientAddress} · DNS {wg.dnsAddress}
                </span>
              </KeyLine>
              <div className={styles.qrActions}>
                <Button size="sm" icon={Download} onClick={() => void download()}>
                  Download .conf
                </Button>
                <CopyButton value={wg.config} label="Copy configuration" toastMessage="Configuration copied" />
              </div>
              <p className={panels.footnote}>The code contains a private key — only show it to devices you want to intercept.</p>
            </div>
          </div>
        </>
      ) : (
        !wg.enabled &&
        !locked && (
          <PanelEmpty icon={Smartphone} title="Intercept a whole phone with one QR code">
            Turn on the WireGuard server, scan the code with the official WireGuard app (iOS, Android, macOS, Windows, Linux) and every app's traffic shows up in Traffic.
          </PanelEmpty>
        )
      )}

      <PanelSection title="Set up a device">
        <SegmentedControl<WireguardDevice> value={device} onValueChange={setDevice} items={WIREGUARD_DEVICES.map((d) => ({ value: d.value, label: d.label }))} size="sm" aria-label="Device" className={panels.selfStart} />
        <Steps steps={wireguardSteps(device).map((s, i) => <Rich key={i} text={s} />)} />
      </PanelSection>

      <PanelSection title="Good to know">
        <ul className={panels.bullets}>
          <li>All TCP traffic is captured: HTTP and HTTPS are decrypted like any other client; other protocols pass through to their destination and show up as tunnels.</li>
          <li>HTTP/3 (QUIC, UDP 443) is blocked so apps fall back to HTTP/2 or HTTP/1.1 over TCP. Other UDP and DNS keep working.</li>
          <li>Apps that pin their certificates refuse Akev's certificate — add their hosts to TLS passthrough in Settings to let them through untouched.</li>
          <li>One configuration is one device: connecting a second device with the same QR code takes over the tunnel. Generate new keys to revoke access.</li>
          <li>Switch the tunnel off when you're done: while it's on and Akev isn't running, the device has no internet.</li>
          <li>Your firewall must allow incoming UDP on port {wg.port}.</li>
        </ul>
      </PanelSection>

      <PanelSection title="Server">
        <div className={styles.serverRow}>
          <label className={styles.portLabel} htmlFor="wg-port">
            UDP port
          </label>
          <NumberInput id="wg-port" value={port ?? wg.port} onValueChange={setPort} min={1} max={65535} size="sm" className={styles.portInput} onKeyDown={(e) => e.key === 'Enter' && savePort()} />
          {portDirty && (
            <Button size="sm" variant="primary" loading={busy === 'port'} onClick={savePort}>
              Apply
            </Button>
          )}
          <span className={panels.grow} />
          <Button size="sm" variant="subtle-danger" icon={KeyRound} loading={busy === 'regenerate'} disabled={busy != null} onClick={() => void regenerate()}>
            New keys
          </Button>
        </div>
      </PanelSection>
    </>
  );
}
