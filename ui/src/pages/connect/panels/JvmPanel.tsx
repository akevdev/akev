import { Cpu, RefreshCw, RotateCcw } from 'lucide-react';
import { useMemo } from 'react';
import type { InterceptorInfo } from '../../../api/types';
import { navigate } from '../../../app/router';
import { ProCallout } from '../../../components/pro';
import { Badge, Button, Checkbox, IconButton, toast } from '../../../components/ui';
import { useFeature } from '../../../state/license';
import { usePagePref } from '../../../state/ui';
import { activateInterceptor, clearActivationResult, deactivateInterceptor, pendingKey, useActivationResult, usePending } from '../actions';
import { agentWarnings, jvmSubtitle, shortProcessName } from '../jvm';
import type { JvmProcess } from '../metadata';
import { Callout, PanelEmpty, PanelSection, ResultBlock, TargetList, TargetRow } from './kit';
import { useRefreshInterceptors, type PanelProps } from './types';
import styles from './panels.module.css';
import own from './JvmPanel.module.css';

const FALLBACK_NOTES = [
  "Intercepts HttpURLConnection (Spring RestTemplate, Feign, Jersey…), java.net.http.HttpClient, and clients created after attaching with the JVM's default settings (OkHttp, Apache HttpClient, Netty).",
  'Clients built before attaching with their own trust settings keep connecting directly.',
];

function JvmRow({ info, process: p, includeLoopback, licensed }: { info: InterceptorInfo; process: JvmProcess; includeLoopback: boolean; licensed: boolean }) {
  const key = pendingKey(info.id, p.pid);
  const stopKey = pendingKey(`${info.id}:stop`, p.pid);
  const attaching = usePending(key);
  const restoring = usePending(stopKey);
  const result = useActivationResult(key);
  const warnings = useMemo(() => agentWarnings(result?.metadata), [result]);
  const title = shortProcessName(p.name);
  const elsewhere = p.attached && !p.active;
  const attach = async () => {
    // Quiet: the daemon's message repeats the agent's warnings, which the row shows itself.
    const res = await activateInterceptor(info, { pid: p.pid, interceptLoopback: includeLoopback }, { key, target: title, verb: 'attach to', quiet: true });
    if (!res?.success) return;
    toast.success(`Attached to ${title}`, {
      description: agentWarnings(res.metadata).length > 0 ? 'With limitations — see the JVM panel.' : 'Its HTTP(S) requests now show up in Traffic.',
      action: { label: 'View traffic', onClick: () => navigate('/traffic') },
    });
  };
  const restore = () => void deactivateInterceptor(info, { pid: p.pid }, { key: stopKey, target: title });

  return (
    <TargetRow
      icon={<Cpu size={16} strokeWidth={1.75} />}
      title={title}
      subtitle={jvmSubtitle(p)}
      badge={
        p.active ? (
          <Badge variant="success" size="sm" dot>
            Attached
          </Badge>
        ) : elsewhere ? (
          <Badge variant="warning" size="sm">
            Other session
          </Badge>
        ) : undefined
      }
      hint={elsewhere ? `Attached by another Akev session${p.attachedPort ? ` (port ${p.attachedPort})` : ''}: restore it, or attach here.` : undefined}
      active={p.active}
      action={
        <span className={own.actions}>
          {p.attached && (
            <Button size="sm" variant="ghost" icon={RotateCcw} loading={restoring} onClick={restore}>
              Restore
            </Button>
          )}
          {!p.active && (
            <Button size="sm" variant="primary" disabled={!info.available || !licensed} loading={attaching} onClick={() => void attach()}>
              {elsewhere ? 'Attach here' : 'Attach'}
            </Button>
          )}
        </span>
      }
    >
      {p.active && result?.success && warnings.length > 0 && (
        <ResultBlock message="Attached — with limitations" onDismiss={() => clearActivationResult(key)}>
          <ul className={styles.bullets}>
            {warnings.map((w) => (
              <li key={w}>{w}</li>
            ))}
          </ul>
        </ResultBlock>
      )}
    </TargetRow>
  );
}

/** JVM attach: running Java processes from metadata; attach (Java agent) / restore per process. */
export function JvmPanel({ info, meta }: PanelProps) {
  const { refreshing, refresh } = useRefreshInterceptors(4000);
  const [loopback, setLoopback] = usePagePref<boolean>('connect.jvmLoopback', false);
  const includeLoopback = loopback === true;
  const notes = meta.notes.length > 0 ? meta.notes : FALLBACK_NOTES;
  const licensed = useFeature('jvm-attach');
  return (
    <>
      {!licensed && (
        <ProCallout feature="jvm-attach" title="JVM attach is part of Akev Pro">
          Intercept running Java, Kotlin and Scala apps without restarting them. Apps attached earlier can still be restored below.
        </ProCallout>
      )}
      {!info.available && (
        <Callout tone="warning" title="Java isn't available">
          {info.unavailableReason ?? 'Install a JDK (or set JAVA_HOME) to attach to running JVMs.'}
        </Callout>
      )}
      <PanelSection
        title="Running JVMs"
        description="Attaching loads a small Java agent into the running app — no restart needed. Pick the app, attach, and its requests show up in Traffic."
        actions={<IconButton label="Refresh processes" icon={RefreshCw} size="sm" loading={refreshing} onClick={refresh} />}
      >
        {meta.processes.length === 0 ? (
          <PanelEmpty
            icon={Cpu}
            title="No Java processes found"
            action={
              <Button size="sm" icon={RefreshCw} loading={refreshing} onClick={refresh}>
                Check again
              </Button>
            }
          >
            Start your Java, Kotlin or Scala app and it appears here.
          </PanelEmpty>
        ) : (
          <TargetList label="Java processes">
            {meta.processes.map((p) => (
              <JvmRow key={p.pid} info={info} process={p} includeLoopback={includeLoopback} licensed={licensed} />
            ))}
          </TargetList>
        )}
        <Checkbox
          checked={includeLoopback}
          onCheckedChange={setLoopback}
          label="Include localhost requests"
          description="Also intercept calls to localhost, 127.0.0.1 and ::1. Off by default so local databases and dev services keep working directly. Applies to the next attach."
        />
      </PanelSection>
      <PanelSection title="What gets intercepted">
        <ul className={styles.bullets}>
          {notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      </PanelSection>
    </>
  );
}
