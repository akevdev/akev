import { FolderPlus, Trash2 } from 'lucide-react';
import { memo, useCallback, useMemo, useState } from 'react';
import { IconButton, SearchInput, SegmentedControl, undoToast } from '../../components/ui';
import { HistoryList } from './HistoryList';
import { suggestedRequestName } from './labels';
import { addSaved, findNode, findSaved, moveNode, parentOf, uniqueName } from './library';
import { draftHash, isDirty, uid, type HistoryEntry, type RequestDraft, type SavedNode } from './model';
import { SavedTree } from './SavedTree';
import { activeTabOf, historyActions, openHistory, openSaved, savedActions, tabsActions, useComposeStore } from './store';
import styles from './Sidebar.module.css';

export type SidebarSection = 'history' | 'saved';

export const SIDEBAR_MIN = 200;
export const SIDEBAR_MAX = 440;
export const SIDEBAR_DEFAULT = 264;

export interface SidebarProps {
  section: SidebarSection;
  onSectionChange: (section: SidebarSection) => void;
  /** Ask the page to open the Save dialog for a draft (history entry → saved). */
  onSaveDraft: (draft: RequestDraft, suggestedName: string) => void;
  onSaveCurrent: () => void;
}

/** Collapsible sidebar: History and Saved requests (the page sizes it with a SplitPane). */
export const Sidebar = memo(function Sidebar({ section, onSectionChange, onSaveDraft, onSaveCurrent }: SidebarProps) {
  const history = useComposeStore((s) => s.history);
  const saved = useComposeStore((s) => s.saved);
  // A string key keeps the set stable while typing in a tab.
  const dirtyKey = useComposeStore((s) =>
    s.tabs
      .filter((t) => t.savedId && isDirty(t))
      .map((t) => t.savedId as string)
      .sort()
      .join('\n'),
  );
  const activeSavedId = useComposeStore((s) => activeTabOf(s).savedId);
  const [queries, setQueries] = useState<Record<SidebarSection, string>>({ history: '', saved: '' });
  const [renameId, setRenameId] = useState<string | null>(null);
  const query = queries[section];

  const dirtyIds = useMemo(() => new Set(dirtyKey ? dirtyKey.split('\n') : []), [dirtyKey]);
  const savedCount = useMemo(() => saved.reduce((n, x) => n + (x.type === 'folder' ? x.children.length : 1), 0), [saved]);

  // ---- history
  const openEntry = useCallback((entry: HistoryEntry, newTab?: boolean) => {
    if (!newTab) {
      // Focus an untouched tab that already shows exactly this request.
      const hash = draftHash(entry.draft);
      const existing = useComposeStore.getState().tabs.find((t) => !isDirty(t) && draftHash(t.draft) === hash);
      if (existing) return tabsActions.select(existing.id);
      openHistory(entry);
      return;
    }
    tabsActions.newTab(entry.draft, entry.exchangeId != null ? { lastExchangeId: entry.exchangeId } : {});
  }, []);
  const removeEntry = useCallback((entry: HistoryEntry) => {
    const prev = useComposeStore.getState().history;
    historyActions.remove(entry.id);
    undoToast('Removed from history', () => historyActions.restore(prev));
  }, []);
  const saveEntry = useCallback((entry: HistoryEntry) => onSaveDraft(entry.draft, suggestedRequestName(entry.draft.url)), [onSaveDraft]);
  const clearHistory = () => {
    const prev = historyActions.clear();
    if (prev.length) undoToast(`Cleared ${prev.length} request${prev.length === 1 ? '' : 's'} from history`, () => historyActions.restore(prev));
  };

  // ---- saved
  const newFolder = useCallback(() => {
    const s = useComposeStore.getState().saved;
    const names = s.filter((n) => n.type === 'folder').map((n) => n.name);
    const id = uid('f');
    savedActions.addFolder({ type: 'folder', id, name: uniqueName('New folder', names), collapsed: false, children: [] });
    onSectionChange('saved');
    setQueries((q) => ({ ...q, saved: '' }));
    setRenameId(id);
  }, [onSectionChange]);
  const deleteNode = useCallback((node: SavedNode) => {
    const prev = savedActions.remove(node.id);
    undoToast(node.type === 'folder' ? `Deleted folder “${node.name}”` : `Deleted “${node.name}”`, () => savedActions.set(prev));
  }, []);
  const duplicate = useCallback((id: string) => {
    const s = useComposeStore.getState().saved;
    const src = findSaved(s, id);
    if (!src) return;
    const parent = parentOf(s, id) ?? null;
    const copy = { ...src, id: uid('s'), name: `${src.name} copy`, updatedAt: Date.now() };
    savedActions.set(moveNode(addSaved(s, copy, parent), copy.id, { parentId: parent, afterId: id }));
    setRenameId(copy.id);
  }, []);
  const openNode = useCallback((id: string, newTab?: boolean) => {
    if (newTab) {
      const req = findSaved(useComposeStore.getState().saved, id);
      if (req) tabsActions.newTab(req.draft, { savedId: id });
      return;
    }
    openSaved(id);
  }, []);
  const rename = useCallback((id: string, name: string) => {
    if (findNode(useComposeStore.getState().saved, id)) savedActions.rename(id, name);
  }, []);

  return (
    <aside className={styles.sidebar} aria-label="Compose library">
      <div className={styles.head} data-window-drag>
        <SegmentedControl
          size="sm"
          value={section}
          onValueChange={onSectionChange}
          items={[
            { value: 'history', label: 'History', count: history.length || undefined },
            { value: 'saved', label: 'Saved', count: savedCount || undefined },
          ]}
          aria-label="Sidebar section"
          className={styles.sections}
        />
        <div className={styles.headActions}>
          {section === 'history' ? (
            <IconButton size="sm" label="Clear history" icon={Trash2} variant="danger" disabled={history.length === 0} onClick={clearHistory} tooltipSide="bottom" />
          ) : (
            <IconButton size="sm" label="New folder" icon={FolderPlus} onClick={newFolder} tooltipSide="bottom" />
          )}
        </div>
      </div>
      <div className={styles.search}>
        <SearchInput
          size="sm"
          value={query}
          onValueChange={(v) => setQueries((q) => ({ ...q, [section]: v }))}
          placeholder={section === 'history' ? 'Search history' : 'Search saved'}
          aria-label={section === 'history' ? 'Search history' : 'Search saved requests'}
        />
      </div>
      <div className={styles.listArea}>
        {section === 'history' ? (
          <HistoryList history={history} query={query} onOpen={openEntry} onSave={saveEntry} onRemove={removeEntry} />
        ) : (
          <SavedTree
            saved={saved}
            query={query}
            activeSavedId={activeSavedId}
            dirtyIds={dirtyIds}
            onChange={savedActions.set}
            onOpen={openNode}
            onRename={rename}
            onDelete={deleteNode}
            onDuplicate={duplicate}
            onNewFolder={newFolder}
            onSaveCurrent={onSaveCurrent}
            renameId={renameId}
            onRenameHandled={() => setRenameId(null)}
          />
        )}
      </div>
    </aside>
  );
});
