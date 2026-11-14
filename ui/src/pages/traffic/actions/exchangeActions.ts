/**
 * Everything the Traffic page can do to exchanges — the one implementation shared by the
 * list (keyboard shortcuts, context menus, toolbar), the detail pane (header actions,
 * multi-selection pane, banners) and palette commands. Feedback goes through toasts; actions
 * never throw. Destructive actions are deferred behind an undo toast (DESIGN.md §1.6).
 */

import { api, errorMessage } from '../../../api/client';
import type { CodegenTarget, ExchangeId, ExchangeSummary, Rule, Side, TagColor, TargetInfo } from '../../../api/types';
import { navigate } from '../../../app/router';
import { toast, undoToast } from '../../../components/ui';
import { formatBytes, pluralize } from '../../../lib/format';
import { summaryUrl } from '../../../lib/http';
import { copyText, downloadUrl, pickFiles } from '../../../lib/platform';
import type { RuleSource, RuleTemplate } from '../../../lib/ruleFromExchange';
import { notStoredMessage, useAppStore } from '../../../state/app';
import { openInCompose, revealRule } from '../../../state/intents';
import { trafficStore } from '../../../state/traffic';
import { useUiStore } from '../../../state/ui';
import { forgetExchangeDetail } from '../detail/hooks/useExchangeDetail';
import { appendClause, clause, pathOnly } from '../filter/filterEdit';
import { redactExports } from '../../../lib/exportRedaction';
import { trafficView } from '../trafficView';
import { codegenTargets } from './codegenTargets';
import { exportHarSelection, saveSessionSelection } from './exportSelection';
import { getHidden, hide, isHidden, unhide, unhideWhenGone, type HideEntry } from './hiddenRows';

/** Undo window for deletes / clears (matches the undo toast). */
const UNDO_MS = 5000;
/** Upper bound for per-exchange fan-out actions (snippets, replays). */
const FANOUT_LIMIT = 200;

function summaries(ids: readonly ExchangeId[]): ExchangeSummary[] {
  const out: ExchangeSummary[] = [];
  for (const id of ids) {
    const s = trafficStore.get(id);
    if (s) out.push(s);
  }
  return out;
}

/** Run `fn` over items with at most `limit` in flight; results keep input order. */
async function mapLimit<T, R>(items: readonly T[], limit: number, fn: (item: T) => Promise<R>): Promise<R[]> {
  const out = new Array<R>(items.length);
  let next = 0;
  const workers = Array.from({ length: Math.min(limit, items.length) }, async () => {
    while (next < items.length) {
      const i = next++;
      out[i] = await fn(items[i] as T);
    }
  });
  await Promise.all(workers);
  return out;
}

/** Selected ids in display order (falls back to id order). */
export function selectedIds(): ExchangeId[] {
  const sel = trafficStore.getSnapshot().selection.ids;
  if (sel.size === 0) return [];
  const list = trafficView.getList();
  if (sel.size === 1 || list.length === 0) return trafficStore.selectedInOrder();
  const ordered = list.filter((id) => sel.has(id));
  return ordered.length === sel.size ? ordered : trafficStore.selectedInOrder();
}

/** Select an exchange and reflect it in the URL. */
export function openExchange(id: ExchangeId): void {
  // Ids that aren't in the list (e.g. a paused exchange that isn't recorded) are resolved by the
  // Traffic page's route handling, not selected blindly.
  if (trafficStore.get(id)) trafficStore.select(id);
  navigate(`/traffic/${id}`, { replace: true });
}

function short(s: ExchangeSummary): string {
  const path = s.path.length > 48 ? `${s.path.slice(0, 47)}…` : s.path;
  return `${s.method} ${s.host}${path}`;
}

function timestamp(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, '0');
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}_${p(d.getHours())}-${p(d.getMinutes())}`;
}

// =====================================================================================
// Copy
// =====================================================================================

async function copyWithToast(text: string, message: string, description?: string): Promise<void> {
  if (await copyText(text)) toast.success(message, description ? { description } : undefined);
  else toast.error("Couldn't copy to the clipboard");
}

export async function copyUrls(ids: readonly ExchangeId[]): Promise<void> {
  const rows = summaries(ids);
  if (rows.length === 0) return;
  const text = rows.map(summaryUrl).join('\n');
  await copyWithToast(text, rows.length === 1 ? 'URL copied' : `Copied ${pluralize(rows.length, 'URL')}`, rows.length === 1 ? text : undefined);
}

/** Copy one snippet per exchange in a codegen target (blank-line separated). */
export async function copyAsSnippet(ids: readonly ExchangeId[], target: CodegenTarget | TargetInfo = 'curl'): Promise<void> {
  const rows = summaries(ids).slice(0, FANOUT_LIMIT);
  if (rows.length === 0) return;
  const targetId = typeof target === 'string' ? target : target.id;
  const name = (typeof target === 'string' ? codegenTargets().find((t) => t.id === target)?.name : target.name) ?? (targetId === 'curl' ? 'cURL' : targetId);
  try {
    const snippets = await mapLimit(rows, 6, (s) => api.exchanges.snippet(s.id, targetId));
    const text = snippets.map((s) => s.code.trimEnd()).join('\n\n');
    await copyWithToast(text, rows.length === 1 ? `Copied as ${name}` : `Copied ${rows.length} requests as ${name}`);
  } catch (e) {
    toast.error(`Couldn't generate ${name}`, { description: errorMessage(e) });
  }
}

export async function copyBody(id: ExchangeId, side: Side): Promise<void> {
  try {
    const view = await api.exchanges.body(id, side);
    if (view.size === 0) {
      toast(`The ${side} has no body`);
      return;
    }
    if (view.text == null) {
      toast.error(`The ${side} body is binary`, {
        description: 'Download it instead.',
        action: { label: 'Download', onClick: () => void downloadBody(id, side) },
      });
      return;
    }
    await copyWithToast(view.text, `${side === 'request' ? 'Request' : 'Response'} body copied`, view.truncated ? 'Truncated for transport — download for the full body.' : formatBytes(view.size));
  } catch (e) {
    toast.error(`Couldn't load the ${side} body`, { description: errorMessage(e) });
  }
}

// =====================================================================================
// Replay & Compose
// =====================================================================================

export async function replay(ids: readonly ExchangeId[]): Promise<void> {
  const rows = summaries(ids).filter((s) => s.kind === 'http' || s.kind === 'web-socket' || s.kind === 'event-stream');
  if (rows.length === 0) {
    toast.error('Nothing to replay', { description: 'Tunnels and TLS failures have no request to resend.' });
    return;
  }
  if (rows.length === 1) {
    const s = rows[0] as ExchangeSummary;
    const pending = toast.loading(`Replaying ${short(s)}…`);
    try {
      const res = await api.exchanges.replay(s.id);
      const status = res.response ? `${res.response.status} ${res.response.statusMessage}`.trim() : res.error ? res.error.message : 'No response';
      // Not stored (recording paused): there's no exchange to view.
      const opts =
        res.recorded === false
          ? { id: pending, description: `${status} · ${notStoredMessage()}` }
          : { id: pending, description: status, action: { label: 'View', onClick: () => openExchange(res.id) } };
      if (res.error) toast.error(`Replayed ${s.method} ${pathOnly(s.path)}`, opts);
      else toast.success(`Replayed ${s.method} ${pathOnly(s.path)}`, opts);
    } catch (e) {
      toast.error('Replay failed', { id: pending, description: errorMessage(e), action: { label: 'Retry', onClick: () => void replay([s.id]) } });
    }
    return;
  }
  const batch = rows.slice(0, FANOUT_LIMIT);
  const pending = toast.loading(`Replaying ${batch.length} requests…`);
  let failed = 0;
  let unstored = 0;
  let lastId: number | null = null;
  for (const s of batch) {
    try {
      const res = await api.exchanges.replay(s.id);
      // Only stored exchanges can be viewed (none while recording is paused).
      if (res.recorded === false) unstored++;
      else lastId = res.id;
      if (res.error) failed++;
    } catch {
      failed++;
    }
  }
  const id = lastId;
  const paused = !useAppStore.getState().recording;
  const notes = [failed ? `${pluralize(failed, 'request')} failed` : null, unstored ? `${pluralize(unstored, 'response')} not stored${paused ? ' (recording is paused)' : ''}` : null].filter(Boolean).join(' · ');
  const opts = { id: pending, action: id != null ? { label: 'View', onClick: () => openExchange(id) } : undefined, description: notes || undefined };
  if (failed) toast.error(`Replayed ${batch.length} requests`, opts);
  else toast.success(`Replayed ${batch.length} requests`, opts);
}

export async function openInComposeFor(id: ExchangeId): Promise<void> {
  const s = trafficStore.get(id);
  if (s && (s.kind === 'tunnel' || s.kind === 'tls-failure')) {
    toast.error('This exchange has no HTTP request to edit');
    return;
  }
  try {
    const request = await api.exchanges.editableRequest(id);
    openInCompose(request, { sourceExchangeId: id, title: s ? `${s.method} ${pathOnly(s.path)}` : undefined });
  } catch (e) {
    toast.error("Couldn't open in Compose", { description: errorMessage(e) });
  }
}

// =====================================================================================
// Pin, highlight, note
// =====================================================================================

export async function setPinned(ids: readonly ExchangeId[], pinned: boolean): Promise<void> {
  if (ids.length === 0) return;
  try {
    await api.exchanges.patch({ ids: [...ids], pinned });
    if (ids.length > 1) toast.success(pinned ? `Pinned ${pluralize(ids.length, 'exchange')}` : `Unpinned ${pluralize(ids.length, 'exchange')}`);
  } catch (e) {
    toast.error(pinned ? "Couldn't pin" : "Couldn't unpin", { description: errorMessage(e) });
  }
}

/** Pin the exchanges, or unpin them when all are pinned already. */
export function togglePin(ids: readonly ExchangeId[]): Promise<void> {
  const rows = summaries(ids);
  return setPinned(
    rows.map((s) => s.id),
    !rows.every((s) => s.pinned),
  );
}

export async function setHighlight(ids: readonly ExchangeId[], color: TagColor | null): Promise<void> {
  if (ids.length === 0) return;
  try {
    await api.exchanges.patch({ ids: [...ids], color });
  } catch (e) {
    toast.error("Couldn't change the highlight", { description: errorMessage(e) });
  }
}

/** Save (or, when blank, remove) an exchange's note. Resolves to whether it was saved. */
export async function saveNote(id: ExchangeId, note: string | null): Promise<boolean> {
  const value = note?.trim() ? note : null;
  try {
    await api.exchanges.patch({ ids: [id], note: value });
    toast.success(value ? 'Note saved' : 'Note removed');
    return true;
  } catch (e) {
    toast.error("Couldn't save the note", { description: errorMessage(e) });
    return false;
  }
}

// =====================================================================================
// Export & download
// =====================================================================================

export function exportHar(scope: 'all' | 'visible' | 'selected', ids?: readonly ExchangeId[]): void {
  const redact = redactExports('har');
  if (scope === 'all') {
    void downloadUrl(api.transfer.harUrl({ redact }), `akev_${timestamp()}.har`);
    return;
  }
  if (scope === 'visible') {
    const name = `akev_${timestamp()}_visible.har`;
    const filter = trafficStore.getSnapshot().filter;
    const list = trafficView.getList();
    // A filter URL stays short for any number of rows; explicit ids when nothing filters.
    if (filter.active && !filter.error) void downloadUrl(api.transfer.harUrl({ filter: filter.query, redact }), name);
    else if (list.length && list.length < trafficStore.getSnapshot().order.length) void exportHarSelection(list, name);
    else void downloadUrl(api.transfer.harUrl({ redact }), name);
    return;
  }
  const sel = ids ?? selectedIds();
  if (sel.length === 0) return;
  const name = sel.length === 1 ? `akev_exchange_${sel[0]}_${timestamp()}.har` : `akev_${sel.length}-exchanges_${timestamp()}.har`;
  // Large selections would overflow the URL: sent as id ranges / chunks (exportSelection).
  void exportHarSelection(sel, name);
}

export function saveSession(ids?: readonly ExchangeId[]): void {
  const name = `akev_${timestamp()}.akev`;
  if (ids?.length) void saveSessionSelection(ids, name);
  else void downloadUrl(api.transfer.sessionUrl({ redact: redactExports('session') }), name);
}

/** Filename for a downloaded body (`users.json`, `index.html`; `….request` for request bodies). */
export function bodyFilename(s: Pick<ExchangeSummary, 'path' | 'id'>, side: Side = 'response'): string {
  const segs = pathOnly(s.path).split('/').filter(Boolean);
  let name = segs[segs.length - 1] ?? '';
  try {
    name = decodeURIComponent(name);
  } catch {
    // keep encoded
  }
  name = name.replace(/[\\/:*?"<>|]+/g, '_') || `exchange-${s.id}`;
  return side === 'request' ? `${name}.request` : name;
}

/** Download a body, decoded (`Content-Encoding` removed) unless `decoded` is false. */
export async function downloadBody(id: ExchangeId, side: Side = 'response', decoded = true): Promise<void> {
  const s = trafficStore.get(id);
  if (s && (side === 'request' ? s.requestSize === 0 : !s.responseSize && s.status == null)) {
    toast(`This exchange has no ${side} body`);
    return;
  }
  await downloadUrl(api.exchanges.rawUrl(id, side, { decoded, download: true }), bodyFilename(s ?? { id, path: '' }, side));
}

export async function importFile(kind: 'har' | 'session'): Promise<void> {
  const files = await pickFiles({ accept: kind === 'har' ? ['.har', 'application/json'] : ['.akev'], multiple: true });
  if (!files?.length) return;
  for (const file of files) {
    const pending = toast.loading(`Importing ${file.name}…`);
    try {
      const res = kind === 'har' ? await api.transfer.importHar(file.data) : await api.transfer.importSession(file.data);
      const first = res.ids[0];
      toast.success(`Imported ${pluralize(res.ids.length, 'exchange')}`, {
        id: pending,
        description: file.name,
        action: first != null ? { label: 'Show', onClick: () => navigate(`/traffic/${first}`) } : undefined,
      });
    } catch (e) {
      toast.error(`Couldn't import ${file.name}`, { id: pending, description: errorMessage(e) });
    }
  }
}

// =====================================================================================
// Filter ▸
// =====================================================================================

export type FilterBy = 'host' | 'path' | 'exclude-host' | 'source';

/** The clause "Filter ▸ …" appends for an exchange (`null` when not applicable). */
export function filterClauseFor(s: ExchangeSummary, by: FilterBy): string | null {
  switch (by) {
    case 'host':
      return clause('host', '=', s.host);
    case 'exclude-host':
      return clause('host', '=', s.host, true);
    case 'path':
      return clause('path', '=', pathOnly(s.path) || '/');
    case 'source':
      return s.source ? clause('source', '=', s.source) : null;
  }
}

/** Append a clause to the filter text and apply it now. */
export function applyFilterClause(c: string): void {
  const next = appendClause(trafficStore.getSnapshot().filter.text, c);
  trafficStore.setFilterText(next);
  trafficStore.flushFilter();
  useUiStore.getState().pushRecentFilter(next);
}

export function filterBy(id: ExchangeId, by: FilterBy): void {
  const s = trafficStore.get(id);
  if (!s) return;
  const c = filterClauseFor(s, by);
  if (c) applyFilterClause(c);
}

// =====================================================================================
// Create rule ▸
// =====================================================================================

const TEMPLATE_TOASTS: Record<RuleTemplate, string> = {
  mock: 'Mock rule created',
  'breakpoint-request': 'Breakpoint rule created',
  'breakpoint-response': 'Breakpoint rule created',
  'block-host': 'Block rule created',
  'map-local': 'Map Local rule created',
  rewrite: 'Rewrite rule created',
};

/** The rule builders (and the rule-tree helpers they use) load with the first rule created. */
const loadBuilders = () => import('../../../lib/ruleFromExchange');

/** Prepend a new rule, report the outcome and reveal it on the Rules page. */
async function saveNewRule(rule: Rule, src: RuleSource, template: RuleTemplate, retry: () => void): Promise<boolean> {
  const app = useAppStore.getState();
  try {
    const { prependRule } = await loadBuilders();
    const current = app.rules ?? (await api.rules.get());
    const result = await app.saveRules(prependRule(current, rule));
    const err = result.errors.find(([ruleId]) => ruleId === rule.id);
    const description = template === 'block-host' ? src.host : `${src.method} ${src.host}${pathOnly(src.path)}`;
    if (err) toast.warning(TEMPLATE_TOASTS[template], { description: err[1] });
    else toast.success(TEMPLATE_TOASTS[template], { description });
    revealRule(rule.id);
    return true;
  } catch (e) {
    toast.error("Couldn't save the rule", { description: errorMessage(e), action: { label: 'Retry', onClick: retry } });
    return false;
  }
}

/** Create a rule from an exchange (prefilled from its method, host and path) and reveal it. */
export async function createRule(id: ExchangeId, template: RuleTemplate): Promise<boolean> {
  const s = trafficStore.get(id);
  if (!s) return false;
  let rule: Rule;
  try {
    const { blockHostRule, bodySourceFromBase64, breakpointRule, mapLocalRule, mockRule, rewriteRule } = await loadBuilders();
    switch (template) {
      case 'mock': {
        if (s.status == null) {
          toast.error('No response to mock yet', { description: 'Wait for the response, or create a breakpoint to respond by hand.' });
          return false;
        }
        // The editable response carries the full decoded body (the body view may be truncated).
        const response = await api.exchanges.editableResponse(id);
        rule = mockRule(s, response, bodySourceFromBase64(response.body));
        break;
      }
      case 'breakpoint-request':
        rule = breakpointRule(s, { request: true, response: false });
        break;
      case 'breakpoint-response':
        rule = breakpointRule(s, { request: false, response: true });
        break;
      case 'block-host':
        rule = blockHostRule(s);
        break;
      case 'map-local':
        rule = mapLocalRule(s, { baseDir: useAppStore.getState().status?.dataDir });
        break;
      case 'rewrite':
        rule = rewriteRule(s);
        break;
    }
  } catch (e) {
    toast.error("Couldn't create the rule", { description: errorMessage(e) });
    return false;
  }
  return saveNewRule(rule, s, template, () => void createRule(id, template));
}

/** A mock rule for an endpoint that failed upstream (starts as an empty 200 to edit). */
export async function createEmptyMock(src: RuleSource): Promise<boolean> {
  const { mockRule } = await loadBuilders();
  const rule = mockRule(src, { status: 200, statusMessage: 'OK', headers: [['content-type', 'application/json']] }, { type: 'empty' });
  return saveNewRule(rule, src, 'mock', () => void createEmptyMock(src));
}

// =====================================================================================
// Delete & clear (deferred, undoable)
// =====================================================================================

interface PendingCommit {
  timer: ReturnType<typeof setTimeout>;
  commit: () => void;
}

const pendingCommits = new Map<number, PendingCommit>();

function schedule(token: number, commit: () => Promise<void>): void {
  const run = () => {
    const p = pendingCommits.get(token);
    if (!p) return;
    clearTimeout(p.timer);
    pendingCommits.delete(token);
    void commit();
  };
  pendingCommits.set(token, { timer: setTimeout(run, UNDO_MS), commit: run });
}

function cancel(token: number): boolean {
  const p = pendingCommits.get(token);
  if (!p) return false;
  clearTimeout(p.timer);
  pendingCommits.delete(token);
  unhide(token);
  return true;
}

/** Commit every pending delete/clear now (page unload). */
export function flushPendingRemovals(): void {
  for (const p of [...pendingCommits.values()]) p.commit();
}

window.addEventListener('pagehide', flushPendingRemovals);

/** Select the row that takes the place of removed rows (Finder-style). */
function selectAfterRemoval(removed: ReadonlySet<ExchangeId>): void {
  const snap = trafficStore.getSnapshot();
  const selected = [...snap.selection.ids];
  if (!selected.some((id) => removed.has(id))) return;
  const remaining = selected.filter((id) => !removed.has(id));
  if (remaining.length > 0) {
    trafficStore.setSelectedIds(remaining, remaining.includes(snap.selection.active ?? -1) ? snap.selection.active : (remaining[0] as number));
    return;
  }
  const list = trafficView.getList();
  const active = snap.selection.active;
  let from = active != null && removed.has(active) ? list.indexOf(active) : -1;
  if (from < 0) for (let i = list.length - 1; i >= 0 && from < 0; i--) if (removed.has(list[i] as number)) from = i;
  let next: ExchangeId | null = null;
  for (let i = from + 1; i < list.length && next == null; i++) if (!removed.has(list[i] as number)) next = list[i] as number;
  for (let i = from - 1; i >= 0 && next == null; i--) if (!removed.has(list[i] as number)) next = list[i] as number;
  trafficStore.select(next);
  if (next != null) trafficView.scrollToId(next);
}

/**
 * Delete exchanges with an undo window: the rows are hidden at once and the daemon delete is
 * sent when the window ends (or the page closes). Undo un-hides them. The traffic store only
 * changes when the daemon reports the removal, so the list never disagrees with the server.
 */
export function deleteExchanges(ids: readonly ExchangeId[]): void {
  const rows = summaries(ids);
  if (rows.length === 0) return;
  const set = new Set(rows.map((s) => s.id));
  const prevSelection = [...trafficStore.getSnapshot().selection.ids];
  const prevActive = trafficStore.getSnapshot().selection.active;
  selectAfterRemoval(set);
  const token = hide({ ids: set });
  const label = rows.length === 1 ? `Deleted ${short(rows[0] as ExchangeSummary)}` : `Deleted ${pluralize(rows.length, 'exchange')}`;
  schedule(token, async () => {
    try {
      await api.exchanges.delete([...set]);
      set.forEach(forgetExchangeDetail);
      unhideWhenGone(token, () => [...set].some((id) => trafficStore.get(id) != null));
    } catch (e) {
      unhide(token);
      toast.error("Couldn't delete", { description: errorMessage(e), action: { label: 'Retry', onClick: () => deleteExchanges([...set]) } });
    }
  });
  undoToast(
    label,
    () => {
      if (!cancel(token)) return;
      const restore = prevSelection.filter((id) => trafficStore.get(id));
      if (restore.length) trafficStore.setSelectedIds(restore, prevActive);
    },
    { duration: UNDO_MS },
  );
}

export interface ClearPlan {
  ids: ExchangeId[];
  maxId: ExchangeId;
  pinned: number;
}

/** What "Clear" would remove right now (rows already hidden excluded). */
export function clearPlan(): ClearPlan | null {
  const snap = trafficStore.getSnapshot();
  const entries = getHidden().entries;
  const ids: ExchangeId[] = [];
  let pinned = 0;
  for (const id of snap.order) {
    const s = snap.summaries.get(id);
    if (!s || isHidden(entries, s)) continue;
    ids.push(id);
    if (s.pinned) pinned++;
  }
  if (ids.length === 0) return null;
  return { ids, maxId: ids[ids.length - 1] as number, pinned };
}

/** Clear traffic with an undo window. `keepPinned` spares pinned rows. */
export function clearTraffic(plan: ClearPlan, keepPinned: boolean): void {
  const removed = keepPinned ? plan.ids.length - plan.pinned : plan.ids.length;
  if (removed === 0) {
    toast('Only pinned exchanges left', { description: 'Unpin them to clear everything.' });
    return;
  }
  const entry: Omit<HideEntry, 'token'> = { upTo: plan.maxId, keepPinned };
  const prevSelection = [...trafficStore.getSnapshot().selection.ids];
  const prevActive = trafficStore.getSnapshot().selection.active;
  const token = hide(entry);
  const sel = trafficStore.getSnapshot().selection.ids;
  const kept = [...sel].filter((id) => {
    const s = trafficStore.get(id);
    return s && !isHidden([{ token, ...entry }], s);
  });
  if (kept.length !== sel.size) trafficStore.setSelectedIds(kept, kept[0] ?? null);
  const matches = (id: ExchangeId) => {
    const s = trafficStore.get(id);
    return s != null && isHidden([{ token, ...entry }], s);
  };
  schedule(token, async () => {
    try {
      const order = trafficStore.getSnapshot().order;
      const newest = order[order.length - 1];
      if (newest == null || newest <= plan.maxId) await api.exchanges.clear({ keepPinned });
      else await api.exchanges.delete(order.filter(matches));
      unhideWhenGone(token, () => trafficStore.getSnapshot().order.some(matches));
    } catch (e) {
      unhide(token);
      toast.error("Couldn't clear traffic", { description: errorMessage(e), action: { label: 'Retry', onClick: () => clearTraffic(plan, keepPinned) } });
    }
  });
  undoToast(
    `Cleared ${pluralize(removed, 'exchange')}`,
    () => {
      if (!cancel(token)) return;
      const restore = prevSelection.filter((id) => trafficStore.get(id));
      if (restore.length) trafficStore.setSelectedIds(restore, prevActive);
    },
    { duration: UNDO_MS, description: keepPinned && plan.pinned ? `Kept ${pluralize(plan.pinned, 'pinned exchange')}` : undefined },
  );
}

// =====================================================================================
// Recording
// =====================================================================================

export function toggleRecording(): void {
  useAppStore
    .getState()
    .setRecording()
    .then(
      () => {
        const recording = useAppStore.getState().recording;
        toast(recording ? 'Recording resumed' : 'Recording paused', { description: recording ? undefined : 'Traffic is still proxied and rules still apply.' });
      },
      (e: unknown) => toast.error("Couldn't change recording state", { description: errorMessage(e) }),
    );
}
