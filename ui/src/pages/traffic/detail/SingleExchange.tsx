import { Copy, FileDown, Link, MessageSquareReply, Pin, RotateCw, SquarePen, Unplug } from 'lucide-react';
import { useEffect, useMemo, useState, type ReactNode } from 'react';
import { api, errorMessage } from '../../../api/client';
import type { CodegenTarget, EditableResponse, ExchangeDetail, ExchangeId, ExchangeSummary, PausedExchange } from '../../../api/types';
import { useCommands } from '../../../app/commands';
import { Button, EmptyState, Skeleton, SplitPane, toast } from '../../../components/ui';
import { useAppStore } from '../../../state/app';
import { useExchangeSummary } from '../../../state/traffic';
import { usePagePref } from '../../../state/ui';
import { COPY_TARGET_PREF } from './components/HeaderActions';
import { ErrorBanner } from './components/ErrorBanner';
import { NoticesBanner } from './components/Notices';
import { ExchangeHeader } from './components/ExchangeHeader';
import { RequestSection, ResponseSection } from './components/ExchangeSections';
import { OriginalBanner, PausedBanner } from './components/PausedBanner';
import { RequestEditor, ResponseEditor } from './components/BreakpointEditors';
import { RespondDialog } from './components/RespondDialog';
import { ClientErrorLayout, TlsFailureLayout, TunnelLayout } from './components/SpecialLayouts';
import { copyAsSnippet, copyUrls, createRule, exportHar, openInComposeFor, replay, setPinned } from '../actions/exchangeActions';
import { useBreakpointDraft } from './hooks/useBreakpointDraft';
import { usePausedControls } from './hooks/usePausedControls';
import { useExchangeDetail } from './hooks/useExchangeDetail';
import { useElementWidth } from './hooks/misc';
import { SIDE_BY_SIDE_MIN } from './layout';
import { headsFor, pausedSince } from './lib/detail';
import styles from './DetailPane.module.css';


export interface SingleExchangeProps {
  id: ExchangeId;
  orientation: 'right' | 'bottom';
}

/** One selected exchange: fetches its detail and picks the layout for its kind. */
export function SingleExchange({ id, orientation }: SingleExchangeProps) {
  const { detail, detailId, loading, stale, error, notFound, reload } = useExchangeDetail(id);
  const shownId = detailId ?? id;
  const live = useExchangeSummary(shownId);
  const pendingSummary = useExchangeSummary(id);

  if (!detail) {
    if (error) {
      return (
        <div className={styles.center}>
          <EmptyState
            icon={Unplug}
            tone={notFound ? 'neutral' : 'danger'}
            title={notFound ? 'This exchange no longer exists' : "Couldn't load this exchange"}
            description={notFound ? 'It was deleted or evicted from memory.' : error}
            actions={!notFound && <Button onClick={reload}>Retry</Button>}
          />
        </div>
      );
    }
    return <LoadingDetail summary={pendingSummary} />;
  }

  const summary = live ?? detail.summary;
  const body = (() => {
    switch (summary.kind) {
      case 'tls-failure':
        return <TlsFailureLayout detail={detail} summary={summary} />;
      case 'tunnel':
        return <TunnelLayout detail={detail} summary={summary} />;
      case 'client-error':
        return <ClientErrorLayout detail={detail} summary={summary} />;
      default:
        return <HttpExchange key={shownId} detail={detail} summary={summary} orientation={orientation} />;
    }
  })();

  return (
    <div className={styles.single} data-stale={(stale && loading) || undefined} aria-busy={loading || undefined}>
      {loading && stale && <div className={styles.progress} aria-hidden />}
      {body}
    </div>
  );
}

function LoadingDetail({ summary }: { summary: ExchangeSummary | undefined }) {
  return (
    <div className={styles.single}>
      <div className={styles.progress} aria-hidden />
      <div className={styles.loadingHeader}>
        {summary ? (
          <span className={styles.loadingTitle}>
            <span className={styles.loadingMethod}>{summary.method}</span>
            <span className="truncate">
              {summary.host}
              {summary.path}
            </span>
          </span>
        ) : (
          <Skeleton width="60%" height={14} />
        )}
        <Skeleton width="40%" height={10} />
      </div>
      <div className={styles.loadingBody}>
        <Skeleton lines={7} height={10} />
      </div>
    </div>
  );
}

// =====================================================================================
// HTTP / WebSocket / SSE
// =====================================================================================

interface HttpExchangeProps {
  detail: ExchangeDetail;
  summary: ExchangeSummary;
  orientation: 'right' | 'bottom';
}

function HttpExchange(props: HttpExchangeProps) {
  const listed = useAppStore((s) => s.paused.find((p) => p.id === props.summary.id));
  const paused = usePausedWithResponse(listed);
  useExchangeActions(props.summary, props.detail);
  if (paused) return <PausedHttpExchange key={`${paused.id}:${paused.stage}`} {...props} paused={paused} />;
  return <ExchangeLayout {...props} />;
}

/**
 * The paused entry, complete enough to edit. A response-stage entry without its response (older
 * daemons, or a `paused` event that raced the snapshot) gets it from `GET …/editable/response`.
 */
function usePausedWithResponse(paused: PausedExchange | undefined): PausedExchange | undefined {
  const missing = paused?.stage === 'response' && !paused.response ? paused.id : null;
  const [fetched, setFetched] = useState<{ id: ExchangeId; response: EditableResponse } | null>(null);
  useEffect(() => {
    if (missing == null) return;
    const ctrl = new AbortController();
    api.exchanges.editableResponse(missing, ctrl.signal).then(
      (response) => setFetched({ id: missing, response }),
      (e: unknown) => {
        if (!ctrl.signal.aborted) toast.error("Couldn't load the paused response", { description: errorMessage(e) });
      },
    );
    return () => ctrl.abort();
  }, [missing]);
  return useMemo(() => {
    if (!paused) return undefined;
    if (missing == null) return paused;
    return fetched?.id === missing ? { ...paused, response: fetched.response } : undefined;
  }, [paused, missing, fetched]);
}

interface LayoutProps extends HttpExchangeProps {
  /** Which side is being edited at a breakpoint (gets more room by default). */
  editing?: 'request' | 'response' | null;
  banner?: ReactNode;
  request?: ReactNode;
  response?: ReactNode;
  waitingForResume?: { onResume: () => void; busy: boolean } | null;
}

function ExchangeLayout({ detail, summary, orientation, editing, banner, request, response, waitingForResume }: LayoutProps) {
  const [showOriginal, setShowOriginal] = useState(false);
  const heads = headsFor(detail, showOriginal);
  const [ref, width] = useElementWidth<HTMLDivElement>();
  const sideBySide = width > SIDE_BY_SIDE_MIN;
  // LLM calls: the conversation (response section) gets most of the room by default.
  const llm = summary.llm != null && !editing;

  const requestEl = request ?? <RequestSection detail={detail} head={heads.request} original={heads.original} />;
  const responseEl = response ?? <ResponseSection detail={detail} summary={summary} head={heads.response} original={heads.original} waitingForResume={waitingForResume} />;

  return (
    <>
      <ExchangeHeader summary={summary} detail={detail} showOriginal={showOriginal} onToggleOriginal={() => setShowOriginal((v) => !v)}>
        {banner ?? (showOriginal ? <OriginalBanner onShowModified={() => setShowOriginal(false)} /> : detail.error && <ErrorBanner error={detail.error} summary={summary} />)}
        {detail.notices && detail.notices.length > 0 && <NoticesBanner notices={detail.notices} stacked={banner != null || showOriginal || detail.error != null} />}
      </ExchangeHeader>
      <div ref={ref} className={styles.sections}>
        {width > 0 &&
          (sideBySide ? (
            <SplitPane
              key={editing ? `h-${editing}` : llm ? 'h-llm' : 'h'}
              id={editing ? `traffic.detail.sections.h.${editing}` : llm ? 'traffic.detail.sections.h.llm' : 'traffic.detail.sections.h'}
              direction="horizontal"
              defaultRatio={editing === 'request' ? 0.58 : editing === 'response' ? 0.42 : llm ? 0.4 : 0.5}
              minFirst={320}
              minSecond={320}
              dividerLabel="Resize request and response"
            >
              {requestEl}
              {responseEl}
            </SplitPane>
          ) : (
            <SplitPane
              key={editing ? `v-${editing}` : llm ? 'v-llm' : 'v'}
              id={editing ? `traffic.detail.sections.v.${editing}` : llm ? 'traffic.detail.sections.v.llm' : 'traffic.detail.sections.v'}
              direction="vertical"
              defaultRatio={editing === 'request' ? 0.64 : editing === 'response' ? 0.34 : llm ? 0.26 : orientation === 'bottom' ? 0.5 : 0.42}
              minFirst={120}
              minSecond={140}
              dividerLabel="Resize request and response"
            >
              {requestEl}
              {responseEl}
            </SplitPane>
          ))}
      </div>
    </>
  );
}

// =====================================================================================
// Paused at a breakpoint
// =====================================================================================

function PausedHttpExchange(props: HttpExchangeProps & { paused: PausedExchange }) {
  const { paused, detail, summary } = props;
  const bp = useBreakpointDraft(paused);
  const { busy, invalid, run, respondOpen, setRespondOpen, othersPaused, openNextPaused } = usePausedControls(paused, bp, `${summary.method} ${summary.host}${summary.path.split('?')[0]}`);

  const banner = (
    <PausedBanner
      stage={paused.stage}
      since={pausedSince(detail, summary)}
      changes={bp.changes}
      invalid={invalid}
      busy={busy}
      onResume={() => void run('resume')}
      onRespond={paused.stage === 'request' ? () => setRespondOpen(true) : undefined}
      onClose={() => void run('close')}
      onReset={() => void run('reset')}
      onRevert={bp.revert}
      othersPaused={othersPaused}
      onNextPaused={openNextPaused}
    />
  );

  return (
    <>
      <ExchangeLayout
        {...props}
        editing={paused.stage}
        banner={banner}
        request={bp.request ? <RequestEditor draft={bp.request} onChange={bp.setRequest} errors={bp.errors} onSubmit={() => void run('resume')} /> : undefined}
        response={bp.response ? <ResponseEditor draft={bp.response} onChange={bp.setResponse} errors={bp.errors} onSubmit={() => void run('resume')} /> : undefined}
        waitingForResume={paused.stage === 'request' ? { onResume: () => void run('resume'), busy: busy === 'resume' } : null}
      />
      {paused.stage === 'request' && <RespondDialog id={paused.id} open={respondOpen} onOpenChange={setRespondOpen} onResponded={bp.clear} />}
    </>
  );
}

// =====================================================================================
// Shortcuts & palette commands for the shown exchange
// =====================================================================================

function useExchangeActions(summary: ExchangeSummary, detail: ExchangeDetail) {
  const id = summary.id;
  const [target] = usePagePref<CodegenTarget>(COPY_TARGET_PREF, 'curl');
  const pinned = summary.pinned === true;
  // Keyboard shortcuts act on the selection and are registered once, by the traffic list.
  useCommands([
    { id: 'exchange.replay', title: 'Replay exchange', subtitle: `#${id}`, group: 'Traffic', icon: RotateCw, shortcut: 'mod+r', run: () => void replay([id]) },
    { id: 'exchange.compose', title: 'Edit in Compose', subtitle: `#${id}`, group: 'Traffic', icon: SquarePen, shortcut: 'mod+e', run: () => void openInComposeFor(id) },
    { id: 'exchange.copy-curl', title: 'Copy as cURL', group: 'Traffic', icon: Copy, shortcut: 'mod+shift+c', run: () => void copyAsSnippet([id], 'curl') },
    ...(target !== 'curl' ? [{ id: 'exchange.copy-last', title: `Copy as ${target}`, group: 'Traffic', icon: Copy, run: () => void copyAsSnippet([id], target) }] : []),
    { id: 'exchange.copy-url', title: 'Copy URL', group: 'Traffic', icon: Link, run: () => void copyUrls([id]) },
    { id: 'exchange.pin', title: pinned ? 'Unpin exchange' : 'Pin exchange', group: 'Traffic', icon: Pin, shortcut: 'mod+p', run: () => void setPinned([id], !pinned) },
    {
      id: 'exchange.mock',
      title: 'Create mock rule from exchange',
      group: 'Rules',
      icon: MessageSquareReply,
      disabled: !detail.response,
      run: () => void createRule(id, 'mock'),
    },
    { id: 'exchange.har', title: 'Export exchange as HAR', group: 'Traffic', icon: FileDown, run: () => exportHar('selected', [id]) },
  ]);
}
