/**
 * Export an explicit selection of exchanges (HAR or `.akev` session) without hitting the
 * daemon's URI length limit: see `lib/exportPlan`. Failures are reported with a toast.
 */

import { api, errorMessage } from '../../../api/client';
import type { ExchangeId } from '../../../api/types';
import { toast } from '../../../components/ui';
import { HarMerger, planSelectionExport } from '../../../lib/exportPlan';
import { redactExports } from '../../../lib/exportRedaction';
import { pluralize } from '../../../lib/format';
import { downloadUrl, saveData } from '../../../lib/platform';

/**
 * Download a HAR of exactly `ids`: one download link when the selection fits in a URL (as id
 * ranges if need be), otherwise fetched in chunks, merged and saved as one file.
 */
export async function exportHarSelection(ids: readonly ExchangeId[], filename: string): Promise<void> {
  const redact = redactExports('har');
  const plan = planSelectionExport(ids, (q) => api.transfer.harUrl({ ...q, redact }));
  if (plan.length === 0) return;
  if (plan.length === 1) {
    await downloadUrl(api.transfer.harUrl({ ...plan[0], redact }), filename);
    return;
  }
  const id = toast.loading(`Exporting ${pluralize(ids.length, 'exchange')}…`);
  try {
    const merger = new HarMerger();
    for (const q of plan) merger.add(await api.transfer.har({ ...q, redact }));
    const saved = await saveData(merger.result(), filename, 'application/json');
    if (saved) toast.success(`Exported ${pluralize(ids.length, 'exchange')}`, { id });
    else toast.dismiss(id);
  } catch (e) {
    toast.error('HAR export failed', { id, description: errorMessage(e) });
  }
}

/** Download a session of exactly `ids` (sessions can't be merged, so it must fit one request). */
export async function saveSessionSelection(ids: readonly ExchangeId[], filename: string): Promise<void> {
  const redact = redactExports('session');
  const plan = planSelectionExport(ids, (q) => api.transfer.sessionUrl({ ...q, redact }));
  if (plan.length === 0) return;
  if (plan.length > 1) {
    toast.error('Selection too large to save as one session', {
      description: `${pluralize(ids.length, 'scattered exchange')} don't fit in one export request — save all traffic, or narrow the selection with a filter.`,
    });
    return;
  }
  await downloadUrl(api.transfer.sessionUrl({ ...plan[0], redact }), filename);
}
