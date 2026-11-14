/**
 * Sessions dialog: save the current traffic as a named session, and open (replace / add),
 * rename, download or delete saved sessions and autosaves. Reachable from the Traffic
 * toolbar's import/export menu and the command palette (`openSessionsDialog`).
 */

import clsx from 'clsx';
import { Archive, ChevronDown, Download, FolderOpen, History, MoreHorizontal, Pencil, Plus, RefreshCw, Save, Settings2, Trash2 } from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, errorMessage } from '../../api/client';
import type { SessionInfo, SessionOpenMode } from '../../api/types';
import { Button, confirm, Dialog, EmptyState, IconButton, Input, Menu, Select, Skeleton, toast, type MenuItems } from '../../components/ui';
import { formatNumber, pluralize } from '../../lib/format';
import { downloadUrl } from '../../lib/platform';
import { useAppStore } from '../../state/app';
import { trafficStore, useTraffic } from '../../state/traffic';
import { navigate } from '../router';
import { hasTraffic, openSession } from './actions';
import { closeSessionsDialog, defaultSessionName, describeSession, formatInterval, saveRequest, scopeCount, sortSessions, useSessionsDialog, validateSessionName, type SaveContext, type SaveScope } from './sessions';
import styles from './SessionsDialog.module.css';

type Load = { phase: 'loading' } | { phase: 'error'; message: string } | { phase: 'ready'; sessions: SessionInfo[] };

/** Snapshot of the traffic list for the save form (selection and filter at open time). */
function saveContext(): SaveContext {
  const snap = trafficStore.getSnapshot();
  const selected = snap.selection.ids.size > 0 ? trafficStore.selectedInOrder() : [];
  const filter = snap.filter.active && !snap.filter.error ? snap.filter.query : null;
  return { total: snap.order.length, visible: snap.visible, selected, filter };
}

function SaveForm({ autoFocus, onSaved }: { autoFocus: boolean; onSaved: (s: SessionInfo) => void }) {
  const total = useTraffic((s) => s.order.length);
  const [ctx, setCtx] = useState(saveContext);
  const [name, setName] = useState(() => defaultSessionName());
  const [scope, setScope] = useState<SaveScope>(() => (ctx.selected.length > 1 ? 'selected' : 'all'));
  const [saving, setSaving] = useState(false);
  const [touched, setTouched] = useState(false);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Keep the counts current while the dialog is open (traffic keeps arriving).
  useEffect(() => setCtx(saveContext()), [total]);
  useEffect(() => {
    if (!autoFocus) return;
    const t = setTimeout(() => inputRef.current?.select(), 30);
    return () => clearTimeout(t);
  }, [autoFocus]);

  const error = touched ? validateSessionName(name) : null;
  const count = scopeCount(scope, ctx);
  const options = [
    { value: 'all' as const, label: `All traffic · ${formatNumber(ctx.total)}` },
    ...(ctx.visible.length < ctx.total ? [{ value: 'visible' as const, label: `Visible · ${formatNumber(ctx.visible.length)}` }] : []),
    ...(ctx.selected.length > 0 ? [{ value: 'selected' as const, label: `Selected · ${formatNumber(ctx.selected.length)}` }] : []),
  ];

  const save = async () => {
    setTouched(true);
    if (validateSessionName(name) || count === 0 || saving) return;
    setSaving(true);
    try {
      const info = await api.sessions.save(saveRequest(name, scope, ctx));
      toast.success(`Saved “${info.name}”`, { description: pluralize(info.exchanges, 'exchange') });
      setName(defaultSessionName());
      setTouched(false);
      onSaved(info);
    } catch (e) {
      toast.error("Couldn't save the session", { description: errorMessage(e) });
    } finally {
      setSaving(false);
    }
  };

  return (
    <form
      className={styles.saveForm}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <div className={styles.saveTitle}>
        <Save size={14} strokeWidth={1.75} aria-hidden />
        Save current traffic
      </div>
      <div className={styles.saveRow}>
        <Input ref={inputRef} value={name} onChange={(e) => setName(e.target.value)} onBlur={() => setTouched(true)} invalid={!!error} aria-label="Session name" placeholder="Session name" className={styles.nameInput} maxLength={400} />
        <Select<SaveScope> value={options.some((o) => o.value === scope) ? scope : 'all'} onValueChange={setScope} options={options} aria-label="What to save" className={styles.scopeSelect} />
        <Button type="submit" variant="primary" icon={Plus} loading={saving} disabled={count === 0}>
          Save
        </Button>
      </div>
      {error ? (
        <p className={styles.formError} role="alert">
          {error}
        </p>
      ) : count === 0 ? (
        <p className={styles.formHint}>Nothing to save yet — captured traffic appears in the list as it arrives.</p>
      ) : null}
    </form>
  );
}

function OpenButton({ session, onOpened }: { session: SessionInfo; onOpened: () => void }) {
  const [busy, setBusy] = useState(false);
  const run = async (mode: SessionOpenMode) => {
    setBusy(true);
    const ok = await openSession(session, mode);
    setBusy(false);
    if (ok) onOpened();
  };
  if (!hasTraffic()) {
    return (
      <Button size="sm" icon={FolderOpen} loading={busy} onClick={() => void run('replace')}>
        Open
      </Button>
    );
  }
  return (
    <Menu
      align="end"
      trigger={
        <Button size="sm" icon={FolderOpen} iconRight={ChevronDown} loading={busy}>
          Open
        </Button>
      }
      items={[
        { key: 'replace', label: 'Replace current traffic', icon: FolderOpen, hint: `${formatNumber(trafficStore.getSnapshot().order.length)} removed`, onSelect: () => void run('replace') },
        { key: 'append', label: 'Add to current traffic', icon: Plus, onSelect: () => void run('append') },
      ]}
    />
  );
}

function RenameInput({ session, onDone }: { session: SessionInfo; onDone: (renamed: SessionInfo | null) => void }) {
  const autosave = session.kind === 'autosave';
  // Keeping an autosave suggests a dated name (Enter keeps it right away).
  const [value, setValue] = useState(() => (autosave ? defaultSessionName(new Date(session.updatedAt)) : session.name));
  const [busy, setBusy] = useState(false);
  const error = validateSessionName(value);
  const commit = async () => {
    if (busy) return;
    if (!autosave && value.trim() === session.name) return onDone(null);
    if (error) return;
    setBusy(true);
    try {
      const renamed = await api.sessions.rename(session.id, value);
      if (autosave) toast.success(`Kept as “${renamed.name}”`, { description: "It won't be overwritten — autosave keeps saving to a new “Autosave”." });
      onDone(renamed);
    } catch (e) {
      toast.error("Couldn't rename the session", { description: errorMessage(e) });
      setBusy(false);
    }
  };
  return (
    <Input
      autoFocus
      size="sm"
      value={value}
      invalid={!!error}
      disabled={busy}
      aria-label={`New name for ${session.name}`}
      className={styles.renameInput}
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setValue(e.target.value)}
      // Keeping an autosave moves it: only on an explicit Enter, never on blur.
      onBlur={() => (autosave ? onDone(null) : void commit())}
      onKeyDown={(e) => {
        if (e.key === 'Enter') {
          e.preventDefault();
          void commit();
        } else if (e.key === 'Escape') {
          e.preventDefault();
          e.stopPropagation();
          onDone(null);
        }
      }}
    />
  );
}

interface RowProps {
  session: SessionInfo;
  now: number;
  highlighted: boolean;
  renaming: boolean;
  onRename: (id: string | null, renamed?: SessionInfo | null) => void;
  onDeleted: (id: string) => void;
  onOpened: () => void;
}

function SessionRow({ session, now, highlighted, renaming, onRename, onDeleted, onOpened }: RowProps) {
  const autosave = session.kind === 'autosave';
  const Icon = autosave ? History : Archive;
  const remove = async () => {
    const ok = await confirm({
      title: `Delete “${session.name}”?`,
      message: `${pluralize(session.exchanges, 'exchange')} will be deleted from disk. This can't be undone — download the file first if you might need it.`,
      destructive: true,
      confirmLabel: 'Delete session',
    });
    if (!ok) return;
    try {
      await api.sessions.remove(session.id);
      toast.success(`Deleted “${session.name}”`);
      onDeleted(session.id);
    } catch (e) {
      toast.error("Couldn't delete the session", { description: errorMessage(e) });
    }
  };
  const items: MenuItems = [
    { key: 'rename', label: autosave ? 'Keep as saved session…' : 'Rename…', icon: Pencil, onSelect: () => onRename(session.id) },
    { key: 'download', label: 'Download .akev file', icon: Download, onSelect: () => void downloadUrl(api.sessions.fileUrl(session.id), `${session.name.replace(/[\\/:*?"<>|]+/g, '_')}.akev`) },
    { type: 'separator', key: 's' },
    { key: 'delete', label: 'Delete', icon: Trash2, destructive: true, onSelect: () => void remove() },
  ];
  return (
    <li className={clsx(styles.row, highlighted && styles.rowHighlighted)} aria-label={session.name}>
      <span className={clsx(styles.rowIcon, autosave && styles.rowIconAutosave)} aria-hidden>
        <Icon size={15} strokeWidth={1.75} />
      </span>
      <div className={styles.rowText}>
        {renaming ? (
          <RenameInput session={session} onDone={(renamed) => onRename(null, renamed)} />
        ) : (
          <span className={styles.rowName}>
            <span className="truncate">{session.name}</span>
          </span>
        )}
        <span className={clsx(styles.rowMeta, 'tnum')}>{describeSession(session, now)}</span>
      </div>
      <div className={styles.rowActions}>
        <OpenButton session={session} onOpened={onOpened} />
        <Menu align="end" trigger={<IconButton label={`More actions for ${session.name}`} icon={MoreHorizontal} size="sm" tooltip={false} />} items={items} />
      </div>
    </li>
  );
}

function SessionsList({ load, now, highlight, onReload, onChange, onOpened }: { load: Load; now: number; highlight: string | null; onReload: () => void; onChange: (next: SessionInfo[]) => void; onOpened: () => void }) {
  const [renaming, setRenaming] = useState<string | null>(null);
  if (load.phase === 'loading') {
    return (
      <div className={styles.skeletons} role="status" aria-busy="true" aria-label="Loading sessions">
        {[0, 1, 2].map((i) => (
          <Skeleton key={i} height={44} />
        ))}
      </div>
    );
  }
  if (load.phase === 'error') {
    return (
      <EmptyState
        size="sm"
        tone="danger"
        icon={Archive}
        title="Couldn't load sessions"
        description={load.message}
        actions={
          <Button icon={RefreshCw} onClick={onReload}>
            Retry
          </Button>
        }
      />
    );
  }
  const sessions = sortSessions(load.sessions);
  const autosaves = sessions.filter((s) => s.kind === 'autosave');
  const saved = sessions.filter((s) => s.kind !== 'autosave');
  const row = (s: SessionInfo) => (
    <SessionRow
      key={s.id}
      session={s}
      now={now}
      highlighted={s.id === highlight}
      renaming={renaming === s.id}
      onRename={(id, renamed) => {
        setRenaming(id);
        // An autosave kept under a new name gets a new id: drop the old entry.
        if (renamed) onChange([renamed, ...load.sessions.filter((x) => x.id !== s.id && x.id !== renamed.id)]);
      }}
      onDeleted={(id) => onChange(load.sessions.filter((x) => x.id !== id))}
      onOpened={onOpened}
    />
  );
  return (
    <div className={styles.lists}>
      {autosaves.length > 0 && (
        <section aria-label="Autosaves">
          <h3 className={styles.groupTitle}>Autosave</h3>
          <ul className={styles.list}>{autosaves.map(row)}</ul>
        </section>
      )}
      <section aria-label="Saved sessions">
        <h3 className={styles.groupTitle}>Saved</h3>
        {saved.length > 0 ? (
          <ul className={styles.list}>{saved.map(row)}</ul>
        ) : (
          <p className={styles.emptyList}>No saved sessions yet. Save the current traffic above to keep it — it stays on this computer until you delete it.</p>
        )}
      </section>
    </div>
  );
}

function AutosaveFooterNote() {
  const s = useAppStore((st) => st.settings?.sessions);
  if (!s) return null;
  const go = () => {
    closeSessionsDialog();
    navigate('/settings/data');
  };
  return (
    <button type="button" className={styles.footerLink} onClick={go}>
      <Settings2 size={13} strokeWidth={1.75} aria-hidden />
      {s.autosave ? `Autosave on · every ${formatInterval(s.autosaveIntervalSecs)}${s.restoreOnStartup ? ' · restored at startup' : ''}` : 'Autosave is off'}
    </button>
  );
}

/** The Sessions dialog body (mounted while open). */
export function SessionsDialogContent({ focusSave, onClose }: { focusSave: boolean; onClose: () => void }) {
  const [load, setLoad] = useState<Load>({ phase: 'loading' });
  const [highlight, setHighlight] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());

  const reload = useCallback(() => {
    const ctrl = new AbortController();
    api.sessions.list(ctrl.signal).then(
      (sessions) => {
        setLoad({ phase: 'ready', sessions });
        setNow(Date.now());
      },
      (e: unknown) => {
        if (!ctrl.signal.aborted) setLoad({ phase: 'error', message: errorMessage(e) });
      },
    );
    return () => ctrl.abort();
  }, []);

  useEffect(() => reload(), [reload]);
  // Relative times and autosave updates stay fresh while the dialog is open.
  useEffect(() => {
    const t = setInterval(() => reload(), 15_000);
    return () => clearInterval(t);
  }, [reload]);

  const sessions = load.phase === 'ready' ? load.sessions : [];
  const count = useMemo(() => sessions.length, [sessions]);

  return (
    <Dialog
      open
      onOpenChange={(open) => !open && onClose()}
      title="Sessions"
      description={count > 0 ? `${pluralize(count, 'session')} saved on this computer. Open one to continue where you left off.` : 'Save captured traffic and reopen it later — even after restarting Akev.'}
      width={680}
      className={styles.dialog}
      focusContent={!focusSave}
      footer={
        <>
          <AutosaveFooterNote />
          <Button variant="ghost" onClick={onClose}>
            Close
          </Button>
        </>
      }
    >
      <SaveForm
        autoFocus={focusSave}
        onSaved={(info) => {
          setLoad((l) => (l.phase === 'ready' ? { phase: 'ready', sessions: [info, ...l.sessions.filter((s) => s.id !== info.id)] } : l));
          setHighlight(info.id);
          setNow(Date.now());
        }}
      />
      <SessionsList load={load} now={now} highlight={highlight} onReload={reload} onChange={(next) => setLoad({ phase: 'ready', sessions: next })} onOpened={onClose} />
    </Dialog>
  );
}

/** Mount once (App shell); renders the dialog while it's open. */
export default function SessionsDialog() {
  const { open, focusSave } = useSessionsDialog();
  if (!open) return null;
  return <SessionsDialogContent focusSave={focusSave} onClose={closeSessionsDialog} />;
}
