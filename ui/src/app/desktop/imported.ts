/**
 * `akev://imported`: the desktop shell imported a capture file opened through a file
 * association (double-click in the file manager, "Open with Akev").
 */

import { toast } from '../../components/ui/toast';
import { pluralize } from '../../lib/format';
import { navigate } from '../router';

function isRecord(v: unknown): v is Record<string, unknown> {
  return typeof v === 'object' && v !== null;
}

/** `akev://imported`: `{file, count, firstId}` or `{file, error}`. */
export function announceImported(payload: unknown): void {
  if (!isRecord(payload)) return;
  const file = typeof payload.file === 'string' && payload.file ? payload.file : 'the file';
  if (typeof payload.error === 'string') {
    const reason = payload.error.trim().replace(/[.\s]+$/, '');
    toast.error(`Couldn't open ${file}`, { description: `${reason}. Check that the file is complete, or export it again.` });
    return;
  }
  const count = typeof payload.count === 'number' ? payload.count : 0;
  const firstId = typeof payload.firstId === 'number' ? payload.firstId : null;
  toast.success(count === 0 ? `${file} has no exchanges` : `Imported ${pluralize(count, 'exchange')}`, {
    description: count === 0 ? 'Nothing was added to the traffic list.' : file,
    action: firstId != null ? { label: 'Show', onClick: () => navigate(`/traffic/${firstId}`) } : undefined,
  });
}

