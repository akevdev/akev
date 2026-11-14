import { CircleOff, CirclePause, Hourglass, Play, Sparkles } from 'lucide-react';
import { lazy, memo, Suspense, useCallback, useMemo, type ReactNode } from 'react';
import type { ExchangeDetail, ExchangeSummary, RequestHead, ResponseHead } from '../../../../api/types';
import { BodyViewer, CacheView, CodeView, ConnectionView, CookiesTable, HeadersTable, TimingView } from '../../../../components/exchange';
import { bodyRefreshKey } from '../../../../components/exchange/model/body';
import { Button, EmptyState, Spinner, toast } from '../../../../components/ui';
import { formatBytes, formatElapsed } from '../../../../lib/format';
import { errorInfo } from '../../../../lib/http';
import { trafficStore } from '../../../../state/traffic';
import { useLastTab } from '../../../../state/ui';
import { useFeature } from '../../../../state/license';
import { detectAuth } from '../lib/auth';
import {
  appendFilterClause,
  bodyDisplaySize,
  countHeaders,
  countRequestCookies,
  countSetCookies,
  headerFilterClause,
  isStreamKind,
  requestTabs,
  resolveTab,
  responseTabGroup,
  responseTabs,
  type RequestTab,
  type ResponseTab,
} from '../lib/detail';
import { extractQuery } from '../lib/query';
import { useNow } from '../hooks/misc';
import { AuthView } from './AuthView';
import { MessagesView } from './MessagesView';
import { DroppedBody } from './Notices';
import { QueryView } from './QueryView';
import { RawView } from './RawView';
import { SectionPanel, SectionTabs } from './SectionTabs';
import styles from './views.module.css';
import { showHeaderColumn } from '../../actions/headerColumns';

const PENDING = new Set(['requesting', 'waiting', 'responding', 'paused']);

/** The LLM conversation view is only needed for LLM API calls: a separate chunk. */
const LlmView = lazy(() => import('./llm/LlmView').then((m) => ({ default: m.LlmView })));

/** "Filter by this header" from the headers table: AND it onto the traffic filter. */
function useFilterByHeader() {
  return useCallback((name: string, value: string) => {
    const clause = headerFilterClause(name, value);
    trafficStore.setFilterText(appendFilterClause(trafficStore.getSnapshot().filter.text, clause));
    trafficStore.flushFilter();
    toast('Filter updated', { description: clause });
  }, []);
}

/** "Show as a list column" from the headers tables. */
const showRequestColumn = (name: string) => void showHeaderColumn('request', name);
const showResponseColumn = (name: string) => void showHeaderColumn('response', name);

// =====================================================================================
// Request
// =====================================================================================

export interface RequestSectionProps {
  detail: ExchangeDetail;
  head: RequestHead;
  original: boolean;
}

export const RequestSection = memo(function RequestSection({ detail, head, original }: RequestSectionProps) {
  const id = detail.summary.id;
  const onFilter = useFilterByHeader();
  const query = useMemo(() => extractQuery(head.path || head.url), [head.path, head.url]);
  const auth = useMemo(
    () =>
      detectAuth(
        head.headers,
        query.params.map((p) => [p.name, p.value] as const),
      ),
    [head.headers, query],
  );
  const [stored, setTab] = useLastTab('traffic.request', 'headers');
  const available = requestTabs({ hasAuth: auth.length > 0 });
  const tab = resolveTab<RequestTab>(available, stored);
  const bodySize = bodyDisplaySize(detail.requestBody);
  // Refetch once the request body is final (it may still be uploading or held at a breakpoint).
  const refreshKey = bodyRefreshKey(detail.summary, 'request');

  const labels: Record<RequestTab, { label: string; count?: number; badge?: ReactNode }> = {
    headers: { label: 'Headers', count: countHeaders(head.headers) },
    query: { label: 'Query', count: query.params.length },
    body: { label: 'Body', badge: bodySize ? formatBytes(bodySize) : undefined },
    cookies: { label: 'Cookies', count: countRequestCookies(head.headers) },
    auth: { label: 'Auth', count: auth.length > 1 ? auth.length : undefined },
    raw: { label: 'Raw' },
    code: { label: 'Code' },
  };

  return (
    <SectionTabs<RequestTab> label="Request" value={tab} onValueChange={setTab} items={available.map((value) => ({ value, ...labels[value] }))}>
      <SectionPanel value="headers" scroll pad={false}>
        <HeadersTable headers={head.headers} httpVersion={head.httpVersion} side="request" onFilter={onFilter} onShowColumn={showRequestColumn} emptyText="No request headers" />
      </SectionPanel>
      <SectionPanel value="query" scroll>
        <QueryView path={head.path || head.url} />
      </SectionPanel>
      <SectionPanel value="body">
        {detail.requestBody.dropped && !original ? (
          <DroppedBody meta={detail.requestBody} side="request" />
        ) : (
          <BodyViewer exchangeId={id} side="request" original={original} path={head.path} contentType={headerValue(head.headers, 'content-type')} refreshKey={refreshKey} />
        )}
      </SectionPanel>
      <SectionPanel value="cookies" scroll pad={false}>
        <CookiesTable headers={head.headers} side="request" />
      </SectionPanel>
      {auth.length > 0 && (
        <SectionPanel value="auth" scroll>
          <AuthView items={auth} exchangeId={id} />
        </SectionPanel>
      )}
      <SectionPanel value="raw">
        <RawView exchangeId={id} side="request" head={head} original={original} refreshKey={refreshKey} bodySize={detail.requestBody.wireSize || bodySize} />
      </SectionPanel>
      <SectionPanel value="code" scroll pad={false}>
        <CodeView exchangeId={id} />
      </SectionPanel>
    </SectionTabs>
  );
});

// =====================================================================================
// Response
// =====================================================================================

export interface ResponseSectionProps {
  detail: ExchangeDetail;
  summary: ExchangeSummary;
  head: ResponseHead | undefined;
  original: boolean;
  /** Paused at the request stage: response tabs explain that nothing was sent yet. */
  waitingForResume?: { onResume: () => void; busy: boolean } | null;
}

export function ResponseSection({ detail, summary, head, original, waitingForResume }: ResponseSectionProps) {
  const id = summary.id;
  const onFilter = useFilterByHeader();
  const stream = isStreamKind(summary.kind);
  const llm = summary.llm != null;
  // Without Pro the LLM tab is a locked teaser: don't open it by default.
  const llmLicensed = useFeature('llm-lens');
  const [stored, setTab] = useLastTab(responseTabGroup(summary.kind, llm), llm && llmLicensed ? 'llm' : stream ? 'messages' : 'body');
  const available = responseTabs({ stream, llm });
  const tab = resolveTab<ResponseTab>(available, stored);
  const bodySize = bodyDisplaySize(detail.responseBody);
  const headers = head?.headers ?? [];
  // Refetch the body once it is complete (not on every streamed byte).
  const refreshKey = bodyRefreshKey(summary, 'response');

  const labels: Record<ResponseTab, { label: string; count?: number; badge?: ReactNode; icon?: typeof Sparkles }> = {
    llm: { label: 'LLM', icon: Sparkles },
    messages: { label: summary.kind === 'event-stream' ? 'Events' : 'Messages', count: summary.messageCount ?? 0 },
    headers: { label: 'Headers', count: head ? countHeaders(headers) : undefined },
    body: { label: 'Body', badge: bodySize ? formatBytes(bodySize) : undefined },
    cookies: { label: 'Cookies', count: head ? countSetCookies(headers) : undefined },
    raw: { label: 'Raw' },
    timing: { label: 'Timing' },
    connection: { label: 'Connection' },
    cache: { label: 'Cache' },
  };

  const noResponse = !head ? <NoResponse summary={summary} detail={detail} waitingForResume={waitingForResume} /> : null;

  return (
    <SectionTabs<ResponseTab> label="Response" value={tab} onValueChange={setTab} items={available.map((value) => ({ value, ...labels[value] }))}>
      {llm && (
        <SectionPanel value="llm" scroll pad={false}>
          <Suspense fallback={<div className={styles.waiting}><Spinner size={13} tone="muted" /></div>}>
            <LlmView summary={summary} />
          </Suspense>
        </SectionPanel>
      )}
      {stream && (
        <SectionPanel value="messages">
          <MessagesView exchangeId={id} kind={summary.kind} startMicros={detail.timing.start} live={summary.state === 'streaming'} messageCount={summary.messageCount} noServer={summary.mocked} />
        </SectionPanel>
      )}
      <SectionPanel value="headers" scroll={!!head} pad={false}>
        {noResponse ?? <HeadersTable headers={headers} httpVersion={head?.httpVersion} side="response" onFilter={onFilter} onShowColumn={showResponseColumn} emptyText="No response headers" />}
      </SectionPanel>
      <SectionPanel value="body">
        {noResponse ??
          (detail.responseBody.dropped && !original ? (
            <DroppedBody meta={detail.responseBody} side="response" />
          ) : (
            <BodyViewer exchangeId={id} side="response" original={original} path={summary.path} contentType={headerValue(headers, 'content-type')} refreshKey={refreshKey} headers={headers} trailers={detail.responseTrailers} />
          ))}
      </SectionPanel>
      <SectionPanel value="cookies" scroll={!!head} pad={false}>
        {noResponse ?? <CookiesTable headers={headers} side="response" />}
      </SectionPanel>
      <SectionPanel value="raw">
        {noResponse ?? (head && <RawView exchangeId={id} side="response" head={head} original={original} refreshKey={refreshKey} bodySize={detail.responseBody.wireSize || bodySize} />)}
      </SectionPanel>
      <SectionPanel value="timing" scroll pad={false}>
        <TimingView detail={detail} />
      </SectionPanel>
      <SectionPanel value="connection" scroll pad={false}>
        <ConnectionView detail={detail} />
      </SectionPanel>
      <SectionPanel value="cache" scroll={!!head} pad={false}>
        {head ? <CacheView detail={detail} /> : noResponse}
      </SectionPanel>
    </SectionTabs>
  );
}

function NoResponse({ summary, detail, waitingForResume }: { summary: ExchangeSummary; detail: ExchangeDetail; waitingForResume?: ResponseSectionProps['waitingForResume'] }) {
  const pending = PENDING.has(summary.state);
  const now = useNow(250, pending && !summary.paused);
  if (summary.paused === 'request' || waitingForResume) {
    return (
      <div className={styles.waiting}>
        <EmptyState
          icon={CirclePause}
          tone="warning"
          size="sm"
          title="Waiting for you to resume"
          description="The request hasn't been sent upstream yet, so there is no response."
          actions={
            waitingForResume && (
              <Button size="sm" variant="primary" icon={Play} kbd="mod+enter" loading={waitingForResume.busy} onClick={waitingForResume.onResume}>
                Resume
              </Button>
            )
          }
        />
      </div>
    );
  }
  if (pending) {
    return (
      <div className={styles.waiting}>
        <EmptyState
          icon={Hourglass}
          size="sm"
          title={
            <span className={styles.toolbar}>
              <Spinner size={13} tone="muted" />
              Waiting for the response…
            </span>
          }
          description={`Sent ${formatElapsed(summary.startedAt, now)} ago${detail.upstream?.remoteAddr ? ` to ${detail.upstream.remoteAddr}` : ''}.`}
        />
      </div>
    );
  }
  return (
    <div className={styles.waiting}>
      <EmptyState
        icon={CircleOff}
        size="sm"
        title="No response"
        description={summary.error ? `${errorInfo(summary.error).title.replaceAll('{host}', summary.host)} — no response was received.` : 'This exchange ended without a response.'}
      />
    </div>
  );
}

function headerValue(headers: readonly [string, string][], name: string): string | undefined {
  const lower = name.toLowerCase();
  for (const [k, v] of headers) if (k.toLowerCase() === lower) return v;
  return undefined;
}
