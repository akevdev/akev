import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, errorMessage } from '../../api/client';
import type { RuleId } from '../../api/types';
import { useRouteParam } from '../../app/router';
import { Skeleton, SplitPane, toast } from '../../components/ui';
import { ruleMatchesQuery } from '../../lib/rules/stats';
import { templateById, type TemplateId } from '../../lib/rules/templates';
import { describePosition, findItem, flatten, walkRules, type DropPosition, type GroupNode, type RuleNode } from '../../lib/rules/tree';
import { appReducers } from '../../state/app';
import { usePagePref } from '../../state/ui';
import type { RuleEditorHandle } from './editor/RuleEditor';
import { useCollapsedGroups } from './hooks/useCollapsedGroups';
import { useDraftStashes } from './hooks/useDraftStashes';
import { useRuleActions } from './hooks/useRuleActions';
import { useRuleNavigation } from './hooks/useRuleNavigation';
import { useRulesCommands } from './hooks/useRulesCommands';
import { useRuleTransfer } from './hooks/useRuleTransfer';
import { ImportDialog } from './ImportDialog';
import { shortLabel } from './itemLabels';
import { itemMenu } from './list/itemMenu';
import type { RuleTreeHandle } from './list/RuleTree';
import type { RowActions } from './list/Rows';
import { RuleEditorPane } from './RuleEditorPane';
import { RuleListPane } from './RuleListPane';
import { RulesToolbar } from './RulesToolbar';
import { TemplateGallery } from './Templates';
import { useRulesModel } from './useRulesModel';
import styles from './RulesPage.module.css';

const LIST_MIN = 360;
const LIST_DEFAULT = 420;
const LIST_MAX = 760;
const EDITOR_MIN = 480;

/**
 * Rules page (DESIGN.md §7): master–detail rule list + live editor. State and behaviour live
 * in hooks (`hooks/`): the data model, collapsed groups, navigation (selection, reveal,
 * scrolling), the undoable actions, import / export, and the page's
 * shortcuts and commands. This component wires them to the toolbar, list and editor panes.
 */
export function RulesPage() {
  const model = useRulesModel();
  const { rules, loaded, getRules, history } = model;
  const routeId = useRouteParam('id');
  const selectedItem = routeId ? findItem(rules, routeId) : undefined;
  const selectedId = selectedItem ? (routeId as string) : null;

  const [listWidthPref, setListWidthPref] = usePagePref<number>('rules.listWidth', LIST_DEFAULT);
  const listWidth = typeof listWidthPref === 'number' ? listWidthPref : LIST_DEFAULT;
  const [lastTemplatePref, setLastTemplate] = usePagePref<string>('rules.lastTemplate', 'mock');
  const lastTemplate: TemplateId = (templateById(lastTemplatePref)?.id ?? 'mock') as TemplateId;

  const [query, setQuery] = useState('');
  const clearSearch = useCallback(() => setQuery(''), []);
  const [renamingId, setRenamingId] = useState<RuleId | null>(null);
  const [templateMenuOpen, setTemplateMenuOpen] = useState(false);
  const [testRequest, setTestRequest] = useState<{ id: RuleId; nonce: number } | null>(null);
  const editorRef = useRef<RuleEditorHandle | null>(null);
  const treeRef = useRef<RuleTreeHandle | null>(null);
  const searchRef = useRef<HTMLInputElement | null>(null);

  const groups = useCollapsedGroups(rules, getRules);
  const { stashes, invalidDrafts, onStash, drop: dropStash } = useDraftStashes(rules);
  // Hit counters and match limits live in Akev, shared by every UI and the CLI/MCP.
  const resetStats = useCallback(async () => {
    try {
      appReducers.ruleStats(await api.rules.resetStats());
      toast.success('Hit counters reset');
    } catch (e) {
      toast.error("Couldn't reset the hit counters", { description: errorMessage(e) });
    }
  }, []);

  // ------------------------------------------------------------------ derived
  const walked = useMemo(() => walkRules(rules), [rules]);
  const walkedById = useMemo(() => new Map(walked.map((w) => [w.rule.id, w])), [walked]);
  const totalRules = walked.length;
  const activeRules = useMemo(() => walked.filter((w) => w.effectiveEnabled).length, [walked]);
  const searching = query.trim().length > 0;
  const rows = useMemo(
    () => flatten(rules, groups.collapsed, searching ? { rule: (r) => ruleMatchesQuery(r, query), group: (g) => g.title.toLowerCase().includes(query.trim().toLowerCase()) } : undefined),
    [rules, groups.collapsed, searching, query],
  );
  const matchingRules = useMemo(() => (searching ? rows.filter((r) => r.kind === 'rule').length : 0), [rows, searching]);

  const nav = useRuleNavigation({ loaded, rules, rows, selectedId, getRules, query, setQuery, expandTo: groups.expandTo, treeRef, editorRef });
  const { select, reveal, announce } = nav;

  const actions = useRuleActions({
    model,
    nav,
    groups,
    rows,
    selectedId,
    searching,
    clearSearch,
    onTemplateUsed: setLastTemplate,
    onRemoved: dropStash,
    setRenamingId,
    editorRef,
    treeRef,
  });
  const transfer = useRuleTransfer({ model, mutate: actions.mutate, reveal, editorRef });

  // Forget a test request once the user moves on.
  useEffect(() => {
    if (testRequest && testRequest.id !== selectedId) setTestRequest(null);
  }, [selectedId, testRequest]);

  const testRule = useCallback(
    (id: RuleId) => {
      reveal(id);
      setTestRequest((t) => ({ id, nonce: (t?.nonce ?? 0) + 1 }));
    },
    [reveal],
  );

  const menuFor = useCallback(
    (id: RuleId) => itemMenu(id, { rules: getRules(), collapsed: groups.collapsed, lastTemplate, actions, toggleCollapse: groups.toggleCollapse, setRenamingId, reveal, testRule }),
    [actions, getRules, groups.collapsed, groups.toggleCollapse, lastTemplate, reveal, testRule],
  );

  // Rows get one stable actions object whose methods always call the latest closures.
  const latest = useRef({ select, reveal, toggle: actions.toggle, toggleCollapse: groups.toggleCollapse, rename: actions.rename, setRenamingId, menuFor });
  latest.current = { select, reveal, toggle: actions.toggle, toggleCollapse: groups.toggleCollapse, rename: actions.rename, setRenamingId, menuFor };
  const rowActions = useMemo<RowActions>(
    () => ({
      select: (id) => latest.current.select(id),
      activate: (id) => {
        const item = findItem(getRules(), id);
        if (item?.type === 'group') latest.current.setRenamingId(id);
        else latest.current.reveal(id, 'editor');
      },
      toggle: (id, enabled) => latest.current.toggle(id, enabled),
      toggleCollapse: (id) => latest.current.toggleCollapse(id),
      rename: (id, title) => latest.current.rename(id, title),
      setRenaming: (id) => latest.current.setRenamingId(id),
      menu: (id) => latest.current.menuFor(id),
    }),
    [getRules],
  );

  const describeDrop = useCallback((id: RuleId, pos: DropPosition) => describePosition(getRules(), pos, (it) => `“${shortLabel(it as RuleNode | GroupNode)}”`) || `item ${id}`, [getRules]);

  useRulesCommands({
    actions,
    lastTemplate,
    totalRules,
    groupCount: groups.allGroupIds.length,
    selectedId,
    searchRef,
    openTemplateMenu: () => setTemplateMenuOpen(true),
    resetStats,
    startImport: transfer.startImport,
    exportRules: transfer.exportRules,
    collapseAll: groups.collapseAll,
    expandAll: groups.expandAll,
  });

  // ------------------------------------------------------------------ render
  const empty = loaded && rules.items.length === 0;
  const selectedRule = selectedItem?.type === 'rule' ? selectedItem : undefined;
  const selectedGroup = selectedItem?.type === 'group' ? selectedItem : undefined;

  return (
    <div className={styles.page}>
      <RulesToolbar
        loaded={loaded}
        empty={empty}
        activeRules={activeRules}
        totalRules={totalRules}
        hasGroups={groups.allGroupIds.length > 0}
        lastTemplate={lastTemplate}
        templateMenuOpen={templateMenuOpen}
        onTemplateMenuOpenChange={setTemplateMenuOpen}
        onCreate={(id) => actions.createFromTemplate(id)}
        onCreateGroup={actions.createGroup}
        searchRef={searchRef}
        query={query}
        onQueryChange={setQuery}
        onSearchEnter={() => {
          const first = rows.find((r) => r.kind === 'rule') ?? rows[0];
          if (first) {
            select(first.id);
            treeRef.current?.focusRow(first.id);
          }
        }}
        undoLabel={history.peekUndo()?.label}
        redoLabel={history.peekRedo()?.label}
        canUndo={history.canUndo}
        canRedo={history.canRedo}
        onUndo={actions.undo}
        onRedo={actions.redo}
        onImport={() => void transfer.startImport()}
        onExport={() => void transfer.exportRules()}
        exportDisabled={rules.items.length === 0}
        onSetAll={actions.setAll}
        onResetStats={resetStats}
        onExpandAll={groups.expandAll}
        onCollapseAll={groups.collapseAll}
      />
      {!loaded ? (
        <div className={styles.body}>
          <div className={styles.list} style={{ flex: `0 0 ${listWidth}px` }}>
            <div className={styles.skeletons} role="status" aria-busy="true" aria-label="Loading rules">
              {Array.from({ length: 6 }, (_, i) => (
                // biome-ignore lint/suspicious/noArrayIndexKey: static placeholders
                <Skeleton key={i} height={28} className={styles.skeletonRow} />
              ))}
            </div>
          </div>
          <div className={styles.editorPane} />
        </div>
      ) : empty ? (
        <TemplateGallery onCreate={(id) => actions.createFromTemplate(id)} onImport={() => void transfer.startImport()} lastTemplate={lastTemplate} />
      ) : (
        <SplitPane
          className={styles.body}
          size={listWidth}
          onSizeChange={setListWidthPref}
          minFirst={LIST_MIN}
          maxFirst={LIST_MAX}
          minSecond={EDITOR_MIN}
          defaultSize={LIST_DEFAULT}
          dividerLabel="Resize rule list"
        >
          <RuleListPane
            treeRef={treeRef}
            rules={rules}
            rows={rows}
            query={query}
            searching={searching}
            matchingRules={matchingRules}
            onClearSearch={clearSearch}
            selectedId={selectedId}
            serverErrors={model.serverErrors}
            invalidDrafts={invalidDrafts}
            renamingId={renamingId}
            actions={rowActions}
            onMove={actions.move}
            onDelete={actions.deleteItem}
            onDuplicate={actions.duplicate}
            onToggleSelected={() => {
              if (selectedItem) actions.toggle(selectedItem.id, !selectedItem.enabled);
            }}
            announce={announce}
            describeDrop={describeDrop}
          />
          <RuleEditorPane
            model={model}
            actions={actions}
            rules={rules}
            selectedRule={selectedRule}
            selectedGroup={selectedGroup}
            selectedOrder={selectedRule ? walkedById.get(selectedRule.id)?.order : undefined}
            totalRules={totalRules}
            stashes={stashes}
            onStash={onStash}
            testNonce={(id) => (testRequest?.id === id ? testRequest.nonce : 0)}
            lastTemplate={lastTemplate}
            menuFor={menuFor}
            reveal={reveal}
            focusRow={(id) => treeRef.current?.focusRow(id)}
            missing={!!routeId && !nav.revealPending()}
            editorRef={editorRef}
          />
        </SplitPane>
      )}
      <ImportDialog pending={transfer.pendingImport} existingCount={totalRules} onCancel={transfer.cancelImport} onImport={transfer.finishImport} />
      <div className="sr-only" role="status" aria-live="polite">
        {nav.announcement}
      </div>
    </div>
  );
}

export default RulesPage;
