/**
 * Structured body views for BodyViewer: JSON tree, decoded binary payloads (protobuf / msgpack /
 * CBOR / gRPC with trailers), multipart parts, server-sent events and form fields.
 */

import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { Binary, ChevronRight, FileText, Inbox, Paperclip, Radio } from 'lucide-react';
import { memo, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import type { BodyView, Headers, MultipartPart, SseEvent } from '../../../api/types';
import { formatBytes, formatNumber, pluralize } from '../../../lib/format';
import { parseJsonLossless } from '../../../lib/json';
import { Badge, Button, CodeEditor, CopyButton, EmptyState, JsonTree, SearchInput, Spinner, Tooltip } from '../../ui';
import { formatJson, formFields, grpcMessages, grpcParseError, grpcStatus, HUGE_JSON, kindFromContentType, multipartParts, parseJsonLines, sseEvents } from '../model/body';
import { Banner, Chip, KindIcon } from '../parts/Parts';
import { NoSchemaNotice, SchemaBar, SchemaNotices, useProtoViewMode } from './Schema';
import styles from './body.module.css';

// =====================================================================================
// JSON tree
// =====================================================================================

type Parsed = { status: 'idle' } | { status: 'confirm' } | { status: 'parsing' } | { status: 'ok'; value: unknown } | { status: 'error'; error: string };

export function TreeView({ text, lines, onShowRaw }: { text: string; lines: boolean; onShowRaw: () => void }) {
  const [parsed, setParsed] = useState<Parsed>({ status: 'idle' });
  const [confirmed, setConfirmed] = useState(false);
  useEffect(() => {
    if (text.length > HUGE_JSON && !confirmed) {
      setParsed({ status: 'confirm' });
      return;
    }
    setParsed({ status: 'parsing' });
    // Yield a frame so the spinner paints before a large synchronous parse.
    const id = window.setTimeout(
      () => {
        try {
          // Lossless: 64-bit ids, long decimals and duplicate keys are shown exactly as sent.
          setParsed({ status: 'ok', value: lines ? parseJsonLines(text) : parseJsonLossless(text) });
        } catch (e) {
          setParsed({ status: 'error', error: e instanceof Error ? e.message : String(e) });
        }
      },
      text.length > 100_000 ? 16 : 0,
    );
    return () => window.clearTimeout(id);
  }, [text, lines, confirmed]);

  if (parsed.status === 'confirm') {
    return (
      <div className={styles.center}>
        <EmptyState
          icon={Binary}
          size="sm"
          title={`Large JSON (${formatBytes(text.length)})`}
          description="Building a tree for a document this size can take a few seconds."
          actions={
            <>
              <Button size="sm" variant="primary" onClick={() => setConfirmed(true)}>
                Build tree
              </Button>
              <Button size="sm" onClick={onShowRaw}>
                Show raw
              </Button>
            </>
          }
        />
      </div>
    );
  }
  if (parsed.status === 'error') {
    return (
      <div className={styles.fill}>
        <Banner
          tone="warning"
          actions={
            <Button size="sm" variant="ghost" onClick={onShowRaw}>
              Show raw
            </Button>
          }
        >
          Not valid JSON: <code>{parsed.error}</code>
        </Banner>
      </div>
    );
  }
  if (parsed.status !== 'ok') {
    return (
      <div className={styles.loadingInline}>
        <Spinner size={16} /> Building tree…
      </div>
    );
  }
  return <JsonTree data={parsed.value} defaultExpandDepth={lines ? 1 : 2} aria-label="JSON tree" />;
}

// =====================================================================================
// Decoded binary payloads
// =====================================================================================

/** Shown above views built from `structured` when the daemon cut the decode short. */
export function StructuredTruncatedBanner({ view }: { view: BodyView }) {
  if (!view.structuredTruncated) return null;
  return (
    <Banner tone="info" className={styles.structuredTruncated}>
      Decoded view truncated — showing the first part.
    </Banner>
  );
}

export function DecodedView({ view, kind, trailers, headers }: { view: BodyView; kind: string; trailers?: Headers; headers?: Headers }) {
  if (kind === 'grpc' || kind === 'grpc-web') return <GrpcView view={view} trailers={trailers} headers={headers} />;
  if (kind === 'protobuf' || view.schema) return <ProtobufView view={view} />;
  if (view.structured == null) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Binary} size="sm" title="Couldn't decode this body" description="It doesn't parse as the declared format. Switch to Hex to inspect the bytes." />
      </div>
    );
  }
  return (
    <div className={styles.fill}>
      <StructuredTruncatedBanner view={view} />
      <JsonTree data={view.structured} aria-label="Decoded payload" />
    </div>
  );
}

/** A protobuf body: typed decode with the imported schema, or the schemaless one. */
function ProtobufView({ view }: { view: BodyView }) {
  const [mode, setMode] = useProtoViewMode();
  const schema = view.schema;
  const typed = schema?.typed;
  const hasTyped = typed !== undefined && typed !== null;
  const hasSchemaless = view.structured != null;
  const showTyped = hasTyped && (mode === 'typed' || !hasSchemaless);
  const data = showTyped ? typed : view.structured;
  return (
    <div className={styles.fill}>
      {schema && <SchemaBar schema={schema} mode={showTyped ? 'typed' : 'schemaless'} onModeChange={setMode} canToggle={hasTyped && hasSchemaless} />}
      <StructuredTruncatedBanner view={view} />
      {schema ? <SchemaNotices schema={schema} showingTyped={showTyped} /> : hasSchemaless && <NoSchemaNotice />}
      {data != null ? (
        <JsonTree data={data} aria-label={showTyped ? `Decoded ${schema?.messageType ?? ''}`.trim() : 'Decoded payload'} />
      ) : (
        <div className={styles.center}>
          <EmptyState icon={Binary} size="sm" title="Couldn't decode this body" description={schema?.error ? `It doesn't decode as ${schema.messageType}. Switch to Hex to inspect the bytes.` : "It doesn't parse as the declared format. Switch to Hex to inspect the bytes."} />
        </div>
      )}
    </div>
  );
}

function StatusStrip({ status }: { status: NonNullable<ReturnType<typeof grpcStatus>> }) {
  return (
    <div className={clsx(styles.grpcStatus, status.ok ? styles.ok : styles.bad)} role="status">
      <span className="caps">grpc-status</span>
      <span className={styles.grpcCode}>
        {status.code} {status.name}
      </span>
      {status.message && <span className={styles.grpcMessage}>— {status.message}</span>}
    </div>
  );
}

function KvTable({ rows, empty, head }: { rows: readonly (readonly [string, string])[]; empty?: string; head?: [string, string] }) {
  if (!rows.length) return <div className={styles.cardNote}>{empty ?? 'None'}</div>;
  const counts = new Map<string, number>();
  for (const [k] of rows) counts.set(k, (counts.get(k) ?? 0) + 1);
  return (
    <table className={styles.kvTable}>
      {head && (
        <thead>
          <tr>
            <th>{head[0]}</th>
            <th>{head[1]}</th>
          </tr>
        </thead>
      )}
      <tbody>
        {rows.map(([k, v], i) => (
          <tr key={i}>
            <td className={styles.kvName}>
              {k || <span className={styles.kvEmpty}>(empty)</span>}
              {(counts.get(k) ?? 0) > 1 && <span className={styles.dupe}>×{counts.get(k)}</span>}
            </td>
            <td className={clsx(styles.kvValue, styles.kvValueCell)}>
              {v === '' ? <span className={styles.kvEmpty}>empty</span> : v}
              <CopyButton value={v} label="Copy value" className={styles.kvCopy} />
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function GrpcView({ view, trailers, headers }: { view: BodyView; trailers?: Headers; headers?: Headers }) {
  const messages = useMemo(() => grpcMessages(view), [view]);
  const [mode, setMode] = useProtoViewMode();
  const schema = view.schema;
  const anyTyped = messages.some((m) => m.typed !== undefined && m.typed !== null);
  const typedMode = anyTyped && mode === 'typed';
  const unknownFields = useMemo(() => [...new Set(messages.flatMap((m) => m.unknownFields ?? []))], [messages]);
  const parseError = grpcParseError(view);
  const frameTrailers = messages.find((m) => m.trailers)?.trailers ?? null;
  const allTrailers = frameTrailers ?? trailers ?? null;
  const status = grpcStatus(frameTrailers, trailers, headers);
  const payloads = messages.filter((m) => m.decoded != null || !m.trailers);
  if (!payloads.length && !status) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Inbox} size="sm" title="No gRPC messages" description="The body contains no length-prefixed messages." />
      </div>
    );
  }
  return (
    <div className={styles.scroll}>
      {schema && <SchemaBar schema={schema} mode={typedMode ? 'typed' : 'schemaless'} onModeChange={setMode} canToggle={anyTyped} />}
      <StructuredTruncatedBanner view={view} />
      {status && <StatusStrip status={status} />}
      {schema ? <SchemaNotices schema={schema} unknownFields={unknownFields} showingTyped={typedMode} /> : payloads.some((m) => m.decoded != null) && <NoSchemaNotice />}
      {parseError && <Banner tone="warning">gRPC framing error: {parseError}. Messages decoded before the error are shown.</Banner>}
      <div className={styles.stack}>
        {payloads.map((m, i) => (
          <section key={i} className={styles.card} aria-label={`Message ${i + 1}`}>
            <header className={styles.cardHead}>
              <span className={styles.cardTitle}>Message {i + 1}</span>
              {payloads.length > 1 && <span className={styles.cardMeta}>of {payloads.length}</span>}
              <span className={styles.cardSpacer} />
              {m.compressed && (
                <Badge size="sm" variant="info">
                  compressed
                </Badge>
              )}
              {m.typedError ? (
                <Tooltip content={m.typedError} multiline>
                  <Badge size="sm" variant="warning" tabIndex={0}>
                    schema mismatch
                  </Badge>
                </Tooltip>
              ) : typedMode && m.typed != null ? (
                <Badge size="sm" variant="accent">
                  typed
                </Badge>
              ) : (
                <Badge size="sm">schemaless</Badge>
              )}
            </header>
            <div className={styles.cardBody}>
              {typedMode && m.typed != null ? (
                <JsonTree data={m.typed} fill={false} maxHeight={360} toolbar={false} aria-label={`Message ${i + 1}`} />
              ) : m.decoded != null ? (
                <JsonTree data={m.decoded} fill={false} maxHeight={360} toolbar={false} aria-label={`Message ${i + 1}`} />
              ) : (
                <div className={styles.cardNote}>This message doesn't decode as protobuf without a schema.</div>
              )}
            </div>
          </section>
        ))}
        {allTrailers && allTrailers.length > 0 && (
          <section className={styles.card} aria-label="Trailers">
            <header className={styles.cardHead}>
              <span className={styles.cardTitle}>Trailers</span>
              <span className={styles.cardMeta}>{pluralize(allTrailers.length, 'field')}</span>
            </header>
            <KvTable rows={allTrailers} />
          </section>
        )}
      </div>
    </div>
  );
}

// =====================================================================================
// Multipart
// =====================================================================================

const PartCard = memo(function PartCard({ part, index, defaultOpen }: { part: MultipartPart; index: number; defaultOpen: boolean }) {
  const [open, setOpen] = useState(defaultOpen);
  const kind = kindFromContentType(part.contentType ?? (part.filename ? '' : 'text/plain'));
  const text = useMemo(() => {
    if (part.text == null) return null;
    if (kind === 'json') {
      const f = formatJson(part.text);
      return f.ok ? f.text : part.text;
    }
    return part.text;
  }, [part.text, kind]);
  return (
    <section className={styles.card} aria-label={`Part ${index + 1}`}>
      <button type="button" className={clsx(styles.cardHead, styles.button)} onClick={() => setOpen((o) => !o)} aria-expanded={open}>
        <ChevronRight size={14} className={clsx(styles.chev, open && styles.chevOpen)} aria-hidden />
        <KindIcon kind={part.filename && kind === 'unknown' ? 'binary' : kind} />
        <span className={clsx(styles.cardTitle, 'mono')}>{part.name ?? `part ${index + 1}`}</span>
        {part.filename && (
          <Chip title="Filename">
            <Paperclip size={10} aria-hidden />
            {part.filename}
          </Chip>
        )}
        <span className={styles.cardSpacer} />
        {part.contentType && <span className={styles.cardMeta}>{part.contentType}</span>}
        <span className={clsx(styles.cardMeta, 'tnum')}>{formatBytes(part.size)}</span>
      </button>
      {open && (
        <div className={styles.cardBody}>
          <KvTable rows={part.headers} empty="No part headers" />
          {text != null ? (
            <CodeEditor value={text} language={kind === 'unknown' ? 'text' : kind} readOnly maxHeight={320} lineNumbers={text.split('\n', 3).length > 2} aria-label={`Content of part ${part.name ?? index + 1}`} />
          ) : (
            <div className={styles.cardNote}>
              <FileText size={12} aria-hidden style={{ display: 'inline', verticalAlign: '-1px', marginRight: 6 }} />
              {part.size === 0 ? 'Empty part.' : `Binary content (${formatBytes(part.size)}) isn't included in the decoded view — download the body to extract it.`}
            </div>
          )}
        </div>
      )}
    </section>
  );
});

export function PartsView({ view }: { view: BodyView }) {
  const parts = useMemo(() => multipartParts(view), [view]);
  if (!parts.length) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Inbox} size="sm" title="No parts" description="The multipart body couldn't be split into parts (missing or mismatched boundary)." />
      </div>
    );
  }
  return (
    <div className={styles.scroll}>
      <StructuredTruncatedBanner view={view} />
      <div className={styles.stack}>
        {parts.map((p, i) => (
          <PartCard key={i} part={p} index={i} defaultOpen={parts.length <= 6} />
        ))}
      </div>
    </div>
  );
}

// =====================================================================================
// Server-sent events
// =====================================================================================

const EVENT_ROW_H = 28;

function prettyData(data: string): { text: string; json: boolean } {
  const t = data.trim();
  if ((t.startsWith('{') || t.startsWith('[')) && t.length < 2_000_000) {
    const f = formatJson(t);
    if (f.ok) return { text: f.text, json: true };
  }
  return { text: data, json: false };
}

export function EventsView({ view, contentId }: { view: BodyView; contentId: string }) {
  const events = useMemo(() => sseEvents(view), [view]);
  const [query, setQuery] = useState('');
  const [selected, setSelected] = useState(0);
  const listRef = useRef<HTMLDivElement | null>(null);
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    const all = events.map((e, i) => ({ e, i }));
    if (!q) return all;
    return all.filter(({ e }) => e.data.toLowerCase().includes(q) || (e.event ?? 'message').toLowerCase().includes(q) || (e.id ?? '').toLowerCase().includes(q));
  }, [events, query]);
  const virtualizer = useVirtualizer({ count: filtered.length, getScrollElement: () => listRef.current, estimateSize: () => EVENT_ROW_H, overscan: 12 });

  useEffect(() => setSelected(0), [contentId, query]);

  const current = filtered[Math.min(selected, filtered.length - 1)];
  const detail = useMemo(() => (current ? prettyData(current.e.data) : null), [current]);

  const onKeyDown = (e: KeyboardEvent) => {
    let next = selected;
    if (e.key === 'ArrowDown' || e.key === 'j') next = Math.min(filtered.length - 1, selected + 1);
    else if (e.key === 'ArrowUp' || e.key === 'k') next = Math.max(0, selected - 1);
    else if (e.key === 'Home') next = 0;
    else if (e.key === 'End') next = filtered.length - 1;
    else if (e.key === 'PageDown') next = Math.min(filtered.length - 1, selected + 10);
    else if (e.key === 'PageUp') next = Math.max(0, selected - 10);
    else return;
    e.preventDefault();
    setSelected(next);
    virtualizer.scrollToIndex(next, { align: 'auto' });
  };

  if (!events.length) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Radio} size="sm" title="No events yet" description="The stream hasn't delivered any complete events." />
      </div>
    );
  }

  return (
    <div className={styles.events}>
      <StructuredTruncatedBanner view={view} />
      <div className={styles.eventsBar}>
        <SearchInput size="sm" value={query} onValueChange={setQuery} placeholder="Filter events" className={styles.eventsSearch} aria-label="Filter events" />
        <span className={clsx(styles.eventsCount, 'tnum')}>{query ? `${formatNumber(filtered.length)} of ${pluralize(events.length, 'event')}` : pluralize(events.length, 'event')}</span>
      </div>
      <div ref={listRef} className={styles.eventsList} tabIndex={0} role="listbox" aria-label="Events" aria-activedescendant={current ? `${contentId}-ev-${current.i}` : undefined} onKeyDown={onKeyDown}>
        <div style={{ height: virtualizer.getTotalSize(), position: 'relative' }}>
          {virtualizer.getVirtualItems().map((it) => {
            const row = filtered[it.index];
            if (!row) return null;
            const ev: SseEvent = row.e;
            return (
              // biome-ignore lint/a11y/useFocusableInteractive: the listbox keeps focus and points at the active event with aria-activedescendant
              <div
                key={it.key}
                id={`${contentId}-ev-${row.i}`}
                role="option"
                aria-selected={it.index === selected}
                className={clsx(styles.eventRow, it.index === selected && styles.selected)}
                style={{ top: it.start }}
                onMouseDown={() => setSelected(it.index)}
              >
                <span className={clsx(styles.eventNum, 'tnum')}>{row.i + 1}</span>
                <span className={styles.eventName}>
                  <Badge size="sm" variant={ev.event && ev.event !== 'message' ? 'accent' : 'neutral'} mono>
                    {ev.event ?? 'message'}
                  </Badge>
                </span>
                <span className={styles.eventId} title={ev.id ?? undefined}>
                  {ev.id ?? ''}
                </span>
                <span className={styles.eventData}>{ev.data.replace(/\s+/g, ' ')}</span>
              </div>
            );
          })}
        </div>
      </div>
      {current && detail && (
        <div className={styles.eventDetail}>
          <div className={styles.eventDetailHead}>
            <strong>Event {current.i + 1}</strong>
            <span>{current.e.event ?? 'message'}</span>
            {current.e.id != null && <span className="mono">id {current.e.id}</span>}
            {current.e.retry != null && <span className="tnum">retry {current.e.retry} ms</span>}
            <span className={styles.cardSpacer} />
            <CopyButton value={current.e.data} label="Copy event data" />
          </div>
          <CodeEditor value={detail.text} language={detail.json ? 'json' : 'text'} readOnly fill wrap contentId={`${contentId}:${current.i}`} aria-label="Event data" />
        </div>
      )}
    </div>
  );
}

// =====================================================================================
// Form fields
// =====================================================================================

export function FormView({ view }: { view: BodyView }) {
  const fields = useMemo(() => formFields(view), [view]);
  if (!fields.length) {
    return (
      <div className={styles.center}>
        <EmptyState icon={Inbox} size="sm" title="No form fields" />
      </div>
    );
  }
  return (
    <div className={styles.scroll}>
      <StructuredTruncatedBanner view={view} />
      <KvTable rows={fields} head={[`Field · ${formatNumber(fields.length)}`, 'Value']} />
    </div>
  );
}
