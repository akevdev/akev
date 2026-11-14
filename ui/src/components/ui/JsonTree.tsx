import { useVirtualizer } from '@tanstack/react-virtual';
import clsx from 'clsx';
import { ChevronRight, ChevronsDownUp, ChevronsUpDown, Copy } from 'lucide-react';
import { memo, useEffect, useId, useMemo, useRef, useState, type KeyboardEvent, type ReactNode } from 'react';
import { isLosslessNumber, JSON_ENTRIES, stringifyJson } from '../../lib/json';
import { copyText } from '../../lib/platform';
import { ContextMenu } from './ContextMenu';
import { IconButton } from './IconButton';
import { SearchInput } from './SearchInput';
import { toast } from './toast';
import styles from './JsonTree.module.css';

export interface JsonTreeProps {
  data: unknown;
  /** Levels expanded initially (default 2). */
  defaultExpandDepth?: number;
  /** Controlled search text (otherwise the built-in toolbar search is used). */
  search?: string;
  /** Show the toolbar (search, expand/collapse all). Default true. */
  toolbar?: boolean;
  /** Fill the parent height (default) or auto-size up to `maxHeight`. */
  fill?: boolean;
  maxHeight?: number;
  className?: string;
  'aria-label'?: string;
}

export type JsonType = 'object' | 'array' | 'string' | 'number' | 'boolean' | 'null' | 'other';

interface FlatRow {
  /** Unique row id (expansion state, React key). Equals `path` except for shadowed duplicate keys. */
  id: string;
  /** jq-style path shown and copied (`$.items[0].id`). */
  path: string;
  key: string | number | null;
  value: unknown;
  depth: number;
  type: JsonType;
  childCount: number;
  /** An earlier occurrence of a duplicate key (most parsers keep only the last one). */
  shadowed: boolean;
}

const ROW_H = 22;
const IDENT = /^[A-Za-z_$][A-Za-z0-9_$]*$/;

function typeOf(v: unknown): JsonType {
  if (v === null) return 'null';
  if (Array.isArray(v)) return 'array';
  if (isLosslessNumber(v)) return 'number';
  switch (typeof v) {
    case 'object':
      return 'object';
    case 'string':
      return 'string';
    case 'number':
    case 'bigint':
      return 'number';
    case 'boolean':
      return 'boolean';
    default:
      return 'other';
  }
}

/** jq-style path segment: `.name`, `["odd key"]`, `[3]`. */
export function childPath(parent: string, key: string | number): string {
  if (typeof key === 'number') return `${parent}[${key}]`;
  return IDENT.test(key) ? `${parent}.${key}` : `${parent}[${JSON.stringify(key)}]`;
}

interface Child {
  key: string | number;
  value: unknown;
  id: string;
  path: string;
  shadowed: boolean;
}

/**
 * Children of a container. Objects parsed with duplicate keys (`lib/json`) list every
 * occurrence; the earlier ones are `shadowed` and get a unique id suffix.
 */
export function childrenOf(v: unknown, parentId: string, parentPath: string): Child[] {
  if (Array.isArray(v)) return v.map((x, i) => ({ key: i, value: x, id: childPath(parentId, i), path: childPath(parentPath, i), shadowed: false }));
  if (!v || typeof v !== 'object' || isLosslessNumber(v)) return [];
  const dupes = (v as { [JSON_ENTRIES]?: [string, unknown][] })[JSON_ENTRIES];
  if (!dupes) return Object.entries(v as Record<string, unknown>).map(([k, x]) => ({ key: k, value: x, id: childPath(parentId, k), path: childPath(parentPath, k), shadowed: false }));
  const last = new Map<string, number>();
  dupes.forEach(([k], i) => last.set(k, i));
  return dupes.map(([k, x], i) => {
    const shadowed = last.get(k) !== i;
    const path = childPath(parentPath, k);
    return { key: k, value: x, id: shadowed ? `${childPath(parentId, k)}#${i}` : childPath(parentId, k), path, shadowed };
  });
}

/** Text shown for a primitive (strings quoted; big/precise numbers exactly as written). */
export function primitiveText(v: unknown, type: JsonType): string {
  if (type === 'string') return JSON.stringify(v);
  if (type === 'null') return 'null';
  if (isLosslessNumber(v)) return v.source;
  return String(v);
}

/** Text "Copy value" puts on the clipboard (exact: big numbers and duplicate keys kept). */
export function copyValueText(v: unknown): string {
  return typeof v === 'string' ? v : (stringifyJson(v, 2) ?? String(v));
}

/** Paths expanded by default up to `depth`. */
function initialExpanded(data: unknown, depth: number): Set<string> {
  const out = new Set<string>();
  const walk = (v: unknown, path: string, d: number) => {
    const t = typeOf(v);
    if ((t !== 'object' && t !== 'array') || d >= depth) return;
    out.add(path);
    let n = 0;
    for (const c of childrenOf(v, path, path)) {
      if (++n > 500) break; // don't pre-expand huge containers
      walk(c.value, c.id, d + 1);
    }
  };
  walk(data, '$', 0);
  return out;
}

/** Paths of matches plus all their ancestors (to auto-expand). Capped for huge documents. */
function searchMatches(data: unknown, query: string): { matches: Set<string>; ancestors: Set<string> } {
  const matches = new Set<string>();
  const ancestors = new Set<string>();
  const q = query.toLowerCase();
  let budget = 200_000;
  const walk = (v: unknown, id: string, key: string | number | null, parents: string[]): void => {
    if (--budget < 0) return;
    const t = typeOf(v);
    const keyHit = key != null && String(key).toLowerCase().includes(q);
    const valueHit = t !== 'object' && t !== 'array' && primitiveText(v, t).toLowerCase().includes(q);
    if (keyHit || valueHit) {
      matches.add(id);
      for (const p of parents) ancestors.add(p);
    }
    if (t === 'object' || t === 'array') {
      const next = [...parents, id];
      for (const c of childrenOf(v, id, id)) walk(c.value, c.id, c.key, next);
    }
  };
  walk(data, '$', null, []);
  return { matches, ancestors };
}

function flatten(data: unknown, expanded: ReadonlySet<string>): FlatRow[] {
  const rows: FlatRow[] = [];
  const walk = (v: unknown, id: string, path: string, key: string | number | null, depth: number, shadowed: boolean) => {
    const type = typeOf(v);
    const kids = type === 'object' || type === 'array' ? childrenOf(v, id, path) : [];
    rows.push({ id, path, key, value: v, depth, type, childCount: kids.length, shadowed });
    if (kids.length && expanded.has(id)) for (const c of kids) walk(c.value, c.id, c.path, c.key, depth + 1, c.shadowed);
  };
  walk(data, '$', '$', null, 0, false);
  return rows;
}

function highlight(text: string, query: string): ReactNode {
  if (!query) return text;
  const i = text.toLowerCase().indexOf(query.toLowerCase());
  if (i < 0) return text;
  return (
    <>
      {text.slice(0, i)}
      <mark className={styles.mark}>{text.slice(i, i + query.length)}</mark>
      {text.slice(i + query.length)}
    </>
  );
}

interface RowViewProps {
  row: FlatRow;
  /** DOM id (the tree's `aria-activedescendant` points at it). */
  id: string;
  index: number;
  open: boolean;
  active: boolean;
  query: string;
  isMatch: boolean;
  onToggle: (path: string) => void;
  onActivate: (index: number) => void;
}

const RowView = memo(function RowView({ row, id, index, open, active, query, isMatch, onToggle, onActivate }: RowViewProps) {
  const container = row.type === 'object' || row.type === 'array';
  const keyLabel = row.key == null ? null : typeof row.key === 'number' ? String(row.key) : row.key;
  return (
    // biome-ignore lint/a11y/useKeyWithClickEvents lint/a11y/useFocusableInteractive: the tree handles the keyboard and points at rows with aria-activedescendant
    <div
      id={id}
      className={clsx(styles.row, isMatch && styles.matchRow, row.shadowed && styles.shadowed)}
      style={{ paddingLeft: 8 + row.depth * 16 }}
      onClick={() => {
        onActivate(index);
        if (container) onToggle(row.id);
      }}
      data-path={row.id}
      data-active={active || undefined}
      aria-selected={active}
      title={row.shadowed ? 'Duplicate key — overridden by a later occurrence (most parsers keep only the last)' : undefined}
      role="treeitem"
      aria-level={row.depth + 1}
      aria-expanded={container ? open : undefined}
    >
      <span className={clsx(styles.chevron, open && styles.open, !container && styles.hidden)} aria-hidden>
        <ChevronRight size={12} strokeWidth={2.25} />
      </span>
      {keyLabel != null && (
        <>
          <span className={typeof row.key === 'number' ? styles.index : styles.key}>{highlight(keyLabel, query)}</span>
          <span className={styles.colon}>:</span>
        </>
      )}
      {container ? (
        <span className={styles.preview}>
          {row.type === 'array' ? '[' : '{'}
          {!open && <span className={styles.ellipsis}>{row.childCount ? '…' : ''}</span>}
          {!open && (row.type === 'array' ? ']' : '}')}
          <span className={styles.count}>
            {row.childCount} {row.type === 'array' ? (row.childCount === 1 ? 'item' : 'items') : row.childCount === 1 ? 'key' : 'keys'}
          </span>
        </span>
      ) : (
        <span className={clsx(styles.value, styles[row.type])}>{highlight(primitiveText(row.value, row.type), query)}</span>
      )}
      {row.shadowed && <span className={styles.dupe}>duplicate</span>}
    </div>
  );
});

/**
 * Collapsible, virtualised JSON tree with type-coloured values, search highlighting
 * (auto-expands to matches), expand/collapse all, and copy path/value via context menu.
 */
export function JsonTree({ data, defaultExpandDepth = 2, search, toolbar = true, fill = true, maxHeight = 480, className, ...aria }: JsonTreeProps) {
  const [expanded, setExpanded] = useState<Set<string>>(() => initialExpanded(data, defaultExpandDepth));
  const [localSearch, setLocalSearch] = useState('');
  const query = (search ?? localSearch).trim();
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    setExpanded(initialExpanded(data, defaultExpandDepth));
  }, [data, defaultExpandDepth]);

  const found = useMemo(() => (query ? searchMatches(data, query) : null), [data, query]);
  const effective = useMemo(() => {
    if (!found) return expanded;
    const merged = new Set(expanded);
    for (const p of found.ancestors) merged.add(p);
    return merged;
  }, [expanded, found]);
  const rows = useMemo(() => flatten(data, effective), [data, effective]);

  const toggle = (path: string) =>
    setExpanded((prev) => {
      const next = new Set(prev);
      if (effective.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const expandAll = () => setExpanded(initialExpanded(data, Infinity));
  const collapseAll = () => setExpanded(new Set(['$']));

  const virtualizer = useVirtualizer({ count: rows.length, getScrollElement: () => scrollRef.current, estimateSize: () => ROW_H, overscan: 16 });

  // ---- keyboard: ↑/↓ Home/End move, → expands / enters, ← collapses / goes to the parent,
  // Enter or Space toggles. The active row is announced through aria-activedescendant.
  const treeId = useId();
  const rowId = (i: number) => `${treeId}-r${i}`;
  const [activeRaw, setActive] = useState(0);
  const active = Math.min(activeRaw, Math.max(0, rows.length - 1));
  const moveTo = (i: number) => {
    const next = Math.max(0, Math.min(rows.length - 1, i));
    setActive(next);
    virtualizer.scrollToIndex(next, { align: 'auto' });
  };
  const onTreeKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    const row = rows[active];
    if (!row) return;
    const container = row.type === 'object' || row.type === 'array';
    const open = effective.has(row.id);
    switch (e.key) {
      case 'ArrowDown':
        moveTo(active + 1);
        break;
      case 'ArrowUp':
        moveTo(active - 1);
        break;
      case 'Home':
        moveTo(0);
        break;
      case 'End':
        moveTo(rows.length - 1);
        break;
      case 'ArrowRight':
        if (container && !open) toggle(row.id);
        else if (container && row.childCount) moveTo(active + 1);
        break;
      case 'ArrowLeft':
        if (container && open) toggle(row.id);
        else {
          let parent = active - 1;
          while (parent >= 0 && (rows[parent]?.depth ?? 0) >= row.depth) parent--;
          if (parent >= 0) moveTo(parent);
        }
        break;
      case 'Enter':
      case ' ':
        if (container) toggle(row.id);
        break;
      default:
        return;
    }
    e.preventDefault();
  };

  const hoveredPath = useRef<string | null>(null);
  const menuFor = () => {
    const id = hoveredPath.current;
    const row = rows.find((r) => r.id === id);
    if (!row) return [];
    const display = row.path === '$' ? '.' : row.path.slice(1);
    return [
      {
        label: 'Copy value',
        icon: Copy,
        onSelect: () => void copyText(copyValueText(row.value)).then((ok) => ok && toast.success('Value copied')),
      },
      { label: 'Copy path', hint: display, onSelect: () => void copyText(display).then((ok) => ok && toast.success('Path copied')) },
      ...(row.type === 'object' || row.type === 'array'
        ? [
            { type: 'separator' as const },
            {
              label: 'Expand all children',
              icon: ChevronsUpDown,
              onSelect: () =>
                setExpanded((prev) => {
                  const next = new Set(prev);
                  for (const p of initialExpanded(row.value, Infinity)) next.add(p === '$' ? row.id : row.id + p.slice(1));
                  return next;
                }),
            },
          ]
        : []),
    ];
  };

  const items = virtualizer.getVirtualItems();
  const height = fill ? undefined : Math.min(maxHeight, rows.length * ROW_H + 12);

  return (
    <div className={clsx(styles.root, fill && styles.fill, className)}>
      {toolbar && (
        <div className={styles.toolbar}>
          {search === undefined && <SearchInput size="sm" value={localSearch} onValueChange={setLocalSearch} placeholder="Search keys & values" className={styles.search} />}
          {found && <span className={clsx(styles.matches, 'tnum')}>{found.matches.size} matches</span>}
          <span className={styles.spacer} />
          <IconButton label="Expand all" icon={ChevronsUpDown} size="sm" onClick={expandAll} />
          <IconButton label="Collapse all" icon={ChevronsDownUp} size="sm" onClick={collapseAll} />
        </div>
      )}
      <ContextMenu items={menuFor}>
        <div
          ref={scrollRef}
          className={styles.scroll}
          style={height ? { height, flex: 'none' } : undefined}
          role="tree"
          aria-label={aria['aria-label'] ?? 'JSON tree'}
          aria-activedescendant={rows.length ? rowId(active) : undefined}
          tabIndex={0}
          onKeyDown={onTreeKeyDown}
          onContextMenu={(e) => {
            hoveredPath.current = (e.target as HTMLElement).closest<HTMLElement>('[data-path]')?.dataset.path ?? null;
          }}
        >
          <div className={styles.sizer} style={{ height: virtualizer.getTotalSize() }}>
            <div className={styles.window} style={{ transform: `translateY(${items[0]?.start ?? 0}px)` }}>
              {items.map((item) => {
                const row = rows[item.index];
                if (!row) return null;
                return (
                  <RowView
                    key={row.id}
                    row={row}
                    id={rowId(item.index)}
                    index={item.index}
                    open={effective.has(row.id)}
                    active={item.index === active}
                    query={query}
                    isMatch={found?.matches.has(row.id) ?? false}
                    onToggle={toggle}
                    onActivate={setActive}
                  />
                );
              })}
            </div>
          </div>
        </div>
      </ContextMenu>
    </div>
  );
}
