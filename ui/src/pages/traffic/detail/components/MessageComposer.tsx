import clsx from 'clsx';
import { ArrowDown, ArrowUp, ChevronDown, History, Send, X } from 'lucide-react';
import { useMemo, useState } from 'react';
import { errorMessage, isApiError } from '../../../../api/client';
import type { Direction, ExchangeId } from '../../../../api/types';
import { Button, CodeEditor, SegmentedControl, Tooltip } from '../../../../components/ui';
import { toast } from '../../../../components/ui/toast';
import { bytesToBase64 } from '../../../../lib/encoding';
import { formatBytes, formatTime } from '../../../../lib/format';
import { displayCombo, shortcutLabel } from '../../../../app/shortcuts';
import { usePagePref } from '../../../../state/ui';
import { clearInjectedHistory, injectMessage, useInjectedHistory, type InjectedEntry } from '../../../../state/wsTools';
import { composedPreview, encodeComposed, formatHex, parseBase64, parseHex, type ComposerMode } from '../lib/wsCompose';
import styles from './WsTools.module.css';

const PLACEHOLDERS: Record<ComposerMode, string> = {
  text: 'Message text…',
  json: '{"type":"ping"}',
  hex: 'de ad be ef',
  base64: '3q2+7w==',
};

const HISTORY_SHOWN = 8;

export interface MessageComposerProps {
  exchangeId: ExchangeId;
  /** The WebSocket is answered by Akev (ws-echo): there is no server to send to. */
  noServer?: boolean;
}

/**
 * Inject a message into a live WebSocket (Messages tab): pick the direction (to the server or
 * to the client), type text, JSON or binary (hex / base64) and send with ⌘↵. Messages sent
 * from here are listed below (click one to load it again) and marked "injected" in the list.
 */
export function MessageComposer({ exchangeId, noServer = false }: MessageComposerProps) {
  const [open, setOpen] = usePagePref<boolean>('traffic.messages.composer', true);
  const [direction, setDirection] = useState<Direction>(noServer ? 'received' : 'sent');
  const [mode, setMode] = useState<ComposerMode>('text');
  const [input, setInput] = useState('');
  const [sending, setSending] = useState(false);
  const history = useInjectedHistory(exchangeId);
  const effectiveDirection: Direction = noServer ? 'received' : direction;

  const encoded = useMemo(() => encodeComposed(mode, input), [mode, input]);
  // Don't nag about an empty JSON document before anything was typed.
  const error = !encoded.ok && !(mode === 'json' && !input.trim()) ? encoded.error : null;

  const send = async () => {
    if (!encoded.ok || sending) return;
    setSending(true);
    try {
      const entry = await injectMessage(exchangeId, { direction: effectiveDirection, ...encoded.body }, mode, input);
      if (entry.seq == null) toast.info('Message queued', { description: 'It goes out as soon as the message currently being relayed is through.' });
    } catch (e) {
      const closed = isApiError(e) && e.status === 409;
      toast.error(closed ? 'The WebSocket is closed' : "Couldn't send the message", { description: errorMessage(e) });
    } finally {
      setSending(false);
    }
  };

  /** Switch input mode; binary payloads are converted between hex and base64. */
  const switchMode = (next: ComposerMode) => {
    if ((mode === 'hex' || mode === 'base64') && (next === 'hex' || next === 'base64') && next !== mode) {
      const bytes = mode === 'hex' ? parseHex(input) : parseBase64(input);
      if (typeof bytes !== 'string') setInput(next === 'hex' ? formatHex(bytes) : bytesToBase64(bytes));
    }
    setMode(next);
  };

  const reuse = (e: InjectedEntry) => {
    setMode(e.mode);
    setInput(e.input);
    if (!noServer) setDirection(e.direction);
  };

  return (
    <section className={clsx(styles.composer, !open && styles.composerClosed)} aria-label="Send a message">
      <div className={styles.composerBar}>
        <button type="button" className={styles.composerToggle} onClick={() => setOpen(!open)} aria-expanded={open}>
          <ChevronDown size={13} strokeWidth={2} className={clsx(styles.chevron, !open && styles.chevronClosed)} aria-hidden />
          <Send size={13} strokeWidth={2} aria-hidden />
          <span className={styles.toggleLabel}>Send a message</span>
        </button>
        {open && (
          <>
            <SegmentedControl<Direction>
              size="sm"
              value={effectiveDirection}
              onValueChange={setDirection}
              aria-label="Send to"
              items={[
                { value: 'sent', icon: ArrowUp, label: 'To server', tooltip: noServer ? 'Answered by a ws-echo rule — there is no server' : 'Inject as if the client sent it', disabled: noServer },
                { value: 'received', icon: ArrowDown, label: 'To client', tooltip: 'Inject as if the server sent it' },
              ]}
            />
            <span className={styles.spacer} />
            {!error && encoded.ok && <span className={clsx(styles.meta, styles.wideOnly, 'tnum')}>{`${encoded.body.kind} · ${formatBytes(encoded.size)}`}</span>}
            <SegmentedControl<ComposerMode>
              size="sm"
              value={mode}
              onValueChange={switchMode}
              aria-label="Payload format"
              items={[
                { value: 'text', label: 'Text' },
                { value: 'json', label: 'JSON' },
                { value: 'hex', label: 'Hex', tooltip: 'Binary message, as hex bytes' },
                { value: 'base64', label: 'Base64', tooltip: 'Binary message, as base64' },
              ]}
            />
          </>
        )}
      </div>
      {open && (
        <>
          <div className={styles.composerRow}>
            <div className={clsx(styles.editor, styles.composerEditor, error && styles.editorInvalid)}>
              <CodeEditor
                value={input}
                onChange={setInput}
                language={mode === 'json' ? 'json' : 'text'}
                placeholder={PLACEHOLDERS[mode]}
                wrap
                lineNumbers={false}
                minHeight={34}
                maxHeight={160}
                onSubmit={() => void send()}
                contentId={`compose:${exchangeId}`}
                transparent
                aria-label="Message payload"
              />
            </div>
            <Tooltip content={`${effectiveDirection === 'sent' ? 'Send to the server' : 'Send to the client'} (${shortcutLabel(displayCombo('mod+enter'))})`}>
              <Button variant="primary" icon={Send} onClick={() => void send()} loading={sending} disabled={!encoded.ok} aria-label={effectiveDirection === 'sent' ? 'Send to the server' : 'Send to the client'}>
                Send
              </Button>
            </Tooltip>
          </div>
          {error && <span className={styles.error}>{error}</span>}
          {history.length > 0 && <ComposerHistory exchangeId={exchangeId} history={history} onReuse={reuse} />}
        </>
      )}
    </section>
  );
}

function ComposerHistory({ exchangeId, history, onReuse }: { exchangeId: ExchangeId; history: InjectedEntry[]; onReuse: (e: InjectedEntry) => void }) {
  const [open, setOpen] = usePagePref<boolean>('traffic.messages.history', false);
  const shown = history.slice(-HISTORY_SHOWN).reverse();
  return (
    <div className={styles.history}>
      <div className={styles.historyHead}>
        <button type="button" className={styles.historyToggle} onClick={() => setOpen(!open)} aria-expanded={open}>
          <ChevronDown size={12} strokeWidth={2} className={clsx(styles.chevron, !open && styles.chevronClosed)} aria-hidden />
          <History size={12} strokeWidth={2} aria-hidden />
          Sent from Akev
          <span className="tnum">({history.length})</span>
        </button>
        <span className={styles.spacer} />
        {open && (
          <Tooltip content="Forget these (the messages stay in the list)">
            <button type="button" className={styles.link} onClick={() => clearInjectedHistory(exchangeId)}>
              <X size={11} strokeWidth={2} aria-hidden />
              Clear
            </button>
          </Tooltip>
        )}
      </div>
      {open && <ul className={styles.historyList}>
        {shown.map((e) => (
          <li key={e.key}>
            <button type="button" className={styles.historyRow} onClick={() => onReuse(e)} title="Load into the editor">
              <span className={clsx(styles.dir, e.direction === 'sent' ? styles.dirSent : styles.dirReceived)} role="img" aria-label={e.direction === 'sent' ? 'To server' : 'To client'}>
                {e.direction === 'sent' ? <ArrowUp size={12} strokeWidth={2.25} /> : <ArrowDown size={12} strokeWidth={2.25} />}
              </span>
              <span className={clsx(styles.historyTime, 'tnum')}>{formatTime(e.at, false)}</span>
              <span className={styles.historyPreview}>{composedPreview(e.mode, e.input)}</span>
              <span className={clsx(styles.tag, e.seq == null ? styles.tagQueued : styles.tagInjected)}>{e.seq == null ? 'queued' : 'injected'}</span>
            </button>
          </li>
        ))}
      </ul>}
    </div>
  );
}
