/**
 * Compose (DESIGN.md §8.1): browser-like request tabs, history & saved requests, environments
 * with `{{variables}}`, the response viewer shared with Traffic, and imports — cURL / fetch() /
 * PowerShell pasted into the URL bar (one tab each), HAR / Postman / OpenAPI files into Saved.
 */

import {
  ArrowUpRight,
  Bookmark,
  ClipboardPaste,
  Copy,
  Eraser,
  FileInput,
  History,
  Layers,
  Link2,
  PanelBottom,
  PanelLeft,
  Plus,
  Repeat,
  RotateCcw,
  Save,
  SaveAll,
  Send,
  Settings2,
  SquareX,
  WandSparkles,
  X,
} from 'lucide-react';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, errorMessage } from '../../api/client';
import { useCommands, type Command } from '../../app/commands';
import { navigate } from '../../app/router';
import { useShortcut, useShortcutDocs } from '../../app/shortcuts';
import { SplitPane, toast, undoToast } from '../../components/ui';
import { copyText, isDesktop } from '../../lib/platform';
import { takeComposeQueue, useIntents } from '../../state/intents';
import { usePagePref } from '../../state/ui';
import { ImportDialog, SaveDialog } from './Dialogs';
import { EnvDialog, EnvMenu } from './Environments';
import { useElementWidth, useStableCallback, useVars } from './hooks';
import { composeKeys, suggestedRequestName } from './labels';
import { collectionSize, type ImportedCollection } from './importers';
import { addSaved, collectionFolders, findSaved, folders, uniqueName } from './library';
import { isDirty, uid, type RequestDraft, type RequestOptions, type SavedRequest } from './model';
import { detectPaste, fetchToEditable, pasteKindLabel, powershellToEditable, splitCurlCommands, splitFetchCalls, splitPowerShellCommands } from './paste';
import { buildRequest, draftFromEditable, formatJson } from './request';
import { RequestEditor } from './RequestEditor';
import { RequestLine } from './RequestLine';
import { ResponsePane } from './ResponsePane';
import { Sidebar, SIDEBAR_DEFAULT, SIDEBAR_MAX, SIDEBAR_MIN, type SidebarSection } from './Sidebar';
import { activeTabOf, cancelSend, envActions, flushCompose, historyActions, restoreResponse, savedActions, sendTab, tabsActions, toSendOptions, useComposeStore } from './store';
import { COMPOSE_PANEL_ID, TabBar, tabTitle } from './TabBar';
import { mergeParamsFromUrl } from './urlParams';
import { referencedVars, resolveEnvVars } from './variables';
import { openRepeatDialog, RepeatHost } from '../../components/repeat/repeat';
import styles from './ComposePage.module.css';

type LayoutPref = 'auto' | 'right' | 'bottom';
type SaveTarget = { kind: 'tab'; tabId: string } | { kind: 'draft'; draft: RequestDraft; name: string };

/** Wide enough (px of the main column) to put the response on the right in `auto` layout. */
const AUTO_RIGHT_MIN = 1300;

/** Advertised tab shortcuts (browsers reserve Ctrl/⌘ T and W). */
const KEYS = composeKeys(isDesktop());

function describeDraft(d: RequestDraft): string {
  return `${d.method} ${d.url.replace(/^[a-z]+:\/\//i, '').split('?')[0]}`;
}

function activeResponseStatus() {
  const s = useComposeStore.getState();
  return s.responses[s.activeId]?.status;
}

export function ComposePage() {
  const tabs = useComposeStore((s) => s.tabs);
  const activeId = useComposeStore((s) => s.activeId);
  const tab = useComposeStore(activeTabOf);
  const saved = useComposeStore((s) => s.saved);
  const responses = useComposeStore((s) => s.responses);
  const canReopen = useComposeStore((s) => s.closed.length > 0);
  const env = useComposeStore((s) => s.env);
  const vars = useVars();
  const response = responses[tab.id];
  const sending = response?.status === 'sending';

  // usePagePref returns a fresh setter each render; stable wrappers keep memoised children
  // (Sidebar, ResponsePane) from re-rendering on every keystroke.
  const [sidebarOpen, setSidebarOpenRaw] = usePagePref<boolean>('compose.sidebarOpen', true);
  const setSidebarOpen = useStableCallback(setSidebarOpenRaw);
  const [sidebarWidthRaw, setSidebarWidthRaw] = usePagePref<number>('compose.sidebarWidth', SIDEBAR_DEFAULT);
  const setSidebarWidth = useStableCallback(setSidebarWidthRaw);
  const sidebarWidth = typeof sidebarWidthRaw === 'number' ? Math.max(SIDEBAR_MIN, Math.min(SIDEBAR_MAX, sidebarWidthRaw)) : SIDEBAR_DEFAULT;
  const [sectionRaw, setSectionRaw] = usePagePref<SidebarSection>('compose.sidebarSection', 'history');
  const setSection = useStableCallback(setSectionRaw);
  const section: SidebarSection = sectionRaw === 'saved' ? 'saved' : 'history';
  const [layoutRaw, setLayoutPref] = usePagePref<LayoutPref>('compose.responseLayout', 'auto');
  const mainRef = useRef<HTMLDivElement | null>(null);
  const mainWidth = useElementWidth(mainRef);
  const layout: 'right' | 'bottom' = layoutRaw === 'right' || layoutRaw === 'bottom' ? layoutRaw : mainWidth >= AUTO_RIGHT_MIN ? 'right' : 'bottom';

  const [urlInvalid, setUrlInvalid] = useState(false);
  const [envDialog, setEnvDialog] = useState(false);
  const [importDialog, setImportDialog] = useState<{ open: boolean; text: string }>({ open: false, text: '' });
  const [saveTarget, setSaveTarget] = useState<SaveTarget | null>(null);
  const urlRef = useRef<HTMLInputElement | null>(null);

  const activeEnv = env.envs.find((e) => e.id === env.activeId) ?? null;
  const sendingKey = Object.entries(responses)
    .filter(([, r]) => r.status === 'sending')
    .map(([id]) => id)
    .join('\n');
  const sendingIds = useMemo(() => new Set(sendingKey ? sendingKey.split('\n') : []), [sendingKey]);

  // ---------------------------------------------------------------- lifecycle
  useEffect(() => () => flushCompose(), []);

  // Traffic → "Edit in Compose": drain queued requests into new tabs.
  useEffect(() => {
    const drain = () => {
      for (const intent of takeComposeQueue()) {
        tabsActions.newTab(draftFromEditable(intent.request), { title: intent.title, sourceExchangeId: intent.sourceExchangeId }, { replacePristine: true });
      }
    };
    drain();
    return useIntents.subscribe((s, prev) => {
      if (s.composeQueue !== prev.composeQueue && s.composeQueue.length) drain();
    });
  }, []);

  // After a reload, rebuild the active tab's last response from the recorded exchange.
  useEffect(() => {
    if (tab.lastExchangeId != null && !response) void restoreResponse(tab.id);
  }, [tab.id, tab.lastExchangeId, response]);

  useEffect(() => setUrlInvalid(false), [tab.id]);

  // ---------------------------------------------------------------- actions
  const focusUrl = useCallback(() => {
    requestAnimationFrame(() => {
      urlRef.current?.focus();
      urlRef.current?.select();
    });
  }, []);

  const send = useCallback(
    async (tabId?: string) => {
      const id = tabId ?? useComposeStore.getState().activeId;
      const out = await sendTab(id);
      if (out.invalid) {
        setUrlInvalid(true);
        focusUrl();
        const tab = useComposeStore.getState().tabs.find((t) => t.id === id);
        const missing = tab ? out.unresolved.filter((n) => referencedVars(tab.draft.url).includes(n)) : [];
        if (out.invalid === 'bad-url' && missing.length) {
          toast.error(missing.length === 1 ? `{{${missing[0]}}} isn't defined` : `${missing.length} URL variables aren't defined`, {
            description: `The URL can't be built until ${missing.map((n) => `{{${n}}}`).join(', ')} ${missing.length === 1 ? 'has' : 'have'} a value in the active environment or globals.`,
            action: { label: 'Define', onClick: () => setEnvDialog(true) },
          });
          return;
        }
        toast.error(out.invalid === 'empty-url' ? 'Enter a URL to send' : "That URL isn't valid", {
          description: out.invalid === 'bad-url' ? 'Use a full URL like https://api.example.com/path — variables are resolved first.' : undefined,
          duration: 3000,
        });
        return;
      }
      if (out.unresolved.length) {
        toast.warning(`Sent with ${out.unresolved.length} undefined variable${out.unresolved.length === 1 ? '' : 's'}`, {
          description: out.unresolved.map((n) => `{{${n}}}`).join(', '),
          action: { label: 'Define', onClick: () => setEnvDialog(true) },
        });
      }
    },
    [focusUrl],
  );

  const cancel = useCallback(() => {
    cancelSend(useComposeStore.getState().activeId);
  }, []);

  /** "Repeat…": the active tab's request, built like Send builds it, sent N times. */
  const repeat = useCallback(() => {
    const s = useComposeStore.getState();
    const t = activeTabOf(s);
    const built = buildRequest(t.draft, resolveEnvVars(s.env));
    let valid = false;
    try {
      valid = !!new URL(built.url).hostname;
    } catch {
      valid = false;
    }
    if (!valid) {
      setUrlInvalid(true);
      focusUrl();
      toast.error(built.url ? "That URL isn't valid" : 'Enter a URL to repeat', { duration: 3000 });
      return;
    }
    openRepeatDialog({ kind: 'request', request: built.request, options: toSendOptions(t.draft.options), label: `${built.request.method} ${built.url}` });
  }, [focusUrl]);

  const retryWith = useCallback(
    (patch: Partial<RequestOptions>) => {
      const id = useComposeStore.getState().activeId;
      tabsActions.updateDraft(id, (d) => ({ ...d, options: { ...d.options, ...patch } }));
      void send(id);
    },
    [send],
  );

  const newTab = useCallback(() => {
    tabsActions.newTab(undefined, {}, { afterActive: true });
    focusUrl();
  }, [focusUrl]);

  const closeTab = useCallback((id: string) => {
    const s = useComposeStore.getState();
    const t = s.tabs.find((x) => x.id === id);
    if (!t) return;
    const title = tabTitle(t, s.saved);
    const dirty = isDirty(t) && !!(t.draft.url || t.savedId);
    tabsActions.close(id);
    if (dirty) undoToast(`Closed “${title || 'New request'}” with unsaved changes`, () => tabsActions.reopenClosed());
  }, []);

  const save = useCallback((tabId?: string, forceDialog = false) => {
    const s = useComposeStore.getState();
    const t = s.tabs.find((x) => x.id === (tabId ?? s.activeId));
    if (!t) return;
    if (!forceDialog && t.savedId && findSaved(s.saved, t.savedId)) {
      if (savedActions.saveTab(t.id)) toast.success(`Saved “${findSaved(useComposeStore.getState().saved, t.savedId)?.name ?? 'request'}”`, { duration: 1600 });
      return;
    }
    setSaveTarget({ kind: 'tab', tabId: t.id });
  }, []);

  const onSaveDialog = (name: string, folderId: string | null, newFolderName?: string) => {
    let folder = folderId;
    if (newFolderName) {
      const id = uid('f');
      savedActions.addFolder({ type: 'folder', id, name: uniqueName(newFolderName, folders(useComposeStore.getState().saved).map((f) => f.name)), collapsed: false, children: [] });
      folder = id;
    }
    if (!saveTarget) return;
    const reveal = { label: 'Show', onClick: () => showSection('saved') };
    if (saveTarget.kind === 'tab') {
      const req = savedActions.saveTabAs(saveTarget.tabId, name, folder);
      if (req) toast.success(`Saved “${req.name}”`, { action: reveal });
    } else {
      const req: SavedRequest = { type: 'request', id: uid('s'), name, draft: saveTarget.draft, updatedAt: Date.now() };
      savedActions.set(addSaved(useComposeStore.getState().saved, req, folder));
      toast.success(`Saved “${name}”`, { action: reveal });
    }
  };

  /**
   * Import cURL / fetch() / PowerShell text (one tab per command). With `replaceTabId`, the first
   * command replaces that tab's request (undoable, which also closes the extra tabs).
   */
  const importText = useCallback(async (text: string, replaceTabId?: string): Promise<boolean> => {
    const kind = detectPaste(text);
    if (!kind) return false;
    let drafts: RequestDraft[] = [];
    try {
      if (kind === 'curl') drafts = (await Promise.all(splitCurlCommands(text).map((c) => api.importCurl(c)))).map(draftFromEditable);
      else if (kind === 'fetch') drafts = splitFetchCalls(text).map((c) => draftFromEditable(fetchToEditable(c)));
      else drafts = splitPowerShellCommands(text).map((c) => draftFromEditable(powershellToEditable(c)));
    } catch (e) {
      toast.error(`Couldn't import the ${pasteKindLabel(kind)}`, { description: errorMessage(e) });
      return false;
    }
    const [first, ...rest] = drafts;
    if (!first) return false;
    const label = pasteKindLabel(kind, drafts.length);
    const target = replaceTabId ? useComposeStore.getState().tabs.find((t) => t.id === replaceTabId) : undefined;
    if (target) {
      const previous = target.draft;
      tabsActions.updateDraft(target.id, () => first);
      if (first.body.mode !== 'none') tabsActions.patch(target.id, { requestTab: 'body' });
      const extra = rest.map((d) => tabsActions.newTab(d, {}, { background: true }));
      undoToast(
        `Imported ${rest.length ? `${drafts.length} ` : ''}${label}`,
        () => {
          tabsActions.updateDraft(target.id, () => previous);
          for (const id of extra) tabsActions.close(id);
        },
        { description: rest.length ? `${describeDraft(first)} · ${rest.length} more in new tabs` : describeDraft(first) },
      );
    } else {
      for (const d of drafts) tabsActions.newTab(d, {}, { replacePristine: true });
      toast.success(drafts.length > 1 ? `Opened ${drafts.length} requests` : `Imported ${label}`, { description: describeDraft(first) });
    }
    return true;
  }, []);

  const importCollectionToSaved = useCallback((c: ImportedCollection) => {
    const s = useComposeStore.getState();
    const prevSaved = s.saved;
    const prevEnv = s.env;
    const added = collectionFolders(c, s.saved, uid);
    savedActions.set([...s.saved, ...added]);
    let envName: string | null = null;
    if (c.variables.length) {
      const id = uid('e');
      envName = uniqueName(c.name, s.env.envs.map((e) => e.name));
      envActions.set({ ...s.env, envs: [...s.env.envs, { id, name: envName, vars: c.variables }], activeId: s.env.activeId ?? id });
    }
    setSidebarOpen(true);
    setSection('saved');
    const n = collectionSize(c);
    const parts = [`${added.length} folder${added.length === 1 ? '' : 's'}`];
    if (envName) parts.push(`variables in environment “${envName}”`);
    if (c.skipped.length) parts.push(`${c.skipped.length} skipped`);
    undoToast(
      `Imported ${n} request${n === 1 ? '' : 's'} from ${c.name}`,
      () => {
        savedActions.set(prevSaved);
        envActions.set(prevEnv);
      },
      { description: parts.join(' · ') },
    );
  }, [setSidebarOpen, setSection]);

  const onImportText = useCallback(
    (text: string): boolean => {
      if (!detectPaste(text)) return false;
      void importText(text, useComposeStore.getState().activeId);
      return true;
    },
    [importText],
  );

  const pasteFromClipboard = useCallback(async () => {
    let text = '';
    try {
      text = (await navigator.clipboard?.readText?.()) ?? '';
    } catch {
      text = '';
    }
    if (text && detectPaste(text)) await importText(text);
    else setImportDialog({ open: true, text: '' });
  }, [importText]);

  const copyAsCurl = useCallback(async () => {
    const t = activeTabOf(useComposeStore.getState());
    try {
      const snippet = await api.codegen.generate(buildRequest(t.draft, vars).request, 'curl');
      if (await copyText(snippet.code)) toast.success('Copied as cURL');
    } catch (e) {
      toast.error("Couldn't generate the cURL command", { description: errorMessage(e) });
    }
  }, [vars]);

  const formatBody = useCallback(() => {
    const t = activeTabOf(useComposeStore.getState());
    if (t.draft.body.mode !== 'json') return;
    const pretty = formatJson(t.draft.body.json);
    if (pretty == null) toast.error('The JSON body has errors — fix them before formatting');
    else tabsActions.updateDraft(t.id, (d) => ({ ...d, body: { ...d.body, json: pretty } }));
  }, []);

  const toggleLayout = useStableCallback(() => setLayoutPref(layout === 'right' ? 'bottom' : 'right'));
  const toggleSidebar = useStableCallback(() => setSidebarOpen(!sidebarOpen));
  const showSection = (s: SidebarSection) => {
    setSidebarOpen(true);
    setSection(s);
  };

  const clearHistory = () => {
    const prev = historyActions.clear();
    if (prev.length) undoToast(`Cleared ${prev.length} request${prev.length === 1 ? '' : 's'} from history`, () => historyActions.restore(prev));
  };

  // ---------------------------------------------------------------- request line handlers (stable per tab)
  const tabId = tab.id;
  const onMethodChange = useCallback((method: string) => tabsActions.updateDraft(tabId, (d) => ({ ...d, method })), [tabId]);
  const onUrlChange = useCallback(
    (url: string) => {
      setUrlInvalid(false);
      tabsActions.updateDraft(tabId, (d) => ({ ...d, url, params: mergeParamsFromUrl(d.params, url) }));
    },
    [tabId],
  );
  const onOptionsChange = useCallback((options: RequestOptions) => tabsActions.updateDraft(tabId, (d) => ({ ...d, options })), [tabId]);
  const sendActive = useCallback(() => void send(), [send]);
  const saveActive = useCallback(() => save(), [save]);
  const openEnvDialog = useCallback(() => setEnvDialog(true), []);
  const saveDraft = useCallback((draft: RequestDraft, name: string) => setSaveTarget({ kind: 'draft', draft, name }), []);

  // ---------------------------------------------------------------- shortcuts
  const group = 'Compose';
  useShortcut(
    'mod+enter',
    () => {
      void send();
    },
    { allowInInputs: true, description: 'Send request', group },
  );
  useShortcut('escape', cancel, { allowInInputs: true, enabled: sending, description: 'Cancel request', group });
  const closeActive = useCallback(() => closeTab(useComposeStore.getState().activeId), [closeTab]);
  const reopen = useCallback(() => tabsActions.reopenClosed(), []);
  // Both variants are bound; only the advertised one is documented.
  useShortcut(KEYS.newTab, newTab, { allowInInputs: true, description: 'New request tab', group });
  useShortcut(KEYS.closeTab, closeActive, { allowInInputs: true, description: 'Close tab', group });
  useShortcut(KEYS.reopenTab, reopen, { allowInInputs: true, description: 'Reopen closed tab', group });
  useShortcut(KEYS.newTab === 'mod+t' ? 'alt+t' : 'mod+t', newTab, { allowInInputs: true });
  useShortcut(KEYS.closeTab === 'mod+w' ? 'alt+w' : 'mod+w', closeActive, { allowInInputs: true });
  useShortcut(KEYS.reopenTab === 'mod+shift+t' ? 'alt+shift+t' : 'mod+shift+t', reopen, { allowInInputs: true });
  useShortcut('mod+s', () => save(), { allowInInputs: true, description: 'Save request', group });
  useShortcut('mod+shift+s', () => save(undefined, true), { allowInInputs: true, description: 'Save as…', group });
  useShortcut('mod+l', focusUrl, { allowInInputs: true, description: 'Focus the URL', group });
  useShortcut('mod+b', toggleSidebar, { allowInInputs: true, description: 'Toggle sidebar', group });
  useShortcut('mod+shift+d', toggleLayout, { allowInInputs: true, description: 'Response below / right', group });
  useShortcut('ctrl+tab', () => tabsActions.selectRelative(1), { allowInInputs: true, description: 'Next tab', group });
  useShortcut('ctrl+shift+tab', () => tabsActions.selectRelative(-1), { allowInInputs: true, description: 'Previous tab', group });
  useShortcut('mod+alt+arrowright', () => tabsActions.selectRelative(1), { allowInInputs: true });
  useShortcut('mod+alt+arrowleft', () => tabsActions.selectRelative(-1), { allowInInputs: true });
  useShortcut('ctrl+pagedown', () => tabsActions.selectRelative(1), { allowInInputs: true });
  useShortcut('ctrl+pageup', () => tabsActions.selectRelative(-1), { allowInInputs: true });
  useShortcut('shift+alt+f', formatBody, { allowInInputs: true, enabled: tab.draft.body.mode === 'json', description: 'Format JSON body', group });
  useShortcutDocs([
    { combo: 'mod+alt+arrowright or ctrl+pagedown', description: 'Next tab', group },
    { combo: 'mod+alt+arrowleft or ctrl+pageup', description: 'Previous tab', group },
    { combo: 'arrowleft or arrowright', description: 'Move between tabs (tab strip focused)', group },
    { combo: 'mod+shift+arrowleft or mod+shift+arrowright', description: 'Reorder tab (tab strip focused)', group },
    { combo: 'f2', description: 'Rename tab / saved request', group },
    { combo: 'alt+arrowup or alt+arrowdown', description: 'Move saved request up / down', group },
  ]);

  // ---------------------------------------------------------------- palette
  const result = response?.status === 'done' ? response.result : undefined;
  useCommands([
    { id: 'compose.new', title: 'New request', group, icon: Plus, shortcut: KEYS.newTab, keywords: ['tab', 'compose'], run: newTab },
    { id: 'compose.paste-curl', title: 'Paste cURL from clipboard', subtitle: 'fetch() and PowerShell work too', group, icon: ClipboardPaste, keywords: ['import', 'curl', 'fetch', 'powershell'], run: pasteFromClipboard },
    { id: 'compose.import', title: 'Import requests…', subtitle: 'cURL, fetch(), HAR, Postman', group, icon: FileInput, keywords: ['paste', 'curl', 'har', 'postman', 'collection'], run: () => setImportDialog({ open: true, text: '' }) },
    { id: 'compose.send', title: 'Send request', group, icon: Send, shortcut: 'mod+enter', disabled: sending, run: () => void send() },
    { id: 'compose.repeat', title: 'Repeat request…', subtitle: 'Send it many times, several at once', group, icon: Repeat, keywords: ['load', 'benchmark', 'concurrency', 'stress', 'n times'], run: repeat },
    { id: 'compose.cancel', title: 'Cancel request', group, icon: X, shortcut: 'escape', when: () => activeResponseStatus() === 'sending', run: cancel },
    { id: 'compose.save', title: tab.savedId ? 'Save request' : 'Save request…', group, icon: Save, shortcut: 'mod+s', run: () => save() },
    { id: 'compose.save-as', title: 'Save request as…', group, icon: SaveAll, shortcut: 'mod+shift+s', run: () => save(undefined, true) },
    { id: 'compose.close-tab', title: 'Close tab', group, icon: SquareX, shortcut: KEYS.closeTab, run: closeActive },
    { id: 'compose.reopen-tab', title: 'Reopen closed tab', group, icon: RotateCcw, shortcut: KEYS.reopenTab, disabled: !canReopen, run: reopen },
    { id: 'compose.duplicate-tab', title: 'Duplicate tab', group, icon: Copy, run: () => tabsActions.duplicate(useComposeStore.getState().activeId) },
    { id: 'compose.copy-curl', title: 'Copy request as cURL', group, icon: Copy, keywords: ['snippet', 'code'], run: copyAsCurl },
    { id: 'compose.focus-url', title: 'Focus URL', group, icon: Link2, shortcut: 'mod+l', run: focusUrl },
    { id: 'compose.toggle-sidebar', title: sidebarOpen ? 'Hide sidebar' : 'Show sidebar', group, icon: PanelLeft, shortcut: 'mod+b', run: toggleSidebar },
    { id: 'compose.show-history', title: 'Show request history', group, icon: History, run: () => showSection('history') },
    { id: 'compose.show-saved', title: 'Show saved requests', group, icon: Bookmark, run: () => showSection('saved') },
    { id: 'compose.toggle-layout', title: layout === 'right' ? 'Show response below' : 'Show response on the right', group, icon: PanelBottom, shortcut: 'mod+shift+d', run: toggleLayout },
    { id: 'compose.environments', title: 'Manage environments…', group, icon: Settings2, keywords: ['variables', 'env'], run: () => setEnvDialog(true) },
    { id: 'compose.env.none', title: 'Use no environment', group, icon: Layers, disabled: env.activeId === null, keywords: ['variables'], run: () => envActions.setActive(null) },
    ...env.envs.map<Command>((e) => ({ id: `compose.env.${e.id}`, title: `Use environment: ${e.name}`, group, icon: Layers, disabled: env.activeId === e.id, keywords: ['variables'], run: () => envActions.setActive(e.id) })),
    { id: 'compose.format-json', title: 'Format JSON body', group, icon: WandSparkles, shortcut: 'shift+alt+f', when: () => activeTabOf(useComposeStore.getState()).draft.body.mode === 'json', run: formatBody },
    { id: 'compose.open-traffic', title: 'Open response in Traffic', group, icon: ArrowUpRight, disabled: result == null, run: () => result && navigate(`/traffic/${result.id}`) },
    { id: 'compose.clear-history', title: 'Clear request history', group, icon: Eraser, run: clearHistory },
  ]);

  // ---------------------------------------------------------------- render
  const saveDialogTab = saveTarget?.kind === 'tab' ? tabs.find((t) => t.id === saveTarget.tabId) : undefined;
  const saveDialogName = saveTarget?.kind === 'draft' ? saveTarget.name : saveDialogTab ? saveDialogTab.title || suggestedRequestName(saveDialogTab.draft.url) : '';

  return (
    <div className={styles.page}>
      <SplitPane
        size={sidebarWidth}
        onSizeChange={setSidebarWidth}
        minFirst={SIDEBAR_MIN}
        maxFirst={SIDEBAR_MAX}
        minSecond={420}
        defaultSize={SIDEBAR_DEFAULT}
        collapsed={sidebarOpen ? null : 'first'}
        dividerLabel="Resize sidebar"
      >
        {sidebarOpen && <Sidebar section={section} onSectionChange={setSection} onSaveDraft={saveDraft} onSaveCurrent={saveActive} />}
        <div ref={mainRef} className={styles.main}>
          <TabBar
            tabs={tabs}
            activeId={activeId}
            saved={saved}
            sendingIds={sendingIds}
            canReopen={canReopen}
            sidebarOpen={sidebarOpen}
            onToggleSidebar={toggleSidebar}
            onSelect={tabsActions.select}
            onClose={closeTab}
            onNew={newTab}
            onMove={tabsActions.move}
            onDuplicate={tabsActions.duplicate}
            onCloseOthers={tabsActions.closeOthers}
            onCloseToRight={tabsActions.closeToRight}
            onReopen={tabsActions.reopenClosed}
            onRename={tabsActions.rename}
            onSave={save}
            trailing={<EnvMenu onManage={openEnvDialog} />}
            keys={KEYS}
          />
          <div className={styles.panelArea} role="tabpanel" id={COMPOSE_PANEL_ID} aria-labelledby={`compose-tab-${tab.id}`}>
            <SplitPane
              key={layout}
              id={layout === 'right' ? 'compose.split.right' : 'compose.split.bottom'}
              direction={layout === 'right' ? 'horizontal' : 'vertical'}
              defaultRatio={layout === 'right' ? 0.5 : 0.52}
              minFirst={layout === 'right' ? 420 : 220}
              minSecond={layout === 'right' ? 320 : 160}
              dividerLabel="Resize request and response"
              className={styles.split}
            >
              <div className={styles.requestPane} role="region" aria-label="Request">
                <RequestLine
                  draft={tab.draft}
                  vars={vars}
                  envName={activeEnv?.name ?? null}
                  sending={sending}
                  urlInvalid={urlInvalid}
                  onMethodChange={onMethodChange}
                  onUrlChange={onUrlChange}
                  onOptionsChange={onOptionsChange}
                  onSend={sendActive}
                  onRepeat={repeat}
                  onCancel={cancel}
                  onImportText={onImportText}
                  onManageEnvironments={openEnvDialog}
                  urlRef={urlRef}
                />
                <RequestEditor tab={tab} vars={vars} onSubmit={sendActive} onSave={saveActive} />
              </div>
              <ResponsePane
                tabId={tab.id}
                responseTab={tab.responseTab}
                lastExchangeId={tab.lastExchangeId}
                response={response}
                layout={layout}
                onToggleLayout={toggleLayout}
                onSend={sendActive}
                onCancel={cancel}
                onRetryWith={retryWith}
                onFocusUrl={focusUrl}
              />
            </SplitPane>
          </div>
        </div>
      </SplitPane>
      <EnvDialog open={envDialog} onOpenChange={setEnvDialog} initialId={env.activeId} />
      <RepeatHost />
      <ImportDialog open={importDialog.open} initialText={importDialog.text} onOpenChange={(open) => setImportDialog((d) => ({ ...d, open }))} onImport={(text) => importText(text)} onImportCollection={importCollectionToSaved} />
      <SaveDialog
        open={saveTarget != null}
        onOpenChange={(open) => !open && setSaveTarget(null)}
        title={saveTarget?.kind === 'draft' ? 'Save to collection' : saveDialogTab?.savedId ? 'Save as new request' : 'Save request'}
        initialName={saveDialogName}
        folders={folders(saved)}
        onSave={onSaveDialog}
      />
    </div>
  );
}

export default ComposePage;
