import { CircleCheck, CirclePause, EyeOff, Play } from 'lucide-react';
import type { EditableRequest, ExchangeId, PausedExchange } from '../../../api/types';
import { navigate } from '../../../app/router';
import { HeadersTable } from '../../../components/exchange';
import { Button, EmptyState, MethodBadge, SplitPane, StatusBadge } from '../../../components/ui';
import { formatBytes } from '../../../lib/format';
import { useAppStore } from '../../../state/app';
import { useLastTab } from '../../../state/ui';
import { RequestEditor, ResponseEditor } from './components/BreakpointEditors';
import { PausedBanner } from './components/PausedBanner';
import { RespondDialog } from './components/RespondDialog';
import { SectionPanel, SectionTabs } from './components/SectionTabs';
import { UrlTitle } from './components/UrlTitle';
import { useElementWidth } from './hooks/misc';
import { useBreakpointDraft } from './hooks/useBreakpointDraft';
import { usePausedControls } from './hooks/usePausedControls';
import { decodeEditableBody } from './lib/breakpoint';
import { SIDE_BY_SIDE_MIN } from './layout';
import paneStyles from './DetailPane.module.css';
import styles from './UnrecordedPausedExchange.module.css';

/**
 * A paused exchange that isn't in the traffic list (recording is paused, the capture settings
 * ignore its host, or it was cleared while waiting). Everything needed to edit and resume it
 * comes with the breakpoint itself, so it gets the same editors and controls as a listed one.
 */
export function UnrecordedPausedExchange({ id, orientation }: { id: ExchangeId; orientation: 'right' | 'bottom' }) {
  const paused = useAppStore((s) => s.paused.find((p) => p.id === id));
  if (!paused) {
    return (
      <div className={paneStyles.center}>
        <EmptyState
          icon={CircleCheck}
          title="No longer paused"
          description="It was resumed, or the client gave up waiting. It wasn't recorded, so there's nothing more to show."
          actions={<Button onClick={() => navigate('/traffic', { replace: true })}>Back to traffic</Button>}
        />
      </div>
    );
  }
  // A response-stage pause of the same exchange is a new pause: fresh drafts and controls.
  return <PausedView key={`${paused.id}:${paused.stage}`} paused={paused} orientation={orientation} />;
}

function urlParts(url: string): { scheme: string; host: string; port: number; path: string } | null {
  try {
    const u = new URL(url);
    const scheme = u.protocol.replace(/:$/, '');
    return { scheme, host: u.hostname, port: u.port ? Number(u.port) : scheme === 'https' || scheme === 'wss' ? 443 : 80, path: `${u.pathname}${u.search}` };
  } catch {
    return null;
  }
}

function PausedView({ paused, orientation }: { paused: PausedExchange; orientation: 'right' | 'bottom' }) {
  const bp = useBreakpointDraft(paused);
  const { method, url } = paused.request;
  const parts = urlParts(url);
  const controls = usePausedControls(paused, bp, `${method} ${parts ? `${parts.host}${parts.path.split('?')[0]}` : url}`);
  const { busy, invalid, run, respondOpen, setRespondOpen } = controls;
  const recording = useAppStore((s) => s.recording);
  const since = paused.pausedAt / 1000;
  const [ref, width] = useElementWidth<HTMLDivElement>();
  const sideBySide = width > SIDE_BY_SIDE_MIN;
  const editing = paused.stage;

  const request =
    paused.stage === 'request' && bp.request ? (
      <RequestEditor draft={bp.request} onChange={bp.setRequest} errors={bp.errors} onSubmit={() => void run('resume')} />
    ) : (
      <SentRequest request={paused.request} />
    );
  const response =
    paused.stage === 'response' && bp.response ? (
      <ResponseEditor draft={bp.response} onChange={bp.setResponse} errors={bp.errors} onSubmit={() => void run('resume')} />
    ) : (
      <div className={paneStyles.center}>
        <EmptyState
          icon={CirclePause}
          tone="warning"
          size="sm"
          title="Waiting for you to resume"
          description="The request hasn't been sent upstream yet, so there is no response."
          actions={
            <Button size="sm" variant="primary" icon={Play} kbd="mod+enter" loading={busy === 'resume'} disabled={invalid != null} onClick={() => void run('resume')}>
              Resume
            </Button>
          }
        />
      </div>
    );

  return (
    <div className={paneStyles.single}>
      <header className={styles.header}>
        <UrlTitle
          scheme={parts?.scheme ?? ''}
          host={parts?.host ?? ''}
          port={parts?.port ?? 0}
          path={parts?.path ?? ''}
          plain={parts ? undefined : url}
          resetKey={paused.id}
          leading={
            <>
              <MethodBadge method={method} variant="pill" />
              <StatusBadge paused={paused.stage} showText />
            </>
          }
        />
        <p className={styles.note}>
          <EyeOff size={13} strokeWidth={2} aria-hidden className={styles.noteIcon} />
          <span>
            <strong>Not recorded</strong> — {recording ? 'the capture settings ignore this host (or it was cleared while paused)' : 'recording is paused'}, so it won't appear in the traffic
            list. You can still edit and resume it here.
          </span>
        </p>
        <PausedBanner
          stage={paused.stage}
          since={since}
          changes={bp.changes}
          invalid={invalid}
          busy={busy}
          onResume={() => void run('resume')}
          onRespond={paused.stage === 'request' ? () => setRespondOpen(true) : undefined}
          onClose={() => void run('close')}
          onReset={() => void run('reset')}
          onRevert={bp.revert}
          othersPaused={controls.othersPaused}
          onNextPaused={controls.openNextPaused}
        />
      </header>
      <div ref={ref} className={paneStyles.sections}>
        {width > 0 &&
          (sideBySide ? (
            <SplitPane
              key={`h-${editing}`}
              id={`traffic.detail.sections.h.${editing}`}
              direction="horizontal"
              defaultRatio={editing === 'request' ? 0.58 : 0.42}
              minFirst={320}
              minSecond={320}
              dividerLabel="Resize request and response"
            >
              {request}
              {response}
            </SplitPane>
          ) : (
            <SplitPane
              key={`v-${editing}`}
              id={`traffic.detail.sections.v.${editing}`}
              direction="vertical"
              defaultRatio={editing === 'request' ? 0.64 : orientation === 'bottom' ? 0.5 : 0.34}
              minFirst={120}
              minSecond={140}
              dividerLabel="Resize request and response"
            >
              {request}
              {response}
            </SplitPane>
          ))}
      </div>
      {paused.stage === 'request' && <RespondDialog id={paused.id} open={respondOpen} onOpenChange={setRespondOpen} onResponded={bp.clear} />}
    </div>
  );
}

type SentTab = 'headers' | 'body';

/** The request as it was sent upstream (read-only: a response breakpoint edits the response). */
function SentRequest({ request }: { request: EditableRequest }) {
  const [tab, setTab] = useLastTab('traffic.breakpoint.sent', 'headers');
  const body = decodeEditableBody(request.body);
  return (
    <SectionTabs<SentTab>
      label="Request"
      value={tab as SentTab}
      onValueChange={setTab}
      items={[
        { value: 'headers', label: 'Headers', count: request.headers.length },
        { value: 'body', label: 'Body', badge: body.size ? formatBytes(body.size) : undefined },
      ]}
    >
      <SectionPanel value="headers" scroll>
        <HeadersTable headers={request.headers} side="request" />
      </SectionPanel>
      <SectionPanel value="body" scroll>
        {body.size === 0 ? (
          <p className={styles.empty}>No body</p>
        ) : body.text != null ? (
          <pre className={styles.body}>{body.text}</pre>
        ) : (
          <p className={styles.empty}>Binary body · {formatBytes(body.size)}</p>
        )}
      </SectionPanel>
    </SectionTabs>
  );
}
