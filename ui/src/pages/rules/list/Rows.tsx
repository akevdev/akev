import { useDraggable } from '@dnd-kit/core';
import clsx from 'clsx';
import { ChevronRight, EllipsisVertical, Folder, FolderOpen, GripVertical, TriangleAlert } from 'lucide-react';
import { memo, useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type MouseEvent } from 'react';
import type { RuleId } from '../../../api/types';
import { ContextMenu, MenuButton, Switch, Tooltip, type MenuItems } from '../../../components/ui';
import { actionInfo } from '../../../lib/rules/actions';
import { summarizeTokens, tokensToText } from '../../../lib/rules/describe';
import type { GroupNode, RuleNode } from '../../../lib/rules/tree';
import { ProBadge } from '../../../components/pro';
import { actionNeedsPro } from '../../../lib/license';
import { iconFor } from '../icons';
import { Tokens } from '../Tokens';
import { HitPill } from './HitPill';
import styles from './RuleTree.module.css';

/** Stable callbacks shared by every row (the page keeps them in a ref). */
export interface RowActions {
  select(id: RuleId): void;
  activate(id: RuleId): void;
  toggle(id: RuleId, enabled: boolean): void;
  toggleCollapse(id: RuleId): void;
  rename(id: RuleId, title: string): void;
  setRenaming(id: RuleId | null): void;
  menu(id: RuleId): MenuItems;
}

interface CommonProps {
  depth: number;
  parentEnabled: boolean;
  selected: boolean;
  tabbable: boolean;
  actions: RowActions;
  /** Hide the drag handle (e.g. while searching). */
  dragDisabled: boolean;
}

export interface RuleRowProps extends CommonProps {
  rule: RuleNode;
  error: string | undefined;
  invalidDraft: boolean;
}

function useRowDrag(id: RuleId, disabled: boolean) {
  const { setNodeRef, setActivatorNodeRef, listeners, attributes, isDragging } = useDraggable({ id, disabled });
  return { setNodeRef, setActivatorNodeRef, listeners, attributes, isDragging };
}

function DragHandle({ drag, label }: { drag: ReturnType<typeof useRowDrag>; label: string }) {
  return (
    <button
      type="button"
      ref={drag.setActivatorNodeRef}
      className={styles.handle}
      aria-label={label}
      tabIndex={-1}
      {...drag.listeners}
      aria-roledescription="draggable"
      aria-describedby={drag.attributes['aria-describedby']}
      onClick={(e) => e.stopPropagation()}
    >
      <GripVertical size={14} strokeWidth={1.75} aria-hidden />
    </button>
  );
}

export const RuleRow = memo(function RuleRow({ rule, depth, parentEnabled, selected, tabbable, actions, dragDisabled, error, invalidDraft }: RuleRowProps) {
  const drag = useRowDrag(rule.id, dragDisabled);
  const tokens = useMemo(() => summarizeTokens(rule), [rule]);
  const summary = useMemo(() => tokensToText(tokens), [tokens]);
  const info = actionInfo(rule.action.type);
  const Icon = iconFor(info.icon);
  const title = rule.title?.trim();
  const active = rule.enabled && parentEnabled;
  const pro = actionNeedsPro(rule.action);

  return (
    <ContextMenu items={() => actions.menu(rule.id)}>
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: tree row: the tree handles the keyboard (arrows, Enter, Space) */}
      <div
        ref={drag.setNodeRef}
        role="treeitem"
        aria-level={depth + 1}
        aria-selected={selected}
        aria-label={`${title ?? summary}${rule.enabled ? '' : ' (disabled)'}`}
        id={`rule-row-${rule.id}`}
        data-row-id={rule.id}
        tabIndex={tabbable ? 0 : -1}
        data-nested={depth > 0 || undefined}
        className={clsx(styles.row, styles.ruleRow, selected && styles.selected, !active && styles.inactive, drag.isDragging && styles.dragSource, error && styles.hasError)}
        style={{ '--depth': depth } as CSSProperties}
        onClick={() => actions.select(rule.id)}
        onDoubleClick={() => actions.activate(rule.id)}
      >
        {!dragDisabled && <DragHandle drag={drag} label={`Reorder “${title ?? summary}”. Drag, or press Alt+↑/↓`} />}
        {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons */}
        <span className={styles.switchCell} onClick={stopClick} onDoubleClick={stopClick}>
          <Tooltip content={!parentEnabled ? 'Its group is disabled' : rule.enabled ? 'Disable rule' : 'Enable rule'} shortcut="space">
            <span>
              <Switch size="sm" checked={rule.enabled} onCheckedChange={(c) => actions.toggle(rule.id, c)} aria-label={rule.enabled ? 'Disable rule' : 'Enable rule'} />
            </span>
          </Tooltip>
        </span>
        <span className={styles.iconTile} data-category={info.category} aria-hidden>
          <Icon size={13} strokeWidth={1.9} />
        </span>
        <span className={styles.content}>
          {title ? (
            <>
              <span className={clsx(styles.title, 'truncate')}>{title}</span>
              <Tokens tokens={tokens} className={clsx(styles.sub, 'truncate')} />
            </>
          ) : (
            <Tokens tokens={tokens} className={clsx(styles.main, 'truncate')} />
          )}
        </span>
        {invalidDraft && (
          <Tooltip content="Unsaved edits with errors — open the rule to fix them">
            <span className={styles.draftDot} role="img" aria-label="Unsaved edits with errors" />
          </Tooltip>
        )}
        {pro && <ProBadge feature={pro} />}
        {error && (
          <Tooltip content={/^inactive\b/i.test(error) ? error : `Not applied: ${error}`} multiline>
            <span className={styles.errorIcon} role="img" aria-label={`Error: ${error}`}>
              <TriangleAlert size={14} strokeWidth={2} />
            </span>
          </Tooltip>
        )}
        <HitPill ruleId={rule.id} maxMatches={rule.maxMatches} />
        <MenuButton label="Rule actions" icon={EllipsisVertical} size="sm" items={() => actions.menu(rule.id)} className={styles.menuButton} tabIndex={-1} isolate />
      </div>
    </ContextMenu>
  );
});

export interface GroupRowProps extends CommonProps {
  group: GroupNode;
  expanded: boolean;
  ruleCount: number;
  renaming: boolean;
}

export const GroupRow = memo(function GroupRow({ group, depth, parentEnabled, selected, tabbable, actions, dragDisabled, expanded, ruleCount, renaming }: GroupRowProps) {
  const drag = useRowDrag(group.id, dragDisabled);
  const active = group.enabled && parentEnabled;
  const FolderIcon = expanded ? FolderOpen : Folder;
  return (
    <ContextMenu items={() => actions.menu(group.id)}>
      {/* biome-ignore lint/a11y/useKeyWithClickEvents: tree row: the tree handles the keyboard (arrows, Enter, Space) */}
      <div
        ref={drag.setNodeRef}
        role="treeitem"
        aria-level={depth + 1}
        aria-selected={selected}
        aria-expanded={expanded}
        aria-label={`Group ${group.title}, ${ruleCount} rules${group.enabled ? '' : ', disabled'}`}
        id={`rule-row-${group.id}`}
        data-row-id={group.id}
        tabIndex={tabbable ? 0 : -1}
        data-nested={depth > 0 || undefined}
        className={clsx(styles.row, styles.groupRow, selected && styles.selected, !active && styles.inactive, drag.isDragging && styles.dragSource)}
        style={{ '--depth': depth } as CSSProperties}
        onClick={() => actions.select(group.id)}
      >
        {!dragDisabled && <DragHandle drag={drag} label={`Reorder group “${group.title}”. Drag, or press Alt+↑/↓`} />}
        <button
          type="button"
          className={styles.chevron}
          aria-label={expanded ? 'Collapse group' : 'Expand group'}
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            actions.toggleCollapse(group.id);
          }}
        >
          <ChevronRight size={14} strokeWidth={2} className={clsx(styles.chevronIcon, expanded && styles.chevronOpen)} aria-hidden />
        </button>
        <FolderIcon size={14} strokeWidth={1.75} className={styles.folderIcon} aria-hidden />
        {renaming ? (
          <RenameInput initial={group.title} onDone={(title) => (title === null ? actions.setRenaming(null) : actions.rename(group.id, title))} />
        ) : (
          // biome-ignore lint/a11y/noStaticElementInteractions: double-click shortcut for Rename, which is also F2 and in the menu
          <span
            className={clsx(styles.groupTitle, 'truncate')}
            onDoubleClick={(e) => {
              e.stopPropagation();
              actions.setRenaming(group.id);
            }}
            title="Double-click to rename"
          >
            {group.title || 'Untitled group'}
          </span>
        )}
        <span className={clsx(styles.count, 'tnum')}>{ruleCount}</span>
        <span className={styles.spacer} />
        {/* biome-ignore lint/a11y/noStaticElementInteractions lint/a11y/useKeyWithClickEvents: only stops the click from reaching the row behind it; the controls inside are buttons */}
        <span className={styles.switchCell} onClick={stopClick} onDoubleClick={stopClick}>
          <Tooltip content={group.enabled ? 'Disable every rule in this group' : 'Enable this group'} shortcut="space">
            <span>
              <Switch size="sm" checked={group.enabled} onCheckedChange={(c) => actions.toggle(group.id, c)} aria-label={group.enabled ? 'Disable group' : 'Enable group'} />
            </span>
          </Tooltip>
        </span>
        <MenuButton label="Group actions" icon={EllipsisVertical} size="sm" items={() => actions.menu(group.id)} className={styles.menuButton} tabIndex={-1} isolate />
      </div>
    </ContextMenu>
  );
});

function stopClick(e: MouseEvent) {
  e.stopPropagation();
}

function RenameInput({ initial, onDone }: { initial: string; onDone: (title: string | null) => void }) {
  const [value, setValue] = useState(initial);
  const ref = useRef<HTMLInputElement | null>(null);
  const done = useRef(false);
  useEffect(() => {
    ref.current?.focus();
    ref.current?.select();
  }, []);
  const finish = (v: string | null) => {
    if (done.current) return;
    done.current = true;
    onDone(v === null ? null : v.trim() || initial);
  };
  return (
    <input
      ref={ref}
      className={styles.renameInput}
      value={value}
      aria-label="Group name"
      onChange={(e) => setValue(e.target.value)}
      onClick={(e) => e.stopPropagation()}
      onDoubleClick={(e) => e.stopPropagation()}
      onKeyDown={(e: KeyboardEvent<HTMLInputElement>) => {
        e.stopPropagation();
        if (e.key === 'Enter') finish(value);
        else if (e.key === 'Escape') finish(null);
      }}
      onBlur={() => finish(value)}
    />
  );
}
