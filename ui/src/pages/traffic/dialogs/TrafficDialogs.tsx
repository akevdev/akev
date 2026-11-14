import { ArrowLeftRight, Pin, StickyNote } from 'lucide-react';
import { lazy, Suspense, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { api } from '../../../api/client';
import type { ExchangeId } from '../../../api/types';
import { Button, Dialog, Kbd, MethodBadge, Spinner, StatusBadge, TextArea } from '../../../components/ui';
import { pluralize } from '../../../lib/format';
import { useExchangeSummary } from '../../../state/traffic';
import { saveNote } from '../actions/exchangeActions';
import styles from './TrafficDialogs.module.css';

const ExchangeCompare = lazy(() => import('../../../components/exchange/ExchangeCompare').then((m) => ({ default: m.ExchangeCompare })));
const HeaderColumnDialog = lazy(() => import('./HeaderColumnDialog'));

/** Which dialog is open (one at a time). */
type DialogState =
  | { kind: 'note'; id: ExchangeId }
  | { kind: 'compare'; a: ExchangeId; b: ExchangeId }
  | { kind: 'clear'; total: number; pinned: number; resolve: (choice: ClearChoice) => void }
  | { kind: 'header-column' }
  | null;

export type ClearChoice = 'cancel' | 'all' | 'keep-pinned';

let state: DialogState = null;
const listeners = new Set<() => void>();
function set(next: DialogState): void {
  if (state?.kind === 'clear' && next !== state) state.resolve('cancel');
  state = next;
  for (const l of listeners) l();
}
function useDialogState(): DialogState {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => state,
    () => state,
  );
}

export function openNoteDialog(id: ExchangeId): void {
  set({ kind: 'note', id });
}

export function openCompareDialog(a: ExchangeId, b: ExchangeId): void {
  set({ kind: 'compare', a: Math.min(a, b), b: Math.max(a, b) });
}

/** "Add header column…": pick a request / response header to show as a list column. */
export function openHeaderColumnDialog(): void {
  set({ kind: 'header-column' });
}

/** Ask how to clear when pinned exchanges exist. */
export function askClear(total: number, pinned: number): Promise<ClearChoice> {
  return new Promise((resolve) => set({ kind: 'clear', total, pinned, resolve }));
}

function ExchangeLine({ id }: { id: ExchangeId }) {
  const s = useExchangeSummary(id);
  if (!s) return <span className="subtle">Exchange #{id}</span>;
  return (
    <span className={styles.line}>
      <MethodBadge method={s.method} size="sm" />
      <StatusBadge summary={s} tooltip={false} />
      <span className={styles.lineUrl}>
        <span className={styles.lineHost}>{s.host}</span>
        <span className={styles.linePath}>{s.path}</span>
      </span>
      <span className={styles.lineId}>#{id}</span>
    </span>
  );
}

function NoteDialog({ id, onClose }: { id: ExchangeId; onClose: () => void }) {
  const [note, setNote] = useState('');
  const [initial, setInitial] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const ref = useRef<HTMLTextAreaElement | null>(null);

  useEffect(() => {
    const ctrl = new AbortController();
    api.exchanges.get(id, ctrl.signal).then(
      (d) => {
        setInitial(d.note ?? '');
        setNote((n) => n || (d.note ?? ''));
        requestAnimationFrame(() => ref.current?.select());
      },
      () => setInitial(''),
    );
    return () => ctrl.abort();
  }, [id]);

  const save = async () => {
    setSaving(true);
    const ok = await saveNote(id, note);
    setSaving(false);
    if (ok) onClose();
  };

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={initial ? 'Edit note' : 'Add note'}
      description={<ExchangeLine id={id} />}
      width={520}
      footer={
        <>
          {initial ? (
            <Button variant="subtle-danger" className={styles.footerStart} onClick={() => setNote('')} disabled={saving || !note}>
              Clear
            </Button>
          ) : null}
          <Button variant="ghost" onClick={onClose}>
            Cancel
          </Button>
          <Button variant="primary" icon={StickyNote} kbd="mod+enter" loading={saving} disabled={initial === null || note === initial} onClick={save}>
            Save note
          </Button>
        </>
      }
    >
      <TextArea
        ref={ref}
        autoFocus
        value={note}
        placeholder={initial === null ? 'Loading…' : 'What is interesting about this exchange?'}
        disabled={initial === null}
        autoResize
        minRows={4}
        maxRows={14}
        onChange={(e) => setNote(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            if (note !== initial) void save();
          }
        }}
        aria-label="Note"
      />
      <p className={styles.hint}>
        Notes are searchable with <code>note:</code> and <code>has:note</code>. <Kbd combo="mod+enter" size="sm" /> saves.
      </p>
    </Dialog>
  );
}

function CompareDialog({ a, b, onClose }: { a: ExchangeId; b: ExchangeId; onClose: () => void }) {
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title={
        <span className={styles.compareTitle}>
          <ArrowLeftRight size={16} strokeWidth={1.75} aria-hidden /> Compare exchanges
        </span>
      }
      description="Request line, headers, bodies and timing side by side — differences highlighted."
      width={1180}
      className={styles.compareDialog}
      flush
      focusContent
    >
      <div className={styles.compareBody}>
        <Suspense fallback={<Spinner size={16} tone="muted" label="Loading comparison" />}>
          <ExchangeCompare a={a} b={b} />
        </Suspense>
      </div>
    </Dialog>
  );
}

function ClearDialog({ total, pinned, resolve }: { total: number; pinned: number; resolve: (c: ClearChoice) => void }) {
  const done = (c: ClearChoice) => {
    resolve(c);
    state = null;
    for (const l of listeners) l();
  };
  return (
    <Dialog
      open
      onOpenChange={(open) => !open && done('cancel')}
      title="Clear traffic?"
      description={
        <>
          {pluralize(total, 'exchange')} will be removed, including <strong className={styles.pinned}>{pluralize(pinned, 'pinned exchange')}</strong>. You can undo for a few seconds.
        </>
      }
      width={460}
      hideClose
      footer={
        <>
          <Button variant="ghost" className={styles.footerStart} onClick={() => done('cancel')}>
            Cancel
          </Button>
          <Button variant="secondary" onClick={() => done('all')}>
            Clear all
          </Button>
          <Button variant="primary" icon={Pin} onClick={() => done('keep-pinned')} autoFocus>
            Keep pinned
          </Button>
        </>
      }
    />
  );
}

/** Renders whichever Traffic dialog is open. Mount once in the page. */
export function TrafficDialogs() {
  const current = useDialogState();
  const close = () => set(null);
  if (!current) return null;
  switch (current.kind) {
    case 'note':
      return <NoteDialog key={current.id} id={current.id} onClose={close} />;
    case 'compare':
      return <CompareDialog a={current.a} b={current.b} onClose={close} />;
    case 'clear':
      return <ClearDialog total={current.total} pinned={current.pinned} resolve={current.resolve} />;
    case 'header-column':
      return (
        <Suspense fallback={null}>
          <HeaderColumnDialog onClose={close} />
        </Suspense>
      );
  }
}
