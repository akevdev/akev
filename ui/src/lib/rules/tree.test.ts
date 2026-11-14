import { describe, expect, test } from 'bun:test';
import type { RuleItem, RuleSet } from '../../api/types';
import {
  canMove,
  countRules,
  currentSlotIndex,
  deepEqual,
  dropAt,
  dropSlots,
  duplicateItem,
  findItem,
  flatten,
  insertAt,
  listGroups,
  locate,
  moveItem,
  positionOf,
  reconcile,
  reidentify,
  removeItem,
  renameGroup,
  replaceRule,
  setAllEnabled,
  setItemEnabled,
  ungroup,
  walkRules,
  type RuleNode,
} from './tree';

const r = (id: string, enabled = true): RuleNode => ({ type: 'rule', id, enabled, matchers: [], action: { type: 'passthrough' } });
const g = (id: string, items: RuleItem[], enabled = true): RuleItem => ({ type: 'group', id, title: id.toUpperCase(), enabled, collapsed: false, items });

function sample(): RuleSet {
  return { version: 1, items: [r('a'), g('g1', [r('b'), r('c', false)]), r('d'), g('g2', [g('g3', [r('e')], false)])] };
}

const ids = (rs: RuleSet): string[] => walkRules(rs).map((w) => w.rule.id);
const shape = (items: readonly RuleItem[]): unknown => items.map((i) => (i.type === 'group' ? { [i.id]: shape(i.items) } : i.id));

describe('lookup', () => {
  test('locate / findItem / walk order & effective enabled', () => {
    const rs = sample();
    const loc = locate(rs.items, 'e');
    expect(loc?.depth).toBe(2);
    expect(loc?.parent?.id).toBe('g3');
    expect(loc?.ancestors.map((a) => a.id)).toEqual(['g2', 'g3']);
    expect(findItem(rs, 'g1')?.type).toBe('group');
    expect(ids(rs)).toEqual(['a', 'b', 'c', 'd', 'e']);
    const walked = walkRules(rs);
    expect(walked.find((w) => w.rule.id === 'c')?.effectiveEnabled).toBe(false);
    expect(walked.find((w) => w.rule.id === 'e')?.effectiveEnabled).toBe(false); // g3 disabled
    expect(walked.find((w) => w.rule.id === 'b')?.order).toBe(1);
    expect(countRules(rs.items)).toBe(5);
    expect(listGroups(rs).map((x) => [x.group.id, x.depth])).toEqual([
      ['g1', 0],
      ['g2', 0],
      ['g3', 1],
    ]);
  });
});

describe('updates', () => {
  test('replaceRule keeps untouched subtrees by identity', () => {
    const rs = sample();
    const next = replaceRule(rs, { ...r('b'), title: 'B' });
    expect((findItem(next, 'b') as RuleNode).title).toBe('B');
    expect(next.items[0]).toBe(rs.items[0]);
    expect(next.items[2]).toBe(rs.items[2]);
    expect(next.items[3]).toBe(rs.items[3]);
    expect(next.items[1]).not.toBe(rs.items[1]);
  });

  test('remove + restore at original position', () => {
    const rs = sample();
    const removed = removeItem(rs, 'c');
    expect(removed?.parentId).toBe('g1');
    expect(removed?.index).toBe(1);
    expect(ids(removed!.set)).toEqual(['a', 'b', 'd', 'e']);
    const restored = insertAt(removed!.set, removed!.item, removed!.parentId, removed!.index);
    expect(deepEqual(restored, rs)).toBe(true);
  });

  test('move before/after/inside/root-end', () => {
    const rs = sample();
    expect(shape(moveItem(rs, 'a', { type: 'after', targetId: 'b' })!.items)).toEqual([{ g1: ['b', 'a', 'c'] }, 'd', { g2: [{ g3: ['e'] }] }]);
    expect(shape(moveItem(rs, 'd', { type: 'before', targetId: 'a' })!.items)).toEqual(['d', 'a', { g1: ['b', 'c'] }, { g2: [{ g3: ['e'] }] }]);
    expect(shape(moveItem(rs, 'a', { type: 'inside', targetId: 'g3', at: 'end' })!.items)).toEqual([{ g1: ['b', 'c'] }, 'd', { g2: [{ g3: ['e', 'a'] }] }]);
    expect(shape(moveItem(rs, 'a', { type: 'inside', targetId: 'g1', at: 'start' })!.items)).toEqual([{ g1: ['a', 'b', 'c'] }, 'd', { g2: [{ g3: ['e'] }] }]);
    expect(shape(moveItem(rs, 'b', { type: 'root-end' })!.items)).toEqual(['a', { g1: ['c'] }, 'd', { g2: [{ g3: ['e'] }] }, 'b']);
  });

  test('invalid and no-op moves', () => {
    const rs = sample();
    expect(moveItem(rs, 'g2', { type: 'inside', targetId: 'g3', at: 'end' })).toBeNull();
    expect(moveItem(rs, 'g2', { type: 'before', targetId: 'e' })).toBeNull();
    expect(canMove(rs, 'g2', { type: 'inside', targetId: 'g2', at: 'end' })).toBe(false);
    expect(moveItem(rs, 'g2', { type: 'before', targetId: 'g2' })).toBe(rs);
    expect(moveItem(rs, 'missing', { type: 'root-end' })).toBeNull();
    expect(moveItem(rs, 'a', { type: 'before', targetId: 'g1' })).toBe(rs); // already there
  });

  test('duplicate inserts a re-identified copy after the source', () => {
    const rs = sample();
    const dup = duplicateItem(rs, 'g1');
    expect(dup).not.toBeNull();
    const copy = findItem(dup!.set, dup!.id);
    expect(copy?.type).toBe('group');
    expect(dup!.set.items[2]?.id).toBe(dup!.id);
    const copyIds = (copy as Extract<RuleItem, { type: 'group' }>).items.map((i) => i.id);
    expect(copyIds).not.toContain('b');
    expect(copyIds.length).toBe(2);
    expect((copy as Extract<RuleItem, { type: 'group' }>).title).toBe('G1 (copy)');
    const re = reidentify(r('x'));
    expect(re.id).not.toBe('x');
  });

  test('enable toggles', () => {
    const rs = sample();
    expect(setItemEnabled(rs, 'a', true)).toEqual(rs);
    expect((findItem(setItemEnabled(rs, 'a', false), 'a') as RuleNode).enabled).toBe(false);
    const all = setAllEnabled(rs, true);
    expect(walkRules(all).every((w) => w.effectiveEnabled)).toBe(true);
    expect(setAllEnabled(all, true)).toBe(all);
    const none = setAllEnabled(rs, false);
    expect(walkRules(none).some((w) => w.rule.enabled)).toBe(false);
  });

  test('rename & ungroup', () => {
    const rs = sample();
    expect((findItem(renameGroup(rs, 'g1', 'API'), 'g1') as { title: string }).title).toBe('API');
    expect(renameGroup(rs, 'g1', 'G1')).toEqual(rs);
    expect(shape(ungroup(rs, 'g1').items)).toEqual(['a', 'b', 'c', 'd', { g2: [{ g3: ['e'] }] }]);
    expect(shape(ungroup(rs, 'g3').items)).toEqual(['a', { g1: ['b', 'c'] }, 'd', { g2: ['e'] }]);
  });
});

describe('reconcile', () => {
  test('reuses equal subtrees from the previous tree', () => {
    const prev = sample();
    const echoed = JSON.parse(JSON.stringify(prev)) as RuleSet;
    expect(reconcile(prev, echoed)).toBe(prev);
    const changed = JSON.parse(JSON.stringify(prev)) as RuleSet;
    (changed.items[2] as RuleNode).title = 'D';
    const out = reconcile(prev, changed);
    expect(out).not.toBe(prev);
    expect(out.items[0]).toBe(prev.items[0]);
    expect(out.items[1]).toBe(prev.items[1]);
    expect(out.items[2]).not.toBe(prev.items[2]);
    expect((out.items[2] as RuleNode).title).toBe('D');
  });

  test('moved items keep their identity', () => {
    const prev = sample();
    const moved = moveItem(prev, 'd', { type: 'before', targetId: 'a' })!;
    const echoed = JSON.parse(JSON.stringify(moved)) as RuleSet;
    const out = reconcile(prev, echoed);
    expect(out.items[0]).toBe(prev.items[2]);
  });

  test('deepEqual treats undefined props as absent', () => {
    expect(deepEqual({ a: 1, b: undefined }, { a: 1 })).toBe(true);
    expect(deepEqual([1, { x: [2] }], [1, { x: [2] }])).toBe(true);
    expect(deepEqual([1], [1, 2])).toBe(false);
    expect(deepEqual({ a: null }, { a: undefined })).toBe(false);
  });
});

describe('flatten', () => {
  test('collapsed groups hide children; orders are global', () => {
    const rs = sample();
    const rows = flatten(rs, new Set(['g2']));
    expect(rows.map((x) => x.id)).toEqual(['a', 'g1', 'b', 'c', 'd', 'g2']);
    expect(rows.find((x) => x.id === 'd')?.order).toBe(4);
    expect(rows.find((x) => x.id === 'g1')?.ruleCount).toBe(2);
    expect(rows.find((x) => x.id === 'c')?.last).toBe(true);
    expect(rows.find((x) => x.id === 'g2')?.expanded).toBe(false);
    const full = flatten(rs, new Set());
    expect(full.find((x) => x.id === 'e')?.parentEnabled).toBe(false);
    expect(full.find((x) => x.id === 'e')?.depth).toBe(2);
  });

  test('search shows matches with their ancestors, expanded', () => {
    const rs = sample();
    const rows = flatten(rs, new Set(['g2']), { rule: (x) => x.id === 'e', group: () => false });
    expect(rows.map((x) => x.id)).toEqual(['g2', 'g3', 'e']);
    expect(rows.find((x) => x.id === 'e')?.order).toBe(5);
    const byGroup = flatten(rs, new Set(), { rule: () => false, group: (x) => x.id === 'g1' });
    expect(byGroup.map((x) => x.id)).toEqual(['g1', 'b', 'c']);
  });
});

describe('drop geometry', () => {
  const rs = sample();
  const rows = flatten(rs, new Set(['g2']));
  const heights = rows.map((x) => (x.kind === 'group' ? 32 : 40));
  // tops: a 0, g1 40, b 72, c 112, d 152, g2 192 (end 224)

  test('rule halves', () => {
    expect(dropAt(rows, heights, 5)?.position).toEqual({ type: 'before', targetId: 'a' });
    expect(dropAt(rows, heights, 30)?.position).toEqual({ type: 'after', targetId: 'a' });
  });

  test('group zones', () => {
    expect(dropAt(rows, heights, 42)?.position).toEqual({ type: 'before', targetId: 'g1' });
    const inside = dropAt(rows, heights, 56);
    expect(inside?.position).toEqual({ type: 'inside', targetId: 'g1', at: 'start' });
    expect(inside?.mode).toBe('group');
    // Bottom quarter of an expanded group: start of its children, line one level deeper.
    const bottom = dropAt(rows, heights, 70);
    expect(bottom?.position).toEqual({ type: 'inside', targetId: 'g1', at: 'start' });
    expect(bottom?.depth).toBe(1);
    // Collapsed group bottom quarter: after the group.
    expect(dropAt(rows, heights, 222)?.position).toEqual({ type: 'after', targetId: 'g2' });
    // Collapsed group middle: inside at the end.
    expect(dropAt(rows, heights, 208)?.position).toEqual({ type: 'inside', targetId: 'g2', at: 'end' });
  });

  test('last child bottom half stays inside its group; below the list = root end', () => {
    const last = dropAt(rows, heights, 140);
    expect(last?.position).toEqual({ type: 'after', targetId: 'c' });
    expect(last?.depth).toBe(1);
    expect(dropAt(rows, heights, 400)?.position).toEqual({ type: 'root-end' });
    expect(dropAt([], [], 10)?.position).toEqual({ type: 'root-end' });
  });

  test('keyboard slots are distinct, valid and include the current position', () => {
    const slots = dropSlots(rs, rows, 'b');
    const results = slots.map((s) => JSON.stringify(positionOf(moveItem(rs, 'b', s.position)!, 'b')));
    expect(new Set(results).size).toBe(results.length);
    const cur = currentSlotIndex(rs, slots, 'b');
    expect(cur).toBeGreaterThanOrEqual(0);
    expect(moveItem(rs, 'b', slots[cur]!.position)).toBe(rs);
    // A group can't be dropped into itself.
    const gSlots = dropSlots(rs, flatten(rs, new Set()), 'g2');
    expect(gSlots.some((s) => s.position.type !== 'root-end' && ['g3', 'e'].includes(s.position.targetId))).toBe(false);
    // The last child of a group can start a keyboard move from where it is.
    const cSlots = dropSlots(rs, rows, 'c');
    expect(currentSlotIndex(rs, cSlots, 'c')).toBeGreaterThanOrEqual(0);
  });
});
