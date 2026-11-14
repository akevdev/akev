import { Check, ChevronDown, RotateCcw, TriangleAlert } from 'lucide-react';
import { useEffect, useState, type ReactNode } from 'react';
import { api, isApiError } from '../../../api/client';
import type { Settings, UpstreamProxyMode } from '../../../api/types';
import { Button, Input, Kbd, Menu, NumberInput, SegmentedControl, Switch, toast, Tooltip } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { explainError, LAN_BIND, LOCAL_BIND, refreshStatus } from '../../connect/actions';
import { formatHostPort, isLanBind } from '../../connect/addresses';
import { setProxy, setUpstream, useDraft, useEdit, useSettingsSync } from '../context';
import { PairTable } from '../PairTable';
import { SettingRow, SettingsCard } from '../SettingRow';
import { StringListEditor } from '../StringListEditor';
import { AccessCard } from './AccessCard';
import { TransparentCard } from './TransparentCard';
import { useTextField } from '../useTextField';
import { BodyIdleTimeoutRow, BreakpointsCard } from './BreakpointsCard';
import { Http3Card } from './Http3Card';
import { TlsProfileCard } from './TlsProfileCard';
import { listenerOverrides, normalizeHost, parsePort, portWarning, storedListenerTarget, validateBindAddress, validateDohUrl, validateHostPattern, validateIpAddress, validatePacUrl, validatePortText, validateProxyUrl } from '../validation';
import styles from './sections.module.css';

const DOH_PRESETS = [
  { label: 'Cloudflare', url: 'https://cloudflare-dns.com/dns-query' },
  { label: 'Google', url: 'https://dns.google/dns-query' },
  { label: 'Quad9', url: 'https://dns.quad9.net/dns-query' },
];

const MODE_HELP: Record<UpstreamProxyMode, string> = {
  direct: 'Akev connects to servers directly.',
  system: 'Use the proxy configured in your operating system. Proxies on localhost are ignored to avoid loops.',
  manual: 'Send all upstream traffic through another HTTP, HTTPS or SOCKS proxy.',
  pac: 'Choose the upstream proxy per request with a proxy auto-config (PAC) script.',
};

function hostLabel(bind: string, port: number): string {
  return bind === '0.0.0.0' || bind === '::' ? `all interfaces, port ${port}` : `${bind}:${port}`;
}

/** Toast title + description after a rebind. */
function rebindMessage(next: { port: number; bindAddress: string }, previous: { port: number; bindAddress: string }): { title: string; description?: string } {
  if (next.port !== previous.port) {
    return { title: `Proxy moved to :${next.port}`, description: `Clients configured with :${previous.port} need updating. Relaunch browsers and terminals started from Akev.` };
  }
  const wasLan = isLanBind(previous.bindAddress);
  const nowLan = isLanBind(next.bindAddress);
  if (nowLan && !wasLan) return { title: 'Other devices can now connect', description: `Listening on ${hostLabel(next.bindAddress, next.port)}.` };
  if (!nowLan && wasLan) return { title: 'Only this computer can connect', description: `Listening on ${hostLabel(next.bindAddress, next.port)}.` };
  return { title: `Proxy now listening on ${hostLabel(next.bindAddress, next.port)}` };
}

/** Why the proxy isn't listening where the stored settings say (CLI overrides, busy port), on
 *  its own line under "Listening on …", with `action` (the "Use port N" button) after it. */
function OverrideNote({ stored, live, action }: { stored: { port: number; bindAddress: string }; live: { port: number; bindAddress: string }; action?: ReactNode }) {
  const o = listenerOverrides(stored, live);
  let text: ReactNode = null;
  if (o.port != null && o.bindAddress != null) {
    text = (
      <>
        Settings say {formatHostPort(o.bindAddress, o.port)}: overridden at startup (<code>--proxy-bind</code>, <code>--proxy-port</code>) or the port was busy
      </>
    );
  } else if (o.port != null) {
    text = (
      <>
        Port {o.port} was busy or overridden at startup (<code>--proxy-port</code>)
      </>
    );
  } else if (o.bindAddress != null) {
    text = (
      <>
        Bind address {o.bindAddress} is overridden for this run (<code>--proxy-bind</code>)
      </>
    );
  }
  if (!text) return null;
  return (
    <span className={styles.overrideLine}>
      <span className={styles.overrideNote}>{text}</span>
      {action}
    </span>
  );
}

/**
 * "Use port 8000": moves the proxy to the stored listener after a busy-port fallback or a
 * `--proxy-port` / `--proxy-bind` override (`POST /api/proxy/rebind`, which always rebinds).
 * When the port is still taken the button stays, so pressing it again retries.
 */
function UseStoredListener({ stored, live, onError }: { stored: { port: number; bindAddress: string }; live: { port: number; bindAddress: string }; onError: (message: string | null) => void }) {
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState(false);
  const target = storedListenerTarget(stored, live);
  if (!target) return null;
  const where = target.what === 'port' ? `port ${target.port}` : target.what === 'bind' ? target.bindAddress : formatHostPort(target.bindAddress, target.port);
  const move = async () => {
    setBusy(true);
    onError(null);
    try {
      const next = await api.proxy.rebind({ port: target.port, bindAddress: target.bindAddress });
      await refreshStatus();
      setFailed(false);
      toast.success(`Proxy moved to ${formatHostPort(next.bindAddress, next.port)}`, {
        description: target.what === 'bind' ? undefined : `Clients set up with :${live.port} need updating. Relaunch browsers and terminals started from Akev.`,
      });
    } catch (e) {
      setFailed(true);
      const busy = isApiError(e) && e.status === 409;
      onError(busy && target.what !== 'bind' ? `Port ${target.port} is still in use by another program. Close it, then retry.` : explainError(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <Tooltip content={`Move the proxy to ${where}, as your settings say`}>
      <Button size="sm" icon={failed ? RotateCcw : undefined} loading={busy} onClick={() => void move()} className={styles.useStored}>
        {failed ? 'Retry' : `Use ${where}`}
      </Button>
    </Tooltip>
  );
}

/** Port + bind address: applied together with an explicit Apply (they rebind the proxy). */
function ListenerCard({ draft }: { draft: Settings }) {
  const sync = useSettingsSync();
  const statusProxy = useAppStore((s) => s.status?.proxy ?? null);
  // What's stored (`GET /api/settings`); `Status.proxy` is what's live.
  const stored = useAppStore((s) => s.settings?.proxy) ?? draft.proxy;
  const connection = useAppStore((s) => s.connection);
  // The port is edited as text and validated as typed — never silently clamped.
  const [portText, setPortText] = useState(String(draft.proxy.port));
  const [bind, setBind] = useState(draft.proxy.bindAddress);
  const [applying, setApplying] = useState(false);
  const [serverError, setServerError] = useState<string | null>(null);

  useEffect(() => setPortText(String(draft.proxy.port)), [draft.proxy.port]);
  useEffect(() => setBind(draft.proxy.bindAddress), [draft.proxy.bindAddress]);

  const port = parsePort(portText);
  const portError = validatePortText(portText);
  const bindError = validateBindAddress(bind);
  const dirty = port !== draft.proxy.port || bind.trim() !== draft.proxy.bindAddress;

  const stepPort = (delta: number) => {
    const base = port ?? draft.proxy.port;
    setPortText(String(Math.min(65535, Math.max(1, base + delta))));
    setServerError(null);
  };
  const lan = isLanBind(draft.proxy.bindAddress);

  const rebind = async (next: { port: number; bindAddress: string }, previous: { port: number; bindAddress: string }, viaUndo = false) => {
    setApplying(true);
    setServerError(null);
    const liveBefore = useAppStore.getState().status?.proxy;
    try {
      await sync.edit((d) => ({ ...d, proxy: { ...d.proxy, ...next } }));
      // The daemon only moves the fields that changed; an unchanged one keeps its live value
      // (a `--proxy-port` / `--proxy-bind` override or fallback port). Report where it is now.
      const liveAfter = (await refreshStatus())?.proxy;
      const msg = rebindMessage(liveAfter ?? next, liveBefore ?? previous);
      toast.success(msg.title, {
        description: msg.description,
        action: viaUndo ? undefined : { label: 'Undo', onClick: () => void rebind(previous, next, true) },
      });
    } catch (e) {
      setServerError(explainError(e));
    } finally {
      setApplying(false);
    }
  };

  const apply = () => {
    if (!dirty || portError || bindError || port == null) return;
    void rebind({ port, bindAddress: bind.trim() }, { port: draft.proxy.port, bindAddress: draft.proxy.bindAddress });
  };

  const listening = statusProxy && connection === 'open';
  return (
    <SettingsCard id="listener" title="Listening" description="Changing these rebinds the proxy immediately — no restart.">
      <SettingRow
        id="port"
        title="Port"
        description={
          <span className={styles.listening}>
            <span className={listening ? styles.listeningDot : `${styles.listeningDot} ${styles.listeningDotOff}`} aria-hidden />
            {listening ? (
              <>
                Listening on <span className={styles.addr}>{formatHostPort(statusProxy.bindAddress, statusProxy.port)}</span>
                {(statusProxy.bindAddress === '0.0.0.0' || statusProxy.bindAddress === '::') && ' — every network interface'}
                <OverrideNote stored={stored} live={statusProxy} action={<UseStoredListener stored={stored} live={statusProxy} onError={setServerError} />} />
              </>
            ) : (
              'Not connected to Akev'
            )}
          </span>
        }
        error={portError ?? serverError ?? undefined}
        warning={!portError && !serverError ? (portWarning(port) ?? undefined) : undefined}
        control={
          <div className={styles.inlineGroup}>
            <Input
              value={portText}
              onChange={(e) => {
                setPortText(e.target.value);
                setServerError(null);
              }}
              inputMode="numeric"
              invalid={!!portError}
              aria-label="Proxy port"
              aria-invalid={!!portError}
              className={styles.numberInput}
              inputClassName="tnum"
              onKeyDown={(e) => {
                if (e.key === 'ArrowUp' || e.key === 'ArrowDown') {
                  e.preventDefault();
                  stepPort((e.key === 'ArrowUp' ? 1 : -1) * (e.shiftKey ? 10 : 1));
                } else if (e.key === 'Enter') {
                  apply();
                } else if (e.key === 'Escape' && dirty) {
                  e.preventDefault();
                  e.stopPropagation();
                  setPortText(String(draft.proxy.port));
                  setBind(draft.proxy.bindAddress);
                }
              }}
            />
            <Tooltip content={dirty ? <>Rebind the proxy <Kbd combo="enter" /></> : 'The proxy is listening on this port'}>
              <Button variant={dirty ? 'primary' : 'secondary'} disabled={!dirty || !!portError || !!bindError} loading={applying} onClick={apply} icon={dirty ? undefined : Check} className={styles.applyButton}>
                {dirty ? 'Apply' : 'Applied'}
              </Button>
            </Tooltip>
          </div>
        }
      />
      <SettingRow
        id="lan"
        title="Allow other devices"
        description="Accept connections from phones, tablets, VMs and other computers on your network."
        control={
          <Switch
            checked={lan}
            disabled={applying}
            onCheckedChange={(on) => void rebind({ port: draft.proxy.port, bindAddress: on ? LAN_BIND : LOCAL_BIND }, { port: draft.proxy.port, bindAddress: draft.proxy.bindAddress })}
            aria-label="Allow other devices"
          />
        }
      >
        {lan && (
          <p className={`${styles.callout} ${styles.calloutWarning}`} role="note">
            <TriangleAlert size={13} strokeWidth={1.9} aria-hidden />
            Anyone on your network can send traffic through Akev and inspect it here. Turn this off on public or untrusted networks.
          </p>
        )}
      </SettingRow>
      <SettingRow
        id="bind"
        title="Bind address"
        description="The interface the proxy listens on. 127.0.0.1 = this computer only, 0.0.0.0 = every interface."
        error={bindError ?? undefined}
        control={
          <Input
            value={bind}
            onChange={(e) => {
              setBind(e.target.value);
              setServerError(null);
            }}
            onKeyDown={(e) => e.key === 'Enter' && apply()}
            mono
            invalid={!!bindError}
            aria-label="Bind address"
            className={styles.numberInput}
          />
        }
      />
    </SettingsCard>
  );
}

function UpstreamCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const sync = useSettingsSync();
  const up = draft.proxy.upstreamProxy;
  const [pendingMode, setPendingMode] = useState<UpstreamProxyMode | null>(null);
  const mode = pendingMode ?? up.mode;
  const needsUrl = mode === 'manual' || mode === 'pac';
  const validate = mode === 'pac' ? validatePacUrl : validateProxyUrl;

  const field = useTextField({
    value: up.url,
    validate: (t) => (needsUrl ? validate(t) : null),
    onCommit: (url) => {
      if (pendingMode) {
        setPendingMode(null);
        void edit(setUpstream({ mode: pendingMode, url: url.trim() }));
      } else {
        void edit(setUpstream({ url: url.trim() }), { debounce: true });
      }
    },
    onSettle: () => void sync.flush(),
  });

  const changeMode = (m: UpstreamProxyMode) => {
    if ((m === 'manual' || m === 'pac') && (m === 'pac' ? validatePacUrl : validateProxyUrl)(up.url)) {
      setPendingMode(m); // wait for a valid URL before applying
      return;
    }
    setPendingMode(null);
    void edit(setUpstream({ mode: m }));
  };

  return (
    <SettingsCard id="upstream" title="Upstream proxy" description="How Akev itself reaches the internet — for corporate proxies or chaining tools.">
      <SettingRow
        id="upstream"
        title="Mode"
        description={MODE_HELP[mode]}
        control={
          <SegmentedControl<UpstreamProxyMode>
            value={mode}
            onValueChange={changeMode}
            items={[
              { value: 'direct', label: 'Direct' },
              { value: 'system', label: 'System' },
              { value: 'manual', label: 'Manual' },
              { value: 'pac', label: 'PAC' },
            ]}
            aria-label="Upstream proxy mode"
          />
        }
      />
      {needsUrl && (
        <SettingRow
          id="upstream-url"
          title={mode === 'pac' ? 'PAC script URL' : 'Proxy URL'}
          description={pendingMode ? `Enter a valid URL to switch to ${mode === 'pac' ? 'PAC' : 'a manual proxy'}.` : mode === 'pac' ? 'http(s):// or file:// URL of the script.' : 'http://, https://, socks4:// or socks5:// with optional user:password@.'}
          error={field.error ?? undefined}
          control={<Input {...field.inputProps} mono placeholder={mode === 'pac' ? 'http://wpad.corp/wpad.dat' : 'http://proxy.corp:3128'} aria-label={mode === 'pac' ? 'PAC script URL' : 'Proxy URL'} className={styles.textInput} autoFocus={pendingMode != null} />}
        />
      )}
      {mode !== 'direct' && (
        <SettingRow id="no-proxy" title="Bypass the upstream proxy for" description="Hosts reached directly, e.g. internal services. Globs like *.corp.example work." layout="stacked">
          <StringListEditor
            items={up.noProxy}
            onChange={(items) => void edit(setUpstream({ noProxy: items }))}
            validate={validateHostPattern}
            normalize={normalizeHost}
            placeholder="localhost, *.internal…"
            noun="host"
            emptyText="Every request goes through the upstream proxy."
            aria-label="Hosts that bypass the upstream proxy"
          />
        </SettingRow>
      )}
    </SettingsCard>
  );
}

function DnsCard({ draft }: { draft: Settings }) {
  const edit = useEdit();
  const sync = useSettingsSync();
  const doh = useTextField({
    value: draft.proxy.dohUrl ?? '',
    validate: validateDohUrl,
    onCommit: (t) => void edit(setProxy('dohUrl', t.trim() || null), { debounce: true }),
    onSettle: () => void sync.flush(),
  });
  return (
    <SettingsCard id="dns" title="DNS">
      <SettingRow id="dns-overrides" title="DNS overrides" description="Resolve these hosts to fixed addresses, like a hosts file that only affects proxied traffic." layout="stacked">
        <PairTable<'host' | 'address'>
          columns={[
            { key: 'host', label: 'Host', placeholder: 'api.example.com', validate: validateHostPattern, normalize: normalizeHost, mono: true },
            { key: 'address', label: 'Address', placeholder: '127.0.0.1', validate: validateIpAddress, mono: true, width: 'minmax(0, 0.8fr)' },
          ]}
          rows={draft.proxy.dnsOverrides}
          onChange={(rows) => void edit(setProxy('dnsOverrides', rows.map((r) => ({ host: r.host ?? '', address: r.address ?? '' }))))}
          noun="override"
          uniqueKey="host"
          emptyText="No overrides — hosts resolve normally."
          aria-label="DNS overrides"
        />
      </SettingRow>
      <SettingRow
        id="doh"
        title="DNS-over-HTTPS"
        description="Resolve upstream hosts with an encrypted resolver. Leave empty to use the system resolver."
        error={doh.error ?? undefined}
        control={
          <div className={styles.inlineGroup}>
            <Input {...doh.inputProps} mono placeholder="System resolver" aria-label="DNS-over-HTTPS URL" className={styles.textInput} />
            <Menu
              trigger={
                <Button iconRight={ChevronDown} aria-label="DNS-over-HTTPS presets">
                  Presets
                </Button>
              }
              items={[
                {
                  type: 'radio',
                  value: draft.proxy.dohUrl ?? 'system',
                  onValueChange: (v: string) => void edit(setProxy('dohUrl', v === 'system' ? null : v)),
                  options: [
                    { value: 'system', label: 'System resolver' },
                    ...DOH_PRESETS.map((p) => ({
                      value: p.url,
                      label: (
                        <span className={styles.presetLabel}>
                          {p.label}
                          <span className={styles.presetHost}>{new URL(p.url).host}</span>
                        </span>
                      ),
                    })),
                  ],
                },
              ]}
            />
          </div>
        }
      />
    </SettingsCard>
  );
}

function secondsOf(ms: number): number {
  return Math.round(ms / 100) / 10;
}

/** Settings → Proxy. */
export function ProxySection() {
  const draft = useDraft();
  const edit = useEdit();
  if (!draft) return null;
  const p = draft.proxy;
  return (
    <>
      <ListenerCard draft={draft} />
      <AccessCard draft={draft} />

      <SettingsCard id="protocols" title="Protocols">
        <SettingRow id="http2" title="HTTP/2" description="Offer HTTP/2 to clients and servers that support it. Turn off to force HTTP/1.1 everywhere." control={<Switch checked={p.http2} onCheckedChange={(v) => void edit(setProxy('http2', v))} aria-label="HTTP/2" />} />
        <SettingRow id="socks" title="SOCKS proxy" description="Also accept SOCKS4 / SOCKS5 connections on the same port." control={<Switch checked={p.socks} onCheckedChange={(v) => void edit(setProxy('socks', v))} aria-label="SOCKS proxy" />} />
        <SettingRow id="header-case" title="Preserve header case" description="Forward HTTP/1 header names exactly as the client sent them (some servers are case-sensitive)." control={<Switch checked={p.preserveHeaderCase} onCheckedChange={(v) => void edit(setProxy('preserveHeaderCase', v))} aria-label="Preserve header case" />} />
      </SettingsCard>

      <Http3Card draft={draft} />

      <SettingsCard id="tls" title="TLS passthrough" description="Traffic to these hosts is tunnelled without decryption — for apps that pin certificates, or hosts you never want to inspect.">
        <SettingRow id="tls-passthrough" title="Hosts" description="Globs work: *.apple.com, api.*.example.com. Passed-through traffic appears as tunnels." layout="stacked">
          <StringListEditor
            items={p.tlsPassthrough}
            onChange={(items) => void edit(setProxy('tlsPassthrough', items))}
            validate={validateHostPattern}
            normalize={normalizeHost}
            placeholder="Add host, e.g. *.apple.com — paste several at once"
            noun="host"
            emptyText="Every HTTPS connection is decrypted."
            aria-label="TLS passthrough hosts"
          />
        </SettingRow>
      </SettingsCard>

      <TlsProfileCard draft={draft} />

      <UpstreamCard draft={draft} />
      <DnsCard draft={draft} />
      <TransparentCard draft={draft} />

      <SettingsCard id="timeouts" title="Timeouts">
        <SettingRow
          id="connect-timeout"
          title="Connect timeout"
          description="How long to wait for a server to accept the connection."
          control={<NumberInput value={secondsOf(p.connectTimeoutMs)} onValueChange={(v) => v != null && void edit(setProxy('connectTimeoutMs', Math.round(v * 1000)))} min={0.5} max={600} step={1} precision={1} unit="s" aria-label="Connect timeout in seconds" className={styles.numberInput} />}
        />
        <SettingRow
          id="response-timeout"
          title="Response timeout"
          description={p.responseTimeoutMs === 0 ? 'No limit — Akev waits as long as the server keeps the connection open.' : 'How long to wait for the server to start answering (its response headers). 0 = no limit.'}
          control={<NumberInput value={secondsOf(p.responseTimeoutMs)} onValueChange={(v) => void edit(setProxy('responseTimeoutMs', Math.round((v ?? 0) * 1000)))} min={0} max={3600} step={5} precision={1} unit="s" aria-label="Response timeout in seconds" className={styles.numberInput} />}
        />
        <BodyIdleTimeoutRow draft={draft} />
      </SettingsCard>

      <BreakpointsCard draft={draft} />
    </>
  );
}
