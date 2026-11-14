import { closestCenter, DndContext, DragOverlay, PointerSensor, useSensor, useSensors, type DragEndEvent, type DragMoveEvent, type DragStartEvent } from '@dnd-kit/core';
import { SortableContext, useSortable, verticalListSortingStrategy } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import clsx from 'clsx';
import { ArrowDown, ArrowUp, Bookmark, ChevronRight, Copy, Folder, FolderInput, FolderOpen, FolderPlus, GripVertical, MoreHorizontal, Pencil, Save, SearchX, SquarePlus, Trash2 } from 'lucide-react';
import { memo, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from 'react';
import { Button, ContextMenu, EmptyState, Kbd, Menu, MethodBadge, type MenuItems } from '../../components/ui';
import { flattenSaved, folders, moveNode, moveToFolder, moveWithinParent, projectDrop, setFolderCollapsed, type DropTarget, type SavedRow } from './library';
import type { SavedNode } from './model';
import styles from './Sidebar.module.css';

export interface SavedTreeProps {
  saved: readonly SavedNode[];
  query: string;
  /** Saved request shown in the active tab. */
  activeSavedId?: string;
  /** Saved requests with unsaved changes in some tab. */
  dirtyIds: ReadonlySet<string>;
  onChange: (nodes: SavedNode[]) => void;
  onOpen: (id: string, newTab?: boolean) => void;
  onRename: (id: string, name: string) => void;
  onDelete: (node: SavedNode) => void;
  onDuplicate: (id: string) => void;
  onNewFolder: () => void;
  onSaveCurrent: () => void;
  /** Start renaming this node (e.g. a folder that was just created). */
  renameId?: string | null;
  onRenameHandled?: () => void;
}

/** Saved requests: folders (one level), drag & drop reorder/move, inline rename, keyboard nav. */
export const SavedTree = memo(function SavedTree({ saved, query, activeSavedId, dirtyIds, onChange, onOpen, onRename, onDelete, onDuplicate, onNewFolder, onSaveCurrent, renameId, onRenameHandled }: SavedTreeProps) {
  const [dragId, setDragId] = useState<string | null>(null);
  const [target, setTarget] = useState<DropTarget | null>(null);
  const [renaming, setRenaming] = useState<string | null>(null);
  const deltaX = useRef(0);
  const listRef = useRef<HTMLDivElement | null>(null);
  const dragging = dragId ? saved.find((n) => n.id === dragId) : undefined;
  const rows = useMemo(() => flattenSaved(saved, query, dragging?.type === 'folder' ? dragging.id : null), [saved, query, dragging]);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 5 } }));
  const searching = query.trim() !== '';

  useEffect(() => {
    if (!renameId) return;
    setRenaming(renameId);
    onRenameHandled?.();
    requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>(`[data-node="${globalThis.CSS.escape(renameId)}"]`)?.scrollIntoView({ block: 'nearest' }));
  }, [renameId, onRenameHandled]);

  if (saved.length === 0) {
    return (
      <EmptyState
        size="sm"
        icon={Bookmark}
        title="No saved requests"
        description={
          <>
            Save the current tab with <Kbd combo="mod+s" size="sm" /> to build a collection. Folders keep them tidy.
          </>
        }
        actions={
          <>
            <Button size="sm" variant="primary" icon={Save} onClick={onSaveCurrent}>
              Save current
            </Button>
            <Button size="sm" icon={FolderPlus} onClick={onNewFolder}>
              New folder
            </Button>
          </>
        }
        className={styles.empty}
      />
    );
  }
  if (rows.length === 0) return <EmptyState size="sm" icon={SearchX} title="No matches" description={`No saved request matches “${query}”.`} className={styles.empty} />;

  const focusRow = (i: number) => requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>(`[data-row="${Math.max(0, Math.min(rows.length - 1, i))}"]`)?.focus());

  const menuFor = (row: SavedRow): MenuItems => {
    const n = row.node;
    if (n.type === 'folder') {
      return [
        { label: n.collapsed ? 'Expand' : 'Collapse', icon: n.collapsed ? FolderOpen : Folder, onSelect: () => onChange(setFolderCollapsed(saved, n.id, !n.collapsed)) },
        { label: 'Rename…', icon: Pencil, shortcut: 'f2', onSelect: () => setTimeout(() => setRenaming(n.id), 0) },
        { label: 'Move up', icon: ArrowUp, shortcut: 'alt+arrowup', onSelect: () => onChange(moveWithinParent(saved, n.id, -1)) },
        { label: 'Move down', icon: ArrowDown, shortcut: 'alt+arrowdown', onSelect: () => onChange(moveWithinParent(saved, n.id, 1)) },
        { type: 'separator' },
        { label: `Delete folder${n.children.length ? ` and ${n.children.length} request${n.children.length === 1 ? '' : 's'}` : ''}`, icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => onDelete(n) },
      ];
    }
    const fs = folders(saved);
    return [
      { label: 'Open', icon: SquarePlus, onSelect: () => onOpen(n.id) },
      { label: 'Open in new tab', icon: SquarePlus, onSelect: () => onOpen(n.id, true) },
      { label: 'Rename…', icon: Pencil, shortcut: 'f2', onSelect: () => setTimeout(() => setRenaming(n.id), 0) },
      { label: 'Duplicate', icon: Copy, onSelect: () => onDuplicate(n.id) },
      {
        type: 'submenu',
        label: 'Move to',
        icon: FolderInput,
        items: [
          { label: 'Top level', disabled: row.parentId === null, onSelect: () => onChange(moveToFolder(saved, n.id, null)) },
          fs.length > 0 && { type: 'separator' },
          ...fs.map((f) => ({ key: f.id, label: f.name, disabled: row.parentId === f.id, onSelect: () => onChange(moveToFolder(saved, n.id, f.id)) })),
        ],
      },
      { label: 'Move up', icon: ArrowUp, shortcut: 'alt+arrowup', onSelect: () => onChange(moveWithinParent(saved, n.id, -1)) },
      { label: 'Move down', icon: ArrowDown, shortcut: 'alt+arrowdown', onSelect: () => onChange(moveWithinParent(saved, n.id, 1)) },
      { type: 'separator' },
      { label: 'Delete', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => onDelete(n) },
    ];
  };

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>, i: number, row: SavedRow) => {
    const n = row.node;
    if (renaming) return;
    if (e.altKey && (e.key === 'ArrowUp' || e.key === 'ArrowDown')) {
      e.preventDefault();
      onChange(moveWithinParent(saved, n.id, e.key === 'ArrowUp' ? -1 : 1));
      requestAnimationFrame(() => listRef.current?.querySelector<HTMLElement>(`[data-node="${globalThis.CSS.escape(n.id)}"]`)?.focus());
      return;
    }
    switch (e.key) {
      case 'ArrowDown':
        e.preventDefault();
        focusRow(i + 1);
        break;
      case 'ArrowUp':
        e.preventDefault();
        focusRow(i - 1);
        break;
      case 'Home':
        e.preventDefault();
        focusRow(0);
        break;
      case 'End':
        e.preventDefault();
        focusRow(rows.length - 1);
        break;
      case 'ArrowRight':
        if (n.type === 'folder' && n.collapsed) {
          e.preventDefault();
          onChange(setFolderCollapsed(saved, n.id, false));
        } else if (n.type === 'folder') {
          e.preventDefault();
          focusRow(i + 1);
        }
        break;
      case 'ArrowLeft':
        e.preventDefault();
        if (n.type === 'folder' && !n.collapsed) onChange(setFolderCollapsed(saved, n.id, true));
        else if (row.parentId) focusRow(rows.findIndex((r) => r.node.id === row.parentId));
        break;
      case 'Enter':
      case ' ':
        e.preventDefault();
        if (n.type === 'folder') onChange(setFolderCollapsed(saved, n.id, !n.collapsed));
        else onOpen(n.id, e.metaKey || e.ctrlKey);
        break;
      case 'F2':
        e.preventDefault();
        setRenaming(n.id);
        break;
      case 'Delete':
      case 'Backspace':
        e.preventDefault();
        onDelete(n);
        break;
    }
  };

  const updateTarget = (activeId: string, overId: string | undefined) => {
    if (!overId) return setTarget(null);
    setTarget(projectDrop(rows, activeId, overId, deltaX.current));
  };

  const onDragStart = (e: DragStartEvent) => {
    deltaX.current = 0;
    setDragId(String(e.active.id));
    setTarget(null);
  };
  const onDragMove = (e: DragMoveEvent) => {
    deltaX.current = e.delta.x;
    updateTarget(String(e.active.id), e.over ? String(e.over.id) : undefined);
  };
  const onDragEnd = (e: DragEndEvent) => {
    const t = e.over ? projectDrop(rows, String(e.active.id), String(e.over.id), e.delta.x) : null;
    setDragId(null);
    setTarget(null);
    if (t) onChange(moveNode(saved, String(e.active.id), t));
  };

  const dragRow = dragId ? rows.find((r) => r.node.id === dragId) : undefined;

  return (
    <div className={styles.scroll}>
      <DndContext sensors={sensors} collisionDetection={closestCenter} onDragStart={onDragStart} onDragMove={onDragMove} onDragEnd={onDragEnd} onDragCancel={() => setDragId(null)}>
        <SortableContext items={rows.map((r) => r.node.id)} strategy={verticalListSortingStrategy} disabled={searching}>
          <div ref={listRef} role="tree" aria-label="Saved requests" className={styles.tree}>
            {rows.map((row, i) => (
              <SavedItem
                key={row.node.id}
                row={row}
                index={i}
                depth={row.node.id === dragId && target ? (target.parentId ? 1 : 0) : row.depth}
                active={row.node.id === activeSavedId}
                dirty={dirtyIds.has(row.node.id)}
                renaming={renaming === row.node.id}
                dragDisabled={searching}
                menu={() => menuFor(row)}
                onKeyDown={onKeyDown}
                onActivate={(newTab) => {
                  if (row.node.type === 'folder') onChange(setFolderCollapsed(saved, row.node.id, !row.node.collapsed));
                  else onOpen(row.node.id, newTab);
                }}
                onRenameEnd={(name) => {
                  setRenaming(null);
                  if (name != null) onRename(row.node.id, name);
                  focusRow(i);
                }}
              />
            ))}
          </div>
        </SortableContext>
        <DragOverlay dropAnimation={null}>{dragRow ? <SavedRowContent row={dragRow} depth={target?.parentId ? 1 : 0} overlay /> : null}</DragOverlay>
      </DndContext>
    </div>
  );
});

interface SavedItemProps {
  row: SavedRow;
  index: number;
  depth: number;
  active: boolean;
  dirty: boolean;
  renaming: boolean;
  dragDisabled: boolean;
  menu: () => MenuItems;
  onActivate: (newTab: boolean) => void;
  onKeyDown: (e: KeyboardEvent<HTMLDivElement>, index: number, row: SavedRow) => void;
  onRenameEnd: (name: string | null) => void;
}

const SavedItem = memo(function SavedItem({ row, index, depth, active, dirty, renaming, dragDisabled, menu, onActivate, onKeyDown, onRenameEnd }: SavedItemProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: row.node.id, disabled: dragDisabled || renaming });
  const style: CSSProperties = { transform: CSS.Translate.toString(transform), transition };
  const n = row.node;
  return (
    <ContextMenu items={menu}>
      <div
        ref={setNodeRef}
        style={style}
        {...attributes}
        {...listeners}
        role="treeitem"
        aria-level={row.depth + 1}
        aria-expanded={n.type === 'folder' ? !n.collapsed : undefined}
        aria-selected={active}
        tabIndex={0}
        data-row={index}
        data-node={n.id}
        className={clsx(styles.treeRow, active && styles.treeActive, isDragging && styles.treeDragging)}
        onClick={(e) => onActivate(e.metaKey || e.ctrlKey)}
        onAuxClick={(e) => {
          if (e.button === 1 && n.type === 'request') onActivate(true);
        }}
        onDoubleClick={(e) => e.preventDefault()}
        onKeyDown={(e) => onKeyDown(e, index, row)}
      >
        <SavedRowContent row={row} depth={depth} dirty={dirty} renaming={renaming} onRenameEnd={onRenameEnd} menu={menu} />
      </div>
    </ContextMenu>
  );
});

function SavedRowContent({ row, depth, dirty, renaming, onRenameEnd, menu, overlay }: { row: SavedRow; depth: number; dirty?: boolean; renaming?: boolean; onRenameEnd?: (name: string | null) => void; menu?: () => MenuItems; overlay?: boolean }) {
  const n = row.node;
  return (
    <div className={clsx(styles.treeInner, overlay && styles.treeOverlay)} style={{ paddingLeft: 8 + depth * 18 }}>
      <GripVertical size={12} strokeWidth={2} className={styles.grip} aria-hidden />
      {n.type === 'folder' ? (
        <>
          <ChevronRight size={13} strokeWidth={2} className={clsx(styles.chevron, !n.collapsed && styles.chevronOpen)} aria-hidden />
          {n.collapsed ? <Folder size={14} strokeWidth={1.75} className={styles.folderIcon} aria-hidden /> : <FolderOpen size={14} strokeWidth={1.75} className={styles.folderIcon} aria-hidden />}
        </>
      ) : (
        <MethodBadge method={n.draft.method} size="sm" className={styles.treeMethod} />
      )}
      {renaming && onRenameEnd ? (
        <TreeRename initial={n.name} onDone={onRenameEnd} />
      ) : (
        <span className={clsx(styles.treeName, n.type === 'folder' && styles.folderName)}>{n.name}</span>
      )}
      {dirty && <span className={styles.treeDirty} role="img" aria-label="Unsaved changes" />}
      {n.type === 'folder' && <span className={clsx(styles.treeCount, 'tnum')}>{n.children.length}</span>}
      {menu && !renaming && (
        // biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons
        <span className={styles.treeMore} onClick={(e) => e.stopPropagation()} onPointerDown={(e) => e.stopPropagation()}>
          <Menu
            align="end"
            trigger={
              <button type="button" className={styles.treeMoreButton} aria-label={`Actions for ${n.name}`} tabIndex={-1}>
                <MoreHorizontal size={14} strokeWidth={1.75} aria-hidden />
              </button>
            }
            items={menu()}
          />
        </span>
      )}
    </div>
  );
}

function TreeRename({ initial, onDone }: { initial: string; onDone: (name: string | null) => void }) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);
  const finish = (v: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(v);
  };
  return (
    <input
      className={styles.treeRename}
      value={value}
      // biome-ignore lint/a11y/noAutofocus: inline rename field: it appears because the user asked to rename
      autoFocus
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(value);
        else if (e.key === 'Escape') finish(null);
      }}
      onBlur={() => finish(value)}
      aria-label="Name"
      spellCheck={false}
    />
  );
}
