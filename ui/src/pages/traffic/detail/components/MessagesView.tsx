import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { ArrowDown, ArrowDownToLine, ArrowUp, ChevronRight, Clock, MessagesSquare, SearchX, Unplug } from 'lucide-react';
import { memo, useCallback, useEffect, useMemo, useRef, useState, type KeyboardEvent } from 'react';
import { api } from '../../../../api/client';
import type { ExchangeId, ExchangeKind, StreamMessageWithPayload } from '../../../../api/types';
import { Button, CodeEditor, CopyButton, EmptyState, IconButton, SearchInput, SegmentedControl, Skeleton } from '../../../../components/ui';
import { formatBytes, formatNumber, formatOffset, formatTime, pluralize } from '../../../../lib/format';
import { trafficStore } from '../../../../state/traffic';
import { usePagePref } from '../../../../state/ui';
import { injectedSeqs, useInjectedHistory } from '../../../../state/wsTools';
import {
  closeCodeMeaning,
  countDirections,
  filterMessages,
  formatPayload,
  mergeMessages,
  messagePreview,
  offsetMs,
  parseCloseFrame,
  type DirectionFilter,
} from '../lib/messages';
import { MessageFeed } from '../lib/messageFeed';
import { MessageComposer } from './MessageComposer';
import { PausedMessages } from './PausedMessages';
import styles from './MessagesView.module.css';

const ROW_H = 28;

export interface MessagesViewProps {
  exchangeId: ExchangeId;
  kind: ExchangeKind;
  /** Exchange start (µs) for relative times. */
  startMicros: number;
  /** The stream is still open (messages may arrive). */
  live: boolean;
  /** The summary's message count: fetches the rest when it exceeds what was received. */
  messageCount?: number;
  /** Answered by Akev itself (ws-echo): messages can only be injected towards the client. */
  noServer?: boolean;
}

/** Why a message is flagged: sent from this UI's composer, or otherwise written by Akev. */
type Origin = 'injected' | 'modified' | null;

/**
 * Messages of a WebSocket / SSE exchange (DESIGN.md §6.4 Messages tab): virtualised rows with
 * direction, relative/absolute time, type, size and preview; click to expand an inline viewer;
 * text + direction filters; follow mode for live streams.
 */
export const MessagesView = memo(function MessagesView({ exchangeId, kind, startMicros, live, messageCount, noServer }: MessagesViewProps) {
  const { messages, loading, error, retry, canLoadOlder, loadOlder } = useStreamMessages(exchangeId, messageCount);
  const injectedHistory = useInjectedHistory(exchangeId);
  const injected = useMemo(() => injectedSeqs(injectedHistory), [injectedHistory]);
  const [query, setQuery] = useState('');
  const [direction, setDirection] = useState<DirectionFilter>('all');
  const [timeMode, setTimeMode] = usePagePref<'relative' | 'absolute'>('traffic.messages.time', 'relative');
  const [follow, setFollow] = useState(true);
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(() => new Set());
  const [unseen, setUnseen] = useState(0);
  const sse = kind === 'event-stream';
  const noun = sse ? 'event' : 'message';

  // Reset per exchange.
  useEffect(() => {
    setQuery('');
    setDirection('all');
    setExpanded(new Set());
    setFollow(true);
    setUnseen(0);
  }, [exchangeId]);

  const filtered = useMemo(() => filterMessages(messages, query, direction), [messages, query, direction]);
  const counts = useMemo(() => countDirections(messages), [messages]);

  const scrollRef = useRef<HTMLDivElement | null>(null);
  const programmatic = useRef(false);
  const virtualizer = useVirtualizer({
    count: filtered.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (expanded.has((filtered[i] as StreamMessageWithPayload).seq) ? 220 : ROW_H),
    getItemKey: (i) => (filtered[i] as StreamMessageWithPayload).seq,
    overscan: 12,
  });

  const scrollToEnd = useCallback(() => {
    if (filtered.length === 0) return;
    programmatic.current = true;
    virtualizer.scrollToIndex(filtered.length - 1, { align: 'end' });
    requestAnimationFrame(() => {
      programmatic.current = false;
    });
    setUnseen(0);
  }, [filtered.length, virtualizer]);

  // Follow: stick to the newest message as they arrive. Counted from the end, so older pages
  // loaded in front are not "new".
  const prevLast = useRef<number | null>(null);
  const lastSeq = filtered.length ? (filtered[filtered.length - 1] as StreamMessageWithPayload).seq : null;
  // biome-ignore lint/correctness/useExhaustiveDependencies: runs when the newest message changes; `filtered` is read for that change
  useEffect(() => {
    const before = prevLast.current;
    prevLast.current = lastSeq;
    if (lastSeq == null || (before != null && lastSeq <= before)) return;
    let added = 0;
    for (let i = filtered.length - 1; i >= 0 && (before == null || (filtered[i] as StreamMessageWithPayload).seq > before); i--) added++;
    if (follow) scrollToEnd();
    else setUnseen((n) => n + added);
  }, [lastSeq, follow, scrollToEnd]);

  // Panels below the list (paused messages, composer) resize it: stay on the newest message.
  const followRef = useRef(follow);
  followRef.current = follow;
  const hasList = filtered.length > 0;
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    let height = el.clientHeight;
    const ro = new ResizeObserver(() => {
      if (el.clientHeight === height) return;
      height = el.clientHeight;
      if (!followRef.current) return;
      // Rows are already measured: jump straight to the end (no animated scrollToIndex).
      programmatic.current = true;
      el.scrollTop = el.scrollHeight;
      requestAnimationFrame(() => {
        programmatic.current = false;
      });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, [hasList]);

  const onScroll = () => {
    const el = scrollRef.current;
    if (!el || programmatic.current) return;
    const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
    if (atBottom) {
      if (!follow && live) setFollow(true);
      setUnseen(0);
    } else if (follow) {
      setFollow(false);
    }
  };

  const toggle = useCallback((seq: number) => {
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(seq)) next.delete(seq);
      else next.add(seq);
      return next;
    });
  }, []);

  // ↑/↓ between rows, Enter/Space toggles (rows are buttons).
  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== 'ArrowDown' && e.key !== 'ArrowUp' && e.key !== 'Home' && e.key !== 'End') return;
    const target = e.target as HTMLElement;
    const idxAttr = target.closest<HTMLElement>('[data-row-index]')?.dataset.rowIndex;
    if (idxAttr == null) return;
    e.preventDefault();
    const cur = Number(idxAttr);
    const next = e.key === 'Home' ? 0 : e.key === 'End' ? filtered.length - 1 : Math.max(0, Math.min(filtered.length - 1, cur + (e.key === 'ArrowDown' ? 1 : -1)));
    if (next === cur) return;
    if (follow) setFollow(false);
    virtualizer.scrollToIndex(next, { align: 'auto' });
    requestAnimationFrame(() => {
      scrollRef.current?.querySelector<HTMLElement>(`[data-row-index="${next}"] [data-row-button]`)?.focus();
    });
  };

  const items = virtualizer.getVirtualItems();

  return (
    <div className={styles.root}>
      <div className={styles.toolbar}>
        <SearchInput value={query} onValueChange={setQuery} size="sm" mono placeholder={sse ? 'Filter events…' : 'Filter messages…'} className={styles.search} aria-label="Filter messages" />
        {!sse && (
          <SegmentedControl
            size="sm"
            value={direction}
            onValueChange={setDirection}
            aria-label="Direction"
            items={[
              { value: 'all', label: 'All', count: counts.all },
              { value: 'sent', icon: ArrowUp, label: 'Sent', count: counts.sent, tooltip: 'Client → server' },
              { value: 'received', icon: ArrowDown, label: 'Received', count: counts.received, tooltip: 'Server → client' },
            ]}
          />
        )}
        <span className={styles.spacer} />
        <IconButton
          size="sm"
          label={timeMode === 'relative' ? 'Show wall-clock times' : 'Show times relative to the connection start'}
          icon={Clock}
          active={timeMode === 'absolute'}
          onClick={() => setTimeMode(timeMode === 'relative' ? 'absolute' : 'relative')}
        />
        <IconButton
          size="sm"
          label={follow ? 'Following new messages — click to stop' : 'Follow new messages'}
          icon={ArrowDownToLine}
          active={follow}
          onClick={() => {
            const next = !follow;
            setFollow(next);
            if (next) scrollToEnd();
          }}
        />
      </div>

      {error ? (
        <EmptyState icon={Unplug} tone="danger" size="sm" title="Couldn't load messages" description={error} actions={<Button size="sm" onClick={retry}>Retry</Button>} />
      ) : loading && messages.length === 0 ? (
        <div className={styles.skeleton}>
          <Skeleton lines={8} height={10} />
        </div>
      ) : messages.length === 0 ? (
        <EmptyState
          icon={MessagesSquare}
          size="sm"
          title={live ? 'Waiting for messages…' : 'No messages'}
          description={live ? `The ${sse ? 'event stream' : 'WebSocket'} is open — frames appear here as they arrive.` : `This ${sse ? 'event stream' : 'WebSocket'} closed without sending any messages.`}
        />
      ) : filtered.length === 0 ? (
        <EmptyState
          icon={SearchX}
          size="sm"
          title="No messages match"
          description={`${pluralize(messages.length, noun)} hidden by the filter.`}
          actions={
            <Button
              size="sm"
              onClick={() => {
                setQuery('');
                setDirection('all');
              }}
            >
              Clear filter
            </Button>
          }
        />
      ) : (
        <div className={styles.listWrap}>
          <div ref={scrollRef} className={styles.scroll} onScroll={onScroll} onKeyDown={onKeyDown} role="list" aria-label="Messages">
            <div className={styles.sizer} style={{ height: virtualizer.getTotalSize() }}>
              {items.map((vi) => {
                const m = filtered[vi.index] as StreamMessageWithPayload;
                return (
                  <div key={vi.key} ref={virtualizer.measureElement} data-index={vi.index} data-row-index={vi.index} className={styles.rowSlot} style={{ transform: `translateY(${vi.start}px)` }} role="listitem">
                    <MessageRow
                      message={m}
                      startMicros={startMicros}
                      timeMode={timeMode}
                      expanded={expanded.has(m.seq)}
                      onToggle={toggle}
                      sse={sse}
                      origin={injected.has(m.seq) ? 'injected' : m.modified ? 'modified' : null}
                    />
                  </div>
                );
              })}
            </div>
          </div>
          {unseen > 0 && !follow && (
            <button type="button" className={styles.newPill} onClick={scrollToEnd}>
              <ArrowDown size={12} strokeWidth={2.25} aria-hidden />
              {formatNumber(unseen)} new
            </button>
          )}
          <div className={styles.footer}>
            <span className="tnum">
              {filtered.length === messages.length ? pluralize(messages.length, noun) : `${formatNumber(filtered.length)} of ${pluralize(messages.length, noun)}`}
              {canLoadOlder && messageCount != null && messageCount > messages.length && ` (${formatNumber(messageCount)} in total)`}
            </span>
            {canLoadOlder && (
              <Button size="sm" variant="ghost" onClick={loadOlder}>
                Load older {sse ? 'events' : 'messages'}
              </Button>
            )}
            {live && (
              <span className={styles.liveTag}>
                <span className={styles.liveDot} aria-hidden />
                Live
              </span>
            )}
          </div>
        </div>
      )}
      {kind === 'web-socket' && <PausedMessages exchangeId={exchangeId} />}
      {kind === 'web-socket' && live && <MessageComposer exchangeId={exchangeId} noServer={noServer} />}
    </div>
  );
});

// =====================================================================================
// Row
// =====================================================================================

interface MessageRowProps {
  message: StreamMessageWithPayload;
  startMicros: number;
  timeMode: 'relative' | 'absolute';
  expanded: boolean;
  onToggle: (seq: number) => void;
  sse: boolean;
  origin: Origin;
}

const ORIGIN_TIPS: Record<Exclude<Origin, null>, string> = {
  injected: 'Injected from this window — never sent by the peer',
  modified: 'Written by Akev: injected, or edited at a WebSocket breakpoint',
};

const MessageRow = memo(function MessageRow({ message: m, startMicros, timeMode, expanded, onToggle, sse, origin }: MessageRowProps) {
  const time = timeMode === 'relative' ? formatOffset(offsetMs(m.at, startMicros)) : formatTime(m.at / 1000);
  const absolute = formatTime(m.at / 1000);

  if (m.kind === 'close') {
    const close = parseCloseFrame(m);
    return (
      <div className={styles.closeRow} title={absolute}>
        <span className={styles.closeLine} aria-hidden />
        <span className={styles.closeText}>
          <Unplug size={12} strokeWidth={2} aria-hidden />
          {m.direction === 'sent' ? 'Client closed the connection' : 'Server closed the connection'}
          {close.code != null && <span className={styles.closeCode}>{close.code}</span>}
          <span>{close.reason || closeCodeMeaning(close.code)}</span>
          <span className={clsx(styles.closeTime, 'tnum')}>{time}</span>
        </span>
        <span className={styles.closeLine} aria-hidden />
      </div>
    );
  }

  const sent = m.direction === 'sent';
  const typeLabel = sse ? (m.event ?? 'message') : m.kind;
  return (
    <div className={clsx(styles.row, expanded && styles.expanded)}>
      <button type="button" className={styles.rowButton} data-row-button onClick={() => onToggle(m.seq)} aria-expanded={expanded}>
        <ChevronRight size={12} strokeWidth={2} className={styles.chevron} aria-hidden />
        <span className={clsx(styles.dir, sent ? styles.sent : styles.received)} title={sent ? 'Sent by the client' : 'Received from the server'}>
          {sent ? <ArrowUp size={13} strokeWidth={2.25} aria-label="Sent" /> : <ArrowDown size={13} strokeWidth={2.25} aria-label="Received" />}
        </span>
        <span className={clsx(styles.time, 'tnum')} title={absolute}>
          {time}
        </span>
        <span className={clsx(styles.type, styles[`type-${m.kind}`])}>{typeLabel}</span>
        <span className={styles.preview}>
          {origin && (
            <span className={clsx(styles.origin, origin === 'injected' && styles.originInjected)} title={ORIGIN_TIPS[origin]}>
              {origin}
            </span>
          )}
          {messagePreview(m) || <span className={styles.empty}>{m.kind === 'ping' || m.kind === 'pong' ? 'no payload' : '(empty)'}</span>}
        </span>
        <span className={clsx(styles.size, 'tnum')}>{formatBytes(m.size)}</span>
      </button>
      {expanded && <MessageDetail message={m} absolute={absolute} origin={origin} />}
    </div>
  );
});

function MessageDetail({ message: m, absolute, origin }: { message: StreamMessageWithPayload; absolute: string; origin: Origin }) {
  const payload = useMemo(() => formatPayload(m), [m]);
  const copyValue = m.text ?? m.base64 ?? '';
  return (
    <div className={styles.detail}>
      <div className={styles.detailBar}>
        <span className="tnum">{absolute}</span>
        <span aria-hidden>·</span>
        <span>{formatBytes(m.size)}</span>
        {m.eventId && (
          <>
            <span aria-hidden>·</span>
            <span>id {m.eventId}</span>
          </>
        )}
        {payload.prefix && (
          <>
            <span aria-hidden>·</span>
            <span>framing prefix “{payload.prefix}”</span>
          </>
        )}
        {payload.language === 'hex' && (
          <>
            <span aria-hidden>·</span>
            <span>binary</span>
          </>
        )}
        {origin && (
          <>
            <span aria-hidden>·</span>
            <span className={styles.modified} title={ORIGIN_TIPS[origin]}>
              {origin === 'injected' ? 'injected by you' : 'modified by Akev'}
            </span>
          </>
        )}
        <span className={styles.spacer} />
        <CopyButton value={copyValue} label={m.text != null ? 'Copy payload' : 'Copy payload as base64'} />
      </div>
      {payload.text === '' ? (
        <div className={styles.noPayload}>No payload</div>
      ) : (
        <CodeEditor
          value={payload.text}
          language={payload.language === 'json' ? 'json' : 'text'}
          readOnly
          wrap={payload.language !== 'hex'}
          lineNumbers={payload.language === 'json'}
          maxHeight={320}
          transparent
          contentId={`msg:${m.seq}`}
          aria-label="Message payload"
        />
      )}
    </div>
  );
}

// =====================================================================================
// Data
// =====================================================================================

/**
 * Messages with payloads, paged: the newest page from `GET /messages?tail=true`, then live
 * batches — stream notifications only carry metadata, so each burst is followed by `?after=`
 * fetches (coalesced while one is in flight). Also catches up after reconnects/resyncs and when
 * the summary's `messageCount` runs ahead of what was received; `loadOlder` fetches the page
 * before the oldest one held (see {@link MessageFeed}).
 */
function useStreamMessages(exchangeId: ExchangeId, expectedCount: number | undefined) {
  const [messages, setMessages] = useState<StreamMessageWithPayload[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [canLoadOlder, setCanLoadOlder] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const feedRef = useRef<MessageFeed | null>(null);
  const expectedRef = useRef(expectedCount);
  expectedRef.current = expectedCount;

  useEffect(() => {
    setMessages([]);
    setLoading(true);
    setError(null);
    setCanLoadOlder(false);
    const feed = new MessageFeed({
      exchangeId,
      fetch: (id, page) => api.exchanges.messages(id, page),
      source: trafficStore,
      onOlderChange: setCanLoadOlder,
      onBatch: (batch) => setMessages((prev) => mergeMessages(prev, batch)),
      onLoaded: () => {
        setError(null);
        setLoading(false);
      },
      onError: (message) => {
        setError(message);
        setLoading(false);
      },
    });
    feedRef.current = feed;
    feed.expect(expectedRef.current);
    const stop = feed.start();
    return () => {
      stop();
      if (feedRef.current === feed) feedRef.current = null;
    };
  }, [exchangeId, attempt]);

  useEffect(() => {
    feedRef.current?.expect(expectedCount);
  }, [expectedCount]);

  return {
    messages,
    loading,
    error,
    canLoadOlder,
    loadOlder: () => void feedRef.current?.loadOlder(),
    retry: () => setAttempt((n) => n + 1),
  };
}
