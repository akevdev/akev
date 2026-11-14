/**
 * Session & import actions shared by the Sessions dialog, the Traffic toolbar's
 * import/export menu, the command palette and the startup toast.
 */

import { api, errorMessage } from '../../api/client';
import type { IdsResult, RestoredSession, SessionInfo, SessionOpenMode } from '../../api/types';
import { toast } from '../../components/ui';
import { pluralize } from '../../lib/format';
import { pickFiles, type PickedFile } from '../../lib/platform';
import { storageGet, storageSet } from '../../lib/storage';
import { trafficStore } from '../../state/traffic';
import { navigate } from '../router';
import {
  CAPTURE_LABELS,
  captureKind,
  IMPORT_LABELS,
  RESTORE_ANNOUNCED_KEY,
  importAccept,
  openSessionsDialog,
  restoredTitle,
  shouldAnnounceRestore,
  type CaptureKind,
  type ForeignImport,
} from './sessions';

/** Show the first of `ids` in the traffic list. */
function showAction(ids: readonly number[]) {
  const first = ids[0];
  return first != null ? { label: 'Show', onClick: () => navigate(`/traffic/${first}`) } : undefined;
}

/** Pick Charles / mitmproxy / Fiddler capture files and import them. */
export async function importForeign(kind: ForeignImport): Promise<void> {
  const files = await pickFiles({ accept: importAccept(kind), multiple: true });
  if (!files?.length) return;
  for (const file of files) {
    const pending = toast.loading(`Importing ${file.name}…`);
    try {
      const res =
        kind === 'charles'
          ? await api.transfer.importCharles(file.data)
          : kind === 'fiddler'
            ? await api.transfer.importFiddler(file.data)
            : await api.transfer.importMitmproxy(file.data);
      toast.success(`Imported ${pluralize(res.ids.length, 'exchange')}`, { id: pending, description: `${IMPORT_LABELS[kind]} · ${file.name}`, action: showAction(res.ids) });
    } catch (e) {
      toast.error(`Couldn't import ${file.name}`, { id: pending, description: errorMessage(e) });
    }
  }
}

function importAs(kind: CaptureKind, data: Uint8Array): Promise<IdsResult> {
  switch (kind) {
    case 'har':
      return api.transfer.importHar(data);
    case 'session':
      return api.transfer.importSession(data);
    case 'charles':
      return api.transfer.importCharles(data);
    case 'fiddler':
      return api.transfer.importFiddler(data);
    case 'mitmproxy':
      return api.transfer.importMitmproxy(data);
  }
}

/**
 * Import capture files of any supported kind (HAR, `.akev`, Charles, Fiddler, mitmproxy),
 * detected per file: drag & drop, "Open capture file…" and the desktop menu. Files that
 * aren't captures are named in one error. Resolves the ids imported.
 */
export async function importCaptureFiles(files: readonly PickedFile[]): Promise<number[]> {
  const imported: number[] = [];
  const unknown: string[] = [];
  for (const file of files) {
    const kind = captureKind(file.name, file.data.subarray(0, 512));
    if (!kind) {
      unknown.push(file.name);
      continue;
    }
    const pending = toast.loading(`Importing ${file.name}…`);
    try {
      const res = await importAs(kind, file.data);
      imported.push(...res.ids);
      toast.success(`Imported ${pluralize(res.ids.length, 'exchange')}`, { id: pending, description: `${CAPTURE_LABELS[kind]} · ${file.name}`, action: showAction(res.ids) });
    } catch (e) {
      toast.error(`Couldn't import ${file.name}`, { id: pending, description: errorMessage(e) });
    }
  }
  if (unknown.length) {
    toast.error(unknown.length === 1 ? `Couldn't import ${unknown[0]}` : `Couldn't import ${unknown.length} files`, {
      description: `${unknown.length === 1 ? "It isn't" : "They aren't"} a capture file. Akev opens HAR, .akev, Charles (.chlsj), Fiddler (.saz) and mitmproxy flow files.`,
    });
  }
  return imported;
}

/** Pick capture files of any supported kind and import them (no type filter: mitmproxy flow files often have no extension). */
export async function openCaptureFiles(): Promise<void> {
  const files = await pickFiles({ multiple: true });
  if (files?.length) await importCaptureFiles(files);
}

/** Load a saved session into the traffic list. Returns whether it worked. */
export async function openSession(session: Pick<SessionInfo, 'id' | 'name'>, mode: SessionOpenMode): Promise<boolean> {
  const pending = toast.loading(`Opening “${session.name}”…`);
  try {
    const res = await api.sessions.open(session.id, mode);
    toast.success(mode === 'replace' ? `Opened “${session.name}”` : `Added “${session.name}”`, {
      id: pending,
      description: mode === 'replace' ? pluralize(res.ids.length, 'exchange') : `${pluralize(res.ids.length, 'exchange')} added to the current traffic`,
      action: showAction(res.ids),
    });
    return true;
  } catch (e) {
    toast.error(`Couldn't open “${session.name}”`, { id: pending, description: errorMessage(e) });
    return false;
  }
}

/** Whether there is traffic an open-with-replace would remove. */
export function hasTraffic(): boolean {
  return trafficStore.getSnapshot().order.length > 0;
}

/**
 * Announce a restored autosave once per daemon run (hello on every reconnect carries it,
 * and so does the `session-restored` message when the restore finishes after connecting).
 */
export function announceRestoredSession(restored: RestoredSession | undefined, startedAt: number): void {
  if (!restored) return;
  if (!shouldAnnounceRestore(startedAt, storageGet(RESTORE_ANNOUNCED_KEY, 'session'))) return;
  storageSet(RESTORE_ANNOUNCED_KEY, String(startedAt), 'session');
  toast.success(restoredTitle(restored), {
    description: 'Traffic from your last session is back. Autosave keeps saving as you go.',
    duration: 8000,
    action: { label: 'Sessions', onClick: () => openSessionsDialog() },
  });
}
