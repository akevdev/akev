/**
 * Compose store: tabs, responses, history, saved requests and environments.
 *
 * Persistence goes through UI page prefs (`useUiStore.setPagePref`), i.e. localStorage + the
 * daemon's `settings.ui` (`PATCH /api/settings/ui`), under four keys. Writes are debounced
 * here (typing in a URL must not re-serialise the whole prefs object on every keystroke) and
 * flushed on `pagehide`. Newer prefs arriving from the daemon (another window) re-hydrate the
 * library slices; tabs only until the user has touched them in this window.
 *
 * Responses are in memory only — after a reload a tab restores its last response from the
 * exchange the daemon recorded (`lastExchangeId`). A response sent while recording is paused
 * isn't stored (`recorded: false`): nothing refers to its exchange id.
 */

import { create } from 'zustand';
import { api, isApiError } from '../../api/client';
import type { EditableRequest, SendOptions, SendResponse } from '../../api/types';
import { debounce } from '../../lib/timing';
import { flushPrefs, useUiStore } from '../../state/ui';
import { addFolder, addHistory, addSaved, findSaved, removeNode, renameNode, updateSaved } from './library';
import {
  draftHash,
  emptyDraft,
  historyDraft,
  newTab,
  persistDraft,
  persistHistory,
  sanitizeEnvState,
  sanitizeHistory,
  sanitizeSaved,
  sanitizeTab,
  uid,
  type ComposeTab,
  type EnvState,
  type HistoryEntry,
  type RequestDraft,
  type RequestOptions,
  type SavedFolder,
  type SavedNode,
  type SavedRequest,
} from './model';
import { buildRequest } from './request';
import * as T from './tabs';
import { resolveEnvVars } from './variables';

export const PREF_TABS = 'compose.tabs';
export const PREF_HISTORY = 'compose.history';
export const PREF_SAVED = 'compose.saved';
export const PREF_ENV = 'compose.env';

// =====================================================================================
// Types
// =====================================================================================

export type ResponseStatus = 'sending' | 'restoring' | 'done' | 'error' | 'cancelled';

export interface ResponseState {
  status: ResponseStatus;
  /** Unix ms. */
  startedAt: number;
  finishedAt?: number;
  /** What was sent (resolved). */
  sent?: EditableRequest;
  url?: string;
  /** `done`: the daemon's result (may still carry an exchange `error`, e.g. DNS failure). */
  result?: SendResponse;
  /** `error`: the API call itself failed (daemon unreachable, invalid request…). */
  error?: { message: string; code: string; status: number };
  /** Rebuilt from the recorded exchange after a reload. */
  restored?: boolean;
  /** Exchange recorded by the send before this one (compare responses between sends). */
  previousId?: number;
}

export interface SendOutcome {
  ok: boolean;
  /** Variables that were sent literally. */
  unresolved: string[];
  /** Validation problem before sending (e.g. empty URL). */
  invalid?: 'empty-url' | 'bad-url';
}

interface Library {
  history: HistoryEntry[];
  saved: SavedNode[];
  env: EnvState;
}

export interface ComposeState extends T.TabsState, Library {
  responses: Record<string, ResponseState>;
}

// =====================================================================================
// Hydration
// =====================================================================================

function readPref(key: string): unknown {
  return useUiStore.getState().pages[key];
}

function hydrateTabs(raw: unknown): T.TabsState {
  const rec = raw && typeof raw === 'object' ? (raw as { tabs?: unknown; activeId?: unknown }) : {};
  const tabs = Array.isArray(rec.tabs) ? rec.tabs.map(sanitizeTab).filter((t): t is ComposeTab => t !== null) : [];
  // Ids must be unique (hand-edited prefs).
  const seen = new Set<string>();
  const unique = tabs.filter((t) => (seen.has(t.id) ? false : (seen.add(t.id), true)));
  return T.initialTabs(unique, typeof rec.activeId === 'string' ? rec.activeId : null);
}

function initialState(): ComposeState {
  return {
    ...hydrateTabs(readPref(PREF_TABS)),
    history: sanitizeHistory(readPref(PREF_HISTORY)),
    saved: sanitizeSaved(readPref(PREF_SAVED)),
    env: sanitizeEnvState(readPref(PREF_ENV)),
    responses: {},
  };
}

export const useComposeStore = create<ComposeState>()(() => initialState());

const get = () => useComposeStore.getState();
const set = (partial: Partial<ComposeState> | ((s: ComposeState) => Partial<ComposeState>)) => useComposeStore.setState(partial);

// =====================================================================================
// Persistence
// =====================================================================================

/** Value identity we last wrote/read per key (to tell our own writes from remote ones). */
const lastSeen = new Map<string, unknown>();
let tabsTouched = false;

function serializeTabs(s: T.TabsState): unknown {
  return {
    activeId: s.activeId,
    tabs: s.tabs.map((t) => ({ ...t, draft: persistDraft(t.draft) })),
  };
}

function writePref(key: string, value: unknown): void {
  lastSeen.set(key, value);
  useUiStore.getState().setPagePref(key, value);
}

const writers = {
  tabs: debounce(() => writePref(PREF_TABS, serializeTabs(get())), 400),
  history: debounce(() => writePref(PREF_HISTORY, persistHistory(get().history)), 400),
  saved: debounce(() => writePref(PREF_SAVED, get().saved.map((n) => (n.type === 'folder' ? { ...n, children: n.children.map((c) => ({ ...c, draft: persistDraft(c.draft) })) } : { ...n, draft: persistDraft(n.draft) }))), 300),
  env: debounce(() => writePref(PREF_ENV, get().env), 300),
};

/** Write pending changes now (before unload / on unmount). */
export function flushCompose(): void {
  writers.tabs.flush();
  writers.history.flush();
  writers.saved.flush();
  writers.env.flush();
}

for (const key of [PREF_TABS, PREF_HISTORY, PREF_SAVED, PREF_ENV]) lastSeen.set(key, readPref(key));

useComposeStore.subscribe((s, prev) => {
  if (s.tabs !== prev.tabs || s.activeId !== prev.activeId) writers.tabs();
  if (s.history !== prev.history) writers.history();
  if (s.saved !== prev.saved) writers.saved();
  if (s.env !== prev.env) writers.env();
});

// Prefs replaced by a newer copy from the daemon (another window, or the first `hello`).
useUiStore.subscribe((u, prev) => {
  if (u.pages === prev.pages) return;
  const changed = (key: string) => {
    const v = u.pages[key];
    if (v === lastSeen.get(key)) return false;
    const same = JSON.stringify(v) === JSON.stringify(lastSeen.get(key));
    lastSeen.set(key, v);
    return !same;
  };
  const patch: Partial<ComposeState> = {};
  if (changed(PREF_HISTORY)) patch.history = sanitizeHistory(u.pages[PREF_HISTORY]);
  if (changed(PREF_SAVED)) patch.saved = sanitizeSaved(u.pages[PREF_SAVED]);
  if (changed(PREF_ENV)) patch.env = sanitizeEnvState(u.pages[PREF_ENV]);
  if (changed(PREF_TABS) && !tabsTouched) Object.assign(patch, hydrateTabs(u.pages[PREF_TABS]));
  if (Object.keys(patch).length) set(patch);
});

window.addEventListener('pagehide', () => {
  flushCompose();
  flushPrefs();
});

// =====================================================================================
// Tab actions
// =====================================================================================

function setTabs(fn: (s: T.TabsState) => T.TabsState): void {
  const s = get();
  const next = fn(s);
  if (next === s) return;
  tabsTouched = true;
  set({ tabs: next.tabs, activeId: next.activeId, closed: next.closed });
}

export function activeTabOf(s: ComposeState): ComposeTab {
  return T.activeTab(s);
}

export const tabsActions = {
  select: (id: string) => setTabs((s) => T.selectTab(s, id)),
  selectRelative: (delta: number) => setTabs((s) => T.selectRelative(s, delta)),
  selectIndex: (position: number) => setTabs((s) => T.selectIndex(s, position)),
  newTab: (draft?: RequestDraft, extra: Partial<ComposeTab> = {}, opts: T.OpenOptions = {}): string => {
    const tab = newTab(draft ?? emptyDraft(), extra);
    setTabs((s) => T.openTab(s, tab, opts));
    return tab.id;
  },
  close: (id: string) => {
    cancelSend(id);
    setTabs((s) => T.closeTab(s, id));
    dropOrphanResponses();
  },
  closeOthers: (id: string) => {
    for (const t of get().tabs) if (t.id !== id) cancelSend(t.id);
    setTabs((s) => T.closeOthers(s, id));
    dropOrphanResponses();
  },
  closeToRight: (id: string) => {
    setTabs((s) => T.closeToRight(s, id));
    dropOrphanResponses();
  },
  reopenClosed: () => setTabs(T.reopenClosed),
  move: (from: number, to: number) => setTabs((s) => T.moveTab(s, from, to)),
  duplicate: (id: string) => setTabs((s) => T.duplicateTab(s, id)),
  patch: (id: string, patch: Partial<ComposeTab> | ((t: ComposeTab) => Partial<ComposeTab>)) => setTabs((s) => T.patchTab(s, id, patch)),
  updateDraft: (id: string, update: (d: RequestDraft) => RequestDraft) => setTabs((s) => T.updateDraft(s, id, update)),
  rename: (id: string, title: string) => {
    const tab = get().tabs.find((t) => t.id === id);
    const clean = title.trim();
    if (!tab) return;
    if (tab.savedId) {
      if (clean) savedActions.rename(tab.savedId, clean);
      return;
    }
    setTabs((s) => T.patchTab(s, id, { title: clean || undefined }));
  },
  /** Open (or focus) a tab for a draft. Reuses a pristine blank active tab. */
  open: (draft: RequestDraft, extra: Partial<ComposeTab> = {}): string => tabsActions.newTab(draft, extra, { replacePristine: true }),
};

function dropOrphanResponses(): void {
  const s = get();
  const ids = new Set(s.tabs.map((t) => t.id));
  for (const c of s.closed) ids.add(c.tab.id);
  const keys = Object.keys(s.responses);
  if (keys.every((k) => ids.has(k))) return;
  const responses: Record<string, ResponseState> = {};
  for (const k of keys) if (ids.has(k)) responses[k] = s.responses[k] as ResponseState;
  set({ responses });
}

// =====================================================================================
// Sending
// =====================================================================================

const controllers = new Map<string, AbortController>();

function setResponse(tabId: string, r: ResponseState | null): void {
  set((s) => {
    const responses = { ...s.responses };
    if (r) responses[tabId] = r;
    else delete responses[tabId];
    return { responses };
  });
}

export function toSendOptions(o: RequestOptions): SendOptions {
  return {
    applyRules: o.applyRules,
    followRedirects: o.followRedirects,
    insecure: o.insecure,
    ...(o.httpVersion !== 'auto' ? { httpVersion: o.httpVersion } : {}),
    ...(o.timeoutMs > 0 ? { timeoutMs: o.timeoutMs } : {}),
  };
}

/** Total duration of a send in ms (from the daemon's timing, else the client clock). */
export function responseDuration(r: ResponseState): number | undefined {
  const t = r.result?.timing;
  const fromTiming = t ? (t.responseBody ?? t.responseHeaders ?? t.aborted) : undefined;
  if (fromTiming != null) return fromTiming;
  return r.finishedAt != null ? r.finishedAt - r.startedAt : undefined;
}

function hostPath(url: string): { host: string; path: string } {
  try {
    const u = new URL(url);
    return { host: u.host, path: `${u.pathname}${u.search}` || '/' };
  } catch {
    return { host: '', path: url };
  }
}

function recordHistory(tab: ComposeTab, url: string, patch: Partial<HistoryEntry>): void {
  const { draft, bodyOmitted } = historyDraft(tab.draft);
  const { host, path } = hostPath(url);
  const entry: HistoryEntry = { id: uid('h'), at: Date.now(), draft, method: (tab.draft.method || 'GET').toUpperCase(), host, path, ...(bodyOmitted ? { bodyOmitted } : {}), ...patch };
  set((s) => ({ history: addHistory(s.history, entry) }));
}

/**
 * The exchange id of a send result if the daemon stored the exchange, i.e. it can be fetched,
 * opened in Traffic or compared.
 */
export function storedExchangeId(result: Pick<SendResponse, 'id' | 'recorded'> | undefined | null): number | undefined {
  return result?.recorded ? result.id : undefined;
}

/** Send a tab's request. Resolves when the response (or failure) is in. */
export async function sendTab(tabId: string): Promise<SendOutcome> {
  const s = get();
  const tab = s.tabs.find((t) => t.id === tabId);
  if (!tab) return { ok: false, unresolved: [] };
  if (controllers.has(tabId)) return { ok: false, unresolved: [] };
  const built = buildRequest(tab.draft, resolveEnvVars(s.env));
  if (!built.url) return { ok: false, unresolved: built.unresolved, invalid: 'empty-url' };
  try {
    const u = new URL(built.url);
    if (!u.hostname) throw new Error('no host');
  } catch {
    return { ok: false, unresolved: built.unresolved, invalid: 'bad-url' };
  }

  const controller = new AbortController();
  controllers.set(tabId, controller);
  const startedAt = Date.now();
  const before = s.responses[tabId];
  const previousId = storedExchangeId(before?.result) ?? before?.previousId ?? tab.lastExchangeId;
  setResponse(tabId, { status: 'sending', startedAt, sent: built.request, url: built.url, previousId });
  try {
    const result = await api.send(built.request, toSendOptions(tab.draft.options), controller.signal);
    if (controllers.get(tabId) !== controller) return { ok: false, unresolved: built.unresolved };
    const finishedAt = Date.now();
    const state: ResponseState = { status: 'done', startedAt, finishedAt, sent: built.request, url: built.url, result, ...(previousId != null && previousId !== result.id ? { previousId } : {}) };
    setResponse(tabId, state);
    // Unstored (recording paused): no id to restore after a reload or to open from history.
    const stored = storedExchangeId(result);
    tabsActions.patch(tabId, { lastExchangeId: stored });
    recordHistory(tab, built.url, { status: result.response?.status, error: result.error?.kind, durationMs: responseDuration(state), ...(stored != null ? { exchangeId: stored } : {}) });
    return { ok: true, unresolved: built.unresolved };
  } catch (e) {
    if (controllers.get(tabId) !== controller) return { ok: false, unresolved: built.unresolved };
    const finishedAt = Date.now();
    if (controller.signal.aborted || (isApiError(e) && e.isAborted)) {
      setResponse(tabId, { status: 'cancelled', startedAt, finishedAt, sent: built.request, url: built.url, previousId });
      recordHistory(tab, built.url, { error: 'Cancelled', durationMs: finishedAt - startedAt });
    } else {
      const error = isApiError(e) ? { message: e.message, code: e.code, status: e.status } : { message: e instanceof Error ? e.message : String(e), code: 'unknown', status: 0 };
      setResponse(tabId, { status: 'error', startedAt, finishedAt, sent: built.request, url: built.url, error, previousId });
      recordHistory(tab, built.url, { error: error.code === 'network' ? 'Network' : 'Failed', durationMs: finishedAt - startedAt });
    }
    return { ok: false, unresolved: built.unresolved };
  } finally {
    if (controllers.get(tabId) === controller) controllers.delete(tabId);
  }
}

export function cancelSend(tabId: string): boolean {
  const c = controllers.get(tabId);
  if (!c) return false;
  c.abort();
  return true;
}

const restoring = new Set<string>();

/** Rebuild a tab's last response from the exchange the daemon recorded (after a reload). */
export async function restoreResponse(tabId: string): Promise<void> {
  const tab = get().tabs.find((t) => t.id === tabId);
  const id = tab?.lastExchangeId;
  if (id == null || get().responses[tabId] || restoring.has(tabId)) return;
  restoring.add(tabId);
  const startedAt = Date.now();
  setResponse(tabId, { status: 'restoring', startedAt });
  try {
    const [detail, body] = await Promise.all([api.exchanges.get(id), api.exchanges.body(id, 'response')]);
    if (get().responses[tabId]?.status !== 'restoring') return;
    const at = detail.timing.start / 1000;
    setResponse(tabId, {
      status: 'done',
      startedAt: at,
      finishedAt: at + (detail.summary.duration ?? 0),
      url: detail.request.url,
      sent: { method: detail.request.method, url: detail.request.url, headers: detail.request.headers, body: '' },
      result: { id, response: detail.response ?? null, error: detail.error ?? null, timing: detail.timing, body, recorded: true },
      restored: true,
    });
  } catch (e) {
    if (get().responses[tabId]?.status !== 'restoring') return;
    setResponse(tabId, null);
    if (isApiError(e) && e.isNotFound) tabsActions.patch(tabId, { lastExchangeId: undefined });
  } finally {
    restoring.delete(tabId);
  }
}

// =====================================================================================
// History actions
// =====================================================================================

export const historyActions = {
  remove: (id: string) => set((s) => ({ history: s.history.filter((h) => h.id !== id) })),
  /** Clear and return the previous list (for Undo). */
  clear: (): HistoryEntry[] => {
    const prev = get().history;
    set({ history: [] });
    return prev;
  },
  restore: (list: HistoryEntry[]) => set({ history: list }),
};

// =====================================================================================
// Saved actions
// =====================================================================================

export const savedActions = {
  set: (saved: SavedNode[]) => set({ saved }),
  /** Save a tab as a new saved request and link the tab to it. */
  saveTabAs: (tabId: string, name: string, folderId: string | null): SavedRequest | null => {
    const tab = get().tabs.find((t) => t.id === tabId);
    if (!tab) return null;
    const req: SavedRequest = { type: 'request', id: uid('s'), name: name.trim() || 'Untitled request', draft: tab.draft, updatedAt: Date.now() };
    set((s) => ({ saved: addSaved(s.saved, req, folderId) }));
    tabsActions.patch(tabId, { savedId: req.id, baseline: draftHash(tab.draft), title: undefined });
    return req;
  },
  /** Update the saved request linked to a tab. Returns false when the tab isn't linked. */
  saveTab: (tabId: string): boolean => {
    const tab = get().tabs.find((t) => t.id === tabId);
    if (!tab?.savedId || !findSaved(get().saved, tab.savedId)) return false;
    set((s) => ({ saved: updateSaved(s.saved, tab.savedId as string, { draft: tab.draft, updatedAt: Date.now() }) }));
    tabsActions.patch(tabId, { baseline: draftHash(tab.draft) });
    return true;
  },
  rename: (id: string, name: string) => set((s) => ({ saved: renameNode(s.saved, id, name) })),
  addFolder: (folder: SavedFolder) => set((s) => ({ saved: addFolder(s.saved, folder) })),
  /** Remove a node; returns the previous tree (for Undo). Linked tabs are unlinked. */
  remove: (id: string): SavedNode[] => {
    const prev = get().saved;
    set((s) => ({ saved: removeNode(s.saved, id) }));
    return prev;
  },
};

/** Open a saved request (focus the tab already showing it). */
export function openSaved(id: string): void {
  const s = get();
  const existing = s.tabs.find((t) => t.savedId === id);
  if (existing) {
    tabsActions.select(existing.id);
    return;
  }
  const req = findSaved(s.saved, id);
  if (!req) return;
  tabsActions.open(req.draft, { savedId: id });
}

/** Open a history entry in a tab. */
export function openHistory(entry: HistoryEntry): void {
  tabsActions.open(entry.draft, entry.exchangeId != null ? { lastExchangeId: entry.exchangeId } : {});
}

// =====================================================================================
// Environment actions
// =====================================================================================

export const envActions = {
  set: (env: EnvState) => set({ env }),
  setActive: (activeId: string | null) => set((s) => ({ env: { ...s.env, activeId } })),
};
