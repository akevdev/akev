import { Code, Copy, Download, Ellipsis, FileDown, FileUp, Highlighter, Link, Pin, RotateCw, SquarePen, StickyNote, Trash2, WandSparkles } from 'lucide-react';
import type { CodegenTarget, ExchangeSummary, TargetInfo } from '../../../../api/types';
import { afterMenuClose, IconButton, MenuButton, SplitMenuButton, toast, type MenuItems } from '../../../../components/ui';
import { copyText } from '../../../../lib/platform';
import { usePagePref } from '../../../../state/ui';
import { useCodegenTargets } from '../../actions/codegenTargets';
import { copyAsSnippet, copyBody, copyUrls, deleteExchanges, downloadBody, exportHar, openInComposeFor, replay, setHighlight, setPinned } from '../../actions/exchangeActions';
import { createRuleItems, highlightItems } from '../../actions/menus';
import styles from './ExchangeHeader.module.css';

/** Targets shown directly in the Copy-as menu; the rest live under "All targets". */
const POPULAR: readonly CodegenTarget[] = ['curl', 'httpie', 'js-fetch', 'node-axios', 'python-requests', 'go-net-http', 'raw-http'];

export const COPY_TARGET_PREF = 'traffic.copyTarget';

export interface HeaderActionsProps {
  summary: ExchangeSummary;
  requestBodySize: number;
  responseBodySize: number;
  note: string | undefined;
  onEditNote: () => void;
  /** `connection`: TLS failures / tunnels — no replay, compose or snippets. */
  variant?: 'http' | 'connection';
}

/** Replay · Edit in Compose · Copy as ▾ · Create rule ▾ · Pin · ⋮ (DESIGN.md §6.4). */
export function HeaderActions({ summary, requestBodySize, responseBodySize, note, onEditNote, variant = 'http' }: HeaderActionsProps) {
  const id = summary.id;
  const targets = useCodegenTargets();
  const [lastTarget, setLastTarget] = usePagePref<CodegenTarget>(COPY_TARGET_PREF, 'curl');
  const current: TargetInfo | undefined = targets?.find((t) => t.id === lastTarget) ?? targets?.find((t) => t.id === 'curl');
  const currentName = current?.name ?? 'cURL';
  const pinned = summary.pinned === true;

  const pick = (t: string) => {
    setLastTarget(t as CodegenTarget);
    void copyAsSnippet([id], (targets?.find((x) => x.id === t) ?? t) as TargetInfo | CodegenTarget);
  };

  const copyItems = (): MenuItems => {
    const list = targets ?? [];
    const popular = list.filter((t) => POPULAR.includes(t.id));
    if (current && !popular.some((t) => t.id === current.id)) popular.unshift(current);
    const groups = new Map<string, TargetInfo[]>();
    for (const t of list) {
      const g = groups.get(t.group) ?? [];
      g.push(t);
      groups.set(t.group, g);
    }
    return [
      { type: 'label', label: 'Copy as' },
      {
        type: 'radio',
        value: current?.id ?? 'curl',
        onValueChange: pick,
        options: popular.map((t) => ({ value: t.id, label: t.name })),
      },
      list.length > popular.length && {
        type: 'submenu',
        label: 'All targets',
        icon: Code,
        items: [...groups.entries()].flatMap(([group, ts]): MenuItems => [
          { type: 'label', key: `l-${group}`, label: group },
          { type: 'radio', key: `r-${group}`, value: current?.id ?? 'curl', onValueChange: pick, options: ts.map((t) => ({ value: t.id, label: t.name })) },
        ]),
      },
      { type: 'separator' },
      { label: 'Copy URL', icon: Link, onSelect: () => void copyUrls([id]) },
      { label: 'Copy request body', icon: Copy, disabled: requestBodySize === 0, onSelect: () => void copyBody(id, 'request') },
      { label: 'Copy response body', icon: Copy, disabled: responseBodySize === 0, onSelect: () => void copyBody(id, 'response') },
    ];
  };

  const connection = variant === 'connection';
  const ruleItems = (): MenuItems => [
    { type: 'label', label: 'Create rule from this exchange' },
    ...createRuleItems(summary, { httpLike: !connection }),
  ];
  const moreItems = (): MenuItems => [
    { label: note ? 'Edit note…' : 'Add note…', icon: StickyNote, onSelect: afterMenuClose(onEditNote) },
    { type: 'submenu', label: 'Highlight', icon: Highlighter, items: highlightItems(summary.color, (c) => void setHighlight([id], c)) },
    { type: 'separator' },
    connection && { label: 'Copy host', icon: Link, onSelect: () => void copyText(`${summary.host}:${summary.port}`).then((ok) => ok && toast.success('Host copied')) },
    !connection && { label: 'Download request body', icon: FileUp, disabled: requestBodySize === 0, onSelect: () => void downloadBody(id, 'request') },
    !connection && { label: 'Download response body', icon: Download, disabled: responseBodySize === 0, onSelect: () => void downloadBody(id, 'response') },
    { label: 'Export as HAR', icon: FileDown, onSelect: () => exportHar('selected', [id]) },
    { type: 'separator' },
    { label: 'Delete', icon: Trash2, destructive: true, shortcut: 'delete', onSelect: () => deleteExchanges([id]) },
  ];

  if (connection) {
    return (
      <div className={styles.actions} role="toolbar" aria-label="Exchange actions">
        <MenuButton label="Create rule" icon={WandSparkles} chevron items={ruleItems} />
        <IconButton label={pinned ? 'Unpin' : 'Pin'} icon={Pin} active={pinned} shortcut="mod+p" onClick={() => void setPinned([id], !pinned)} />
        <MenuButton label="More actions" icon={Ellipsis} items={moreItems} />
      </div>
    );
  }

  return (
    <div className={styles.actions} role="toolbar" aria-label="Exchange actions">
      <IconButton label="Replay" icon={RotateCw} shortcut="mod+r" onClick={() => void replay([id])} />
      <IconButton label="Edit in Compose" icon={SquarePen} shortcut="mod+e" onClick={() => void openInComposeFor(id)} />
      <SplitMenuButton
        label={`Copy as ${currentName}`}
        icon={Copy}
        shortcut={current?.id === 'curl' || !current ? 'mod+shift+c' : undefined}
        onClick={() => void copyAsSnippet([id], current ?? 'curl')}
        menuLabel="Copy as…"
        items={copyItems}
      />
      <MenuButton label="Create rule" icon={WandSparkles} chevron items={ruleItems} />
      <IconButton label={pinned ? 'Unpin' : 'Pin'} icon={Pin} active={pinned} shortcut="mod+p" onClick={() => void setPinned([id], !pinned)} />
      <MenuButton label="More actions" icon={Ellipsis} items={moreItems} />
    </div>
  );
}
