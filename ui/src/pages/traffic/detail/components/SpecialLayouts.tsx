import clsx from 'clsx';
import { ArrowDown, ArrowRight, ArrowUp, Binary, Cable, Check, Clock, FileWarning, Fingerprint, Lightbulb, LockKeyhole, Pin, Plug, ScanSearch, ShieldCheck, Smartphone } from 'lucide-react';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { api, errorMessage } from '../../../../api/client';
import type { ExchangeDetail, ExchangeSummary } from '../../../../api/types';
import { useCommands } from '../../../../app/commands';
import { navigate } from '../../../../app/router';
import { AppIcon, Button, CopyButton, EmptyState, HexViewer, Skeleton } from '../../../../components/ui';
import { formatBytes, formatDateTime, formatDuration, formatElapsed, microsToMillis } from '../../../../lib/format';
import { useAppStore } from '../../../../state/app';
import { setPinned } from '../../actions/exchangeActions';
import { isHostListed, setHostListed } from '../../actions/hostLists';
import { tlsLabel } from '../lib/detail';
import { escapedPrefix, sniffProtocol, type ProtocolId } from '../lib/sniff';
import { useNow } from '../hooks/misc';
import { ErrorBanner } from './ErrorBanner';
import { ExchangeHeader } from './ExchangeHeader';
import banner from './Banners.module.css';
import styles from './SpecialLayouts.module.css';
import views from './views.module.css';

interface LayoutProps {
  detail: ExchangeDetail;
  summary: ExchangeSummary;
}

/** Pin palette command for connection-level layouts (the traffic list binds Mod+P). */
function useConnectionActions(summary: ExchangeSummary) {
  const pinned = summary.pinned === true;
  const toggle = () => void setPinned([summary.id], !pinned);
  useCommands([{ id: 'exchange.pin', title: pinned ? 'Unpin exchange' : 'Pin exchange', group: 'Traffic', icon: Pin, shortcut: 'mod+p', run: toggle }]);
}

function Shell({ detail, summary, children }: LayoutProps & { children: ReactNode }) {
  useConnectionActions(summary);
  return (
    <>
      <ExchangeHeader summary={summary} detail={detail} showOriginal={false} onToggleOriginal={() => undefined} variant="connection" />
      <div className={styles.scroll}>
        <div className={styles.content}>{children}</div>
      </div>
    </>
  );
}

function Card({ icon, title, aside, children }: { icon: ReactNode; title: ReactNode; aside?: ReactNode; children: ReactNode }) {
  return (
    <section className={views.card}>
      <header className={views.cardHead}>
        <span className={views.cardTitle}>
          {icon}
          {title}
        </span>
        <span className={views.spacer} />
        {aside}
      </header>
      <div className={views.cardBody}>{children}</div>
    </section>
  );
}

function Row({ label, children, copy }: { label: string; children: ReactNode; copy?: string }) {
  return (
    <>
      <dt className={views.dt}>{label}</dt>
      <dd className={views.dd}>
        {children}
        {copy && (
          <span className={views.inlineActions}>
            <CopyButton value={copy} label={`Copy ${label.toLowerCase()}`} />
          </span>
        )}
      </dd>
    </>
  );
}

function Mono({ children }: { children: ReactNode }) {
  return <span className={clsx(views.mono, 'selectable')}>{children}</span>;
}

function ClientRows({ detail }: { detail: ExchangeDetail }) {
  const c = detail.client;
  return (
    <>
      <Row label="Client" copy={c.remoteAddr}>
        <Mono>{c.remoteAddr}</Mono>
        {isLoopback(c.remoteAddr) ? <span className={views.hint}>this machine</span> : isPrivate(c.remoteAddr) ? <span className={views.hint}>a device on your network</span> : null}
      </Row>
      {(c.process || detail.summary.source) && (
        <Row label="Application">
          <span className={styles.app}>
            <AppIcon name={c.interceptor ?? detail.summary.source ?? c.process?.name} size={14} />
            {detail.summary.source ?? c.process?.name}
          </span>
          {c.process && <span className={views.hint}>pid {c.process.pid}</span>}
        </Row>
      )}
    </>
  );
}

// =====================================================================================
// TLS failure
// =====================================================================================

const ANDROID_HINTS = /okhttp|android|dalvik|retrofit|ktor/i;

/** A TLS handshake that failed between the client and Akev (DESIGN.md §6.3/§19 analysis). */
export function TlsFailureLayout({ detail, summary }: LayoutProps) {
  const settings = useAppStore((s) => s.settings);
  const tls = detail.client.tls;
  const sni = tls?.sni ?? summary.host;
  const passthrough = isHostListed(settings, 'tlsPassthrough', sni);
  const android = ANDROID_HINTS.test(`${summary.source ?? ''} ${detail.client.process?.name ?? ''} ${detail.client.interceptor ?? ''}`) || detail.client.interceptor === 'android-adb';
  const rejected = !detail.error || detail.error.kind === 'client-tls-rejected';
  const [busy, setBusy] = useState(false);

  return (
    <Shell detail={detail} summary={summary}>
      {detail.error ? (
        <ErrorBanner error={detail.error} summary={summary} fixes={false} className={banner.card} />
      ) : (
        <div className={clsx(banner.banner, banner.error, banner.card)}>
          <LockKeyhole size={16} strokeWidth={2} className={banner.icon} aria-hidden />
          <div className={banner.text}>
            <div className={banner.title}>TLS handshake with the client failed</div>
            <p className={banner.body}>No HTTP request was received — the connection ended during the handshake.</p>
          </div>
        </div>
      )}

      <Card icon={<Fingerprint size={14} strokeWidth={1.75} aria-hidden />} title="Handshake">
        <dl className={views.dl}>
          <Row label="Server name" copy={sni}>
            <Mono>{sni}</Mono>
            {!tls?.sni && <span className={views.hint}>no SNI sent — from the CONNECT target</span>}
          </Row>
          <Row label="Port">
            <Mono>{summary.port}</Mono>
          </Row>
          <ClientRows detail={detail} />
          {tls?.version && <Row label="TLS version">{tlsLabel(tls.version)}</Row>}
          {tls?.alpnOffered?.length ? (
            <Row label="ALPN offered">
              <Mono>{tls.alpnOffered.join(', ')}</Mono>
            </Row>
          ) : null}
          {tls?.cipher && (
            <Row label="Cipher">
              <Mono>{tls.cipher}</Mono>
            </Row>
          )}
          {tls?.ja3 && (
            <Row label="JA3" copy={tls.ja3}>
              <Mono>{tls.ja3}</Mono>
            </Row>
          )}
          {tls?.ja4 && (
            <Row label="JA4" copy={tls.ja4}>
              <Mono>{tls.ja4}</Mono>
            </Row>
          )}
          <Row label="When">
            {formatDateTime(summary.startedAt, true)}
            {summary.duration != null && <span className={views.hint}>failed after {formatDuration(summary.duration)}</span>}
          </Row>
        </dl>
      </Card>

      <Card icon={<Lightbulb size={14} strokeWidth={1.75} aria-hidden />} title="How to fix it">
        <ol className={styles.steps}>
          {rejected && (
            <li className={styles.step}>
              <span className={styles.stepNum}>1</span>
              <div className={styles.stepText}>
                <strong>Trust the Akev certificate on this client.</strong>
                <span>Most failures mean the client doesn't trust the Akev CA yet. The Connect page shows how to install it for each browser, OS and device.</span>
              </div>
              <Button size="sm" icon={ShieldCheck} onClick={() => navigate('/connect')}>
                Set up client
              </Button>
            </li>
          )}
          <li className={styles.step}>
            <span className={styles.stepNum}>{rejected ? 2 : 1}</span>
            <div className={styles.stepText}>
              <strong>Pinned certificates can't be intercepted.</strong>
              <span>
                If the app pins its server certificate, let its traffic through untouched: hosts on the TLS passthrough list are tunnelled without decryption, so the app keeps working.
              </span>
            </div>
            <Button
              size="sm"
              icon={passthrough ? Check : Cable}
              disabled={passthrough}
              loading={busy}
              onClick={async () => {
                setBusy(true);
                await setHostListed('tlsPassthrough', sni, true);
                setBusy(false);
              }}
            >
              {passthrough ? 'In passthrough list' : `Add ${sni} to passthrough`}
            </Button>
          </li>
          {android && (
            <li className={styles.step}>
              <span className={styles.stepNum}>
                <Smartphone size={12} strokeWidth={2} aria-hidden />
              </span>
              <div className={styles.stepText}>
                <strong>Android 7+ apps ignore user-installed CAs.</strong>
                <span>Debug builds can opt in with a network security config; otherwise inject the CA as a system certificate via Connect → Android (ADB) on a rooted device or emulator.</span>
              </div>
              <Button size="sm" variant="ghost" onClick={() => navigate('/connect/android-adb')}>
                Android setup
              </Button>
            </li>
          )}
        </ol>
      </Card>
    </Shell>
  );
}

// =====================================================================================
// Tunnel
// =====================================================================================

/** A CONNECT tunnel Akev relayed without intercepting (TLS passthrough or non-HTTP). */
export function TunnelLayout({ detail, summary }: LayoutProps) {
  const settings = useAppStore((s) => s.settings);
  const open = summary.state === 'streaming' || summary.state === 'requesting';
  const now = useNow(1000, open);
  const listed = isHostListed(settings, 'tlsPassthrough', summary.host);
  const tunnel = detail.tunnel;
  const sent = tunnel?.bytesSent ?? summary.requestSize;
  const received = tunnel?.bytesReceived ?? summary.responseSize;
  const [busy, setBusy] = useState(false);

  return (
    <Shell detail={detail} summary={summary}>
      <div className={clsx(banner.banner, banner.info, banner.card)}>
        <Cable size={16} strokeWidth={2} className={banner.icon} aria-hidden />
        <div className={banner.text}>
          <div className={banner.title}>Tunnelled without interception</div>
          <p className={banner.body}>
            {tunnel?.reason ? `${tunnel.reason}. ` : ''}Akev relayed the encrypted bytes between the client and {summary.host}:{summary.port} without decrypting them, so there are no requests to inspect.
          </p>
        </div>
      </div>

      <div className={styles.stats}>
        <Stat icon={<ArrowUp size={13} strokeWidth={2} aria-hidden />} label="Sent" value={formatBytes(sent)} tone="accent" />
        <Stat icon={<ArrowDown size={13} strokeWidth={2} aria-hidden />} label="Received" value={formatBytes(received)} tone="success" />
        <Stat
          icon={<Clock size={13} strokeWidth={2} aria-hidden />}
          label={open ? 'Open for' : 'Duration'}
          value={open ? formatElapsed(summary.startedAt, now) : formatDuration(summary.duration)}
          live={open}
        />
      </div>

      <Card icon={<Plug size={14} strokeWidth={1.75} aria-hidden />} title="Connection">
        <div className={styles.route}>
          <span className={styles.endpoint}>
            <AppIcon name={detail.client.interceptor ?? summary.source} size={14} />
            <Mono>{detail.client.remoteAddr}</Mono>
          </span>
          <ArrowRight size={14} strokeWidth={1.75} className={styles.routeArrow} aria-hidden />
          <span className={styles.endpoint}>
            <span className={styles.akev}>Akev</span>
          </span>
          <ArrowRight size={14} strokeWidth={1.75} className={styles.routeArrow} aria-hidden />
          <span className={styles.endpoint}>
            <Mono>
              {summary.host}:{summary.port}
            </Mono>
            {detail.upstream?.remoteAddr && <span className={views.hint}>{detail.upstream.remoteAddr}</span>}
          </span>
        </div>
        <dl className={views.dl}>
          <ClientRows detail={detail} />
          <Row label="Opened">{formatDateTime(microsToMillis(detail.timing.start), true)}</Row>
        </dl>
      </Card>

      {listed && (
        <Card icon={<Lightbulb size={14} strokeWidth={1.75} aria-hidden />} title="Want to see this traffic?">
          <div className={styles.step}>
            <div className={styles.stepText}>
              <span>
                <Mono>{summary.host}</Mono> is on the TLS passthrough list. Remove it to intercept new connections — the client must trust the Akev CA, and apps that pin certificates will fail.
              </span>
            </div>
            <Button
              size="sm"
              loading={busy}
              onClick={async () => {
                setBusy(true);
                await setHostListed('tlsPassthrough', summary.host, false);
                setBusy(false);
              }}
            >
              Intercept this host
            </Button>
          </div>
        </Card>
      )}
    </Shell>
  );
}

function Stat({ icon, label, value, tone, live }: { icon: ReactNode; label: string; value: string; tone?: 'accent' | 'success'; live?: boolean }) {
  return (
    <div className={styles.stat}>
      <span className={clsx(styles.statLabel, tone && styles[tone])}>
        {icon}
        {label}
        {live && <span className={styles.liveDot} aria-hidden />}
      </span>
      <span className={clsx(styles.statValue, 'tnum')}>{value}</span>
    </div>
  );
}

// =====================================================================================
// Client error
// =====================================================================================

/** Guesses whose first bytes read well as escaped text (show more of them). */
const TEXTUAL = new Set<ProtocolId>(['http', 'text', 'h2c', 'ssh', 'proxy-protocol']);

/** The client sent something Akev couldn't parse as HTTP: what arrived, what it looks like, fixes. */
export function ClientErrorLayout({ detail, summary }: LayoutProps) {
  const settings = useAppStore((s) => s.settings);
  const [bytes, setBytes] = useState<Uint8Array | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const guess = useMemo(() => (bytes ? sniffProtocol(bytes) : null), [bytes]);
  const host = detail.client.tls?.sni ?? (summary.host || null);
  const listed = host ? isHostListed(settings, 'tlsPassthrough', host) : false;

  useEffect(() => {
    let alive = true;
    setBytes(null);
    setError(null);
    api.exchanges.rawBytes(summary.id, 'request').then(
      (b) => alive && setBytes(b),
      (e: unknown) => alive && setError(errorMessage(e)),
    );
    return () => {
      alive = false;
    };
  }, [summary.id]);

  return (
    <Shell detail={detail} summary={summary}>
      {detail.error ? (
        <ErrorBanner error={detail.error} summary={summary} className={banner.card} />
      ) : (
        <div className={clsx(banner.banner, banner.error, banner.card)}>
          <FileWarning size={16} strokeWidth={2} className={banner.icon} aria-hidden />
          <div className={banner.text}>
            <div className={banner.title}>The client sent something that isn't valid HTTP</div>
            <p className={banner.body}>Akev closed the connection because it couldn't parse the request. The bytes it received are shown below.</p>
          </div>
        </div>
      )}
      <Card
        icon={<Binary size={14} strokeWidth={1.75} aria-hidden />}
        title="What Akev received"
        aside={bytes && bytes.length > 0 ? <span className={views.hint}>{formatBytes(bytes.length)}</span> : undefined}
      >
        {error ? (
          <p className={views.hint}>Couldn't load the captured bytes: {error}</p>
        ) : !bytes ? (
          <Skeleton lines={4} height={10} />
        ) : bytes.length === 0 ? (
          <EmptyState size="sm" title="No bytes captured" description="The connection ended before anything usable arrived." />
        ) : (
          <>
            {guess && (
              <div className={styles.guess}>
                <span className={styles.guessLabel}>
                  <ScanSearch size={13} strokeWidth={2} aria-hidden />
                  Looks like {guess.label}
                </span>
                <p className={styles.guessText}>{guess.explanation}</p>
              </div>
            )}
            <div className={styles.prefix}>
              <span className={views.caps}>Starts with</span>
              <code className={clsx(styles.prefixCode, 'selectable')}>{escapedPrefix(bytes, guess && TEXTUAL.has(guess.id) ? 160 : 32)}</code>
            </div>
            <HexViewer data={bytes} fill={false} maxHeight={280} />
          </>
        )}
      </Card>
      <Card icon={<Plug size={14} strokeWidth={1.75} aria-hidden />} title="Connection">
        <dl className={views.dl}>
          <ClientRows detail={detail} />
          {host && (
            <Row label={detail.client.tls?.sni ? 'Server name' : 'Host'} copy={host}>
              <Mono>{host}</Mono>
            </Row>
          )}
          <Row label="When">{formatDateTime(summary.startedAt, true)}</Row>
        </dl>
        <div className={styles.step}>
          <div className={styles.stepText}>
            <span>
              Usually a bug in the client, or a non-HTTP protocol sent through the proxy. If this client speaks another protocol, add its host to TLS passthrough so it's tunnelled untouched.
            </span>
          </div>
          {host && (
            <Button
              size="sm"
              icon={listed ? Check : Cable}
              disabled={listed}
              loading={busy}
              onClick={async () => {
                setBusy(true);
                await setHostListed('tlsPassthrough', host, true);
                setBusy(false);
              }}
            >
              {listed ? 'In passthrough list' : 'Add to passthrough'}
            </Button>
          )}
        </div>
      </Card>
    </Shell>
  );
}

// =====================================================================================

function ipOf(addr: string): string {
  if (addr.startsWith('[')) return addr.slice(1, addr.indexOf(']'));
  const i = addr.lastIndexOf(':');
  return i > 0 && addr.indexOf(':') === i ? addr.slice(0, i) : addr;
}

function isLoopback(addr: string): boolean {
  const ip = ipOf(addr);
  return ip.startsWith('127.') || ip === '::1' || ip === '::ffff:127.0.0.1';
}

function isPrivate(addr: string): boolean {
  const ip = ipOf(addr).replace(/^::ffff:/, '');
  return /^10\./.test(ip) || /^192\.168\./.test(ip) || /^172\.(1[6-9]|2\d|3[01])\./.test(ip) || /^f[cd]/i.test(ip) || /^fe80:/i.test(ip);
}
