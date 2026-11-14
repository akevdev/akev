/**
 * Pure, immutable operations on a {@link RuleSet} tree (groups nest arbitrarily).
 *
 * Every update returns a new tree with structural sharing: untouched subtrees keep their
 * identity, so list rows memoised on item identity only re-render when they change. Ids are
 * stable UUIDs; everything addresses items by id (never by index paths).
 */

import type { Rule, RuleGroup, RuleId, RuleItem, RuleSet } from '../../api/types';
import { RULESET_VERSION } from '../../api/types';

export type RuleNode = { type: 'rule' } & Rule;
export type GroupNode = { type: 'group' } & RuleGroup;

// =====================================================================================
// ids & equality
// =====================================================================================

let fallbackSeq = 0;

/** A fresh UUID (v4) for rules and groups. */
export function newId(): RuleId {
  const c = (globalThis as { crypto?: Crypto }).crypto;
  if (c && typeof c.randomUUID === 'function') return c.randomUUID();
  fallbackSeq += 1;
  const rnd = () => Math.floor(Math.random() * 0x10000).toString(16).padStart(4, '0');
  return `${rnd()}${rnd()}-${rnd()}-4${rnd().slice(1)}-a${rnd().slice(1)}-${rnd()}${rnd()}${(fallbackSeq % 0x10000).toString(16).padStart(4, '0')}`;
}

/** Structural equality for JSON-like values (`undefined` properties count as absent). */
export function deepEqual(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== typeof b || a === null || b === null || typeof a !== 'object') return false;
  if (Array.isArray(a)) {
    if (!Array.isArray(b) || a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) if (!deepEqual(a[i], b[i])) return false;
    return true;
  }
  if (Array.isArray(b)) return false;
  const ao = a as Record<string, unknown>;
  const bo = b as Record<string, unknown>;
  const ak = Object.keys(ao).filter((k) => ao[k] !== undefined);
  const bk = Object.keys(bo).filter((k) => bo[k] !== undefined);
  if (ak.length !== bk.length) return false;
  for (const k of ak) if (!deepEqual(ao[k], bo[k])) return false;
  return true;
}

/**
 * Return `next`, but reuse every subtree of `prev` that is structurally equal — so a rule set
 * echoed back by the daemon (new object identities, same content) keeps identities stable.
 */
export function reconcile<T>(prev: T, next: T): T {
  if (prev === next) return prev;
  if (prev === null || next === null || typeof prev !== 'object' || typeof next !== 'object') return next;
  if (Array.isArray(prev) !== Array.isArray(next)) return next;
  if (Array.isArray(prev) && Array.isArray(next)) {
    let same = prev.length === next.length;
    const out = next.map((n, i) => {
      // Match rule/group items by id so a moved item still reuses its old identity.
      const p = matchById(prev, n, i);
      const r = p === undefined ? n : reconcile(p, n);
      if (r !== prev[i]) same = false;
      return r;
    });
    return (same ? prev : out) as T;
  }
  const po = prev as Record<string, unknown>;
  const no = next as Record<string, unknown>;
  let same = true;
  const out: Record<string, unknown> = {};
  for (const k of Object.keys(no)) {
    const r = reconcile(po[k], no[k]);
    out[k] = r;
    if (r !== po[k]) same = false;
  }
  if (same && Object.keys(po).filter((k) => po[k] !== undefined).length !== Object.keys(no).filter((k) => no[k] !== undefined).length) same = false;
  return (same ? prev : out) as T;
}

function matchById(prev: readonly unknown[], n: unknown, index: number): unknown {
  if (n && typeof n === 'object' && 'id' in n && 'type' in n) {
    const id = (n as { id: unknown }).id;
    const atIndex = prev[index];
    if (atIndex && typeof atIndex === 'object' && (atIndex as { id?: unknown }).id === id) return atIndex;
    return prev.find((p) => p && typeof p === 'object' && (p as { id?: unknown }).id === id);
  }
  return prev[index];
}

// =====================================================================================
// lookup
// =====================================================================================

export function emptyRuleSet(): RuleSet {
  return { version: RULESET_VERSION, items: [] };
}

export interface Located {
  item: RuleItem;
  /** Containing group, or `null` at the root. */
  parent: GroupNode | null;
  index: number;
  depth: number;
  /** Ancestor groups, outermost first. */
  ancestors: GroupNode[];
}

/** Find an item (rule or group) anywhere in the tree. */
export function locate(items: readonly RuleItem[], id: RuleId): Located | null {
  const rec = (list: readonly RuleItem[], parent: GroupNode | null, ancestors: GroupNode[]): Located | null => {
    for (let i = 0; i < list.length; i++) {
      const item = list[i] as RuleItem;
      if (item.id === id) return { item, parent, index: i, depth: ancestors.length, ancestors };
      if (item.type === 'group') {
        const found = rec(item.items, item, [...ancestors, item]);
        if (found) return found;
      }
    }
    return null;
  };
  return rec(items, null, []);
}

export function findItem(rs: RuleSet | null | undefined, id: RuleId | null | undefined): RuleItem | undefined {
  if (!rs || !id) return undefined;
  return locate(rs.items, id)?.item;
}

export function findRule(rs: RuleSet | null | undefined, id: RuleId | null | undefined): RuleNode | undefined {
  const item = findItem(rs, id);
  return item?.type === 'rule' ? item : undefined;
}

export interface WalkedRule {
  rule: RuleNode;
  /** Enabled itself and every ancestor group enabled. */
  effectiveEnabled: boolean;
  depth: number;
  parentId: RuleId | null;
  /** 0-based position in evaluation order (depth-first). */
  order: number;
}

/** All rules depth-first (evaluation order), like `RuleSet::walk`. */
export function walkRules(rs: RuleSet | null | undefined): WalkedRule[] {
  const out: WalkedRule[] = [];
  if (!rs) return out;
  const rec = (list: readonly RuleItem[], parentEnabled: boolean, depth: number, parentId: RuleId | null) => {
    for (const item of list) {
      if (item.type === 'rule') out.push({ rule: item, effectiveEnabled: parentEnabled && item.enabled, depth, parentId, order: out.length });
      else rec(item.items, parentEnabled && item.enabled, depth + 1, item.id);
    }
  };
  rec(rs.items, true, 0, null);
  return out;
}

/** Number of rules (not groups) inside an item list, recursively. */
export function countRules(items: readonly RuleItem[]): number {
  let n = 0;
  for (const i of items) n += i.type === 'rule' ? 1 : countRules(i.items);
  return n;
}

/** Number of groups inside an item list, recursively. */
export function countGroups(items: readonly RuleItem[]): number {
  let n = 0;
  for (const i of items) if (i.type === 'group') n += 1 + countGroups(i.items);
  return n;
}

/** Every rule and group id in the tree. */
export function allIds(items: readonly RuleItem[], out: RuleId[] = []): RuleId[] {
  for (const i of items) {
    out.push(i.id);
    if (i.type === 'group') allIds(i.items, out);
  }
  return out;
}

/** Whether `id` is `groupId` itself or nested anywhere inside it. */
export function isWithin(rs: RuleSet, id: RuleId, groupId: RuleId): boolean {
  if (id === groupId) return true;
  const group = findItem(rs, groupId);
  if (!group || group.type !== 'group') return false;
  return locate(group.items, id) !== null;
}

// =====================================================================================
// updates (immutable, structural sharing)
// =====================================================================================

function mapList(list: readonly RuleItem[], id: RuleId, fn: (item: RuleItem) => RuleItem | null): RuleItem[] | null {
  for (let i = 0; i < list.length; i++) {
    const item = list[i] as RuleItem;
    if (item.id === id) {
      const replaced = fn(item);
      const out = list.slice();
      if (replaced === null) out.splice(i, 1);
      else out[i] = replaced;
      return out;
    }
    if (item.type === 'group') {
      const inner = mapList(item.items, id, fn);
      if (inner) {
        const out = list.slice();
        out[i] = { ...item, items: inner };
        return out;
      }
    }
  }
  return null;
}

/** Replace one item via `fn` (return `null` to remove it). Returns `rs` unchanged when not found. */
export function updateItem(rs: RuleSet, id: RuleId, fn: (item: RuleItem) => RuleItem | null): RuleSet {
  const items = mapList(rs.items, id, fn);
  return items ? { ...rs, items } : rs;
}

/** Replace a rule by id (keeps `type: 'rule'`). */
export function replaceRule(rs: RuleSet, rule: Rule): RuleSet {
  return updateItem(rs, rule.id, (item) => (item.type === 'rule' ? ({ ...rule, type: 'rule' } as RuleNode) : item));
}

export interface Removed {
  set: RuleSet;
  item: RuleItem;
  /** Where it was: parent group id (`null` = root) and index. */
  parentId: RuleId | null;
  index: number;
}

export function removeItem(rs: RuleSet, id: RuleId): Removed | null {
  const loc = locate(rs.items, id);
  if (!loc) return null;
  return { set: updateItem(rs, id, () => null), item: loc.item, parentId: loc.parent?.id ?? null, index: loc.index };
}

/** Where to put an item. */
export type DropPosition =
  | { type: 'before'; targetId: RuleId }
  | { type: 'after'; targetId: RuleId }
  /** Inside a group, at the start or the end of its children. */
  | { type: 'inside'; targetId: RuleId; at: 'start' | 'end' }
  /** At the end of the root list. */
  | { type: 'root-end' };

/** Insert `item` at a position. Returns `null` when the target doesn't exist. */
export function insertItem(rs: RuleSet, item: RuleItem, pos: DropPosition): RuleSet | null {
  if (pos.type === 'root-end') return { ...rs, items: [...rs.items, item] };
  if (pos.type === 'inside') {
    const target = findItem(rs, pos.targetId);
    if (!target || target.type !== 'group') return null;
    return updateItem(rs, pos.targetId, (g) =>
      g.type === 'group' ? { ...g, items: pos.at === 'start' ? [item, ...g.items] : [...g.items, item] } : g,
    );
  }
  const loc = locate(rs.items, pos.targetId);
  if (!loc) return null;
  const at = pos.type === 'before' ? loc.index : loc.index + 1;
  if (!loc.parent) {
    const items = rs.items.slice();
    items.splice(at, 0, item);
    return { ...rs, items };
  }
  return updateItem(rs, loc.parent.id, (g) => {
    if (g.type !== 'group') return g;
    const items = g.items.slice();
    items.splice(at, 0, item);
    return { ...g, items };
  });
}

/** Insert at a parent/index (used to restore deleted items). Clamps the index. */
export function insertAt(rs: RuleSet, item: RuleItem, parentId: RuleId | null, index: number): RuleSet {
  if (parentId === null) {
    const items = rs.items.slice();
    items.splice(Math.max(0, Math.min(index, items.length)), 0, item);
    return { ...rs, items };
  }
  const parent = findItem(rs, parentId);
  if (!parent || parent.type !== 'group') return { ...rs, items: [...rs.items, item] };
  return updateItem(rs, parentId, (g) => {
    if (g.type !== 'group') return g;
    const items = g.items.slice();
    items.splice(Math.max(0, Math.min(index, items.length)), 0, item);
    return { ...g, items };
  });
}

/** Whether moving `id` to `pos` is allowed (not into itself / its own descendants). */
export function canMove(rs: RuleSet, id: RuleId, pos: DropPosition): boolean {
  const item = findItem(rs, id);
  if (!item) return false;
  if (pos.type === 'root-end') return true;
  // Dropping an item right before/after itself is a valid no-op.
  if (pos.targetId === id) return pos.type !== 'inside';
  if (item.type === 'group' && isWithin(rs, pos.targetId, id)) return false;
  return findItem(rs, pos.targetId) !== undefined;
}

/** Move an item. Returns `null` when the move is invalid; `rs` itself when it is a no-op. */
export function moveItem(rs: RuleSet, id: RuleId, pos: DropPosition): RuleSet | null {
  if (!canMove(rs, id, pos)) return null;
  if (pos.type !== 'root-end' && pos.targetId === id) return rs;
  const removed = removeItem(rs, id);
  if (!removed) return null;
  const next = insertItem(removed.set, removed.item, pos);
  if (!next) return null;
  return deepEqual(next.items, rs.items) ? rs : next;
}

/** Location of an item as `{ parentId, index }` (for undo/restore and no-op detection). */
export function positionOf(rs: RuleSet, id: RuleId): { parentId: RuleId | null; index: number } | null {
  const loc = locate(rs.items, id);
  return loc ? { parentId: loc.parent?.id ?? null, index: loc.index } : null;
}

/** Deep copy with fresh ids for the item and everything inside it. */
export function reidentify<T extends RuleItem>(item: T): T {
  if (item.type === 'group') return { ...item, id: newId(), items: item.items.map((i) => reidentify(i)) } as T;
  return { ...structuredCloneJson(item), id: newId() } as T;
}

/** Fresh ids for a whole item list. */
export function reidentifyAll(items: readonly RuleItem[]): RuleItem[] {
  return items.map((i) => reidentify(i));
}

function structuredCloneJson<T>(v: T): T {
  return JSON.parse(JSON.stringify(v)) as T;
}

/** Duplicate an item right after itself (fresh ids). */
export function duplicateItem(rs: RuleSet, id: RuleId): { set: RuleSet; id: RuleId } | null {
  const item = findItem(rs, id);
  if (!item) return null;
  const copy = reidentify(item);
  if (copy.type === 'rule' && copy.title) copy.title = `${copy.title} (copy)`;
  if (copy.type === 'group') copy.title = `${copy.title} (copy)`;
  const set = insertItem(rs, copy, { type: 'after', targetId: id });
  return set ? { set, id: copy.id } : null;
}

export function setItemEnabled(rs: RuleSet, id: RuleId, enabled: boolean): RuleSet {
  return updateItem(rs, id, (item) => (item.enabled === enabled ? item : { ...item, enabled }));
}

/** Enable/disable every rule and group. Returns `rs` when nothing changes. */
export function setAllEnabled(rs: RuleSet, enabled: boolean): RuleSet {
  let changed = false;
  const rec = (list: readonly RuleItem[]): RuleItem[] =>
    list.map((item) => {
      const items = item.type === 'group' ? rec(item.items) : null;
      if (item.enabled === enabled && (!items || items.every((x, i) => x === (item as GroupNode).items[i]))) return item;
      changed = true;
      return items ? { ...item, enabled, items } : { ...item, enabled };
    });
  const items = rec(rs.items);
  return changed ? { ...rs, items } : rs;
}

export function renameGroup(rs: RuleSet, id: RuleId, title: string): RuleSet {
  return updateItem(rs, id, (item) => (item.type === 'group' && item.title !== title ? { ...item, title } : item));
}

/** Replace a group with its children (in place). */
export function ungroup(rs: RuleSet, id: RuleId): RuleSet {
  const loc = locate(rs.items, id);
  if (!loc || loc.item.type !== 'group') return rs;
  const children = loc.item.items;
  if (!loc.parent) {
    const items = rs.items.slice();
    items.splice(loc.index, 1, ...children);
    return { ...rs, items };
  }
  return updateItem(rs, loc.parent.id, (g) => {
    if (g.type !== 'group') return g;
    const items = g.items.slice();
    items.splice(loc.index, 1, ...children);
    return { ...g, items };
  });
}

/** All groups, depth-first, with their depth (for "Move to group ▸" menus). */
export function listGroups(rs: RuleSet | null | undefined): { group: GroupNode; depth: number }[] {
  const out: { group: GroupNode; depth: number }[] = [];
  if (!rs) return out;
  const rec = (list: readonly RuleItem[], depth: number) => {
    for (const i of list) {
      if (i.type === 'group') {
        out.push({ group: i, depth });
        rec(i.items, depth + 1);
      }
    }
  };
  rec(rs.items, 0);
  return out;
}

// =====================================================================================
// flattening for the list
// =====================================================================================

export interface FlatRow {
  kind: 'rule' | 'group';
  id: RuleId;
  item: RuleItem;
  depth: number;
  parentId: RuleId | null;
  /** Index among siblings. */
  index: number;
  /** Is the last child of its parent. */
  last: boolean;
  /** Every ancestor group is enabled. */
  parentEnabled: boolean;
  /** Evaluation order (rules only, counted over the whole tree, 1-based). */
  order: number;
  /** Groups: shown expanded. */
  expanded: boolean;
  /** Groups: number of rules inside (recursive). */
  ruleCount: number;
}

/**
 * Flatten the tree into visible rows. Collapsed groups hide their children. With a
 * `match` predicate (search), only matching rules (and groups containing matches, or whose
 * own title matches) are shown and every group is expanded.
 */
export function flatten(
  rs: RuleSet | null | undefined,
  collapsed: ReadonlySet<RuleId>,
  match?: { rule: (rule: RuleNode) => boolean; group: (group: GroupNode) => boolean },
): FlatRow[] {
  const rows: FlatRow[] = [];
  if (!rs) return rows;
  let order = 0;
  // Assign evaluation order over the full tree first (search must not renumber).
  const orders = new Map<RuleId, number>();
  for (const w of walkRules(rs)) orders.set(w.rule.id, w.order + 1);
  const visibleUnder = (g: GroupNode): boolean => {
    if (!match) return true;
    if (match.group(g)) return true;
    return g.items.some((i) => (i.type === 'rule' ? match.rule(i) : visibleUnder(i)));
  };
  const rec = (list: readonly RuleItem[], depth: number, parentId: RuleId | null, parentEnabled: boolean, groupMatched: boolean) => {
    const shown = list.filter((i) => !match || groupMatched || (i.type === 'rule' ? match.rule(i) : visibleUnder(i)));
    shown.forEach((item, idx) => {
      const index = list.indexOf(item);
      const last = idx === shown.length - 1;
      if (item.type === 'rule') {
        order = orders.get(item.id) ?? order + 1;
        rows.push({ kind: 'rule', id: item.id, item, depth, parentId, index, last, parentEnabled, order, expanded: false, ruleCount: 0 });
      } else {
        const expanded = match ? true : !collapsed.has(item.id);
        rows.push({ kind: 'group', id: item.id, item, depth, parentId, index, last, parentEnabled, order: 0, expanded, ruleCount: countRules(item.items) });
        if (expanded) rec(item.items, depth + 1, item.id, parentEnabled && item.enabled, groupMatched || (match ? match.group(item) : false));
      }
    });
  };
  rec(rs.items, 0, null, true, false);
  return rows;
}

// =====================================================================================
// drag & drop geometry
// =====================================================================================

/** Drop indicator for the list: a line before/after a row, or a highlighted group. */
export interface DropIndicator {
  position: DropPosition;
  /** Row index the indicator is drawn at (`rows.length` = after the last row). */
  rowIndex: number;
  /** Line edge (`top` of rowIndex) or the group highlight. */
  mode: 'line' | 'group';
  /** Indentation depth of the line. */
  depth: number;
}

/**
 * Work out where a dragged item would land when the probe (the dragged row's centre) is at
 * `y` px from the top of the list content. Rows have the given heights.
 *
 * Zones: on a rule row the top half drops before it and the bottom half after it; on a
 * group header the top quarter drops before the group, the middle half inside it (at the
 * start), and the bottom quarter after it (or at the start of its children when expanded).
 * Below the last row drops at the end of the root list.
 */
export function dropAt(rows: readonly FlatRow[], heights: readonly number[], y: number): DropIndicator | null {
  if (rows.length === 0) return { position: { type: 'root-end' }, rowIndex: 0, mode: 'line', depth: 0 };
  let top = 0;
  for (let i = 0; i < rows.length; i++) {
    const row = rows[i] as FlatRow;
    const h = heights[i] ?? 0;
    if (y < top + h || (i === 0 && y < top)) {
      const f = h > 0 ? Math.max(0, Math.min(1, (y - top) / h)) : 0;
      if (row.kind === 'rule') {
        return f < 0.5
          ? { position: { type: 'before', targetId: row.id }, rowIndex: i, mode: 'line', depth: row.depth }
          : afterRow(rows, i);
      }
      if (f < 0.25) return { position: { type: 'before', targetId: row.id }, rowIndex: i, mode: 'line', depth: row.depth };
      if (f < 0.75 || !row.expanded) {
        if (f < 0.75) return { position: { type: 'inside', targetId: row.id, at: row.expanded ? 'start' : 'end' }, rowIndex: i, mode: 'group', depth: row.depth };
        return afterRow(rows, i);
      }
      // Bottom quarter of an expanded group: start of its children.
      return { position: { type: 'inside', targetId: row.id, at: 'start' }, rowIndex: i + 1, mode: 'line', depth: row.depth + 1 };
    }
    top += h;
  }
  return { position: { type: 'root-end' }, rowIndex: rows.length, mode: 'line', depth: 0 };
}

/** "After row i" — for a collapsed group / rule, after the item itself (same depth). */
function afterRow(rows: readonly FlatRow[], i: number): DropIndicator {
  const row = rows[i] as FlatRow;
  // Line drawn at the top of the next row (or the end).
  let next = i + 1;
  if (row.kind === 'group' && row.expanded) {
    while (next < rows.length && (rows[next] as FlatRow).depth > row.depth) next++;
  }
  return { position: { type: 'after', targetId: row.id }, rowIndex: next, mode: 'line', depth: row.depth };
}

/**
 * Every distinct drop slot for keyboard moves, top to bottom: before each row, inside each
 * group, after the last child of each group and the root end. Slots that are invalid (into
 * the dragged group itself) are skipped and slots producing the same result are merged. The
 * dragged item's current position is included, so a keyboard move can start from it.
 */
export function dropSlots(rs: RuleSet, rows: readonly FlatRow[], draggedId: RuleId): DropIndicator[] {
  const candidates: DropIndicator[] = [];
  rows.forEach((row, i) => {
    candidates.push({ position: { type: 'before', targetId: row.id }, rowIndex: i, mode: 'line', depth: row.depth });
    if (row.kind === 'group') {
      candidates.push({ position: { type: 'inside', targetId: row.id, at: row.expanded ? 'start' : 'end' }, rowIndex: i, mode: 'group', depth: row.depth });
    }
    if (row.last && row.parentId) candidates.push({ position: { type: 'after', targetId: row.id }, rowIndex: i + 1, mode: 'line', depth: row.depth });
  });
  candidates.push({ position: { type: 'root-end' }, rowIndex: rows.length, mode: 'line', depth: 0 });
  const seen = new Set<string>();
  const out: DropIndicator[] = [];
  for (const c of candidates) {
    const moved = moveItem(rs, draggedId, c.position);
    if (!moved) continue;
    const p = positionOf(moved, draggedId);
    if (!p) continue;
    const key = `${p.parentId ?? ''}:${p.index}`;
    if (seen.has(key)) continue;
    seen.add(key);
    out.push(c);
  }
  return out;
}

/** Index of the slot that leaves `draggedId` where it is (the keyboard-move start). */
export function currentSlotIndex(rs: RuleSet, slots: readonly DropIndicator[], draggedId: RuleId): number {
  return slots.findIndex((s) => moveItem(rs, draggedId, s.position) === rs);
}

/** A short human description of a drop position (for screen-reader announcements). */
export function describePosition(rs: RuleSet, pos: DropPosition, label: (item: RuleItem) => string): string {
  if (pos.type === 'root-end') return 'at the end of the list';
  const target = findItem(rs, pos.targetId);
  if (!target) return '';
  if (pos.type === 'inside') return `${pos.at === 'start' ? 'at the top of' : 'at the end of'} group ${label(target)}`;
  return `${pos.type} ${label(target)}`;
}
