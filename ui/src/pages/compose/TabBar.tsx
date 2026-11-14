import { closestCenter, DndContext, PointerSensor, useSensor, useSensors, type DragEndEvent, type Modifier } from '@dnd-kit/core';
import { horizontalListSortingStrategy, SortableContext, useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import clsx from 'clsx';
import { ArrowRightToLine, Copy, PanelLeftClose, PanelLeftOpen, Pencil, Plus, RotateCcw, Save, X, XCircle } from 'lucide-react';
import { memo, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type KeyboardEvent, type ReactNode } from 'react';
import { ContextMenu, IconButton, Tooltip, type MenuItems } from '../../components/ui';
import { methodColorVar } from '../../lib/http';
import { useStableCallback } from './hooks';
import { findSaved } from './library';
import { shortUrlLabel, type ComposeKeys } from './labels';
import { isDirty, type ComposeTab, type SavedNode } from './model';
import styles from './TabBar.module.css';

/** Tab title: the saved request's name, a custom title, or the URL's path (host when none). */
export function tabTitle(tab: ComposeTab, saved: readonly SavedNode[]): string {
  if (tab.savedId) {
    const s = findSaved(saved, tab.savedId);
    if (s) return s.name;
  }
  return tab.title || shortUrlLabel(tab.draft.url) || '';
}

/** DOM id of the panel the tab strip controls. */
export const COMPOSE_PANEL_ID = 'compose-request-panel';

const restrictToX: Modifier = ({ transform }) => ({ ...transform, y: 0 });

export interface TabBarProps {
  tabs: readonly ComposeTab[];
  activeId: string;
  saved: readonly SavedNode[];
  sendingIds: ReadonlySet<string>;
  canReopen: boolean;
  sidebarOpen: boolean;
  onToggleSidebar: () => void;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onNew: () => void;
  onMove: (from: number, to: number) => void;
  onDuplicate: (id: string) => void;
  onCloseOthers: (id: string) => void;
  onCloseToRight: (id: string) => void;
  onReopen: () => void;
  onRename: (id: string, title: string) => void;
  onSave: (id: string) => void;
  /** Right-aligned content (environment selector). */
  trailing?: ReactNode;
  /** Shortcuts to advertise (browser vs desktop). */
  keys: ComposeKeys;
}

/** Browser-like request tabs: drag to reorder, middle-click to close, dirty dots, overflow scroll. */
export const TabBar = memo(function TabBar(props: TabBarProps) {
  const { tabs, activeId, saved, sendingIds, onSelect, onMove } = props;
  const scroller = useRef<HTMLDivElement | null>(null);
  const [fade, setFade] = useState({ left: false, right: false });
  const [renaming, setRenaming] = useState<string | null>(null);
  const sensors = useSensors(useSensor(PointerSensor, { activationConstraint: { distance: 6 } }));

  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const update = () => {
      const left = el.scrollLeft > 1;
      const right = el.scrollLeft + el.clientWidth < el.scrollWidth - 1;
      setFade((f) => (f.left === left && f.right === right ? f : { left, right }));
    };
    update();
    el.addEventListener('scroll', update, { passive: true });
    const ro = new ResizeObserver(update);
    ro.observe(el);
    for (const child of Array.from(el.children)) ro.observe(child);
    return () => {
      el.removeEventListener('scroll', update);
      ro.disconnect();
    };
  }, [tabs.length]);

  // Keep the active tab visible.
  useLayoutEffect(() => {
    const el = scroller.current?.querySelector<HTMLElement>(`[data-tab-id="${globalThis.CSS.escape(activeId)}"]`);
    el?.scrollIntoView({ block: 'nearest', inline: 'nearest' });
  }, [activeId, tabs.length]);

  const onDragEnd = (e: DragEndEvent) => {
    if (!e.over || e.active.id === e.over.id) return;
    const from = tabs.findIndex((t) => t.id === e.active.id);
    const to = tabs.findIndex((t) => t.id === e.over?.id);
    if (from >= 0 && to >= 0) onMove(from, to);
  };

  const menuFor = useStableCallback((id: string): MenuItems => {
    const i = tabs.findIndex((t) => t.id === id);
    const tab = tabs[i];
    if (!tab) return [];
    return [
      { label: 'New request', icon: Plus, shortcut: props.keys.newTab, onSelect: props.onNew },
      { label: 'Duplicate', icon: Copy, onSelect: () => props.onDuplicate(tab.id) },
      { label: 'Rename…', icon: Pencil, shortcut: 'f2', onSelect: () => setTimeout(() => setRenaming(tab.id), 0) },
      { label: tab.savedId ? 'Save' : 'Save…', icon: Save, shortcut: 'mod+s', onSelect: () => setTimeout(() => props.onSave(tab.id), 0) },
      { type: 'separator' },
      { label: 'Close', icon: X, shortcut: props.keys.closeTab, onSelect: () => props.onClose(tab.id) },
      { label: 'Close others', icon: XCircle, disabled: tabs.length < 2, onSelect: () => props.onCloseOthers(tab.id) },
      { label: 'Close tabs to the right', icon: ArrowRightToLine, disabled: i === tabs.length - 1, onSelect: () => props.onCloseToRight(tab.id) },
      { type: 'separator' },
      { label: 'Reopen closed tab', icon: RotateCcw, shortcut: props.keys.reopenTab, disabled: !props.canReopen, onSelect: props.onReopen },
    ];
  });

  const onRenameEnd = useStableCallback((id: string, title: string | null) => {
    setRenaming(null);
    if (title != null) props.onRename(id, title);
    focusTab(id);
  });

  const focusTab = (id: string) => requestAnimationFrame(() => scroller.current?.querySelector<HTMLElement>(`[data-tab-id="${globalThis.CSS.escape(id)}"]`)?.focus());

  const onKeyDown = useStableCallback((e: KeyboardEvent<HTMLDivElement>, index: number) => {
    const n = tabs.length;
    let target: number | null = null;
    if (e.key === 'ArrowRight') target = (index + 1) % n;
    else if (e.key === 'ArrowLeft') target = (index - 1 + n) % n;
    else if (e.key === 'Home') target = 0;
    else if (e.key === 'End') target = n - 1;
    if (target !== null) {
      e.preventDefault();
      const id = (tabs[target] as ComposeTab).id;
      if (e.shiftKey && (e.ctrlKey || e.metaKey) && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) {
        onMove(index, target);
        focusTab((tabs[index] as ComposeTab).id);
        return;
      }
      onSelect(id);
      focusTab(id);
      return;
    }
    const tab = tabs[index] as ComposeTab;
    if (e.key === 'Delete' || e.key === 'Backspace') {
      e.preventDefault();
      props.onClose(tab.id);
    } else if (e.key === 'F2') {
      e.preventDefault();
      setRenaming(tab.id);
    }
  });

  return (
    <div className={styles.bar} data-window-drag>
      <IconButton
        label={props.sidebarOpen ? 'Hide sidebar' : 'Show sidebar'}
        icon={props.sidebarOpen ? PanelLeftClose : PanelLeftOpen}
        shortcut="mod+b"
        onClick={props.onToggleSidebar}
        className={styles.sidebarToggle}
        tooltipSide="bottom"
      />
      <div ref={scroller} className={styles.scroller} data-fade-left={fade.left || undefined} data-fade-right={fade.right || undefined} onWheel={(e) => {
        if (Math.abs(e.deltaY) > Math.abs(e.deltaX) && scroller.current) scroller.current.scrollLeft += e.deltaY;
      }}>
        <DndContext sensors={sensors} collisionDetection={closestCenter} modifiers={[restrictToX]} onDragEnd={onDragEnd}>
          <SortableContext items={tabs.map((t) => t.id)} strategy={horizontalListSortingStrategy}>
            <div className={styles.list} role="tablist" aria-label="Requests">
              {tabs.map((tab, i) => (
                <TabItem
                  key={tab.id}
                  tab={tab}
                  index={i}
                  title={tabTitle(tab, saved)}
                  active={tab.id === activeId}
                  dirty={isDirty(tab)}
                  sending={sendingIds.has(tab.id)}
                  renaming={renaming === tab.id}
                  onRenameEnd={onRenameEnd}
                  onKeyDown={onKeyDown}
                  menu={menuFor}
                  onSelect={onSelect}
                  onClose={props.onClose}
                  onStartRename={setRenaming}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
        <Tooltip content="New request" shortcut={props.keys.newTab} side="bottom">
          <button type="button" className={styles.newTab} onClick={props.onNew} aria-label="New request">
            <Plus size={15} strokeWidth={1.75} aria-hidden />
          </button>
        </Tooltip>
      </div>
      {props.trailing && <div className={styles.trailing}>{props.trailing}</div>}
    </div>
  );
});

interface TabItemProps {
  tab: ComposeTab;
  index: number;
  title: string;
  active: boolean;
  dirty: boolean;
  sending: boolean;
  renaming: boolean;
  menu: (id: string) => MenuItems;
  onSelect: (id: string) => void;
  onClose: (id: string) => void;
  onKeyDown: (e: KeyboardEvent<HTMLDivElement>, index: number) => void;
  onStartRename: (id: string) => void;
  onRenameEnd: (id: string, title: string | null) => void;
}

const TabItem = memo(function TabItem({ tab, index, title, active, dirty, sending, renaming, menu, onSelect, onClose, onKeyDown, onStartRename, onRenameEnd }: TabItemProps) {
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } = useSortable({ id: tab.id });
  const method = (tab.draft.method || 'GET').toUpperCase();
  const style: CSSProperties = { transform: CSS.Translate.toString(transform), transition, '--method-color': methodColorVar(method) } as CSSProperties;
  const full = `${method} ${tab.draft.url || 'New request'}`;

  return (
    <ContextMenu items={() => menu(tab.id)}>
      <div
        ref={setNodeRef}
        style={style}
        {...attributes}
        {...listeners}
        role="tab"
        aria-selected={active}
        aria-label={`${title || 'New request'}${dirty ? ' (unsaved changes)' : ''}`}
        tabIndex={active ? 0 : -1}
        data-tab-id={tab.id}
        id={`compose-tab-${tab.id}`}
        aria-controls={active ? COMPOSE_PANEL_ID : undefined}
        title={full}
        className={clsx(styles.tab, active && styles.active, isDragging && styles.dragging)}
        onClick={() => onSelect(tab.id)}
        onMouseDown={(e) => {
          if (e.button === 1) e.preventDefault();
        }}
        onAuxClick={(e) => {
          if (e.button === 1) {
            e.preventDefault();
            onClose(tab.id);
          }
        }}
        onDoubleClick={() => onStartRename(tab.id)}
        onKeyDown={(e) => {
          if (!renaming) onKeyDown(e, index);
        }}
      >
        <span className={styles.method}>{method.length > 6 ? method.slice(0, 5) + '…' : method}</span>
        {renaming ? (
          <RenameInput initial={title} onDone={(t) => onRenameEnd(tab.id, t)} />
        ) : (
          <span className={clsx(styles.title, !title && styles.untitled)}>{title || 'New request'}</span>
        )}
        <span className={styles.end}>
          {sending ? (
            <span className={styles.sendingDot} role="img" aria-label="Sending" />
          ) : (
            <>
              {dirty && <span className={styles.dirty} aria-hidden />}
              <button
                type="button"
                className={clsx(styles.close, dirty && styles.closeDirty)}
                aria-label={`Close ${title || 'tab'}`}
                tabIndex={-1}
                onPointerDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  onClose(tab.id);
                }}
              >
                <X size={12} strokeWidth={2} aria-hidden />
              </button>
            </>
          )}
        </span>
      </div>
    </ContextMenu>
  );
});

function RenameInput({ initial, onDone }: { initial: string; onDone: (title: string | null) => void }) {
  const [value, setValue] = useState(initial);
  const done = useRef(false);
  const finish = (v: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(v);
  };
  return (
    <input
      className={styles.rename}
      value={value}
      // biome-ignore lint/a11y/noAutofocus: inline rename field: it appears because the user asked to rename
      autoFocus
      onFocus={(e) => e.currentTarget.select()}
      onChange={(e) => setValue(e.target.value)}
      onPointerDown={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(value);
        else if (e.key === 'Escape') finish(null);
      }}
      onBlur={() => finish(value)}
      aria-label="Tab name"
      spellCheck={false}
    />
  );
}
