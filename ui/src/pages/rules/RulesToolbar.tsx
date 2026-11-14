import clsx from 'clsx';
import { ChevronsDownUp, ChevronsUpDown, Download, EllipsisVertical, FolderPlus, Redo2, RotateCcw, ToggleLeft, ToggleRight, Undo2, Upload } from 'lucide-react';
import type { RefObject } from 'react';
import { IconButton, MenuButton, SearchInput, Toolbar, ToolbarSeparator, Tooltip } from '../../components/ui';
import type { TemplateId } from '../../lib/rules/templates';
import { NewRuleButton } from './Templates';
import styles from './RulesPage.module.css';

export interface RulesToolbarProps {
  loaded: boolean;
  /** No rules at all (the template gallery is showing). */
  empty: boolean;
  activeRules: number;
  totalRules: number;
  hasGroups: boolean;
  lastTemplate: TemplateId;
  templateMenuOpen: boolean;
  onTemplateMenuOpenChange(open: boolean): void;
  onCreate(id: TemplateId): void;
  onCreateGroup(): void;
  searchRef: RefObject<HTMLInputElement | null>;
  query: string;
  onQueryChange(query: string): void;
  /** ↓ or Enter in the search box: move into the results. */
  onSearchEnter(): void;
  undoLabel: string | undefined;
  redoLabel: string | undefined;
  canUndo: boolean;
  canRedo: boolean;
  onUndo(): void;
  onRedo(): void;
  onImport(): void;
  onExport(): void;
  /** Nothing to export (no rules). */
  exportDisabled: boolean;
  onSetAll(enabled: boolean): void;
  onResetStats(): void;
  onExpandAll(): void;
  onCollapseAll(): void;
}

/** Rules page title bar: counts, new rule / group, search, undo / redo, import / export, More. */
export function RulesToolbar(props: RulesToolbarProps) {
  const { loaded, empty, activeRules, totalRules, hasGroups } = props;
  return (
    <Toolbar
      titleBar
      className={styles.toolbar}
      left={
        <>
          <h1 className={styles.title}>Rules</h1>
          {loaded && totalRules > 0 && (
            <Tooltip content={`${activeRules} of ${totalRules} rules are active`}>
              <span className={clsx(styles.count, 'tnum')} tabIndex={-1}>
                {activeRules}/{totalRules} active
              </span>
            </Tooltip>
          )}
          <ToolbarSeparator />
          <NewRuleButton lastTemplate={props.lastTemplate} onCreate={props.onCreate} open={props.templateMenuOpen} onOpenChange={props.onTemplateMenuOpenChange} />
          <IconButton label="New group" icon={FolderPlus} shortcut="shift+n" onClick={props.onCreateGroup} />
          {!empty && (
            <SearchInput
              ref={props.searchRef}
              value={props.query}
              onValueChange={props.onQueryChange}
              placeholder="Search rules"
              shortcut="mod+f"
              className={styles.search}
              size="sm"
              aria-label="Search rules"
              onKeyDown={(e) => {
                if (e.key === 'ArrowDown' || e.key === 'Enter') {
                  e.preventDefault();
                  props.onSearchEnter();
                }
              }}
            />
          )}
        </>
      }
      right={
        <>
          <IconButton label={props.undoLabel ? `Undo: ${props.undoLabel}` : 'Nothing to undo'} icon={Undo2} shortcut="mod+z" onClick={props.onUndo} disabled={!props.canUndo} />
          <IconButton label={props.redoLabel ? `Redo: ${props.redoLabel}` : 'Nothing to redo'} icon={Redo2} shortcut="mod+shift+z" onClick={props.onRedo} disabled={!props.canRedo} />
          <ToolbarSeparator />
          <IconButton label="Import rules… (Akev JSON or HTTP Toolkit .htkrules)" icon={Upload} onClick={props.onImport} />
          <IconButton label="Export rules as JSON" icon={Download} onClick={props.onExport} disabled={props.exportDisabled} />
          <MenuButton
            label="More"
            icon={EllipsisVertical}
            items={() => [
              { label: 'Enable all rules', icon: ToggleRight, onSelect: () => props.onSetAll(true), disabled: totalRules === 0 },
              { label: 'Disable all rules', icon: ToggleLeft, onSelect: () => props.onSetAll(false), disabled: totalRules === 0 },
              { label: 'Reset hit counters', icon: RotateCcw, onSelect: props.onResetStats, disabled: totalRules === 0 },
              { type: 'separator' },
              { label: 'Expand all groups', icon: ChevronsUpDown, onSelect: props.onExpandAll, disabled: !hasGroups },
              { label: 'Collapse all groups', icon: ChevronsDownUp, onSelect: props.onCollapseAll, disabled: !hasGroups },
            ]}
          />
        </>
      }
    />
  );
}
