import clsx from 'clsx';
import { ArrowUpRight, CirclePause, CircleSlash, GitCompareArrows, History, PanelBottom, PanelRight, RefreshCw, Send, ServerCrash, ShieldOff, Timer, TriangleAlert, Unplug } from 'lucide-react';
import { memo, useEffect, useMemo, useState, type ReactNode } from 'react';
import { api, errorMessage } from '../../api/client';
import type { BodyView, ErrorKind, ExchangeDetail, ExchangeId, SendResponse } from '../../api/types';
import { BodyViewer, ConnectionView, CookiesTable, ExchangeCompare, HeadersTable, TimingView } from '../../components/exchange';
import { Button, CodeEditor, CopyButton, Dialog, EmptyState, IconButton, Kbd, Skeleton, StatusBadge, TabList, Tabs, TabsContent, Tooltip } from '../../components/ui';
import { navigate } from '../../app/router';
import { durationSeverity, formatBytes, formatDuration, formatRelativeTime } from '../../lib/format';
import { describeError, getHeader, renderRawMessage } from '../../lib/http';
import { LruCache } from '../../lib/lru';
import { useElapsed } from './hooks';
import type { RequestOptions, ResponseTabId } from './model';
import { notStoredMessage, useAppStore } from '../../state/app';
import { responseDuration, storedExchangeId, tabsActions, type ResponseState } from './store';
import styles from './ResponsePane.module.css';

const details = new LruCache<number, ExchangeDetail>(30);

function useExchangeDetail(id: number | undefined, enabled: boolean): { detail: ExchangeDetail | null; error: string | null } {
  const [state, setState] = useState<{ id?: number; detail: ExchangeDetail | null; error: string | null }>({ detail: null, error: null });
  useEffect(() => {
    if (id == null || !enabled) return;
    const cached = details.get(id);
    if (cached) {
      setState({ id, detail: cached, error: null });
      return;
    }
    const ctrl = new AbortController();
    setState({ id, detail: null, error: null });
    api.exchanges.get(id, ctrl.signal).then(
      (d) => {
        details.set(id, d);
        setState({ id, detail: d, error: null });
      },
      (e: unknown) => !ctrl.signal.aborted && setState({ id, detail: null, error: errorMessage(e) }),
    );
    return () => ctrl.abort();
  }, [id, enabled]);
  return state.id === id ? { detail: state.detail, error: state.error } : { detail: null, error: null };
}

/**
 * BodyViewer source for a Compose response: the already-fetched view plus the recorded
 * exchange, so previews, Hex and the default download use the *decoded* bytes (with a separate
 * "as received" download for gzip/br responses) — a bare raw URL would serve the encoded bytes.
 * An unstored response (recording paused) has no exchange to fetch from: just the view.
 */
export function composeBodySource(result: Pick<SendResponse, 'id' | 'body' | 'recorded'>): { view: BodyView; exchangeId?: ExchangeId; side: 'response' } {
  const id = storedExchangeId(result);
  return id != null ? { view: result.body, exchangeId: id, side: 'response' } : { view: result.body, side: 'response' };
}

export interface ResponsePaneProps {
  tabId: string;
  responseTab: ResponseTabId;
  lastExchangeId?: number;
  response: ResponseState | undefined;
  layout: 'right' | 'bottom';
  onToggleLayout: () => void;
  onSend: () => void;
  onCancel: () => void;
  /** Change options and send again (contextual fixes, e.g. ignore TLS errors). */
  onRetryWith: (patch: Partial<RequestOptions>) => void;
  onFocusUrl: () => void;
}

/** The response of the active tab: summary line, Headers/Body/Raw/Timing, errors, empty state. */
export const ResponsePane = memo(function ResponsePane({ tabId, responseTab, lastExchangeId, response, layout, onToggleLayout, onSend, onCancel, onRetryWith, onFocusUrl }: ResponsePaneProps) {
  const result = response?.status === 'done' ? response.result : undefined;
  const status = response?.status;
  const sending = status === 'sending';
  const elapsed = useElapsed(response?.startedAt, sending);
  const duration = response ? responseDuration(response) : undefined;
  // Only a stored exchange can be opened, compared or fetched.
  const exchangeId = storedExchangeId(result);
  const unstored = result != null && exchangeId == null;
  const recording = useAppStore((s) => s.recording);
  const previousId = response?.previousId;
  const [comparing, setComparing] = useState(false);
  useEffect(() => setComparing(false), [tabId]);

  const layoutButton = (
    <IconButton
      size="sm"
      label={layout === 'right' ? 'Show response below' : 'Show response on the right'}
      icon={layout === 'right' ? PanelBottom : PanelRight}
      shortcut="mod+shift+d"
      onClick={onToggleLayout}
    />
  );

  let summary: ReactNode = <span className={styles.headLabel}>Response</span>;
  if (sending) {
    summary = (
      <span className={styles.summary}>
        <span className={styles.pendingDot} aria-hidden />
        <span className={styles.summaryMuted}>Waiting for response</span>
        <span className={clsx(styles.summaryValue, 'tnum')}>{formatDuration(elapsed)}</span>
      </span>
    );
  } else if (result) {
    const res = result.response;
    const wire = result.body.wireSize;
    const decoded = result.body.size;
    summary = (
      <span className={styles.summary}>
        {res ? <StatusBadge status={res.status} statusMessage={res.statusMessage} showText className={styles.status} /> : result.error ? <StatusBadge error={result.error.kind} /> : null}
        {duration != null && (
          <Tooltip content="Total time (request sent → response complete)">
            {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
            <span className={clsx(styles.metric, styles[`dur-${durationSeverity(duration)}`], 'tnum')} tabIndex={0}>
              {formatDuration(duration)}
            </span>
          </Tooltip>
        )}
        {res && (
          <Tooltip content={result.body.encodings.length ? `${result.body.encodings.join(', ')} · ${formatBytes(wire)} → ${formatBytes(decoded)} decoded` : 'Response body size'}>
            {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
            <span className={clsx(styles.metric, 'tnum')} tabIndex={0}>
              {formatBytes(wire || decoded)}
            </span>
          </Tooltip>
        )}
        {res && <span className={styles.metricSubtle}>{res.httpVersion}</span>}
        {response?.restored && (
          <Tooltip content="Restored from the exchange recorded in Traffic">
            {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
            <span className={styles.restored} tabIndex={0}>
              <History size={11} strokeWidth={2} aria-hidden />
              {formatRelativeTime(response.startedAt)}
            </span>
          </Tooltip>
        )}
      </span>
    );
  }

  return (
    <section className={styles.pane} aria-label="Response">
      <header className={clsx(styles.head, layout === 'right' && styles.headInline)}>
        {summary}
        <div className={styles.headActions}>
          {exchangeId != null && previousId != null && (
            <IconButton size="sm" label="Compare with the previous response" icon={GitCompareArrows} onClick={() => setComparing(true)} tooltipSide="bottom" />
          )}
          {exchangeId != null && (
            <Tooltip content={`Open exchange #${exchangeId} in Traffic`}>
              <Button size="sm" variant="ghost" iconRight={ArrowUpRight} onClick={() => navigate(`/traffic/${exchangeId}`)}>
                Open in Traffic
              </Button>
            </Tooltip>
          )}
          {unstored && <UnstoredNote recording={recording} />}
          {layoutButton}
        </div>
      </header>
      <div className={styles.body}>
        {!response && <EmptyResponse lastExchangeId={lastExchangeId} />}
        {(status === 'sending' || status === 'restoring') && (
          <div className={styles.loading} aria-busy>
            <Skeleton lines={4} height={10} />
            {sending && (
              <Button size="sm" onClick={onCancel} kbd="escape">
                Cancel request
              </Button>
            )}
          </div>
        )}
        {status === 'cancelled' && (
          <EmptyState
            className={styles.fill}
            icon={CircleSlash}
            title="Request cancelled"
            description="Akev stopped waiting. The server may already have received the request and could still be processing it."
            actions={
              <Button icon={Send} onClick={onSend}>
                Send again
              </Button>
            }
          />
        )}
        {status === 'error' && response?.error && <ApiFailure error={response.error} onRetry={onSend} />}
        {result && (
          <>
            {result.error && (
              <ExchangeFailure
                kind={result.error.kind}
                message={result.error.message}
                code={result.error.code}
                url={response?.url ?? ''}
                partial={!!result.response}
                exchangeId={result.id}
                onRetry={onSend}
                onRetryWith={onRetryWith}
                onFocusUrl={onFocusUrl}
              />
            )}
            {result.response && <ResponseTabs tabId={tabId} value={responseTab} response={response as ResponseState} size={layout === 'right' ? 'md' : 'sm'} />}
          </>
        )}
      </div>
      {exchangeId != null && previousId != null && (
        <Dialog
          open={comparing}
          onOpenChange={setComparing}
          title="Compare with the previous response"
          description={`Exchange #${previousId} (before) → #${exchangeId} (latest send)`}
          width={1120}
          flush
          focusContent
        >
          <div className={styles.compare}>
            <ExchangeCompare a={previousId} b={exchangeId} />
          </div>
        </Dialog>
      )}
    </section>
  );
});

// =====================================================================================

function UnstoredNote({ recording }: { recording: boolean }) {
  return (
    <Tooltip content="It isn't in Traffic, so it can't be opened there, compared or restored after a reload.">
      {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
      <span className={styles.unstored} tabIndex={0} role="note">
        <CirclePause size={11} strokeWidth={2} aria-hidden />
        {notStoredMessage(recording)}
      </span>
    </Tooltip>
  );
}

function EmptyResponse({ lastExchangeId }: { lastExchangeId?: number }) {
  return (
    <EmptyState
      className={styles.fill}
      icon={Send}
      title="Send a request to see the response"
      description={
        <>
          Press <Kbd combo="mod+enter" size="sm" /> anywhere in the request. Responses are recorded in Traffic too.
          {lastExchangeId != null && <span className={styles.emptyNote}> The previous response is no longer available.</span>}
        </>
      }
    />
  );
}

function ResponseTabs({ tabId, value: requested, response, size }: { tabId: string; value: ResponseTabId; response: ResponseState; size: 'sm' | 'md' }) {
  const result = response.result;
  const res = result?.response;
  const cookieCount = useMemo(() => res?.headers.filter(([n]) => n.toLowerCase() === 'set-cookie').length ?? 0, [res]);
  // Timing and connection details come from the stored exchange; an unstored response
  // (recording paused) has none.
  const stored = storedExchangeId(result);
  const detailTabs = stored != null;
  // The Cookies tab only exists when the response sets cookies.
  const value: ResponseTabId = (requested === 'cookies' && cookieCount === 0) || (!detailTabs && (requested === 'timing' || requested === 'connection')) ? 'body' : requested;
  const { detail, error } = useExchangeDetail(stored, value === 'timing' || value === 'connection');
  const raw = useMemo(() => {
    if (!res || !result || value !== 'raw') return '';
    return renderRawMessage(`${res.httpVersion === 'HTTP/2' ? 'HTTP/2' : res.httpVersion} ${res.status}${res.statusMessage ? ` ${res.statusMessage}` : ''}`, res.headers, result.body.text, result.body.size);
  }, [res, result, value]);
  if (!res || !result) return null;
  const setTab = (responseTab: ResponseTabId) => tabsActions.patch(tabId, { responseTab });
  let path = '/';
  try {
    path = new URL(response.url ?? '').pathname;
  } catch {
    // keep '/'
  }
  const bodySize = result.body.size;

  return (
    <Tabs value={value} onValueChange={setTab} className={styles.tabs}>
      <TabList
        size={size}
        aria-label="Response"
        className={styles.tabList}
        items={[
          { value: 'body', label: 'Body', badge: bodySize ? formatBytes(bodySize) : undefined },
          { value: 'headers', label: 'Headers', count: res.headers.length },
          ...(cookieCount ? [{ value: 'cookies' as const, label: 'Cookies', count: cookieCount }] : []),
          { value: 'raw', label: 'Raw' },
          ...(detailTabs
            ? [
                { value: 'timing' as const, label: 'Timing' },
                { value: 'connection' as const, label: 'Connection' },
              ]
            : []),
        ]}
        trailing={value === 'raw' ? <CopyButton value={raw} label="Copy raw response" /> : undefined}
      />
      <TabsContent value="body" className={styles.panel}>
        <BodyViewer {...composeBodySource(result)} contentType={getHeader(res.headers, 'content-type') ?? null} path={path} refreshKey={result.id} />
      </TabsContent>
      <TabsContent value="headers" className={clsx(styles.panel, styles.scroll)}>
        <HeadersTable headers={res.headers} httpVersion={res.httpVersion} side="response" emptyText="No response headers" />
      </TabsContent>
      <TabsContent value="raw" className={styles.panel}>
        <CodeEditor value={raw} language="http" readOnly fill wrap contentId={`${result.id}:raw`} aria-label="Raw response" />
      </TabsContent>
      {cookieCount > 0 && (
        <TabsContent value="cookies" className={clsx(styles.panel, styles.scroll)}>
          <CookiesTable headers={res.headers} side="response" />
        </TabsContent>
      )}
      <TabsContent value="timing" className={clsx(styles.panel, styles.scroll)}>
        <DetailState detail={detail} error={error} what="timing">
          {(d) => <TimingView detail={d} />}
        </DetailState>
      </TabsContent>
      <TabsContent value="connection" className={clsx(styles.panel, styles.scroll)}>
        <DetailState detail={detail} error={error} what="connection details">
          {(d) => <ConnectionView detail={d} />}
        </DetailState>
      </TabsContent>
    </Tabs>
  );
}

/** Loading / error / content for views that need the full exchange. */
function DetailState({ detail, error, what, children }: { detail: ExchangeDetail | null; error: string | null; what: string; children: (detail: ExchangeDetail) => ReactNode }) {
  if (detail) return <>{children(detail)}</>;
  if (error) return <EmptyState size="sm" icon={TriangleAlert} title={`Couldn't load ${what}`} description={error} />;
  return (
    <div className={styles.loading} aria-busy>
      <Skeleton lines={5} height={10} />
    </div>
  );
}

// =====================================================================================
// Failures
// =====================================================================================

function ApiFailure({ error, onRetry }: { error: NonNullable<ResponseState['error']>; onRetry: () => void }) {
  const network = error.code === 'network' || error.code === 'timeout';
  return (
    <div className={styles.failure} role="alert">
      <div className={styles.failureIcon}>{network ? <Unplug size={18} strokeWidth={1.75} aria-hidden /> : <ServerCrash size={18} strokeWidth={1.75} aria-hidden />}</div>
      <div className={styles.failureText}>
        <h3 className={styles.failureTitle}>{network ? "Couldn't reach Akev" : 'Akev rejected the request'}</h3>
        <p className={styles.failureExplain}>
          {network ? "The request never left: Akev isn't responding. It may be restarting — the UI reconnects automatically." : 'Akev refused to send this request. Check the URL, method and headers.'}
        </p>
        <code className={styles.failureCode}>
          {error.status ? `${error.status} · ` : ''}
          {error.code} — {error.message}
        </code>
        <div className={styles.failureActions}>
          <Button size="sm" variant="primary" icon={RefreshCw} onClick={onRetry}>
            Try again
          </Button>
        </div>
      </div>
    </div>
  );
}

interface ExchangeFailureProps {
  kind: ErrorKind;
  message: string;
  code?: string;
  url: string;
  partial: boolean;
  exchangeId: number;
  onRetry: () => void;
  onRetryWith: (patch: Partial<RequestOptions>) => void;
  onFocusUrl: () => void;
}

function ExchangeFailure({ kind, message, code, url, partial, onRetry, onRetryWith, onFocusUrl }: ExchangeFailureProps) {
  let host = '';
  try {
    host = new URL(url).host;
  } catch {
    host = '';
  }
  const info = describeError(kind, host);
  const tls = kind === 'upstream-cert-invalid' || kind === 'upstream-tls-failed';
  const urlish = kind === 'dns-failed' || kind === 'connection-refused' || kind === 'host-unreachable';
  return (
    <div className={clsx(styles.failure, partial && styles.failureCompact)} role="alert">
      <div className={styles.failureIcon}>{tls ? <ShieldOff size={18} strokeWidth={1.75} aria-hidden /> : kind === 'timeout' ? <Timer size={18} strokeWidth={1.75} aria-hidden /> : <TriangleAlert size={18} strokeWidth={1.75} aria-hidden />}</div>
      <div className={styles.failureText}>
        <h3 className={styles.failureTitle}>{partial ? `The response was cut short — ${info.title.charAt(0).toLowerCase()}${info.title.slice(1)}` : info.title}</h3>
        {!partial && (
          <p className={styles.failureExplain}>
            {info.explanation}
            {info.fix && <span className={styles.failureFix}> {info.fix}</span>}
          </p>
        )}
        <code className={styles.failureCode}>
          {code ? `${code} — ` : ''}
          {message}
        </code>
        {!partial && (
          <div className={styles.failureActions}>
            {tls && (
              <Button size="sm" variant="primary" icon={ShieldOff} onClick={() => onRetryWith({ insecure: true })}>
                Retry ignoring TLS errors
              </Button>
            )}
            {kind === 'timeout' && (
              <Button size="sm" variant="primary" icon={Timer} onClick={() => onRetryWith({ timeoutMs: 60_000 })}>
                Retry with a 60 s timeout
              </Button>
            )}
            <Button size="sm" variant={tls || kind === 'timeout' ? 'secondary' : 'primary'} icon={RefreshCw} onClick={onRetry}>
              Try again
            </Button>
            {urlish && (
              <Button size="sm" variant="ghost" onClick={onFocusUrl}>
                Edit URL
              </Button>
            )}
            {kind === 'upstream-proxy-failed' && (
              <Button size="sm" variant="ghost" onClick={() => navigate('/settings/proxy')}>
                Proxy settings
              </Button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
