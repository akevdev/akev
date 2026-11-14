import clsx from 'clsx';
import { AlertTriangle, Code2, Download, WrapText } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { api, errorMessage } from '../../api/client';
import type { CodegenTarget, EditableRequest, ExchangeId, Snippet, TargetInfo } from '../../api/types';
import { useShortcut } from '../../app/shortcuts';
import { LruCache } from '../../lib/lru';
import { saveData } from '../../lib/platform';
import { usePagePref } from '../../state/ui';
import { Button, CodeEditor, CopyButton, EmptyState, IconButton, Select, Skeleton, toast, type CodeEditorHandle } from '../ui';
import { groupTargets, pickTarget, requestKey, snippetFilename } from './model/code';
import styles from './CodeView.module.css';

export interface CodeViewProps {
  /** Generate from a captured exchange (`GET /api/exchanges/:id/snippet`)… */
  exchangeId?: ExchangeId;
  /** …or from an arbitrary request (`POST /api/codegen`, used by Compose). */
  request?: EditableRequest | null;
  className?: string;
  /** Optional: fill the parent's height (default true). */
  fill?: boolean;
}

/** Shared with the detail pane's "Copy as" button, so both remember the same target. */
export const CODE_TARGET_PREF = 'traffic.copyTarget';
const WRAP_PREF = 'exchange.code.wrap';
/** Compose edits re-generate after the user pauses typing. */
const REQUEST_DEBOUNCE = 250;

// ---------------------------------------------------------------- data (module caches)

let targetsPromise: Promise<TargetInfo[]> | null = null;
function loadTargets(): Promise<TargetInfo[]> {
  targetsPromise ??= api.codegen.targets().catch((e: unknown) => {
    targetsPromise = null;
    throw e;
  });
  return targetsPromise;
}

/** Snippets embed the request body, so bound the cache by size too (~32 MiB of UTF-16). */
const snippets = new LruCache<string, Snippet>(60, { maxWeight: 32 << 20, weigh: (s) => 2 * s.code.length });

function useTargets(): { targets: TargetInfo[] | null; error: string | null; retry: () => void } {
  const [state, setState] = useState<{ targets: TargetInfo[] | null; error: string | null }>({ targets: null, error: null });
  const [nonce, setNonce] = useState(0);
  useEffect(() => {
    let cancelled = false;
    loadTargets().then(
      (targets) => !cancelled && setState({ targets, error: null }),
      (e: unknown) => !cancelled && setState({ targets: null, error: errorMessage(e) }),
    );
    return () => {
      cancelled = true;
    };
  }, [nonce]);
  return { ...state, retry: () => setNonce((n) => n + 1) };
}

type Source = { type: 'exchange'; id: ExchangeId } | { type: 'request'; request: EditableRequest; key: string };

function useSnippet(source: Source | null, target: CodegenTarget | undefined) {
  const cacheKey = source && target ? `${source.type === 'exchange' ? `x${source.id}` : `r${source.key}`}:${target}` : null;
  const [state, setState] = useState<{ key: string | null; snippet: Snippet | null; error: string | null }>({ key: null, snippet: null, error: null });
  const [nonce, setNonce] = useState(0);
  const sourceRef = useRef(source);
  sourceRef.current = source;

  useEffect(() => {
    const src = sourceRef.current;
    if (!cacheKey || !src || !target) return;
    const hit = snippets.get(cacheKey);
    if (hit) {
      setState({ key: cacheKey, snippet: hit, error: null });
      return;
    }
    const ctrl = new AbortController();
    const run = () => {
      const p = src.type === 'exchange' ? api.exchanges.snippet(src.id, target, ctrl.signal) : api.codegen.generate(src.request, target, ctrl.signal);
      p.then(
        (snippet) => {
          snippets.set(cacheKey, snippet);
          if (!ctrl.signal.aborted) setState({ key: cacheKey, snippet, error: null });
        },
        (e: unknown) => {
          if (!ctrl.signal.aborted) setState((s) => ({ key: cacheKey, snippet: s.snippet, error: errorMessage(e) }));
        },
      );
    };
    // Captured exchanges load immediately; Compose requests wait for typing to settle.
    const timer = src.type === 'request' ? window.setTimeout(run, REQUEST_DEBOUNCE) : (run(), 0);
    return () => {
      window.clearTimeout(timer);
      ctrl.abort();
    };
  }, [cacheKey, target, nonce]);

  const fresh = state.key === cacheKey;
  const cached = cacheKey ? snippets.peek(cacheKey) : undefined;
  return {
    /** The snippet to show: fresh, cached, or the previous one while the next loads. */
    snippet: fresh ? state.snippet : (cached ?? state.snippet),
    loading: cacheKey != null && !fresh && !cached,
    error: fresh ? state.error : null,
    retry: () => {
      if (cacheKey) snippets.delete(cacheKey);
      setNonce((n) => n + 1);
    },
  };
}

// ---------------------------------------------------------------- component

/**
 * Code tab (DESIGN.md §6.4): grouped target picker, the generated snippet in a read-only
 * editor with the target's highlighting, copy, save as file and wrap. The chosen target is
 * remembered and shared with "Copy as".
 */
export function CodeView({ exchangeId, request, className, fill = true }: CodeViewProps) {
  const targetsState = useTargets();
  const [remembered, setRemembered] = usePagePref<CodegenTarget>(CODE_TARGET_PREF, 'curl');
  const [wrap, setWrap] = usePagePref<boolean>(WRAP_PREF, false);
  const targets = targetsState.targets;
  const target = targets ? pickTarget(targets, remembered) : undefined;

  const reqKey = request ? requestKey(request) : null;
  const source = useMemo<Source | null>(() => {
    if (exchangeId != null) return { type: 'exchange', id: exchangeId };
    if (request && reqKey) return { type: 'request', request, key: reqKey };
    return null;
    // `reqKey` captures the request's content; its identity changes on every Compose render.
  }, [exchangeId, reqKey]);
  const { snippet, loading, error, retry } = useSnippet(source, target?.id);

  const editorRef = useRef<CodeEditorHandle | null>(null);
  const [active, setActive] = useState(false);
  const code = snippet?.code ?? '';
  const language = snippet?.language ?? target?.language ?? 'text';

  const save = async () => {
    if (!snippet || !target) return;
    try {
      await saveData(snippet.code, snippetFilename(target.id, snippet.language), 'text/plain');
    } catch (e) {
      toast.error("Couldn't save the snippet", { description: errorMessage(e) });
    }
  };

  useShortcut('alt+z', () => setWrap(!wrap), { enabled: active, allowInInputs: true, description: 'Toggle line wrap', group: 'Code' });
  useShortcut('mod+s', () => void save(), { enabled: active && !!snippet, allowInInputs: true, description: 'Save snippet as file', group: 'Code' });

  const options = useMemo(
    () =>
      targets
        ? groupTargets(targets).map((g) => ({
            label: g.label,
            options: g.targets.map((t) => ({ value: t.id, label: t.name, textValue: t.name })),
          }))
        : [],
    [targets],
  );

  // ---------------------------------------------------------------- states
  let content;
  if (!source) {
    content = (
      <div className={styles.state}>
        <EmptyState size="sm" icon={Code2} title="Nothing to generate" description="Enter a URL to see this request as code." />
      </div>
    );
  } else if (targetsState.error) {
    content = (
      <div className={styles.state}>
        <EmptyState
          size="sm"
          icon={AlertTriangle}
          tone="danger"
          title="Couldn't load code targets"
          description={targetsState.error}
          actions={
            <Button size="sm" onClick={targetsState.retry}>
              Retry
            </Button>
          }
        />
      </div>
    );
  } else if (error && !snippet) {
    content = (
      <div className={styles.state}>
        <EmptyState
          size="sm"
          icon={AlertTriangle}
          tone="danger"
          title={`Couldn't generate ${target?.name ?? 'the'} snippet`}
          description={error}
          actions={
            <Button size="sm" onClick={retry}>
              Retry
            </Button>
          }
        />
      </div>
    );
  } else if (!snippet) {
    content = (
      <div className={styles.loading} role="status" aria-busy aria-label="Generating snippet">
        {[38, 72, 64, 80, 46].map((w, i) => (
          <Skeleton key={i} height={11} width={`${w}%`} />
        ))}
      </div>
    );
  } else {
    content = (
      <CodeEditor
        ref={editorRef}
        value={code}
        language={language}
        readOnly
        wrap={wrap}
        fill={fill}
        maxHeight={fill ? undefined : 480}
        contentId={`${exchangeId ?? reqKey ?? ''}:${snippet.target}`}
        aria-label={`${target?.name ?? 'Code'} snippet`}
        className={clsx(loading && styles.stale)}
      />
    );
  }

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: tracks focus within the viewer to scope its shortcuts; not a control
    <div
      className={clsx(styles.root, fill && styles.fill, className)}
      onFocus={() => setActive(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setActive(false);
      }}
    >
      <div className={styles.toolbar} role="toolbar" aria-label="Code snippet">
        {targets ? (
          <Select<CodegenTarget> size="sm" aria-label="Code target" value={target?.id} onValueChange={setRemembered} options={options} className={styles.target} disabled={!source} />
        ) : targetsState.error ? null : (
          <Skeleton height={24} width={176} radius={6} />
        )}
        {error && snippet && (
          <span className={styles.inlineError} title={error}>
            <AlertTriangle size={12} strokeWidth={2.25} aria-hidden />
            Couldn&apos;t refresh
            <button type="button" className={styles.retry} onClick={retry}>
              Retry
            </button>
          </span>
        )}
        <span className={styles.spacer} />
        {snippet && (
          <span className={clsx(styles.meta, 'tnum')}>
            {code.split('\n').length} lines
          </span>
        )}
        <div className={styles.actions}>
          <IconButton label={wrap ? 'Disable line wrap' : 'Wrap lines'} icon={WrapText} size="sm" shortcut="alt+z" active={wrap} disabled={!snippet} onClick={() => setWrap(!wrap)} />
          <IconButton label={`Save as ${target && snippet ? snippetFilename(target.id, snippet.language) : 'file'}`} icon={Download} size="sm" shortcut="mod+s" disabled={!snippet} onClick={() => void save()} />
          <CopyButton value={code} label={`Copy ${target?.name ?? ''} snippet`.replace(/\s+/g, ' ')} disabled={!snippet || loading} />
        </div>
      </div>
      <div className={styles.content}>{content}</div>
    </div>
  );
}
