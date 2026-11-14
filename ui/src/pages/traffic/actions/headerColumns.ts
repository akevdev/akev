/** Custom header columns of the traffic list: add / remove with feedback. */

import { toast } from '../../../components/ui';
import { headerColumnId, headerDisplayName, isHeaderColumn, MAX_HEADER_COLUMNS, useUiStore, type HeaderColumnId } from '../../../state/ui';

/** Show a header's value as a list column (no-op with a note when it's already shown). */
export function showHeaderColumn(side: 'request' | 'response', name: string): boolean {
  const id = headerColumnId(side, name);
  const label = `${headerDisplayName(id.slice(4))} (${side})`;
  if (!isHeaderColumn(id)) {
    toast.error("Couldn't add the column", { description: `“${name}” isn't a valid header name.` });
    return false;
  }
  const ui = useUiStore.getState();
  if (ui.columns.some((c) => c.id === id)) {
    toast(`${label} is already a column`);
    return true;
  }
  if (!ui.addHeaderColumn(side, name)) {
    toast.error("Couldn't add the column", { description: `Up to ${MAX_HEADER_COLUMNS} header columns can be shown. Remove one from the column menu first.` });
    return false;
  }
  toast.success(`Showing ${label} as a column`, { action: { label: 'Undo', onClick: () => removeHeaderColumn(id) } });
  return true;
}

export function removeHeaderColumn(id: HeaderColumnId): void {
  useUiStore.getState().setColumnVisible(id, false);
}
