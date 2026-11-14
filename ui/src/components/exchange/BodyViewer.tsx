import clsx from 'clsx';
import { AlertTriangle, ChevronDown, Download, FileX2, Info, Inbox, Search, WrapText } from 'lucide-react';
import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { api } from '../../api/client';
import type { BodyView, ExchangeId, Headers, Side } from '../../api/types';
import { navigate } from '../../app/router';
import { useShortcut } from '../../app/shortcuts';
import { formatBytes } from '../../lib/format';
import { contentKindLabel, getHeader } from '../../lib/http';
import { stringifyJson } from '../../lib/json';
import { downloadUrl, saveData } from '../../lib/platform';
import { usePagePref } from '../../state/ui';
import { Button, CodeEditor, CopyButton, EmptyState, IconButton, Menu, SegmentedControl, Select, Skeleton, Spinner, Tooltip, toast, type CodeEditorHandle } from '../ui';
import { useBodyView } from './body/data';
import { HexView } from './body/HexView';
import { prettyAsync } from './body/pretty';
import { AudioPreview, CsvPreview, FontPreview, HtmlPreview, ImagePreview, MarkdownPreview, PdfPreview, VideoPreview } from './body/Previews';
import { DecodedView, EventsView, FormView, PartsView, TreeView } from './body/Structured';
import {
  analyzeBody,
  bodyInfo,
  downloadFilename,
  essence,
  formatJson,
  formatJsonLines,
  imageInfo,
  LARGE_TEXT,
  MODE_LABELS,
  MODE_TOOLTIPS,
  resolveMode,
  type BodyMode,
  type BodyModel,
  type ConcreteMode,
} from './model/body';
import { Banner, KindIcon } from './parts/Parts';
import styles from './BodyViewer.module.css';

export interface BodyViewerProps {
  /** Fetch the body of this exchange (`GET /api/exchanges/:id/body/:side`)… */
  exchangeId?: ExchangeId;
  side?: Side;
  /** …the pre-modification body (`?original=1`). */
  original?: boolean;
  /** …or render an already-fetched view (Compose responses, breakpoints). */
  view?: BodyView | null;
  /** URL of the raw bytes, for previews (images/audio/video/PDF) and download. Defaults to the
   *  exchange's raw endpoint when `exchangeId`+`side` are given. */
  rawUrl?: string;
  /** Content-Type header value (helps pick the default mode when `view` is given). */
  contentType?: string | null;
  /** URL path (for download filenames / extension hints). */
  path?: string;
  /** Extra controls rendered at the end of the body toolbar. */
  toolbarExtra?: ReactNode;
  /** Bump to refetch (e.g. when the exchange finished streaming). */
  refreshKey?: string | number;
  /** Fill the parent's height (default true). */
  fill?: boolean;
  className?: string;
  /** Optional: the message headers (Content-Disposition filename, gRPC trailers-only status). */
  headers?: Headers;
  /** Optional: HTTP trailers (`detail.responseTrailers`) — gRPC status is read from them. */
  trailers?: Headers;
}

const MODES_PREF = 'exchange.body.modes';
const WRAP_PREF = 'exchange.body.wrap';
const PRETTY_SYNC_LIMIT = 512 * 1024;

const ENCODING_EXT: Readonly<Record<string, string>> = { gzip: 'gz', 'x-gzip': 'gz', br: 'br', zstd: 'zst', deflate: 'zz' };

type Pretty = { text: string | null; error: string | null; pending: boolean };

/**
 * Pretty text for the current view: daemon-formatted, else formatted here — synchronously for
 * small JSON, in a Web Worker above {@link PRETTY_SYNC_LIMIT} so big bodies never block typing
 * or scrolling.
 */
function usePretty(view: BodyView | null, model: BodyModel | null, active: boolean): Pretty {
  const text = view?.text ?? null;
  const formatted = view?.formatted ?? null;
  const kind = model?.kind;
  const needsClient = active && formatted == null && text != null && (kind === 'json' || kind === 'graph-ql' || kind === 'json-lines');
  const sync = needsClient && (text?.length ?? 0) <= PRETTY_SYNC_LIMIT;
  const syncResult = useMemo<Pretty | null>(() => {
    if (!sync || text == null) return null;
    if (kind === 'json-lines') return { text: formatJsonLines(text), error: null, pending: false };
    const f = formatJson(text);
    return f.ok ? { text: f.text, error: null, pending: false } : { text, error: f.error, pending: false };
  }, [sync, text, kind]);
  const [deferred, setDeferred] = useState<{ src: string; result: Pretty } | null>(null);
  useEffect(() => {
    if (!needsClient || sync || text == null) return;
    if (deferred?.src === text) return;
    let cancelled = false;
    void prettyAsync(text, kind === 'json-lines').then((r) => {
      if (cancelled) return;
      setDeferred({ src: text, result: r.ok ? { text: r.text, error: null, pending: false } : { text, error: r.error, pending: false } });
    });
    return () => {
      cancelled = true;
    };
  }, [needsClient, sync, text, kind, deferred?.src]);

  if (!active) return { text: null, error: null, pending: false };
  if (formatted != null) return { text: formatted, error: null, pending: false };
  if (syncResult) return syncResult;
  if (needsClient) return deferred?.src === text ? deferred.result : { text: null, error: null, pending: true };
  return { text, error: null, pending: false };
}

/** Text the copy button copies for a mode (null = nothing sensible to copy). */
function copyTextFor(mode: ConcreteMode, view: BodyView, model: BodyModel, pretty: string | null): string | null {
  switch (mode) {
    case 'pretty':
      return pretty ?? view.text;
    case 'raw':
    case 'form':
    case 'events':
    case 'parts':
      return view.text;
    case 'tree':
      return view.formatted ?? view.text;
    case 'decoded':
      return view.structured != null ? (stringifyJson(view.structured, 2) ?? null) : null;
    case 'preview':
      return model.preview === 'html' || model.preview === 'markdown' || model.preview === 'csv' || model.preview === 'svg' ? view.text : null;
    default:
      return null;
  }
}

/**
 * The body viewer (DESIGN.md §6.4): contextual view modes (remembered per content kind), wrap,
 * search, copy, download, encoding info, truncation/decode-failure notices, and previews for
 * HTML (sandboxed), Markdown (safe renderer), images, SVG, PDF, audio, video and fonts.
 */
export function BodyViewer(props: BodyViewerProps) {
  const { exchangeId, side, original = false, view: given, rawUrl: rawUrlProp, contentType, path, toolbarExtra, refreshKey, fill = true, className, headers, trailers } = props;
  const body = useBodyView(exchangeId, side, original, refreshKey, given);
  const view = body.view;
  const fromExchange = exchangeId != null && side != null && !original;
  const canFetchBytes = fromExchange || !!rawUrlProp;
  const ct = contentType ?? (headers ? getHeader(headers, 'content-type') : undefined) ?? null;
  const model = useMemo(() => (view ? analyzeBody(view, ct, canFetchBytes) : null), [view, ct, canFetchBytes]);

  const [storedModes, setStoredModes] = usePagePref<Record<string, BodyMode>>(MODES_PREF, {});
  const [wrap, setWrap] = usePagePref<boolean>(WRAP_PREF, false);
  const contentKey = `${exchangeId ?? 'view'}:${side ?? ''}:${original ? 1 : 0}`;
  const [override, setOverride] = useState<{ key: string; mode: ConcreteMode } | null>(null);
  const [active, setActive] = useState(false);
  const editorRef = useRef<CodeEditorHandle | null>(null);

  const modes = storedModes && typeof storedModes === 'object' ? storedModes : {};
  const stored = model ? modes[model.prefKey] : undefined;
  const sessionMode = model && override?.key === contentKey && model.modes.includes(override.mode) ? override.mode : null;
  const mode: ConcreteMode | null = model ? (sessionMode ?? resolveMode(model, stored)) : null;
  const selected: BodyMode = sessionMode ?? (model && stored && stored !== 'auto' && model.modes.includes(stored) ? stored : 'auto');

  const pretty = usePretty(view, model, mode === 'pretty');
  const textMode = mode === 'pretty' || mode === 'raw';
  const displayedText = mode === 'pretty' ? pretty.text : mode === 'raw' ? (view?.text ?? '') : null;
  const isEmpty = !body.loading && !body.error && (view == null || (view.size === 0 && !view.text && view.decodeError == null));

  const choose = (m: BodyMode) => {
    if (!model) return;
    setOverride(null);
    const next = { ...modes };
    if (m === 'auto') delete next[model.prefKey];
    else next[model.prefKey] = m;
    setStoredModes(next);
  };

  const filename = model ? downloadFilename({ path, contentType: ct, kind: model.kind, side, headers }) : 'body';
  const encodings = (view?.encodings ?? []).filter((e) => e && e.toLowerCase() !== 'identity');

  const download = async (asReceived = false) => {
    if (!view) return;
    try {
      if (fromExchange && exchangeId != null && side) {
        const last = encodings[encodings.length - 1]?.toLowerCase() ?? '';
        const name = asReceived && encodings.length ? `${filename}.${ENCODING_EXT[last] ?? 'bin'}` : filename;
        await downloadUrl(api.exchanges.rawUrl(exchangeId, side, { decoded: !asReceived, download: true }), name);
      } else if (rawUrlProp) await downloadUrl(rawUrlProp, filename);
      else if (view.text != null) await saveData(view.text, filename, essence(ct) || 'text/plain');
      else toast.error('Nothing to download', { description: 'This body has no reachable bytes.' });
    } catch (e) {
      toast.error('Download failed', { description: e instanceof Error ? e.message : undefined });
    }
  };
  const canDownload = !!view && !isEmpty && (fromExchange || !!rawUrlProp || view.text != null);
  const copyValue = view && model && mode ? copyTextFor(mode, view, model, pretty.text) : null;

  useShortcut('alt+z', () => setWrap(!wrap), { enabled: active && textMode, allowInInputs: true, description: 'Toggle line wrap', group: 'Body viewer' });
  useShortcut('mod+f', () => editorRef.current?.openSearch(), { enabled: active && textMode, allowInInputs: true, description: 'Search in body', group: 'Body viewer' });
  useShortcut('mod+s', () => void download(false), { enabled: active && canDownload, allowInInputs: true, description: 'Download body', group: 'Body viewer' });

  // ---------------------------------------------------------------- toolbar
  const info = view ? bodyInfo(view) : null;
  const infoTitle = view && model && info ? `${contentKindLabel(model.kind).toUpperCase()} · ${info.title}${view.captureTruncated ? ' · capture truncated' : ''}${view.truncated ? ' · truncated for transport' : ''}` : undefined;
  const modeItems = model
    ? (['auto', ...model.modes] as BodyMode[]).map((m) => ({
        value: m,
        label: MODE_LABELS[m],
        // Not `tooltip`: the primitive's item tooltips clobber the selected state (data-state).
        description: m === 'auto' ? `${MODE_TOOLTIPS.auto} (${MODE_LABELS[model.auto]})` : MODE_TOOLTIPS[m],
        'aria-label': m === 'auto' ? `Auto (${MODE_LABELS[model.auto]})` : MODE_LABELS[m],
      }))
    : [];

  const toolbar = (
    <div className={styles.toolbar} role="toolbar" aria-label={`${side === 'request' ? 'Request' : side === 'response' ? 'Response' : ''} body view`.trim()}>
      {body.loading && !view ? (
        <Skeleton height={20} radius={6} className={styles.skeletonModes} />
      ) : model && model.modes.length > 0 && !isEmpty ? (
        <>
          <SegmentedControl size="sm" aria-label="View mode" value={selected} onValueChange={choose} items={modeItems.map(({ description: _d, ...item }) => item)} className={styles.modes} />
          <Select
            size="sm"
            aria-label="View mode"
            value={selected}
            onValueChange={choose}
            options={modeItems.map((m) => ({ value: m.value, label: m.value === 'auto' ? `Auto · ${MODE_LABELS[model.auto]}` : m.label, description: m.description }))}
            className={styles.modeSelect}
          />
          <div className={styles.actions}>
            <IconButton label={wrap ? 'Disable line wrap' : 'Wrap lines'} icon={WrapText} size="sm" shortcut="alt+z" active={wrap} disabled={!textMode} onClick={() => setWrap(!wrap)} />
            <IconButton label="Search in body" icon={Search} size="sm" shortcut="mod+f" disabled={!textMode} onClick={() => editorRef.current?.openSearch()} />
            <CopyButton value={() => copyValue ?? ''} label={copyValue == null ? 'Nothing to copy as text' : 'Copy body'} disabled={copyValue == null || copyValue === ''} />
            {fromExchange && encodings.length > 0 ? (
              <Menu
                trigger={<IconButton label="Download body" icon={Download} size="sm" shortcut="mod+s" disabled={!canDownload} />}
                items={[
                  { label: 'Download decoded', icon: Download, hint: filename, onSelect: () => void download(false) },
                  { label: `Download as received (${encodings.join(' + ')})`, icon: ChevronDown, onSelect: () => void download(true) },
                ]}
              />
            ) : (
              <IconButton label={`Download ${filename}`} icon={Download} size="sm" shortcut="mod+s" disabled={!canDownload} onClick={() => void download(false)} />
            )}
          </div>
        </>
      ) : null}
      <span className={styles.spacer} />
      {view && model && !isEmpty && info && (
        <>
          <Tooltip content={infoTitle} multiline>
            <span className={clsx(styles.info, 'tnum')}>
              <span className={styles.infoKind}>
                <KindIcon kind={model.kind} size={12} />
                {contentKindLabel(model.kind)}
              </span>
              <span className={styles.infoDot} aria-hidden />
              {info.text}
              {(view.captureTruncated || view.truncated) && (
                <span className={styles.truncPill}>
                  <AlertTriangle size={11} strokeWidth={2.25} aria-hidden />
                  truncated
                </span>
              )}
            </span>
          </Tooltip>
          <IconButton label={infoTitle ?? info.text} icon={Info} size="sm" className={styles.infoIcon} />
        </>
      )}
      {isEmpty && <span className={styles.info}>No body</span>}
      {toolbarExtra && <div className={styles.extra}>{toolbarExtra}</div>}
    </div>
  );

  // ---------------------------------------------------------------- banners
  const banners: ReactNode[] = [];
  if (view && model) {
    if (view.decodeError) {
      banners.push(
        <Banner key="decode" tone="warning">
          Couldn&apos;t decode the {encodings.length ? <code>{encodings.join(' + ')}</code> : null} body: <code>{view.decodeError}</code>
          {model.modes.includes('hex') ? ' — showing the bytes as received.' : '.'}
        </Banner>,
      );
    }
    if (view.captureTruncated) {
      banners.push(
        <Banner
          key="capture"
          tone="warning"
          actions={
            <Button size="sm" variant="ghost" onClick={() => navigate('/settings/capture')}>
              Capture settings
            </Button>
          }
        >
          Only the first {formatBytes(view.size)} of this body were captured — it hit the capture size limit, so it&apos;s incomplete.
        </Banner>,
      );
    }
    if (view.truncated) {
      banners.push(
        <Banner
          key="transport"
          tone="info"
          actions={
            canDownload ? (
              <Button size="sm" variant="ghost" icon={Download} onClick={() => void download(false)}>
                Download full body
              </Button>
            ) : undefined
          }
        >
          Showing the first {formatBytes(view.text?.length ?? 0)} of {formatBytes(view.size)} — the rest is too large to display here.
        </Banner>,
      );
    }
    if (model.large && mode === 'raw' && selected === 'auto' && model.modes.includes('pretty')) {
      banners.push(
        <Banner
          key="large"
          tone="neutral"
          actions={
            <Button size="sm" variant="ghost" onClick={() => setOverride({ key: contentKey, mode: 'pretty' })}>
              Format &amp; highlight
            </Button>
          }
        >
          Large body ({formatBytes(view.text?.length ?? view.size)}) — shown unformatted for speed.
        </Banner>,
      );
    }
    if (mode === 'pretty' && pretty.error) {
      banners.push(
        <Banner key="invalid" tone="warning">
          Not valid JSON — showing the text as received. <code>{pretty.error}</code>
        </Banner>,
      );
    }
  }

  // ---------------------------------------------------------------- content
  let content: ReactNode;
  let sized = !fill;
  if (body.error) {
    content = (
      <div className={styles.state}>
        <EmptyState
          icon={body.notFound ? FileX2 : AlertTriangle}
          size="sm"
          tone={body.notFound ? 'neutral' : 'danger'}
          title={body.notFound ? 'This exchange no longer exists' : "Couldn't load the body"}
          description={body.notFound ? 'It was deleted or evicted from memory.' : body.error}
          actions={
            body.notFound ? undefined : (
              <Button size="sm" onClick={body.retry}>
                Retry
              </Button>
            )
          }
        />
      </div>
    );
  } else if (body.loading && !view) {
    content = (
      <div className={styles.loading} role="status" aria-busy aria-label="Loading body">
        {[86, 64, 92, 48, 76, 58].map((w, i) => (
          <Skeleton key={i} height={11} width={`${w}%`} />
        ))}
      </div>
    );
  } else if (isEmpty || !view || !model || !mode) {
    content = (
      <div className={styles.state}>
        <EmptyState icon={Inbox} size="sm" title="No body" description={side === 'request' ? 'This request was sent without a body.' : side === 'response' ? 'This response has no body.' : undefined} />
      </div>
    );
  } else if (model.modes.length === 0) {
    content = (
      <div className={styles.state}>
        <EmptyState
          icon={FileX2}
          size="sm"
          title="Nothing to display"
          description={model.decodeFailed ? "The body couldn't be decoded and its raw bytes aren't available here." : "This body has no text and its raw bytes aren't available here."}
        />
      </div>
    );
  } else if (textMode) {
    sized = false;
    if (mode === 'pretty' && pretty.pending) {
      content = (
        <div className={styles.formatting}>
          <Spinner size={16} /> Formatting {formatBytes(view.text?.length ?? 0)}…
        </div>
      );
    } else {
      const large = (displayedText?.length ?? 0) > LARGE_TEXT;
      const highlight = !(mode === 'raw' && (model.large || large));
      content = (
        <CodeEditor
          ref={editorRef}
          value={displayedText ?? ''}
          language={highlight ? model.language : 'text'}
          readOnly
          wrap={wrap}
          fill={fill}
          maxHeight={fill ? undefined : 480}
          folding={highlight ? undefined : false}
          contentId={`${contentKey}:${mode}`}
          aria-label={`${side ?? ''} body`.trim()}
        />
      );
    }
  } else {
    const previewUrl = rawUrlProp ?? (exchangeId != null && side ? api.exchanges.rawUrl(exchangeId, side, { decoded: true }) : undefined);
    switch (mode) {
      case 'tree':
        content = <TreeView text={view.formatted ?? view.text ?? ''} lines={model.kind === 'json-lines'} onShowRaw={() => choose('raw')} />;
        break;
      case 'decoded':
        content = <DecodedView view={view} kind={model.kind} trailers={trailers} headers={headers} />;
        break;
      case 'parts':
        content = <PartsView view={view} />;
        break;
      case 'events':
        content = <EventsView view={view} contentId={contentKey} />;
        break;
      case 'form':
        content = <FormView view={view} />;
        break;
      case 'hex':
        content = (
          <HexView
            exchangeId={exchangeId}
            side={side}
            original={original}
            rawUrl={rawUrlProp}
            text={view.text}
            encodings={encodings}
            size={view.size}
            wireSize={view.wireSize}
            decodeFailed={model.decodeFailed}
            refreshKey={refreshKey}
          />
        );
        break;
      case 'preview':
        switch (model.preview) {
          case 'html':
            content = <HtmlPreview html={view.text ?? ''} />;
            break;
          case 'markdown':
            content = <MarkdownPreview text={view.text ?? ''} />;
            break;
          case 'csv':
            content = <CsvPreview text={view.text ?? ''} />;
            break;
          case 'image':
          case 'svg': {
            const url = previewUrl ?? (view.text != null && model.preview === 'svg' ? `data:image/svg+xml;charset=utf-8,${encodeURIComponent(view.text)}` : '');
            content = <ImagePreview url={url} info={imageInfo(view)} size={view.size} contentType={essence(ct) || null} onShowHex={model.modes.includes('hex') ? () => choose('hex') : undefined} />;
            break;
          }
          case 'audio':
            content = <AudioPreview url={previewUrl ?? ''} title={filename} size={view.size} contentType={essence(ct) || null} />;
            break;
          case 'video':
            content = <VideoPreview url={previewUrl ?? ''} size={view.size} contentType={essence(ct) || null} />;
            break;
          case 'pdf':
            content = <PdfPreview url={previewUrl ?? ''} />;
            break;
          case 'font':
            content = <FontPreview url={previewUrl ?? ''} name={filename} />;
            break;
          default:
            content = null;
        }
        break;
      default:
        content = null;
    }
  }

  // An empty body needs no toolbar (unless the host put controls in it).
  const showToolbar = !isEmpty || toolbarExtra != null;

  return (
    // biome-ignore lint/a11y/noStaticElementInteractions: tracks focus within the viewer to scope its shortcuts; not a control
    <div
      className={clsx(styles.root, fill && styles.fill, className)}
      onFocus={() => setActive(true)}
      onBlur={(e) => {
        if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setActive(false);
      }}
      data-body-mode={mode ?? undefined}
    >
      {showToolbar && toolbar}
      {banners}
      <div className={clsx(styles.content, sized && styles.sized)}>{content}</div>
    </div>
  );
}
