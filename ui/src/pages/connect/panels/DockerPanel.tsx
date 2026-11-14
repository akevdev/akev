import { Box, RefreshCw, Unplug } from 'lucide-react';
import { useMemo } from 'react';
import type { InterceptorInfo } from '../../../api/types';
import { Badge, Button, IconButton, SegmentedControl, Tooltip } from '../../../components/ui';
import { usePagePref } from '../../../state/ui';
import { activateInterceptor, clearActivationResult, deactivateInterceptor, pendingKey, useActivationResult, usePending } from '../actions';
import { CommandBlock } from '../CommandBlock';
import { parseActivation, type DockerContainer } from '../metadata';
import { dockerRunSnippet } from '../snippets';
import { Callout, PanelEmpty, PanelSection, ResultBlock, TargetList, TargetRow } from './kit';
import { useRefreshInterceptors, type PanelProps } from './types';
import styles from './panels.module.css';

const ENGINE_LABEL: Record<string, string> = { docker: 'Docker', podman: 'Podman' };

function engineLabel(kind: string): string {
  return ENGINE_LABEL[kind] ?? kind.charAt(0).toUpperCase() + kind.slice(1);
}

function ContainerRow({ info, container: c, showEngine }: { info: InterceptorInfo; container: DockerContainer; showEngine: boolean }) {
  const key = pendingKey(info.id, c.id);
  const stopKey = pendingKey(`${info.id}:stop`, c.id);
  const attaching = usePending(key);
  const detaching = usePending(stopKey);
  const result = useActivationResult(key);
  const details = useMemo(() => (result ? parseActivation(result.metadata) : null), [result]);
  const subtitle = [c.image || 'unknown image', c.status ?? c.id.slice(0, 12)].join(' · ');

  return (
    <TargetRow
      icon={<Box size={16} strokeWidth={1.75} />}
      title={c.name}
      subtitle={subtitle}
      badge={
        <>
          {showEngine && c.engine && <Badge size="sm">{engineLabel(c.engine)}</Badge>}
          {c.active && (
            <Badge variant="success" size="sm" dot>
              Attached
            </Badge>
          )}
        </>
      }
      active={c.active}
      action={
        c.active ? (
          <Button size="sm" variant="ghost" icon={Unplug} loading={detaching} onClick={() => void deactivateInterceptor(info, { containerId: c.id }, { key: stopKey, target: c.name })}>
            Detach
          </Button>
        ) : (
          <Button size="sm" variant="primary" disabled={!info.available} loading={attaching} onClick={() => void activateInterceptor(info, { containerId: c.id }, { key, target: c.name, verb: 'attach to', quiet: true })}>
            Attach
          </Button>
        )
      }
    >
      {result && details && (
        <ResultBlock message={result.message ?? `Attached to ${c.name}.`} onDismiss={() => clearActivationResult(key)}>
          {details.execCommand && <CommandBlock code={details.execCommand} title="Intercepted shell inside the container" wrap={false} copyToast="Command copied — run it in a terminal" />}
        </ResultBlock>
      )}
    </TargetRow>
  );
}

type SnippetKind = string; // `run:<engine>` | `compose` | `host`

/** Docker: attach to running containers, snippets for new ones. */
export function DockerPanel({ info, meta, proxy }: PanelProps) {
  const { refreshing, refresh } = useRefreshInterceptors(4000);
  const containers = meta.containers;
  const engines = meta.engines;
  const showEngine = new Set(containers.map((c) => c.engine).filter(Boolean)).size > 1;
  const showEngines = engines.length > 1 || engines.some((e) => !e.available);

  const variants = useMemo(() => {
    const out: { value: SnippetKind; label: string; code: string; note?: string }[] = [];
    const d = meta.docker;
    if (d) {
      for (const r of d.run) out.push({ value: `run:${r.engine}`, label: `${r.engine} run`, code: r.command });
      if (d.compose) out.push({ value: 'compose', label: 'Compose', code: d.compose });
      if (d.runHostNetwork) out.push({ value: 'host', label: 'Host network', code: d.runHostNetwork, note: "Shares the host's network, so it works while the proxy only accepts local connections." });
    }
    if (out.length === 0) out.push({ value: 'run:docker', label: 'docker run', code: meta.command ?? dockerRunSnippet(proxy.port ?? 8000, meta.caPath ?? undefined) });
    return out;
  }, [meta.docker, meta.command, meta.caPath, proxy.port]);
  const [storedVariant, setVariant] = usePagePref<SnippetKind>('connect.dockerSnippet', variants[0]?.value ?? 'run:docker');
  const variant = variants.find((v) => v.value === storedVariant) ?? variants[0];
  const notes = meta.docker?.notes.length ? meta.docker.notes : ['Save the CA certificate (PEM) to the mounted path first. On Linux, host.docker.internal needs the --add-host flag shown.'];

  return (
    <>
      {!info.available && (
        <Callout tone="warning" title="No container engine is reachable">
          {info.unavailableReason ?? 'Start Docker Desktop (or the Docker / Podman service) and refresh.'}
        </Callout>
      )}
      <PanelSection
        title="Running containers"
        description="Attaching gives you an intercepted shell inside the container and makes it trust the Akev CA. To capture a container's main process, start it with the settings below."
        actions={<IconButton label="Refresh containers" icon={RefreshCw} size="sm" loading={refreshing} onClick={refresh} />}
      >
        {showEngines && (
          <div className={styles.engines} role="group" aria-label="Container engines">
            {engines.map((e) =>
              e.available ? (
                <Badge key={e.kind} variant="success" size="sm" dot>
                  {engineLabel(e.kind)}
                </Badge>
              ) : (
                <Tooltip key={e.kind} content={e.error ?? 'Not reachable'} multiline>
                  {/* biome-ignore lint/a11y/noNoninteractiveTabindex: focusable so keyboard users can open its tooltip */}
                  <span tabIndex={0} className={styles.focusable}>
                    <Badge variant="warning" size="sm">
                      {engineLabel(e.kind)} unavailable
                    </Badge>
                  </span>
                </Tooltip>
              ),
            )}
          </div>
        )}
        {containers.length === 0 ? (
          <PanelEmpty
            icon={Box}
            title="No running containers"
            action={
              <Button size="sm" icon={RefreshCw} loading={refreshing} onClick={refresh}>
                Check again
              </Button>
            }
          >
            Start a container and it appears here, or launch new containers with the settings below.
          </PanelEmpty>
        ) : (
          <TargetList label="Containers">
            {containers.map((c) => (
              <ContainerRow key={c.id} info={info} container={c} showEngine={showEngine} />
            ))}
          </TargetList>
        )}
      </PanelSection>
      <PanelSection title="New containers" description="Start containers with these settings to intercept them from their very first request.">
        {variants.length > 1 && (
          <SegmentedControl<SnippetKind> value={variant?.value ?? ''} onValueChange={setVariant} items={variants.map((v) => ({ value: v.value, label: v.label }))} size="sm" aria-label="Snippet" className={styles.selfStart} />
        )}
        {variant && <CommandBlock code={variant.code} wrap={variant.value === 'compose' ? false : undefined} aria-label={variant.label} />}
        {variant?.note && <p className={styles.footnote}>{variant.note}</p>}
        <ul className={styles.bullets}>
          {notes.map((n) => (
            <li key={n}>{n}</li>
          ))}
        </ul>
      </PanelSection>
    </>
  );
}
