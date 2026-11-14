import clsx from 'clsx';
import { ArrowDown, ArrowUp, CirclePause, Play, RotateCcw, Trash2 } from 'lucide-react';
import { memo, useMemo, useState } from 'react';
import { errorMessage } from '../../../../api/client';
import type { ExchangeId, PausedMessage } from '../../../../api/types';
import { useShortcut } from '../../../../app/shortcuts';
import { Button, CodeEditor, SegmentedControl, Tooltip } from '../../../../components/ui';
import { toast } from '../../../../components/ui/toast';
import { bytesToBase64 } from '../../../../lib/encoding';
import { formatBytes, formatElapsed } from '../../../../lib/format';
import { resumeAllPausedMessages, resumePausedMessage, usePausedMessages } from '../../../../state/wsTools';
import { PAUSE_WARN_MS } from '../lib/detail';
import { editablePaused, encodeComposed, formatHex, forwardResume, parseBase64, parseHex, type ComposerMode } from '../lib/wsCompose';
import { useNow } from '../hooks/misc';
import styles from './WsTools.module.css';

/**
 * WebSocket messages of one exchange held at a `ws-breakpoint` rule, shown at the end of the
 * message list (where they would appear once sent): edit the payload, then Forward (⌘↵) or Drop
 * each one, or Forward / Drop all.
 */
export function PausedMessages({ exchangeId }: { exchangeId: ExchangeId }) {
  const paused = usePausedMessages(exchangeId);
  const [busy, setBusy] = useState<'forward' | 'drop' | null>(null);
  if (paused.length === 0) return null;

  const all = async (action: 'forward' | 'drop') => {
    setBusy(action);
    try {
      await resumeAllPausedMessages(exchangeId, action);
    } catch (e) {
      toast.error(action === 'forward' ? "Couldn't forward the messages" : "Couldn't drop the messages", { description: errorMessage(e) });
    } finally {
      setBusy(null);
    }
  };

  return (
    <section className={styles.paused} aria-label="Paused messages">
      <header className={styles.pausedHead}>
        <CirclePause size={15} strokeWidth={2} className={styles.pausedIcon} aria-hidden />
        <div className={styles.pausedTitle}>
          <span>{paused.length === 1 ? 'Message paused' : `${paused.length} messages paused`}</span>
          <span className={styles.pausedSub}>Held by a WebSocket breakpoint — only its direction waits</span>
        </div>
        {paused.length > 1 && (
          <>
            <Button size="sm" variant="ghost" icon={Trash2} onClick={() => void all('drop')} loading={busy === 'drop'} disabled={busy != null}>
              Drop all
            </Button>
            <Button size="sm" icon={Play} onClick={() => void all('forward')} loading={busy === 'forward'} disabled={busy != null}>
              Forward all
            </Button>
          </>
        )}
      </header>
      <div className={styles.pausedList}>
        {paused.map((p, i) => (
          <PausedCard key={p.pauseId} message={p} first={i === 0} />
        ))}
      </div>
    </section>
  );
}

const PausedCard = memo(function PausedCard({ message: p, first }: { message: PausedMessage; first: boolean }) {
  const initial = useMemo(() => editablePaused(p), [p]);
  const [mode, setMode] = useState<ComposerMode>(initial.mode);
  const [input, setInput] = useState(initial.input);
  const [busy, setBusy] = useState<'forward' | 'drop' | null>(null);
  const now = useNow(1000);
  const since = p.at / 1000;
  const late = now - since >= PAUSE_WARN_MS;

  const outcome = useMemo(() => forwardResume(p, mode, input), [p, mode, input]);
  const size = useMemo(() => {
    const e = encodeComposed(mode === 'json' ? 'text' : mode, input);
    return e.ok ? e.size : null;
  }, [mode, input]);
  const changed = outcome.ok ? outcome.changed : true;
  const sent = p.direction === 'sent';

  const forward = async () => {
    if (!outcome.ok || busy) return;
    setBusy('forward');
    try {
      await resumePausedMessage(p.pauseId, outcome.resume);
    } catch (e) {
      toast.error("Couldn't forward the message", { description: errorMessage(e) });
      setBusy(null);
    }
  };
  const drop = async () => {
    if (busy) return;
    setBusy('drop');
    try {
      await resumePausedMessage(p.pauseId, { action: 'drop' });
    } catch (e) {
      toast.error("Couldn't drop the message", { description: errorMessage(e) });
      setBusy(null);
    }
  };
  // ⌘↵ outside an editor forwards the oldest paused message (inside one: that message).
  useShortcut('mod+enter', () => void forward(), { enabled: first && outcome.ok && busy == null, description: 'Forward the paused WebSocket message', group: 'Breakpoints' });

  const revert = () => {
    setMode(initial.mode);
    setInput(initial.input);
  };
  /** Switch how a binary payload is edited (keeps the bytes when the input is valid). */
  const switchBinary = (next: ComposerMode) => {
    const bytes = mode === 'hex' ? parseHex(input) : parseBase64(input);
    if (typeof bytes !== 'string') setInput(next === 'hex' ? formatHex(bytes) : bytesToBase64(bytes));
    setMode(next);
  };

  return (
    <div className={styles.card}>
      <div className={styles.cardBar}>
        <span className={clsx(styles.dir, sent ? styles.dirSent : styles.dirReceived)} aria-hidden>
          {sent ? <ArrowUp size={13} strokeWidth={2.25} /> : <ArrowDown size={13} strokeWidth={2.25} />}
        </span>
        <span className={styles.cardWhat}>{sent ? 'Client → server' : 'Server → client'}</span>
        <span className={clsx(styles.kind, p.kind === 'binary' && styles.kindBinary)}>{p.kind}</span>
        <Tooltip content={late ? 'Peers may time out while a message waits — decide soon' : 'Time this message has been held'}>
          <span className={clsx(styles.timer, 'tnum', late && styles.timerLate)}>waiting {formatElapsed(since, now)}</span>
        </Tooltip>
        {outcome.ok && (
          <span className={clsx(styles.meta, 'tnum')}>
            {size != null ? formatBytes(size) : ''}
            {changed ? ' · edited' : ''}
          </span>
        )}
        <span className={styles.spacer} />
        {changed && outcome.ok && (
          <button type="button" className={styles.link} onClick={revert}>
            <RotateCcw size={11} strokeWidth={2} aria-hidden />
            Revert
          </button>
        )}
        {p.kind === 'binary' && (
          <SegmentedControl<ComposerMode>
            size="sm"
            value={mode}
            onValueChange={switchBinary}
            items={[
              { value: 'hex', label: 'Hex' },
              { value: 'base64', label: 'Base64' },
            ]}
            aria-label="Edit binary payload as"
          />
        )}
        {/* Actions sit in the bar so they stay visible when space is tight. */}
        <span className={styles.cardActions}>
          <Button size="sm" variant="subtle-danger" icon={Trash2} onClick={() => void drop()} loading={busy === 'drop'} disabled={busy != null}>
            Drop
          </Button>
          <Button size="sm" variant="primary" icon={Play} kbd="mod+enter" onClick={() => void forward()} loading={busy === 'forward'} disabled={busy != null || !outcome.ok}>
            {changed ? 'Forward edited' : 'Forward'}
          </Button>
        </span>
      </div>
      <div className={clsx(styles.editor, !outcome.ok && styles.editorInvalid)}>
        <CodeEditor
          value={input}
          onChange={setInput}
          language={mode === 'json' ? 'json' : 'text'}
          wrap
          lineNumbers={false}
          minHeight={36}
          maxHeight={160}
          onSubmit={() => void forward()}
          contentId={`paused:${p.pauseId}`}
          transparent
          aria-label="Paused message payload"
        />
      </div>
      {!outcome.ok && <span className={styles.error}>{outcome.error}</span>}
    </div>
  );
});
