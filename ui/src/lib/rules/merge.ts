/**
 * Three-way merge of rule sets, by item id.
 *
 * Undo/redo and in-flight saves on the Rules page must not replace the daemon's rule set with
 * a stale snapshot: rules can change elsewhere at any time (MCP `akev_add_mock`, another
 * window). Instead the change a history entry made (`from` → `to`) is re-applied on top of the
 * current rules, and local edits are rebased onto remote ones.
 *
 * Per item, each top-level field (`title`, `enabled`, `matchers`, `action`, …) and its position
 * (parent group, order among siblings) merge independently: whatever only one side changed is
 * taken from that side. When both sides changed the same thing differently, `policy` picks the
 * winner and the item is reported as a conflict.
 */

import type { RuleId, RuleItem, RuleSet } from '../../api/types';
import { deepEqual } from './tree';

export type ConflictPolicy = 'ours' | 'theirs';

export interface MergeResult {
  set: RuleSet;
  /** Items both sides changed in incompatible ways (resolved by the policy). */
  conflicts: RuleId[];
}

type Parent = RuleId | null;

interface Indexed {
  items: Map<RuleId, RuleItem>;
  parent: Map<RuleId, Parent>;
  children: Map<Parent, RuleId[]>;
}

function index(rs: RuleSet): Indexed {
  const items = new Map<RuleId, RuleItem>();
  const parent = new Map<RuleId, Parent>();
  const children = new Map<Parent, RuleId[]>();
  const walk = (list: readonly RuleItem[], p: Parent) => {
    const ids: RuleId[] = [];
    for (const item of list) {
      if (items.has(item.id)) continue; // malformed (duplicate id): first one wins
      items.set(item.id, item);
      parent.set(item.id, p);
      ids.push(item.id);
      if (item.type === 'group') walk(item.items, item.id);
    }
    children.set(p, ids);
  };
  walk(rs.items, null);
  return { items, parent, children };
}

/** An item's own fields (a group's children are merged separately). */
function fields(item: RuleItem): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const [k, v] of Object.entries(item)) if (k !== 'items' && v !== undefined) out[k] = v;
  return out;
}

/** Order of `seq` restricted to `common`. */
function restricted(seq: readonly RuleId[], common: ReadonlySet<RuleId>): RuleId[] {
  return seq.filter((id) => common.has(id));
}

function sameOrder(a: readonly RuleId[], b: readonly RuleId[]): boolean {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
}

export function mergeRuleSets(base: RuleSet, ours: RuleSet, theirs: RuleSet, policy: ConflictPolicy): MergeResult {
  // Trivial cases keep object identity.
  if (ours === theirs || deepEqual(ours, theirs)) return { set: ours, conflicts: [] };
  if (deepEqual(base, theirs)) return { set: ours, conflicts: [] };
  if (deepEqual(base, ours)) return { set: theirs, conflicts: [] };

  const B = index(base);
  const O = index(ours);
  const T = index(theirs);
  const conflicts = new Set<RuleId>();
  const pick = <V>(id: RuleId, o: V, t: V): V => {
    conflicts.add(id);
    return policy === 'ours' ? o : t;
  };

  // ---- which items exist
  const ids: RuleId[] = [];
  const seenIds = new Set<RuleId>();
  for (const src of [O, T, B]) for (const id of src.items.keys()) if (!seenIds.has(id)) (seenIds.add(id), ids.push(id));

  const changedFrom = (side: Indexed, id: RuleId): boolean => {
    const b = B.items.get(id);
    const s = side.items.get(id);
    if (!b || !s) return true;
    return B.parent.get(id) !== side.parent.get(id) || !deepEqual(fields(b), fields(s));
  };

  const present = new Set<RuleId>();
  for (const id of ids) {
    const inB = B.items.has(id);
    const inO = O.items.has(id);
    const inT = T.items.has(id);
    let keep: boolean;
    if (inO && inT) keep = true;
    else if (!inO && !inT) keep = false;
    else if (!inB) keep = true; // added on one side
    else if (!inO) keep = changedFrom(T, id) ? pick(id, false, true) : false; // ours deleted it
    else keep = changedFrom(O, id) ? pick(id, true, false) : false; // theirs deleted it
    if (keep) present.add(id);
  }

  // ---- fields
  const merged = new Map<RuleId, Record<string, unknown>>();
  for (const id of present) {
    const b = B.items.get(id);
    const o = O.items.get(id);
    const t = T.items.get(id);
    if (!o || !t) {
      merged.set(id, fields((o ?? t) as RuleItem));
      continue;
    }
    const bf = b ? fields(b) : undefined;
    const of = fields(o);
    const tf = fields(t);
    const out: Record<string, unknown> = {};
    for (const k of new Set([...Object.keys(of), ...Object.keys(tf), ...Object.keys(bf ?? {})])) {
      const ov = of[k];
      const tv = tf[k];
      let v: unknown;
      if (deepEqual(ov, tv)) v = ov;
      else if (bf && deepEqual(ov, bf[k])) v = tv;
      else if (bf && deepEqual(tv, bf[k])) v = ov;
      else v = pick(id, ov, tv);
      if (v !== undefined) out[k] = v;
    }
    merged.set(id, out);
  }

  // ---- parents
  const parentOf = new Map<RuleId, Parent>();
  for (const id of present) {
    const inO = O.items.has(id);
    const inT = T.items.has(id);
    const po = O.parent.get(id) ?? null;
    const pt = T.parent.get(id) ?? null;
    let p: Parent;
    if (!inO) p = pt;
    else if (!inT) p = po;
    else if (po === pt) p = po;
    else if (B.items.has(id) && po === (B.parent.get(id) ?? null)) p = pt;
    else if (B.items.has(id) && pt === (B.parent.get(id) ?? null)) p = po;
    else p = pick(id, po, pt);
    parentOf.set(id, p);
  }
  const isGroup = (id: RuleId) => present.has(id) && merged.get(id)?.type === 'group';
  // A parent that no longer exists (deleted by the other side): hoist to the nearest surviving
  // ancestor (as any side knew it), else the root.
  const ancestorOf = (p: RuleId): Parent => {
    for (const src of [O, T, B]) if (src.parent.has(p)) return src.parent.get(p) ?? null;
    return null;
  };
  for (const id of present) {
    let p = parentOf.get(id) ?? null;
    const visited = new Set<RuleId>();
    while (p !== null && !isGroup(p) && !visited.has(p)) {
      visited.add(p);
      p = ancestorOf(p);
    }
    parentOf.set(id, p !== null && isGroup(p) ? p : null);
  }
  // Conflicting moves can create cycles (A into B on one side, B into A on the other).
  for (const id of present) {
    const chain = new Set<RuleId>([id]);
    let p = parentOf.get(id) ?? null;
    while (p !== null) {
      if (chain.has(p)) {
        conflicts.add(id);
        parentOf.set(id, null);
        break;
      }
      chain.add(p);
      p = parentOf.get(p) ?? null;
    }
  }

  // ---- sibling order
  const kids = new Map<Parent, RuleId[]>();
  for (const id of present) {
    const p = parentOf.get(id) ?? null;
    let list = kids.get(p);
    if (!list) kids.set(p, (list = []));
    list.push(id);
  }
  const ordered = new Map<Parent, RuleId[]>();
  for (const [p, members] of kids) {
    const set = new Set(members);
    const seqO = (O.children.get(p) ?? []).filter((id) => set.has(id));
    const seqT = (T.children.get(p) ?? []).filter((id) => set.has(id));
    const seqB = (B.children.get(p) ?? []).filter((id) => set.has(id));
    const common = new Set(seqB.filter((id) => seqO.includes(id) && seqT.includes(id)));
    const cb = restricted(seqB, common);
    const oursMoved = !sameOrder(cb, restricted(seqO, common));
    const theirsMoved = !sameOrder(cb, restricted(seqT, common));
    let primary: RuleId[];
    if (oursMoved && !theirsMoved) primary = seqO;
    else if (theirsMoved && !oursMoved) primary = seqT;
    else {
      if (oursMoved && theirsMoved && !sameOrder(restricted(seqO, common), restricted(seqT, common))) for (const id of common) conflicts.add(id);
      primary = policy === 'ours' ? seqO : seqT;
    }
    const secondary = primary === seqO ? seqT : seqO;
    const out = [...primary];
    const placed = new Set(out);
    // Items only the other side has here go after their predecessor there (else first).
    for (const seq of [secondary, seqB, members]) {
      seq.forEach((id, i) => {
        if (placed.has(id)) return;
        let at = 0;
        for (let j = i - 1; j >= 0; j--) {
          const k = out.indexOf(seq[j] as RuleId);
          if (k >= 0) {
            at = k + 1;
            break;
          }
        }
        out.splice(at, 0, id);
        placed.add(id);
      });
    }
    ordered.set(p, out);
  }

  // ---- build
  const build = (p: Parent): RuleItem[] =>
    (ordered.get(p) ?? []).map((id) => {
      const f = merged.get(id) as Record<string, unknown>;
      return (f.type === 'group' ? { ...f, items: build(id) } : { ...f }) as RuleItem;
    });
  const set: RuleSet = { ...theirs, ...ours, items: build(null) };
  if (deepEqual(set, theirs)) return { set: theirs, conflicts: [...conflicts] };
  if (deepEqual(set, ours)) return { set: ours, conflicts: [...conflicts] };
  return { set, conflicts: [...conflicts] };
}

/**
 * Re-apply the change `from` → `to` (an undo/redo step) on top of `current`, keeping every
 * change made since `from` elsewhere. Where the same field was changed both by the step and
 * since, the newer (current) value is kept and the item is reported.
 */
export function rebaseChange(current: RuleSet, from: RuleSet, to: RuleSet): MergeResult {
  if (current === from || deepEqual(current, from)) return { set: to, conflicts: [] };
  return mergeRuleSets(from, to, current, 'theirs');
}
