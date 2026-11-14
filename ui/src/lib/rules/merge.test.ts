import { describe, expect, test } from 'bun:test';
import type { RuleItem, RuleSet } from '../../api/types';
import { History } from './history';
import { mergeRuleSets, rebaseChange } from './merge';
import { findRule, insertAt, moveItem, removeItem, renameGroup, replaceRule, setItemEnabled, type RuleNode } from './tree';

const r = (id: string, patch: Partial<RuleNode> = {}): RuleNode => ({ type: 'rule', id, enabled: true, matchers: [], action: { type: 'passthrough' }, ...patch });
const g = (id: string, items: RuleItem[]): RuleItem => ({ type: 'group', id, title: id.toUpperCase(), enabled: true, collapsed: false, items });
const set = (...items: RuleItem[]): RuleSet => ({ version: 1, items });
const shape = (items: readonly RuleItem[]): unknown => items.map((i) => (i.type === 'group' ? { [i.id]: shape(i.items) } : i.id));
const mock = r('mock', { title: 'Mock from MCP', action: { type: 'close' } });

describe('undo/redo re-apply their step on top of changes made elsewhere', () => {
  test('rename, then an agent adds a mock via MCP, then undo: the mock survives (regression)', () => {
    const h = new History<RuleSet>();
    const before = set(r('a'), r('b'));
    const after = replaceRule(before, { ...r('a'), title: 'Renamed' });
    h.push({ label: 'Rename rule', before, after });
    // akev_add_mock prepends a rule; the page adopts the broadcast.
    const current = set(mock, ...after.items);
    const entry = h.undo()!;
    // Before the fix the page PUT `entry.before` — deleting the agent's mock.
    expect(findRule(entry.before, 'mock')).toBeUndefined();
    const { set: undone, conflicts } = rebaseChange(current, entry.after, entry.before);
    expect(conflicts).toEqual([]);
    expect(shape(undone.items)).toEqual(['mock', 'a', 'b']);
    expect(findRule(undone, 'a')?.title).toBeUndefined();
    expect(findRule(undone, 'mock')).toEqual(mock);
    // Redo brings the rename back, still keeping the mock.
    const redoEntry = h.redo()!;
    const { set: redone } = rebaseChange(undone, redoEntry.before, redoEntry.after);
    expect(findRule(redone, 'a')?.title).toBe('Renamed');
    expect(shape(redone.items)).toEqual(['mock', 'a', 'b']);
  });

  test('without changes elsewhere, undo/redo restore the exact snapshots', () => {
    const before = set(r('a'), r('b'));
    const after = setItemEnabled(before, 'b', false);
    expect(rebaseChange(after, after, before).set).toBe(before);
    expect(rebaseChange(before, before, after).set).toBe(after);
  });

  test('undo of a delete puts the rule back where it was, next to rules added since', () => {
    const before = set(r('a'), r('b'), r('d'), r('c'));
    const after = removeItem(before, 'd')!.set;
    const current = set(mock, ...after.items, r('z'));
    const { set: undone } = rebaseChange(current, after, before);
    expect(shape(undone.items)).toEqual(['mock', 'a', 'b', 'd', 'c', 'z']);
  });

  test('undo of a move into a group, and of an add', () => {
    const before = set(r('a'), g('g', [r('b')]), r('c'));
    const moved = moveItem(before, 'c', { type: 'inside', targetId: 'g', at: 'start' })!;
    const current = set(...moved.items, mock);
    expect(shape(rebaseChange(current, moved, before).set.items)).toEqual(['a', { g: ['b'] }, 'c', 'mock']);
    const added = insertAt(before, r('new'), 'g', 1);
    expect(shape(rebaseChange(set(mock, ...added.items), added, before).set.items)).toEqual(['mock', 'a', { g: ['b'] }, 'c']);
  });

  test('different fields of the same rule merge; the same field changed since wins and is reported', () => {
    const before = set(r('a', { title: 'A' }));
    const after = replaceRule(before, { ...r('a'), title: 'A2' });
    // Elsewhere: the action changed (different field) → undo still reverts the title.
    const otherField = replaceRule(after, { ...findRule(after, 'a')!, action: { type: 'reset' } });
    const res1 = rebaseChange(otherField, after, before);
    expect(findRule(res1.set, 'a')).toMatchObject({ title: 'A', action: { type: 'reset' } });
    expect(res1.conflicts).toEqual([]);
    // Elsewhere: the title itself changed again → keep the newer title, report a conflict.
    const sameField = replaceRule(after, { ...findRule(after, 'a')!, title: 'From agent' });
    const res2 = rebaseChange(sameField, after, before);
    expect(findRule(res2.set, 'a')?.title).toBe('From agent');
    expect(res2.conflicts).toEqual(['a']);
  });

  test('undoing an add of a rule that was edited elsewhere keeps it; a rule deleted elsewhere stays deleted', () => {
    const before = set(r('a'));
    const after = set(r('a'), r('n'));
    const edited = replaceRule(after, { ...r('n'), title: 'edited elsewhere' });
    const res = rebaseChange(edited, after, before);
    expect(findRule(res.set, 'n')?.title).toBe('edited elsewhere');
    expect(res.conflicts).toEqual(['n']);
    // Undo a title edit of a rule someone else deleted meanwhile: it isn't resurrected.
    const b2 = set(r('a'), r('b'));
    const a2 = replaceRule(b2, { ...r('b'), title: 'T' });
    const deleted = removeItem(a2, 'b')!.set;
    expect(findRule(rebaseChange(deleted, a2, b2).set, 'b')).toBeUndefined();
  });
});

describe('mergeRuleSets (rebasing unsaved edits onto remote changes)', () => {
  test('local edits + remote additions are both kept', () => {
    const base = set(r('a'), g('g', [r('b')]));
    const ours = renameGroup(setItemEnabled(base, 'a', false), 'g', 'Mine');
    const theirs = insertAt(set(mock, ...base.items), r('x'), 'g', 1);
    const { set: m, conflicts } = mergeRuleSets(base, ours, theirs, 'ours');
    expect(conflicts).toEqual([]);
    expect(shape(m.items)).toEqual(['mock', 'a', { g: ['b', 'x'] }]);
    expect(findRule(m, 'a')?.enabled).toBe(false);
    expect((m.items[2] as { title: string }).title).toBe('Mine');
  });

  test('echoes and already-applied changes are no-ops', () => {
    const base = set(r('a'));
    const ours = set(r('a'), r('b'));
    expect(mergeRuleSets(base, ours, ours, 'ours').set).toBe(ours);
    // Theirs already contains our in-flight change plus one of its own.
    const theirs = set(r('a'), r('b'), mock);
    expect(shape(mergeRuleSets(base, ours, theirs, 'ours').set.items)).toEqual(['a', 'b', 'mock']);
  });

  test('a group deleted on one side keeps children the other side added (hoisted)', () => {
    const base = set(r('a'), g('g', [r('b')]));
    const ours = removeItem(base, 'g')!.set;
    const theirs = insertAt(base, r('x'), 'g', 1);
    const { set: m } = mergeRuleSets(base, ours, theirs, 'ours');
    expect(shape(m.items)).toEqual(['a', 'x']);
  });

  test('conflicting moves never produce a cycle', () => {
    const base = set(g('g1', []), g('g2', []));
    const ours = moveItem(base, 'g1', { type: 'inside', targetId: 'g2', at: 'end' })!;
    const theirs = moveItem(base, 'g2', { type: 'inside', targetId: 'g1', at: 'end' })!;
    const { set: m, conflicts } = mergeRuleSets(base, ours, theirs, 'ours');
    const seen: string[] = [];
    const walk = (items: readonly RuleItem[]) => items.forEach((i) => (seen.push(i.id), i.type === 'group' && walk(i.items)));
    walk(m.items);
    expect(seen.sort()).toEqual(['g1', 'g2']);
    expect(conflicts.length).toBeGreaterThan(0);
  });
});
